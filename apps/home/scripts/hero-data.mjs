// The hero's data files (DESIGN §Data and honesty). Node >= 22.18 (imports the TypeScript model directly).
//   node scripts/hero-data.mjs ais <recording.jsonl> [out.json]   pack a real aisstream.io recording -> public/hero/ais.json
//   node scripts/hero-data.mjs synthetic                          write the 200-line synthetic replay (test/fixtures/ais.synthetic.jsonl)
//                                                                 and its packed copy public/hero/ais.synthetic.json (gitignored, dev only)
//   node scripts/hero-data.mjs portwatch <query.json> <url>       IMF PortWatch daily chokepoint rows -> public/hero/portwatch.json
//   node scripts/hero-data.mjs scene <image> <world file> <scene id> <YYYY-MM-DD>
//                                     the founder's Sentinel-2 export -> public/hero/scene.avif (<= 350 KB), scene-828.avif (phones)
//                                     and scene.json (size, world file, crs read from it: metres = EPSG:3857, degrees = EPSG:4326)
import { readFileSync, writeFileSync } from "node:fs";
import { BBOX, pack, parseWorldFile } from "../src/features/hero/model.ts";

const app = new URL("..", import.meta.url).pathname;
const [cmd, a1, a2] = process.argv.slice(2);
const lines = (file) => readFileSync(file, "utf8").split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l));
const write = (file, value) => {
  writeFileSync(file, typeof value === "string" || Buffer.isBuffer(value) ? value : JSON.stringify(value));
  console.log(`wrote ${file}`);
};

if (cmd === "ais") {
  if (!a1 || /synthetic/.test(a1)) throw new Error("ais: give the real recording (a synthetic file never becomes public/hero/ais.json)");
  // one fix per vessel per 5 minutes keeps the file small enough to fetch after LCP
  const replay = pack(lines(a1), { synthetic: false, source: "aisstream.io" }, 5 * 60_000);
  write(a2 ?? `${app}public/hero/ais.json`, replay);
  console.log(`${replay.vessels.length} vessels, ${new Date(replay.from).toISOString()} to ${new Date(replay.to).toISOString()}`);
} else if (cmd === "synthetic") {
  // Clearly not real: MMSIs 999000001.. (999 is no country's MID), a fixed made-up window, lanes drawn by hand.
  let seed = 7;
  const rnd = () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
  const lanes = [
    [[57.45, 25.25], [56.8, 26.1], [56.45, 26.45], [56.05, 26.55], [55.35, 26.3], [54.6, 26.05]], // inbound
    [[54.6, 26.15], [55.35, 26.42], [56.05, 26.66], [56.5, 26.56], [56.9, 26.15], [57.5, 25.35]], // outbound
  ];
  const from = Date.parse("2026-10-18T00:00:00Z");
  const out = [];
  for (let k = 0; k < 20; k++) {
    const mmsi = 999000001 + k;
    const anchored = k >= 15; // five at anchor off Fujairah
    const type = [80, 80, 81, 70, 71, 84, 89, 70, 60, 80, 79, 82, 70, 52, 80][k % 15];
    const lane = lanes[k % 2];
    const segs = lane.slice(1).map((p, i) => [lane[i], p, Math.hypot((p[0] - lane[i][0]) * Math.cos((26 * Math.PI) / 180), p[1] - lane[i][1])]);
    const length = segs.reduce((s, x) => s + x[2], 0);
    const speed = 9 + rnd() * 5; // kn
    let s = rnd() * length * 0.7;
    const anchor = [56.38 + rnd() * 0.1, 25.08 + rnd() * 0.12];
    for (let i = 0; i < 10; i++) {
      const ts = new Date(from + i * 10 * 60_000).toISOString();
      if (anchored) {
        out.push({ mmsi, lat: +(anchor[1] + rnd() * 0.002).toFixed(5), lon: +(anchor[0] + rnd() * 0.002).toFixed(5), sog: +(rnd() * 0.3).toFixed(1), cog: Math.round(rnd() * 359), type: [80, 70, 80, 89, 70][k - 15], ts });
        continue;
      }
      let at = s, j = 0;
      while (j < segs.length - 1 && at > segs[j][2]) at -= segs[j++][2];
      const [p, q, len] = segs[j];
      const f = Math.min(1, at / len);
      const lon = p[0] + (q[0] - p[0]) * f, lat = p[1] + (q[1] - p[1]) * f;
      const cog = Math.round(((Math.atan2((q[0] - p[0]) * Math.cos((26 * Math.PI) / 180), q[1] - p[1]) * 180) / Math.PI + 360) % 360);
      out.push({ mmsi, lat: +lat.toFixed(5), lon: +lon.toFixed(5), sog: +speed.toFixed(1), cog, type, ts });
      s += (speed * 10) / 60 / 60; // nautical miles in 10 minutes, in degrees of latitude
    }
  }
  write(`${app}test/fixtures/ais.synthetic.jsonl`, out.map((r) => JSON.stringify(r)).join("\n") + "\n");
  write(`${app}public/hero/ais.synthetic.json`, pack(out, { synthetic: true, source: "synthetic" }));
  console.log(`${out.length} lines, bbox ${BBOX.join(",")}`);
} else if (cmd === "portwatch") {
  if (!a1 || !a2) throw new Error("portwatch: give the saved query reply and its URL");
  const body = JSON.parse(readFileSync(a1, "utf8"));
  const days = body.features.map(({ attributes: a }) => ({
    date: typeof a.date === "string" ? a.date.slice(0, 10) : new Date(a.date).toISOString().slice(0, 10),
    total: a.n_total, tanker: a.n_tanker, container: a.n_container, dryBulk: a.n_dry_bulk, generalCargo: a.n_general_cargo, roro: a.n_roro,
  }));
  write(`${app}public/hero/portwatch.json`, { source: a2, fetchedAt: new Date().toISOString(), days });
} else if (cmd === "scene") {
  const [, , , , worldFile, id, date] = process.argv;
  if (!a1 || !worldFile || !id || !/^\d{4}-\d{2}-\d{2}$/.test(date ?? "")) throw new Error("scene: <image> <world file> <scene id> <YYYY-MM-DD>");
  const sharp = (await import("sharp")).default;
  const { width, height } = await sharp(a1).metadata();
  const wld = parseWorldFile(readFileSync(worldFile, "utf8"));
  if (wld.length !== 6 || wld.some((x) => !Number.isFinite(x)) || wld[1] || wld[2]) throw new Error("scene: the world file needs six numbers and no rotation");
  const crs = Math.abs(wld[4]) > 360 ? "EPSG:3857" : "EPSG:4326";
  // the hero image stays <= 350 KB (DESIGN §Acceptance): step the AVIF quality down until it fits
  const encode = async (w, max) => {
    for (let q = 60; q >= 20; q -= 5) {
      const buf = await sharp(a1).resize({ width: Math.min(w, width) }).avif({ quality: q, effort: 6 }).toBuffer();
      if (buf.length <= max) return buf;
    }
    throw new Error(`scene: cannot fit ${w} px under ${max} bytes`);
  };
  const full = await encode(2560, 350_000);
  const phone = await encode(828, 120_000);
  write(`${app}public/hero/scene.avif`, full);
  write(`${app}public/hero/scene-828.avif`, phone);
  const src = "/hero/scene.avif";
  write(`${app}public/hero/scene.json`, { id, date, width, height, crs, wld, src, srcSet: `/hero/scene-828.avif 828w, ${src} ${Math.min(2560, width)}w` });
  console.log(`${width}x${height} ${crs}; ${full.length} B and ${phone.length} B`);
} else {
  console.error("usage: hero-data.mjs ais <recording.jsonl> [out] | synthetic | portwatch <query.json> <url>");
  process.exit(2);
}
