// Ported from LingSiewWin/arcane web/apps/web/src/lib/pixel-avatar.ts (MIT)
// A face for a name, never for a hash (founder, 2026-10-18): an id (an agent's or a payee's address, kept in memory) hashes to
// a seed, a seeded RNG fills a mask, and the filled cells are drawn in the ink token only. Agents get the humanoid mask, payees
// the seal (a ring with a 4 x 4 core). Same id => the same cells, every time; the address itself is never drawn or written.
// Kept from the source: hashSeed, mulberry32, avatarPixels, HUMANOID_MASK. Dropped: AGENT_PALETTE (no colour but ink).
import { createElement } from "react";

export const GRID = 16;

/**
 * Left half (8 columns) of a 16-row humanoid. Mirrored to the right at draw time. 1 = a cell the RNG may fill; 0 = forced
 * empty. Editing this changes the body shape (shoulders, arms, build) for every agent at once.
 */
export const HUMANOID_MASK: ReadonlyArray<ReadonlyArray<0 | 1>> = [
  [0, 0, 0, 0, 0, 0, 0, 0], // 0  top margin
  [0, 0, 0, 0, 0, 1, 1, 1], // 1  top of head
  [0, 0, 0, 0, 1, 1, 1, 1], // 2  head
  [0, 0, 0, 0, 1, 1, 1, 1], // 3  head
  [0, 0, 0, 0, 1, 1, 1, 1], // 4  head
  [0, 0, 0, 0, 0, 1, 1, 1], // 5  chin
  [0, 0, 0, 0, 0, 0, 1, 1], // 6  neck
  [0, 0, 0, 1, 1, 1, 1, 1], // 7  shoulders
  [0, 0, 1, 1, 1, 1, 1, 1], // 8  upper chest / arms
  [0, 1, 1, 1, 1, 1, 1, 1], // 9  mid chest
  [1, 1, 1, 0, 0, 1, 1, 1], // 10 arms separating
  [1, 1, 1, 0, 0, 1, 1, 1], // 11 arms
  [1, 1, 1, 0, 0, 1, 1, 1], // 12 arms
  [1, 1, 0, 0, 0, 1, 1, 1], // 13 lower / legs
  [1, 1, 0, 0, 0, 1, 1, 1], // 14 legs
  [1, 1, 0, 0, 0, 1, 1, 1], // 15 legs
];

/**
 * The payee's seal, all 16 x 16: a ring (cells 5 to 7.5 cells from the centre) around a 4 x 4 core, symmetric left-right
 * and top-bottom. Its left half is what the RNG walks; the right half mirrors it, as for the humanoid.
 */
export const SEAL_MASK: ReadonlyArray<ReadonlyArray<0 | 1>> = [
  [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
  [0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0],
  [0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0],
  [0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0],
  [0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0],
  [0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0],
  [0, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 0],
  [0, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 0],
  [0, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 0],
  [0, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 0],
  [0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0],
  [0, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0],
  [0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0],
  [0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0],
  [0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0],
  [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
];

export type MarkKind = "agent" | "payee";
export interface AvatarPixel {
  x: number;
  y: number;
}

/** FNV-1a hash of a string to a 32-bit unsigned int. */
export function hashSeed(id: string): number {
  let h = 2166136261;
  for (let i = 0; i < id.length; i++) {
    h ^= id.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** mulberry32 seeded PRNG → () => float in [0, 1). */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return function () {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const cache = new Map<string, AvatarPixel[]>();

/**
 * Deterministic cells for an id (an address or any stable string): walks the mask's left half, fills cells by a per-id
 * density, and mirrors each cell to the right half. The source's colour draw is still taken (and ignored), so an agent's
 * cells match the ones arcane draws for the same id.
 */
export function avatarPixels(id: string, kind: MarkKind = "agent"): AvatarPixel[] {
  const key = `${kind}:${id}`;
  const cached = cache.get(key);
  if (cached) return cached;

  const mask = kind === "agent" ? HUMANOID_MASK : SEAL_MASK;
  const rnd = mulberry32(hashSeed(id));
  const density = 0.62 + rnd() * 0.22; // 0.62–0.84 fill probability
  const pixels: AvatarPixel[] = [];

  for (let y = 0; y < GRID; y++) {
    for (let x = 0; x < GRID / 2; x++) {
      if (mask[y][x] !== 1) continue;
      if (rnd() > density) continue;
      rnd(); // the source's colour draw: no palette here, the ink token is the only colour
      pixels.push({ x, y });
      pixels.push({ x: GRID - 1 - x, y }); // mirror
    }
  }

  cache.set(key, pixels);
  return pixels;
}

/** The cells as one SVG path: a 1 x 1 square per cell. */
export const markPath = (pixels: readonly AvatarPixel[]) => pixels.map((p) => `M${p.x} ${p.y}h1v1h-1z`).join("");

/**
 * `<Mark id kind size />`: 16 x 16 cells as one inline SVG in the ink token (currentColor on --ink), 24 px in a row, 40 px in a
 * sheet. Decorative: the name beside it carries the meaning, so it is hidden from assistive tech.
 */
export function Mark({ id, kind, size = 24 }: { id: string; kind: MarkKind; size?: 24 | 40 }) {
  return createElement(
    "svg",
    {
      width: size,
      height: size,
      viewBox: `0 0 ${GRID} ${GRID}`,
      "aria-hidden": true,
      focusable: false,
      shapeRendering: "crispEdges",
      className: "shrink-0 text-(--ink)",
    },
    createElement("path", { d: markPath(avatarPixels(id, kind)), fill: "currentColor" }),
  );
}
