// Every word the hero shows (DESIGN §The hero, the words column; §Copy). No string lives in the components.

export const HERO = {
  statement: "Trade moves before money does.",
  body: "Every ship through this strait is a promise someone has to keep. Tameion is the book your agents keep, in dollars that hold still.",
  open: "Open the book",
  tryIt: "Try it in 5 minutes",
  stages: [
    "The sea becomes a page.",
    "Every vessel is seen, named and timed.",
    "Your agents pass through a strait too. Every call, every bill, every refusal: one book.",
  ],
  // the hand-over table's columns, in order (DESIGN: date · who · amount · why · state)
  columns: ["Date", "Who", "Amount", "Why", "State"],
  emptyBook: "The demo account has no payments yet.",
  unreadBook: "The demo account could not be read just now.",
  bookLabel: "The demo account, as its owner sees it",
  canvasLabel: "The Strait of Hormuz becomes a ledger page as you scroll",
} as const;

/** The caption names the scene, the AIS window and the demo account (DESIGN §Data and honesty). Sentences, not dot-joined meta. */
export const CAPTION = {
  sceneA: (date: string) => `Strait of Hormuz from Sentinel-2 L2A, ${date}. Contains modified Copernicus Sentinel data (2026).`,
  placeholderA: "Placeholder scene: Natural Earth coast in two tones until the Sentinel-2 export lands.",
  sceneB: "Strait of Hormuz, Natural Earth 10 m coast (public domain).",
  sceneC: (date: string) => `Transits through the Strait of Hormuz on ${date}, IMF PortWatch.`,
  ais: (from: string, to: string) => `Vessels: aisstream.io recording, ${from} to ${to} UTC.`,
  aisSynthetic: "Vessels: synthetic replay for layout, not a recording.",
  aisNone: "Vessels: no recording yet.",
  rows: "Rows: demo account.",
} as const;

/** Treatment C's figures (the day's transits beside our AIS window). */
export const FIGURES = {
  transits: (date: string) => `transits through the strait on ${date}, IMF PortWatch`,
  window: (synthetic: boolean) => (synthetic ? "vessels in the synthetic replay" : "vessels reporting in our AIS window"),
  types: { tanker: "tanker", container: "container", dryBulk: "dry bulk", generalCargo: "general cargo", roro: "ro-ro" },
  source: "IMF PortWatch",
  unit: "transits",
} as const;
