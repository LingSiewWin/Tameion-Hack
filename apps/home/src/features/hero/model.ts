// The hero's pure parts: the scroll phases, the grid, the land/sea threshold, the AIS replay and the scene's projection.
// No DOM here, so node --test and the scripts (scripts/ais-pack.mjs) import it as is.

// ---- scroll -------------------------------------------------------------------------------------------------------

export const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
export const seg = (p: number, a: number, b: number) => clamp01((p - a) / (b - a));
const ease = (t: number) => t * t * (3 - 2 * t);
export const lerp = (a: number, b: number, t: number) => a + (b - a) * t;

/** The six keyframes sit at p = 0, .2, .4, .6, .8, 1 (DESIGN §The hero); each transition is eased and complete just
 *  before its keyframe, so every keyframe is whole at its p and holds for a moment of scroll. */
export function phases(p: number) {
  return {
    quant: ease(seg(p, 0.03, 0.18)), // the cell grows from 2 px to the row module
    drain: ease(seg(p, 0.24, 0.37)), // colour out, two tones, the sea ruled, vessels as ink
    track: ease(seg(p, 0.45, 0.57)), // underline and label per vessel
    hand: ease(seg(p, 0.63, 0.77)), // labels slide into rows and fade; the account's records take the rows
    book: ease(seg(p, 0.85, 0.96)), // the Owner home
  };
}

/** Reduced motion (DESIGN §Motion): three stills, the photograph, the grid and the book, that swap and never tween. */
export const STILLS = [0, 0.2, 0.8] as const;
export const stillAt = (raw: number) => STILLS[Math.min(STILLS.length - 1, Math.floor(raw * STILLS.length))];

/** Which words the plate holds: 0 the statement (p 0 and .2), then one stage line each for .4, .6, .8, then the product. */
export const stageOf = (p: number) => (p < 0.3 ? 0 : p < 0.5 ? 1 : p < 0.7 ? 2 : p < 0.9 ? 3 : 4);

/** The ledger's row module and column unit, in CSS px (DESIGN: row height 40, columns in multiples of 8). */
export const MODULE = 40;
export const COLUMN = 8;
/** Cell size for quantisation progress q: 2 px at 0, the row module at 1, geometric so each step reads as a step. */
export const cellAt = (q: number) => 2 * Math.pow(MODULE / 2, q);

/** Otsu's threshold on 0..255 luminances: the level that best splits the scene into two tones (sea and land). */
export function otsu(lum: ArrayLike<number>): number {
  const hist = new Array<number>(256).fill(0);
  for (let i = 0; i < lum.length; i++) hist[Math.max(0, Math.min(255, Math.round(lum[i])))]++;
  const total = lum.length;
  let sum = 0;
  for (let i = 0; i < 256; i++) sum += i * hist[i];
  let sumB = 0, wB = 0, best = 0, at = 127;
  for (let t = 0; t < 256; t++) {
    wB += hist[t];
    if (!wB) continue;
    const wF = total - wB;
    if (!wF) break;
    sumB += t * hist[t];
    const between = wB * wF * (sumB / wB - (sum - sumB) / wF) ** 2;
    if (between > best) [best, at] = [between, t];
  }
  return at;
}
export const luma = (r: number, g: number, b: number) => 0.2126 * r + 0.7152 * g + 0.0722 * b;

// ---- the AIS replay -----------------------------------------------------------------------------------------------

/** The recording's bbox (DESIGN §Data and honesty): 54.5–58.0 E × 24.5–27.5 N, as [west, south, east, north]. */
export const BBOX = [54.5, 24.5, 58.0, 27.5] as const;
/** One position report as scripts/ais-record.mjs writes it (one per line of ais.jsonl). */
export type Report = { mmsi: number | string; lat: number; lon: number; sog?: number | null; cog?: number | null; type?: number | null; ts: string | number };
/** [time ms, lon, lat, speed over ground kn, course over ground deg]: only what the report held. */
export type Fix = [number, number, number, number | null, number | null];
export type Vessel = { mmsi: string; type: number | null; fixes: Fix[] };
export type Replay = { synthetic: boolean; source: string; from: number; to: number; vessels: Vessel[] };

const inBox = (lon: number, lat: number) => lon >= BBOX[0] && lon <= BBOX[2] && lat >= BBOX[1] && lat <= BBOX[3];
const r5 = (x: number) => Math.round(x * 1e5) / 1e5;

/** Packs position reports into one track per vessel, keeping at most one fix per `everyMs` per vessel. A ship type the
 *  vessel sent in any of its own reports applies to all of them (the type is static data); nothing else is filled in. */
export function pack(reports: Report[], meta: { synthetic: boolean; source: string }, everyMs = 0): Replay {
  const by = new Map<string, Vessel>();
  let from = Infinity, to = -Infinity;
  for (const r of reports) {
    const t = typeof r.ts === "number" ? r.ts : Date.parse(r.ts);
    if (!Number.isFinite(t) || !Number.isFinite(r.lon) || !Number.isFinite(r.lat) || !inBox(r.lon, r.lat)) continue;
    const id = String(r.mmsi);
    const v = by.get(id) ?? { mmsi: id, type: null, fixes: [] };
    if (typeof r.type === "number" && r.type > 0) v.type = r.type;
    v.fixes.push([t, r5(r.lon), r5(r.lat), r.sog ?? null, r.cog ?? null]);
    by.set(id, v);
    from = Math.min(from, t);
    to = Math.max(to, t);
  }
  const vessels = [...by.values()].map((v) => {
    const fixes = v.fixes.sort((a, b) => a[0] - b[0]).filter((f, i, all) => i === 0 || f[0] - all[i - 1][0] >= everyMs || i === all.length - 1);
    return { ...v, fixes };
  });
  return { ...meta, from: Number.isFinite(from) ? from : 0, to: Number.isFinite(to) ? to : 0, vessels };
}

/** The vessel's last recorded fix at or before t (never an interpolated position), or null before its first. */
export function fixAt(v: Vessel, t: number): Fix | null {
  let hit: Fix | null = null;
  for (const f of v.fixes) {
    if (f[0] > t) break;
    hit = f;
  }
  return hit;
}

/** AIS ship type code to a plain word (ITU-R M.1371); null when the vessel did not send one. */
export function typeWord(code: number | null): string | null {
  if (code === null || code <= 0) return null;
  const d = Math.floor(code / 10);
  if (d === 8) return "tanker";
  if (d === 7) return "cargo";
  if (d === 6) return "passenger";
  if (d === 4) return "high-speed craft";
  if (code === 30) return "fishing";
  if (code === 31 || code === 32 || code === 52) return "tug";
  if (code === 33) return "dredger";
  if (code === 35) return "military";
  if (code === 36) return "sailing";
  if (code === 37) return "pleasure craft";
  if (code === 50) return "pilot";
  if (code === 51) return "search and rescue";
  if (code === 55) return "law enforcement";
  return "other";
}

const hhmm = (ms: number) => new Date(ms).toISOString().slice(11, 16);
/** The tracking label: only recorded fields, joined by two spaces (DESIGN: `MMSI 4…  tanker  11.8 kn  06:41`). */
export function labelOf(v: Vessel, f: Fix): string {
  const type = typeWord(v.type);
  return [`MMSI ${v.mmsi}`, type, f[3] === null ? null : `${f[3].toFixed(1)} kn`, hhmm(f[0])].filter(Boolean).join("  ");
}
export const windowWords = (r: Replay) => ({ from: new Date(r.from).toISOString().slice(0, 16).replace("T", " "), to: hhmm(r.to) });

// ---- the scene and its world file ---------------------------------------------------------------------------------

/** A georeferenced image: its size in px and its world file [A, D, B, E, C, F] in `crs` (EPSG:3857 metres or EPSG:4326
 *  degrees); `src` null = the placeholder drawn from the coast. */
export type Scene = { id: string; date: string | null; width: number; height: number; crs: "EPSG:3857" | "EPSG:4326"; wld: number[]; src: string | null };

const EARTH = 6378137;
const merc = (lon: number, lat: number): [number, number] => [
  (EARTH * lon * Math.PI) / 180,
  EARTH * Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360)),
];

/** lon/lat to the image's pixel coordinates (edges at 0 and width), from its world file (no rotation terms). */
export function toPixel(scene: Scene, lon: number, lat: number): [number, number] {
  const [x, y] = scene.crs === "EPSG:3857" ? merc(lon, lat) : [lon, lat];
  const [A, , , E, C, F] = scene.wld;
  return [(x - C) / A + 0.5, (y - F) / E + 0.5];
}

/** The placeholder's frame: the recording's bbox in Web Mercator at `width` px, so it has the export's aspect ratio. */
export function placeholderScene(width = 1024): Scene {
  const [x0, y1] = merc(BBOX[0], BBOX[3]);
  const [x1, y0] = merc(BBOX[2], BBOX[1]);
  const A = (x1 - x0) / width;
  return { id: "placeholder", date: null, width, height: Math.round((y1 - y0) / A), crs: "EPSG:3857", wld: [A, 0, 0, -A, x0 + A / 2, y1 - A / 2], src: null };
}

/** Parses an ESRI world file's six lines (A D B E C F). */
export const parseWorldFile = (text: string) => text.trim().split(/\s+/).slice(0, 6).map(Number);

/** The strait's bend (between Musandam and Larak): the canvas's focal third, where text never sits. */
export const FOCUS = { lon: 56.3, lat: 26.55 } as const;

// ---- IMF PortWatch (treatment C) ----------------------------------------------------------------------------------

export type PortWatchDay = { date: string; total: number; tanker: number; container: number; dryBulk: number; generalCargo: number; roro: number };
export type PortWatch = { source: string; fetchedAt: string; days: PortWatchDay[] };
