// Every word the Agent view shows (DESIGN Rules: no hard-coded strings in components). Field names are the service's
// own (inbox.balance, status.rate …): the Agent view labels each figure by where it comes from.

export const SOURCES = { status: "The status", inbox: "The inbox", symbolon: "The records" } as const;

export const copy = {
  title: "tameion top",
  mode: (m: string) => `mode ${m}`,
  asOf: (t: string) => `as of ${t}`,
  didNotAnswer: (what: string, since: string | null) => `${what} did not answer${since ? `; shown as of ${since}` : ""}.`,
  tryAgain: "Try again",
  details: "What the service said",
  recordsMissing: "The records did not load; the figures above still stand.",
  none: "–",
  top: {
    region: "The account now",
    table: "The account now, by unit",
    unit: "Unit",
    source: "source",
    usdc: "USDC",
    held: { head: "Held", source: "inbox.balance" },
    reserved: { head: "Reserved", source: (days?: number) => `inbox.runway.horizonTotal${days ? ` (${days} d)` : ""}` },
    consumed: { head: "Consumed", source: "inbox.paidTotal" },
    rate: { head: "Rate / min", source: (days?: number) => `status.rate${days ? ` (${days} d)` : ""} ÷ 1440` },
    runway: { head: "Runway", source: "status.runway.byRateDays" },
    shortfall: { head: "First shortfall", source: "status.runway.firstShortfallDays" },
    days: (n: number) => `${n} d`,
    noFigure: (fields: string[]) => `– the service gave no figure: ${fields.join(", ")}.`,
  },
  stream: {
    records: (n: number) => `Records ${n}`,
    countsLabel: "Records by how many slots stand behind them",
    counts: { single: "Single", double: "Double", triple: "Triple", unsigned: "Unsigned" },
    unread: (n: number) => `Anchoring not read this time for ${n} ${n === 1 ? "root" : "roots"}; their witness shows ?.`,
    locked:
      "This account is read with its owner's device or a read key. Anyone can check each record's leaf; its fields and slots stay with them.",
    empty: "No records yet. The agent writes one on each pass.",
    region: "Records, newest first",
    head: ["When", "Seq", "Kind", "Verdict", "Counterparty", "Qty", "Unit", "Agent", "Seller", "Witness", "Leaf", "Anchor"],
    headLocked: ["Seq", "Leaf", "Root"],
    kinds: {
      SETTLEMENT: "settlement",
      DECISION: "decision",
      FLOWCHECK: "flow check",
      METERCHECK: "meter check",
      PRICETABLE: "price table",
    } as Record<string, string>,
    refused: "refused",
    refusedCode: "block",
    slots: { agent: "Agent", counterparty: "Seller", witness: "Witness" },
    mark: {
      verified: (alg: string, keyId: string) => `verified, ${alg}, ${keyId}`,
      unread: "the anchoring could not be read this time",
      none: "none",
    },
    detail: {
      root: "Root",
      leaf: "Leaf",
      head: ["Slot", "Alg", "Key id", "Verified"],
      slotNames: { agent: "agent", counterparty: "counterparty", witness: "witness" },
      yes: "yes",
      no: "no",
      unread: "not read this time",
      anchorTx: "Anchor tx",
      paymentTx: "Payment tx",
      note: "Note",
    },
  },
} as const;
