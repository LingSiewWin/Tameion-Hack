// The scroll-bound canvas (DESIGN §The hero): photograph -> grid -> drain -> tracking -> hand-over. Treatment A starts
// from the scene (or its two-tone placeholder), treatment B from an ink line chart of the coast. Colours come from the
// tokens (tokens.ts) of the view the page is in; nothing is drawn that the scene or the recording does not hold.
import { palette, type View } from "../../tokens.ts";
import { cellAt, COLUMN, FOCUS, fixAt, handBeats, labelOf, lerp, luma, MODULE, otsu, phases, seg, toPixel, type Replay, type Scene, type Vessel } from "./model.ts";

export type Rect = { x: number; y: number; w: number; h: number };
export type Layout = { W: number; H: number; plate: Rect; rows: { x: number; y: number } };
type Coast = { features: { geometry: { coordinates: number[][][] } }[] };
type Label = { v: Vessel; text: string; x: number; y: number; w: number };

const channels = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
const meets = (a: Rect, b: Rect) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

/** The two-tone placeholder (A until the Sentinel-2 export lands): coast lines drawn, the sea flood-filled from two
 *  seeds (the Gulf of Oman and the Gulf), everything else land. Real geography, labelled placeholder in the caption. */
export function drawPlaceholder(scene: Scene, coast: Coast): HTMLCanvasElement {
  const c = document.createElement("canvas");
  [c.width, c.height] = [scene.width, scene.height];
  const g = c.getContext("2d", { willReadFrequently: true })!;
  g.fillStyle = palette.owner.plate;
  g.fillRect(0, 0, c.width, c.height);
  strokeCoast(g, scene, coast, palette.owner.ink, 2);
  const img = g.getImageData(0, 0, c.width, c.height);
  const d = img.data, W = c.width, H = c.height;
  const sea = new Uint8Array(W * H);
  const open = (i: number) => !sea[i] && d[i * 4] >= 200; // a coast pixel stops the fill
  // scanline fill: one run per row segment, one seed per run above and below (a pixel-by-pixel fill blocked the main
  // thread for a second on a throttled phone)
  const stack: number[] = [];
  for (const [lon, lat] of [[57.6, 25.0], [55.0, 26.6]]) {
    const [x, y] = toPixel(scene, lon, lat).map(Math.floor);
    stack.push(y * W + x);
  }
  while (stack.length) {
    const i = stack.pop()!;
    if (!open(i)) continue;
    const row = i - (i % W);
    let x0 = i - row, x1 = x0;
    while (x0 > 0 && open(row + x0 - 1)) x0--;
    while (x1 < W - 1 && open(row + x1 + 1)) x1++;
    for (let x = x0; x <= x1; x++) sea[row + x] = 1;
    for (const next of [row - W, row + W]) {
      if (next < 0 || next >= W * H) continue;
      for (let x = x0; x <= x1; x++) if (open(next + x) && (x === x0 || !open(next + x - 1))) stack.push(next + x);
    }
  }
  const [land, water] = [channels(palette.owner.rule), channels(palette.agent.paper)];
  for (let i = 0, j = 0; i < W * H; i++, j += 4) {
    const t = sea[i] ? water : land;
    d[j] = t[0];
    d[j + 1] = t[1];
    d[j + 2] = t[2];
    d[j + 3] = 255;
  }
  g.putImageData(img, 0, 0);
  return c;
}

function strokeCoast(g: CanvasRenderingContext2D, scene: Scene, coast: Coast, color: string, width: number, map = (x: number, y: number) => [x, y]) {
  g.strokeStyle = color;
  g.lineWidth = width;
  g.lineJoin = "round";
  g.beginPath();
  for (const f of coast.features)
    for (const line of f.geometry.coordinates)
      line.forEach(([lon, lat], i) => {
        const [px, py] = toPixel(scene, lon, lat);
        const [x, y] = map(px, py);
        if (i) g.lineTo(x, y);
        else g.moveTo(x, y);
      });
  g.stroke();
}

/** Treatment B's starting picture: the coast as an ink line on paper, at the scene's frame. */
export function drawChart(scene: Scene, coast: Coast, view: View): HTMLCanvasElement {
  const c = document.createElement("canvas");
  [c.width, c.height] = [scene.width, scene.height];
  const g = c.getContext("2d")!;
  g.fillStyle = palette[view].paper;
  g.fillRect(0, 0, c.width, c.height);
  strokeCoast(g, scene, coast, palette[view].ink, 1.5);
  return c;
}

export type StageInput = {
  treatment: "A" | "B";
  scene: Scene;
  source: CanvasImageSource | null;
  replay: Replay | null;
  view: View;
  monoFont: string;
  /** C: no picture; the page itself is ruled as the rows arrive */
  ruled?: boolean;
};

export function createStage(canvas: HTMLCanvasElement) {
  const ctx = canvas.getContext("2d")!;
  const small = document.createElement("canvas");
  const sctx = small.getContext("2d", { willReadFrequently: true })!;
  let layout: Layout | null = null;
  let labels: Label[] = [];
  let labelsFor: unknown = null;

  /** Cover-fit the scene with the strait's bend at the focal third, clear of the plate. */
  function fit(scene: Scene, L: Layout) {
    const s = Math.max(L.W / scene.width, L.H / scene.height);
    const [fx, fy] = toPixel(scene, FOCUS.lon, FOCUS.lat);
    const wide = L.W >= 640;
    const want = [wide ? L.W * 0.7 : L.W * 0.5, wide ? L.H * 0.45 : L.H * 0.28];
    const ox = Math.min(0, Math.max(L.W - scene.width * s, want[0] - fx * s));
    const oy = Math.min(0, Math.max(L.H - scene.height * s, want[1] - fy * s));
    return { s, ox, oy };
  }

  function resize(L: Layout) {
    layout = L;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    canvas.width = Math.round(L.W * dpr);
    canvas.height = Math.round(L.H * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    labelsFor = null;
  }

  /** Up to six labels (four on a phone) that clear the plate and each other, chosen once per layout at the
   *  tracking moment, so a label never jumps to another vessel while scrolling. */
  function pickLabels(inp: StageInput, L: Layout, t: number, place: (lon: number, lat: number) => [number, number]) {
    ctx.font = `400 13px ${inp.monoFont}`;
    const out: Label[] = [];
    const max = L.W >= 640 ? 6 : 4;
    const vs = [...(inp.replay?.vessels ?? [])].sort((a, b) => (fixAt(b, t)?.[3] ?? 0) - (fixAt(a, t)?.[3] ?? 0));
    for (const v of vs) {
      if (out.length >= max) break;
      const f = fixAt(v, t);
      if (!f) continue;
      const [x, y] = place(f[1], f[2]);
      const text = labelOf(v, f);
      const lx = Math.floor(x / MODULE) * MODULE; // the label starts on a grid column
      const gy = Math.floor(y / COLUMN) * COLUMN; // the cluster's top
      const w = Math.ceil(ctx.measureText(text).width / COLUMN) * COLUMN;
      const r = { x: lx, y: gy, w, h: MODULE };
      if (r.x < 0 || r.x + r.w > L.W || r.y < 0 || r.y + r.h > L.H || meets(r, L.plate)) continue;
      if (out.some((o) => meets(r, { x: o.x, y: o.y, w: o.w, h: MODULE }))) continue;
      out.push({ v, text, x: lx, y: gy, w });
    }
    return out;
  }

  function draw(p: number, inp: StageInput) {
    const L = layout;
    if (!L) return;
    const tk = palette[inp.view];
    const ph = phases(p);
    const { s, ox, oy } = fit(inp.scene, L);
    const place = (lon: number, lat: number): [number, number] => {
      const [px, py] = toPixel(inp.scene, lon, lat);
      return [ox + px * s, oy + py * s];
    };
    const sat = seg(ph.drain, 0, 0.5);
    const lev = seg(ph.drain, 0.4, 1);
    ctx.imageSmoothingEnabled = true;
    ctx.fillStyle = tk.paper;
    ctx.fillRect(0, 0, L.W, L.H);

    // 1. the picture, quantised: drawn small (area average), scaled back up nearest-neighbour on the grid
    if (inp.source) {
      const c = cellAt(ph.quant);
      if (c < 2.5 && ph.drain === 0) {
        ctx.drawImage(inp.source, ox, oy, inp.scene.width * s, inp.scene.height * s);
      } else {
        const cols = Math.ceil(L.W / c), rows = Math.ceil(L.H / c);
        if (small.width !== cols || small.height !== rows) [small.width, small.height] = [cols, rows];
        sctx.setTransform(1 / c, 0, 0, 1 / c, 0, 0);
        sctx.imageSmoothingEnabled = true;
        sctx.imageSmoothingQuality = "high";
        sctx.drawImage(inp.source, ox, oy, inp.scene.width * s, inp.scene.height * s);
        let sea: Uint8Array | null = null;
        if (ph.drain > 0) {
          // 2. the drain: saturation to zero, then levels to two tones (land = paper, sea = plate, ruled below)
          const img = sctx.getImageData(0, 0, cols, rows);
          const d = img.data;
          const lum = new Uint8Array(cols * rows);
          for (let i = 0; i < lum.length; i++) lum[i] = luma(d[i * 4], d[i * 4 + 1], d[i * 4 + 2]);
          const thr = inp.treatment === "A" ? otsu(lum) : 256; // B: the chart has no land fill; the whole page is ruled
          sea = new Uint8Array(cols * rows);
          const [land, water] = [channels(tk.paper), channels(tk.plate)];
          for (let i = 0; i < lum.length; i++) {
            sea[i] = lum[i] < thr ? 1 : 0;
            const tone = sea[i] ? water : land;
            for (let k = 0; k < 3; k++) {
              const grey = lerp(d[i * 4 + k], lum[i], sat);
              d[i * 4 + k] = lerp(grey, tone[k], inp.treatment === "A" ? lev : lev * 0.85);
            }
          }
          sctx.putImageData(img, 0, 0);
        }
        ctx.imageSmoothingEnabled = false;
        ctx.drawImage(small, 0, 0, cols, rows, 0, 0, cols * c, rows * c);
        // the sea becomes ruled paper: a rule along the foot of every sea cell at the row module
        if (sea && lev > 0) {
          ctx.globalAlpha = lev;
          ctx.fillStyle = tk.rule;
          for (let r = 0; r < rows; r++)
            for (let q = 0; q < cols; q++) if (sea[r * cols + q]) ctx.fillRect(q * c, (r + 1) * c - 1, c, 1);
          ctx.globalAlpha = 1;
        }
      }
    }

    // the hand-over turns the whole stage into one ruled page (C: ruled from the drain on)
    const page = inp.ruled ? Math.max(lev, ph.hand) : ph.hand;
    if (page > 0) {
      ctx.globalAlpha = page;
      ctx.fillStyle = tk.paper;
      ctx.fillRect(0, 0, L.W, L.H);
      ctx.fillStyle = tk.rule;
      for (let y = MODULE - 1; y < L.H; y += MODULE) ctx.fillRect(0, y, L.W, 1);
      ctx.globalAlpha = 1;
    }

    // 3. vessels from the recording: marks (B, from the start) that drain to ink clusters on the 8 px columns
    const rep = inp.replay;
    if (!rep) return;
    const t = lerp(rep.from, rep.to, seg(p, 0.26, 0.82));
    const shown = inp.treatment === "B" ? 1 : lev;
    if (shown > 0 && ph.hand < 1) {
      ctx.globalAlpha = shown * (1 - seg(ph.hand, 0, 0.6));
      for (const v of rep.vessels) {
        const f = fixAt(v, t);
        if (!f) continue;
        const [x, y] = place(f[1], f[2]);
        if (lev === 0) {
          ctx.fillStyle = tk.transit;
          ctx.fillRect(Math.round(x) - 2, Math.round(y) - 2, 4, 4);
        } else {
          const gx = Math.floor(x / COLUMN) * COLUMN, gy = Math.floor(y / COLUMN) * COLUMN;
          const n = v.type !== null && v.type >= 80 && v.type < 90 ? 3 : v.type !== null && v.type >= 70 && v.type < 80 ? 2 : 1;
          ctx.fillStyle = tk.ink;
          ctx.fillRect(gx, gy, n * COLUMN, COLUMN);
        }
      }
      ctx.globalAlpha = 1;
    }

    // 4. tracking: a thin underline and the recording's own fields, on the grid's columns; 5. they slide into rows and fade
    if (ph.track > 0) {
      const key = [L.W, L.H, rep, inp.monoFont];
      if (labelsFor === null || (labelsFor as unknown[]).some((k, i) => k !== key[i])) {
        labels = pickLabels(inp, L, lerp(rep.from, rep.to, seg(0.6, 0.26, 0.82)), place);
        labelsFor = key;
      }
      ctx.font = `400 13px ${inp.monoFont}`;
      ctx.textBaseline = "alphabetic";
      const { slide, labels: shownLabels } = handBeats(ph.hand);
      ctx.globalAlpha = ph.track * shownLabels;
      labels.forEach((lb, i) => {
        const f = fixAt(lb.v, t);
        const text = f ? labelOf(lb.v, f) : lb.text;
        const top = L.rows.y + i * MODULE;
        const x = lerp(lb.x, L.rows.x, slide);
        // the underline sits under the cluster, the label under it; in a row the underline becomes the row's rule
        const under = lerp(lb.y + COLUMN + 3, top + MODULE - 1, slide);
        const base = lerp(lb.y + COLUMN + 18, top + 26, slide);
        ctx.fillStyle = tk.ink;
        ctx.fillRect(x, under, lb.w * ph.track, 1);
        ctx.fillText(text, x, base);
      });
      ctx.globalAlpha = 1;
    }
  }

  return { resize, draw };
}
