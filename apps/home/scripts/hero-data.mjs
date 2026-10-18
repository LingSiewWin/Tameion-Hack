// The hero's data files (DESIGN §Data and honesty). Node >= 22.18 (imports the TypeScript model directly).
//   node scripts/hero-data.mjs ais <recording.jsonl> [out.json]   pack a real aisstream.io recording -> public/hero/ais.json
//   node scripts/hero-data.mjs tracks                             the illustrative tracks on the strait's traffic lanes -> public/hero/ais.json
//   node scripts/hero-data.mjs portwatch <query.json> <url>       IMF PortWatch daily chokepoint rows -> public/hero/portwatch.json
//   node scripts/hero-data.mjs scene <image> <world file> <scene id> <YYYY-MM-DD> <credit> [source URL]
//                                     the founder's Sentinel-2 export -> public/hero/scene.avif (<= 350 KB), scene-828.avif (phones)
//                                     and scene.json (size, world file, crs read from it: metres = EPSG:3857, degrees = EPSG:4326)
import { readFileSync, writeFileSync } from "node:fs";
import { crossesCoast, pack, parseWorldFile } from "../src/features/hero/model.ts";

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
} else if (cmd === "tracks") {
  // Illustrative tracks (founder, 2026-10-18 08:44: the hero is design, its data prebuilt; there is no live recording).
  // Inbound (westbound) lane: OpenStreetMap way 220138792, seamark separation_lane, north of the Musandam peninsula; the
  // scheme's northern boundary is way 220138843. Outbound lane: 4 nm to its south (IMO: 2 nm lanes either side of a 2 nm
  // separation zone). The ends run out into the Gulf of Oman and the Gulf on waypoints checked against the coast below.
  // Deterministic: no randomness, no network.
  const NM = 1 / 60; // one nautical mile in degrees of latitude
  const k = Math.cos((26.5 * Math.PI) / 180); // a degree of longitude here, in degrees of latitude
  const osmLane = [[56.3452, 26.5951], [56.465, 26.6241], [56.5916, 26.6238], [56.6734, 26.5035]]; // west to east
  const inboundWE = [[55.35, 26.42], [55.85, 26.5], ...osmLane, [56.95, 26.2], [57.2, 25.55]];
  // a polyline moved d nm to the right of its direction (west to east: to the south)
  const offset = (line, d) =>
    line.map((p, i) => {
      const a = line[Math.max(0, i - 1)], b = line[Math.min(line.length - 1, i + 1)];
      const dx = (b[0] - a[0]) * k, dy = b[1] - a[1], len = Math.hypot(dx, dy);
      return [p[0] + ((dy / len) * d * NM) / k, p[1] - (dx / len) * d * NM];
    });
  const lanes = { in: [...inboundWE].reverse(), out: offset(inboundWE, 4) };
  // every lane, and both edges of it (1 nm either side), stays at sea
  const coast = JSON.parse(readFileSync(`${app}public/hero/coastline-hormuz.geojson`, "utf8")).features.flatMap((f) => f.geometry.coordinates);
  for (const [name, line] of Object.entries(lanes))
    for (const edge of [line, offset(line, 1), offset(line, -1)])
      for (let i = 1; i < edge.length; i++)
        if (crossesCoast(edge[i - 1], edge[i], coast)) throw new Error(`tracks: the ${name} lane crosses the coast between ${edge[i - 1]} and ${edge[i]}`);
  const segLen = (a, b) => Math.hypot((b[0] - a[0]) * k, b[1] - a[1]) / NM; // nm
  const along = (line, s) => {
    for (let i = 1; i < line.length; i++) {
      const a = line[i - 1], b = line[i], len = segLen(a, b);
      if (s <= len || i === line.length - 1) {
        const f = Math.min(1, s / len);
        const cog = ((Math.atan2((b[0] - a[0]) * k, b[1] - a[1]) * 180) / Math.PI + 360) % 360;
        return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, Math.round(cog)];
      }
      s -= len;
    }
  };
  const total = (line) => line.slice(1).reduce((sum, p, i) => sum + segLen(line[i], p), 0);
  const from = Date.parse("2026-10-18T06:00:00Z");
  // [lane, nm along it at the window's start, speed kn, AIS ship type]
  const fleet = [
    ["in", 42, 12.4, 80], ["in", 51, 14.1, 70], ["in", 60, 11.2, 84], ["in", 69, 16.0, 71],
    ["out", 40, 12.8, 80], ["out", 49, 13.3, 81], ["out", 57, 13.0, 79], ["out", 65, 10.6, 80],
  ];
  const reports = [];
  fleet.forEach(([lane, s0, kn, type], i) => {
    for (let m = 0; m <= 90; m += 2) {
      const s = s0 + (kn * m) / 60;
      if (s > total(lanes[lane])) break;
      const [lon, lat, cog] = along(lanes[lane], s);
      reports.push({ mmsi: 999000001 + i, lat, lon, sog: kn, cog, type, ts: from + m * 60_000 });
    }
  });
  const about =
    "Illustrative tracks for the hero's design, not ship positions: eight vessels with made-up identifiers (MMSI prefix 999 belongs to no country) " +
    "moving at steady speeds along the Strait of Hormuz traffic separation scheme. Inbound lane: OpenStreetMap way 220138792 (seamark separation_lane), " +
    "(c) OpenStreetMap contributors, ODbL; outbound lane 4 nm south of it (IMO: 2 nm lanes either side of a 2 nm separation zone); lane ends checked " +
    "against the Natural Earth 10 m coast (public domain). Made by scripts/hero-data.mjs tracks.";
  write(`${app}public/hero/ais.json`, { about, ...pack(reports, { synthetic: true, source: "illustrative" }) });
  console.log(`${reports.length} fixes, ${fleet.length} vessels`);
} else if (cmd === "portwatch") {
  if (!a1 || !a2) throw new Error("portwatch: give the saved query reply and its URL");
  const body = JSON.parse(readFileSync(a1, "utf8"));
  const days = body.features.map(({ attributes: a }) => ({
    date: typeof a.date === "string" ? a.date.slice(0, 10) : new Date(a.date).toISOString().slice(0, 10),
    total: a.n_total, tanker: a.n_tanker, container: a.n_container, dryBulk: a.n_dry_bulk, generalCargo: a.n_general_cargo, roro: a.n_roro,
  }));
  write(`${app}public/hero/portwatch.json`, { source: a2, fetchedAt: new Date().toISOString(), days });
} else if (cmd === "scene") {
  const [, , , , worldFile, id, date, credit, source] = process.argv;
  if (!a1 || !worldFile || !id || !/^\d{4}-\d{2}-\d{2}$/.test(date ?? "") || !credit) throw new Error("scene: <image> <world file> <scene id> <YYYY-MM-DD> <credit> [source URL]");
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
  write(`${app}public/hero/scene.json`, { id, date, credit, source, width, height, crs, wld, src, srcSet: `/hero/scene-828.avif 828w, ${src} ${Math.min(2560, width)}w` });
  console.log(`${width}x${height} ${crs}; ${full.length} B and ${phone.length} B`);
} else {
  console.error("usage: hero-data.mjs ais <recording.jsonl> [out] | tracks | portwatch <query.json> <url> | scene <image> <world file> <id> <date> <credit> [source]");
  process.exit(2);
}
