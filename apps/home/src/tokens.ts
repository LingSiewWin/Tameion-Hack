// Every value DESIGN.md names, typed (the founder's design, D-HOME-1 "The strait becomes the book"; .21st/design.json reads
// this file). src/tokens.css carries the same values as CSS variables; scripts/contrast.mjs fails the build when the two
// files disagree or a text pair falls under 4.5:1, and scripts/tokens-lint.mjs fails it on a colour literal anywhere else.

export type View = "owner" | "agent";
export const VIEWS: readonly View[] = ["owner", "agent"];
/** Where the capsule remembers the view, per device. */
export const VIEW_KEY = "tameion.home.view";
/** Inline in <head> before paint (src/app/layout.tsx): a remembered Agent view never flashes Owner. Storage may throw
 *  in a private window; Owner then stays. Kept here, not in lib/view.ts, because the server layout cannot import that. */
export const VIEW_BOOT = `try{if(localStorage.getItem(${JSON.stringify(VIEW_KEY)})==="agent")document.documentElement.dataset.view="agent"}catch(e){}`;

/** The palette per view, keyed by CSS variable name (without `--`). Owner = light, "land"; Agent = dark, "sea". */
export const palette = {
  owner: {
    paper: "#EEF2EA",
    plate: "#FFFFFF",
    "plate-2": "#E4EAE1",
    ink: "#000000",
    "ink-2": "#5B625B",
    rule: "#D5DCD2",
    attention: "#B3261E",
    transit: "#3FB8B0",
    // one step darker than DESIGN's #2F7A00 (4.39:1 on plate-2) so every pair reaches 4.5:1 (PM ruling 07:2x, founder may veto)
    up: "#2E7800",
    down: "#B3261E",
    scrim: "#000000B3",
    skeleton: "#DEE4DA",
  },
  agent: {
    paper: "#0E1B22",
    plate: "#15242C",
    "plate-2": "#1D2F38",
    ink: "#EEF2EA",
    "ink-2": "#A9B4AD",
    rule: "#2A3C45",
    attention: "#FF6B5E",
    transit: "#3FB8B0",
    up: "#9FD66A",
    down: "#FF6B5E",
    scrim: "#000000B3",
    skeleton: "#22343D",
  },
} as const satisfies Record<View, Record<string, string>>;

export type ColorToken = keyof (typeof palette)["owner"];

/** Text and the surfaces it sits on: each pair must reach 4.5:1 in both views (scripts/contrast.mjs). */
export const textPairs: readonly { text: ColorToken; on: ColorToken }[] = [
  ...(["ink", "ink-2", "attention", "up", "down"] as const).flatMap((text) =>
    (["paper", "plate", "plate-2"] as const).map((on) => ({ text, on })),
  ),
  { text: "paper", on: "ink" }, // the one primary action
];

/** The sheet is the only raised thing (and the capsule while it slides). */
export const sheetShadow = "0 -4px 16px #0000001F";

/** Values shared by both views, keyed by CSS variable name (without `--`). */
export const shared = {
  "radius-mark": "4px",
  "radius-inner": "8px",
  "radius-button": "12px",
  "radius-plate": "16px",
  "radius-sheet": "24px",
  "radius-capsule": "999px",
  "space-1": "4px",
  "space-2": "8px",
  "space-3": "12px",
  "space-4": "16px",
  "space-5": "24px",
  "space-6": "32px",
  "space-7": "48px",
  "space-8": "64px",
  "gutter-narrow": "20px",
  "gutter-wide": "32px",
  "content-max": "1120px",
  measure: "68ch",
  touch: "44px",
  button: "52px",
  "button-compact": "44px",
  "drawer-width": "400px",
  "press-in": "100ms",
  "press-out": "160ms",
  "press-scale": "0.98",
  crossfade: "200ms",
  roll: "160ms",
  "sheet-in": "320ms",
  "sheet-out": "200ms",
  "ease-sheet": "cubic-bezier(0.32, 0.72, 0, 1)",
} as const;

/** Type roles: size / line in px, weight; base 16, ratio 1.25 (13 16 20 25 31 39 49 61). */
export const typeRoles = {
  heroStatement: { size: 49, line: 56, weight: 500, narrow: { size: 31, line: 38 } },
  authority: { size: 31, line: 38, weight: 500 },
  balance: { size: 39, line: 44, weight: 500 },
  sheetTitle: { size: 25, line: 32, weight: 500 },
  rowTitle: { size: 16, line: 24, weight: 600 },
  row: { size: 16, line: 24, weight: 400 },
  meta: { size: 13, line: 18, weight: 400 },
  monoDense: { size: 13, line: 20, weight: 400 },
  monoRead: { size: 15, line: 22, weight: 400 },
} as const;
/** The CSS class of each role (src/tokens.css). */
export const roleClass: Record<keyof typeof typeRoles, string> = {
  heroStatement: "hero-statement",
  authority: "authority",
  balance: "balance",
  sheetTitle: "sheet-title",
  rowTitle: "row-title",
  row: "row",
  meta: "meta",
  monoDense: "mono-dense",
  monoRead: "mono-read",
};

/** Under this width the hero statement steps down to its narrow size, and the gutter is the narrow one. */
export const NARROW_PX = 640;
/** From this width a sheet opens as a right drawer. */
export const DRAWER_PX = 1024;
/** A skeleton, once shown, stays at least this long (DESIGN §States). */
export const SKELETON_MIN_MS = 300;
