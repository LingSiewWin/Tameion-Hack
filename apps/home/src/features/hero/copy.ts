// Every word the hero shows (DESIGN §The hero, the words column; §Copy). No string lives in the components.

export const HERO = {
  statement: "Trade moves before money does.",
  body: "Every ship through this strait is a promise someone has to keep. Tami is the book your agents keep, in dollars that hold still.",
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

/** The caption names the scene and its date, what the vessels are, and the demo account (DESIGN §Data and honesty).
 *  Sentences, not dot-joined meta. Founder, 2026-10-18 08:44: the hero is design and its data prebuilt, so nothing here
 *  calls the ships or the scene live, recorded, tracked or real. */
export const CAPTION = {
  scene: (credit: string, date: string) => `Strait of Hormuz from ${credit}, ${date}.`,
  placeholderA: "Placeholder scene: Natural Earth coast in two tones until the Sentinel-2 export lands.",
  sceneB: "Strait of Hormuz, Natural Earth 10 m coast (public domain).",
  sceneC: (date: string) => `Transits through the Strait of Hormuz on ${date}, IMF PortWatch.`,
  ais: (from: string, to: string) => `Vessels: aisstream.io position reports, ${from} to ${to} UTC.`,
  aisIllustrative: "Vessels: illustrative, drawn on the strait's traffic lanes.",
  rows: "Rows: demo account.",
} as const;

/** Treatment C's figures: the day's transits; beside them a count of vessels only for aisstream.io position reports. */
export const FIGURES = {
  transits: (date: string) => `transits through the strait on ${date}, IMF PortWatch`,
  window: "vessels in the AIS window",
  types: { tanker: "tanker", container: "container", dryBulk: "dry bulk", generalCargo: "general cargo", roro: "ro-ro" },
  source: "IMF PortWatch",
  unit: "transits",
} as const;
