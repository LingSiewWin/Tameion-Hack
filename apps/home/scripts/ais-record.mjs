// Records aisstream.io position reports for the hero's bbox (DESIGN §Data and honesty) until a stop time.
//   node scripts/ais-record.mjs <out.jsonl> <stop, ISO 8601>
// One position report per line: mmsi, lat, lon, sog, cog, type, ts (the report's UTC time), rx (when it arrived).
// The ship type comes from the vessel's own ShipStaticData; null until it sends one (scripts/hero-data.mjs ais fills a
// track from the vessel's later static report, nothing else). The key is read from ~/.tameion/aisstream.key, never printed.
// Node's global WebSocket (Node >= 22); aisstream allows no browser connections, so this runs on a machine we hold.
import { appendFileSync, readFileSync } from "node:fs";
import { homedir } from "node:os";

const [out, stop] = process.argv.slice(2);
const end = Date.parse(stop ?? "");
if (!out || !Number.isFinite(end)) throw new Error("usage: ais-record.mjs <out.jsonl> <stop ISO 8601>");
const key = readFileSync(`${homedir()}/.tameion/aisstream.key`, "utf8").trim();
// [[lat, lon], [lat, lon]] corners (aisstream's order): 24.5–27.5 N × 54.5–58.0 E
const BOX = [[[24.5, 54.5], [27.5, 58.0]]];
const POSITION = new Set(["PositionReport", "StandardClassBPositionReport"]);
const types = new Map();
let lines = 0, first = null, last = null, backoff = 1000;
const seen = new Set();

// AIS "not available" values (ITU-R M.1371): SOG 102.3, COG 360, lat 91, lon 181
const sogOf = (v) => (typeof v === "number" && v < 102.3 ? v : null);
const cogOf = (v) => (typeof v === "number" && v < 360 ? v : null);
// MetaData.time_utc reads like "2026-10-18 00:41:07.123456 +0000 UTC"; without it, the arrival time stands
const tsOf = (meta, rx) => {
  const m = typeof meta?.time_utc === "string" ? /^(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?)/.exec(meta.time_utc) : null;
  return m ? new Date(`${m[1]}T${m[2]}Z`).toISOString() : rx;
};

function onMessage(text) {
  const msg = JSON.parse(text);
  if (msg.error) {
    console.error(`aisstream: ${msg.error}`);
    return;
  }
  const body = msg.Message?.[msg.MessageType];
  if (!body) return;
  const mmsi = body.UserID ?? msg.MetaData?.MMSI;
  if (msg.MessageType === "ShipStaticData") {
    if (typeof body.Type === "number" && body.Type > 0) types.set(mmsi, body.Type);
    return;
  }
  if (!POSITION.has(msg.MessageType)) return;
  const lat = body.Latitude, lon = body.Longitude;
  if (typeof lat !== "number" || typeof lon !== "number" || Math.abs(lat) > 90 || Math.abs(lon) > 180) return;
  const rx = new Date().toISOString();
  const line = { mmsi, lat, lon, sog: sogOf(body.Sog), cog: cogOf(body.Cog), type: types.get(mmsi) ?? null, ts: tsOf(msg.MetaData, rx), rx };
  appendFileSync(out, JSON.stringify(line) + "\n");
  lines++;
  seen.add(mmsi);
  first ??= line.ts;
  last = line.ts;
}

function connect() {
  if (Date.now() >= end) return finish();
  const ws = new WebSocket("wss://stream.aisstream.io/v0/stream");
  ws.binaryType = "arraybuffer";
  ws.onopen = () => {
    backoff = 1000;
    ws.send(JSON.stringify({ APIKey: key, BoundingBoxes: BOX, FilterMessageTypes: [...POSITION, "ShipStaticData"] }));
    console.log(`${new Date().toISOString()} connected (extensions: ${ws.extensions || "none"})`);
  };
  ws.onmessage = (e) => {
    try {
      onMessage(typeof e.data === "string" ? e.data : Buffer.from(e.data).toString("utf8"));
    } catch (err) {
      console.error(`skipped a message: ${err.message}`);
    }
  };
  ws.onerror = (e) => console.error(`${new Date().toISOString()} socket error: ${e.message ?? "unknown"}`);
  ws.onclose = (e) => {
    console.log(`${new Date().toISOString()} closed ${e.code} ${e.reason || ""}`);
    if (Date.now() < end) setTimeout(connect, (backoff = Math.min(backoff * 2, 30_000)));
    else finish();
  };
  const timer = setInterval(() => {
    if (Date.now() >= end) {
      clearInterval(timer);
      ws.close(1000, "window over");
    }
  }, 1000);
  ws.addEventListener("close", () => clearInterval(timer));
}

let done = false;
function finish() {
  if (done) return;
  done = true;
  clearInterval(progress);
  console.log(`window ${first} to ${last} UTC: ${lines} position reports, ${seen.size} vessels -> ${out}`);
}
const progress = setInterval(() => console.log(`${new Date().toISOString()} ${lines} reports, ${seen.size} vessels, ${types.size} types`), 60_000);
connect();
