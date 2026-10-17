// Every value DESIGN.md names, typed (DESIGN Rule 2). src/tokens.css carries the same values as CSS variables;
// scripts/contrast.mjs fails the build when the two files disagree or a text pair falls under 4.5:1.

export type View = "owner" | "agent";
export const VIEWS: readonly View[] = ["owner", "agent"];
/** Where the capsule remembers the view, per device (DESIGN §Page grammar). */
export const VIEW_KEY = "tameion.home.view";
/** Inline in <head> before paint (src/app/layout.tsx): a remembered Agent view never flashes Owner. Storage may throw
 *  in a private window; Owner then stays. Kept here, not in lib/view.ts, because the server layout cannot import that. */
export const VIEW_BOOT = `try{if(localStorage.getItem(${JSON.stringify(VIEW_KEY)})==="agent")document.documentElement.dataset.view="agent"}catch(e){}`;

/** The colour tokens per view, keyed by their CSS variable name (without `--`). */
export const palette = {
  owner: {
    paper: "#EEF2EA",
    plate: "#FFFFFF",
    ink: "#101410",
    "text-2": "#5B625B",
    rule: "#D5DCD2",
    red: "#B3261E",
    accent: "#1E5F46",
    up: "#1E5F46",
    down: "#7A4B1E",
  },
  agent: {
    paper: "#0F1511",
    plate: "#161D18",
    ink: "#E8EEE6",
    "text-2": "#9AA69C",
    rule: "#2A332C",
    red: "#E5534B",
    accent: "#7FD1A8",
    // DESIGN gives --up/--down "same" for night; the day values read 2.4:1 on night paper, under the 4.5:1 rule,
    // so night takes the night accent for up (as day up is the day accent) and day down's hue lifted to pass.
    up: "#7FD1A8",
    down: "#D9A06A",
  },
} as const satisfies Record<View, Record<string, string>>;

export type ColorToken = keyof (typeof palette)["owner"];

/** Text colours and the surfaces they sit on: every pair must reach 4.5:1 in both views (scripts/contrast.mjs). */
export const textPairs: readonly { text: ColorToken; on: ColorToken }[] = [
  ...(["ink", "text-2", "red", "accent", "up", "down"] as const).flatMap((text) =>
    (["paper", "plate"] as const).map((on) => ({ text, on })),
  ),
  { text: "plate", on: "accent" }, // the active capsule segment
  { text: "plate", on: "red" }, // a filled Stop
  { text: "paper", on: "ink" }, // an inverted plate
];

/** The sheet is the only raised thing (DESIGN §Palette). */
export const sheetShadow: Record<View, string> = { owner: "0 -8px 24px rgba(16,20,16,.12)", agent: "none" };

/** Values shared by both views, keyed by their CSS variable name (without `--`). */
export const shared = {
  "radius-plate": "8px",
  "radius-capsule": "999px",
  "radius-mark": "6px",
  "space-1": "4px",
  "space-2": "8px",
  "space-3": "12px",
  "space-4": "16px",
  "space-5": "24px",
  "space-6": "32px",
  "space-7": "48px",
  "gutter-narrow": "16px",
  "gutter-wide": "24px",
  "content-owner": "720px",
  touch: "44px",
  cta: "56px",
  crossfade: "200ms",
  pulse: "1.2s",
} as const;

/** Type roles (DESIGN §Typography roles): size / line in px, weight. The Agent view sets every role to `mono`. */
export const typeRoles = {
  statementLine: { size: 44, line: 48, weight: 500, narrow: { size: 32, line: 36 } },
  balance: { size: 36, line: 40, weight: 500 },
  rowTitle: { size: 16, line: 22, weight: 600 },
  row: { size: 15, line: 22, weight: 400 },
  meta: { size: 13, line: 18, weight: 400 },
  mono: { size: 13, line: 20, weight: 400 },
} as const;

/** Under this width the statement line steps down to its narrow size. */
export const NARROW_PX = 390;
/** A skeleton, once shown, stays at least this long (DESIGN §States). */
export const SKELETON_MIN_MS = 300;
