// The Agent view's contract and the logic that has to be right before anything is drawn, all pure
// so `test/agent-feed.test.mts` runs it without a browser: the frame the service sends once a second
// (`GET /api/desk/live`), a parser for the Server-Sent Events it travels in (a network read cuts a frame
// anywhere), the three bands of the margin meter, how a number is printed in a column, and how an
// account event is put in words. No React, no I/O, no `@/` imports.

/** Nothing arrives for this long: the numbers on screen are no longer the venue's. */
export const STALE_AFTER_MS = 5000;
/** The venue's own reading has not changed for this long (the service never sends one older than 2 s). */
export const VENUE_STALE_AFTER_MS = 3000;

// ---- the frame ---------------------------------------------------------------------------------
// Decimals travel as strings. `null` is a field the frame did not carry: the page shows a dash, it
// never fills a number in.
export type Position = { coin: string; size: string | null; entry: string | null; mark: string | null; pnl: string | null };
export type Order = {
  at: number | null;
  coin: string;
  side: string | null;
  size: string | null;
  oid: string | null;
  /** the venue's own position read again after the order: it matched the order or it did not */
  readBack: "match" | "mismatch" | null;
};
export type Frame = {
  /** unix seconds */
  at: number;
  /** the venue's own clock for the reading, ms; null when the venue's numbers are not sent */
  venueTime: number | null;
  /** when the service last read the funding payments, unix seconds (they are read at the hour, not each second) */
  fundingReadAt: number | null;
  venue: string | null;
  /** `[]` is a flat account; `null` is "not known right now" (the venue did not answer in time, see `venueError`) */
  positions: Position[] | null;
  /** why the venue's numbers are missing, when they are: the service never sends one older than 2 s */
  venueError: string | null;
  marginUsed: string | null;
  maintenanceMarginUsed: string | null;
  accountValue: string | null;
  /** account value ÷ maintenance margin, as the service computed it (docs/106 S3-D1); null with no position */
  kRatio: number | null;
  /** the desk's own rule: hold at least this many times the maintenance margin */
  kPolicy: number | null;
  /** USDC; positive = the desk received, negative = it paid (crates/hedge/src/book.rs `funding`) */
  fundingLastHour: string | null;
  funding24h: string | null;
  orders: Order[];
  halt: { reason: string; at: number | null } | null;
  passLatencyMs: number | null;
  anchor: { tx: string; at: number | null } | null;
  lastPassAt: number | null;
};

const str = (v: unknown): string | null =>
  typeof v === "string" && v.trim() !== "" ? v : typeof v === "number" && Number.isFinite(v) ? String(v) : null;
const num = (v: unknown): number | null => {
  const n = typeof v === "number" ? v : typeof v === "string" && v.trim() !== "" ? Number(v) : NaN;
  return Number.isFinite(n) ? n : null;
};
/** Unix seconds; a stamp in milliseconds (past year 33658 in seconds) is brought down to seconds. */
const secs = (v: unknown): number | null => {
  const n = num(v);
  return n == null ? null : n > 1e12 ? Math.round(n / 1000) : n;
};
/** Milliseconds; a stamp in seconds is taken up to milliseconds. */
const msec = (v: unknown): number | null => {
  const n = num(v);
  return n == null ? null : n < 1e12 ? n * 1000 : n;
};
const obj = (v: unknown): Record<string, unknown> | null => (typeof v === "object" && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : null);

/** What the network sent, as a frame the page can draw, or the reason it cannot. A frame that says
 *  nothing about a halt is not read as "no halt": only an explicit `null` is. */
export function readFrame(raw: unknown): { frame: Frame } | { error: string } {
  const r = obj(raw);
  if (!r) return { error: "the feed sent something that is not a frame" };
  const at = secs(r.at);
  if (at == null) return { error: "the frame has no time (at)" };
  if (r.positions != null && !Array.isArray(r.positions)) return { error: "the frame's positions are not a list" };
  if (!("halt" in r)) return { error: "the frame does not say whether the desk is halted (halt)" };
  const positions = Array.isArray(r.positions)
    ? r.positions.flatMap((p): Position[] => {
        const o = obj(p);
        const coin = str(o?.coin);
        return o && coin ? [{ coin, size: str(o.size), entry: str(o.entry), mark: str(o.mark), pnl: str(o.pnl) }] : [];
      })
    : null;
  const orders = (Array.isArray(r.lastOrders) ? r.lastOrders : []).flatMap((p): Order[] => {
    const o = obj(p);
    const coin = str(o?.coin);
    if (!o || !coin) return [];
    const rb = o.readBack;
    return [{ at: secs(o.at), coin, side: str(o.side), size: str(o.size), oid: str(o.oid), readBack: rb === "match" || rb === "mismatch" ? rb : null }];
  });
  // Anything but null or false on `halt` is a halt: an odd shape fails toward showing the stop.
  const h = r.halt;
  const halt = h == null || h === false ? null : obj(h) ? { reason: str(obj(h)?.reason) ?? "no reason given", at: secs(obj(h)?.at) } : { reason: str(h) ?? "halted", at: null };
  const a = obj(r.lastAnchor);
  const tx = str(a?.tx);
  return {
    frame: {
      at,
      venueTime: msec(r.venueTime),
      fundingReadAt: secs(r.fundingReadAt),
      venue: str(r.venue),
      positions,
      venueError: str(r.venueError),
      marginUsed: str(r.marginUsed),
      maintenanceMarginUsed: str(r.maintenanceMarginUsed),
      accountValue: str(r.accountValue),
      kRatio: num(r.kRatio),
      kPolicy: num(r.kPolicy),
      fundingLastHour: str(r.fundingLastHour),
      funding24h: str(r.funding24h),
      orders,
      halt,
      passLatencyMs: num(r.passLatencyMs),
      anchor: a && tx ? { tx, at: secs(a.at) } : null,
      lastPassAt: secs(r.lastPassAt),
    },
  };
}

// ---- Server-Sent Events --------------------------------------------------------------------------
export type SseEvent = { event: string; data: string };

/** A parser for the text/event-stream format (WHATWG HTML, "Server-sent events"): push decoded text as
 *  it arrives, in pieces of any size; `onEvent` is called once per complete event. A line ends in LF,
 *  CRLF or CR; an event ends at a blank line; `:` starts a comment; an event with no `data` is dropped;
 *  an event cut off by the end of the stream is never delivered (it would be half a frame). */
export function createSseParser(onEvent: (e: SseEvent) => void): (chunk: string) => void {
  let buf = "";
  let event = "";
  let data: string[] = [];
  const line = (l: string) => {
    if (l === "") {
      if (data.length) onEvent({ event: event || "message", data: data.join("\n") });
      event = "";
      data = [];
      return;
    }
    if (l.startsWith(":")) return;
    const i = l.indexOf(":");
    const field = i < 0 ? l : l.slice(0, i);
    const value = i < 0 ? "" : l.slice(i + 1).replace(/^ /, "");
    if (field === "event") event = value;
    else if (field === "data") data.push(value);
    // `id` and `retry` are the browser's business; this reader reconnects on its own schedule
  };
  return (chunk) => {
    buf += chunk;
    // A CR at the very end may be half of a CRLF: wait for the next piece before ending that line.
    const hold = buf.endsWith("\r") ? "\r" : "";
    const lines = (hold ? buf.slice(0, -1) : buf).replace(/\r\n|\r/g, "\n").split("\n");
    buf = (lines.pop() ?? "") + hold;
    lines.forEach(line);
  };
}

/** When this page last saw the venue's own clock move: the arrival time of the frame that first carried this `venueTime`.
 *  A reading that stops changing is old whatever the clocks say, so nothing here subtracts one machine's time from another's. */
export function venueSeen(prev: { venueTime: number | null; venueSeenAt: number | null } | null, venueTime: number | null, now: number): number | null {
  if (venueTime == null) return null;
  return prev && prev.venueTime === venueTime && prev.venueSeenAt != null ? prev.venueSeenAt : now;
}

// ---- the margin meter -----------------------------------------------------------------------------
export type Band = "critical" | "below-rule" | "ok";
/** Red under 1.0×, amber from 1.0× to the desk's rule, green at the rule or above. */
export const bandOf = (k: number, rule: number): Band => (k < 1 ? "critical" : k < rule ? "below-rule" : "ok");
/** How full the meter is: the ratio against the rule, 0 to 1. */
export const fillOf = (k: number, rule: number) => (rule > 0 ? Math.max(0, Math.min(1, k / rule)) : 0);
export const ratioText = (k: number) => `${k >= 100 ? k.toFixed(0) : k.toFixed(2)}×`;

// ---- numbers in columns -----------------------------------------------------------------------------
const NUM = /^(-?)(\d+)(?:\.(\d+))?$/;
const group = (digits: string) => digits.replace(/\B(?=(\d{3})+$)/g, ",");
/** A real minus sign: it is as wide as the plus, so signed figures line up. */
export const MINUS = "−";

/** A decimal with its thousands grouped and its fraction padded with zeros to `dp` places; never rounded,
 *  so a column of prices lines up on the point and says only what the feed said. */
export function fixed(s: string | null, dp = 0): string {
  if (s == null) return "—";
  const m = NUM.exec(s.trim());
  if (!m) return s;
  const frac = (m[3] ?? "").padEnd(dp, "0");
  return (m[1] ? MINUS : "") + group(m[2]) + (frac ? `.${frac}` : "");
}
/** Places in the longest value of a column, for `fixed`. */
export const places = (xs: (string | null)[]) => Math.max(0, ...xs.map((s) => NUM.exec((s ?? "").trim())?.[3]?.length ?? 0));

/** USDC the way a bank prints it: to the cent from a cent up, the digits as sent below a cent. */
export function money(s: string | null): string {
  if (s == null) return "—";
  const m = NUM.exec(s.trim());
  if (!m) return s;
  const n = Number(s);
  if (Math.abs(n) >= 0.01 || n === 0) {
    const [i, f] = Math.abs(n).toFixed(2).split(".");
    return (n < 0 ? MINUS : "") + group(i) + "." + f;
  }
  return (m[1] ? MINUS : "") + m[2] + "." + (m[3] ?? "0").replace(/0+$/, "");
}
/** `money` with a plus on a gain; `sign` is what colours it (and never the only thing that says it). */
export function signed(s: string | null): { text: string; sign: -1 | 0 | 1 } {
  if (s == null || !NUM.test(s.trim())) return { text: s ?? "—", sign: 0 };
  const n = Number(s);
  const text = money(s);
  return n > 0 ? { text: `+${text}`, sign: 1 } : n < 0 ? { text, sign: -1 } : { text, sign: 0 };
}

/** `signed` at a fixed number of places, for a pair of small figures that must be read against each other (funding:
 *  +0.0013 beside +0.0551, not +0.0013 beside +0.06). Rounds half away from zero at `dp`; a figure that rounds to zero has no sign. */
export function signedAt(s: string | null, dp: number): { text: string; sign: -1 | 0 | 1 } {
  if (s == null || !NUM.test(s.trim())) return { text: s ?? "—", sign: 0 };
  const n = Number(s);
  const [i, f] = Math.abs(n).toFixed(dp).split(".");
  const body = group(i) + (f ? `.${f}` : "");
  if (Number(Math.abs(n).toFixed(dp)) === 0) return { text: body, sign: 0 };
  return n > 0 ? { text: `+${body}`, sign: 1 } : { text: `${MINUS}${body}`, sign: -1 };
}

// ---- time -------------------------------------------------------------------------------------------
export const clock = (sec: number) =>
  new Date(sec * 1000).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23" });
/** The time of day, or the date too once it is not today. */
export function stamp(sec: number, nowMs: number): string {
  const d = new Date(sec * 1000);
  return d.toDateString() === new Date(nowMs).toDateString()
    ? clock(sec)
    : d.toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).replace(",", "");
}
export function age(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  return s < 90 ? `${s} s` : s < 5400 ? `${Math.round(s / 60)} min` : s < 172800 ? `${Math.round(s / 3600)} h` : `${Math.round(s / 86400)} d`;
}
export const shortTx = (tx: string) => (tx.length > 14 ? `${tx.slice(0, 8)}…${tx.slice(-4)}` : tx);

// ---- the account's events ------------------------------------------------------------------------
// `GET /api/level1/events` (tameion-app.rs `serve_events`): `hello`, `pass` and `inbox`. The events carry
// no time of their own; the row's time is when this page received it.
export type Tone = "info" | "ok" | "warn" | "bad";
export type EventRow = { id: number; at: number; source: string; text: string; tone: Tone };

const INBOX: Record<string, string> = {
  resync: "The stream caught up after a gap",
  preview: "The first read of the wallet finished",
  "change applied": "A waiting change was applied",
};

/** An event in words. Unknown event names are shown as they came, never dropped. */
export function describeEvent(name: string, data: unknown): Pick<EventRow, "source" | "text" | "tone"> {
  const d = obj(data) ?? {};
  if (name === "hello") return { source: "Stream", text: "Connected", tone: "info" };
  if (name === "pass") {
    const status = str(d.status) ?? "";
    const p = obj(d.progress);
    const scanned = num(p?.scannedBlocks);
    const total = num(p?.totalBlocks);
    if (status === "queued") return { source: "Check", text: "Check queued", tone: "info" };
    if (status === "running") {
      const phase = str(p?.phase);
      return {
        source: "Check",
        text: scanned != null && total ? `Check running, ${Math.min(100, Math.round((100 * scanned) / total))} % read` : phase ? `Check running: ${phase}` : "Check running",
        tone: "info",
      };
    }
    if (status === "done") {
      const tx = str(d.anchorTx);
      return { source: "Check", text: `Check done: paid ${num(d.paid) ?? 0}, needs you ${num(d.needsYou) ?? 0}${tx ? `, recorded on chain ${shortTx(tx)}` : ""}`, tone: "ok" };
    }
    if (status === "failed") return { source: "Check", text: `Check failed: ${str(d.error) ?? "no reason given"}`, tone: "bad" };
    return { source: "Check", text: `Check ${status || "update"}`, tone: "info" };
  }
  if (name === "inbox") {
    const cause = str(d.cause);
    return { source: "Account", text: cause ? (INBOX[cause] ?? `Account changed: ${cause}`) : "Account changed", tone: "info" };
  }
  const pairs = Object.entries(d)
    .slice(0, 4)
    .map(([k, v]) => `${k}: ${typeof v === "object" ? JSON.stringify(v) : String(v)}`.slice(0, 60));
  return { source: name, text: pairs.length ? pairs.join(", ") : name, tone: "info" };
}

// ---- the account's own record: what its agents did ------------------------------------------------------------
// The inbox (`GET /api/level1/inbox`) is the account's book. Its strings are the service's own ("0.002000 USDC", "the price … is above the
// most you allowed"); the only things computed here are an order (newest first) and a share (spent of the daily cap).
// Structural types, not the console's `Inbox`: this file stays free of `@/` imports so the tests can run it.
export type PaidIn = { counterparty: string; amount: string; at?: number; bought?: string; reason?: string; by?: string; explorer?: string; tx?: string };
export type RefusedIn = { counterparty: string; amount: string; why?: string };
export type PurchaseIn = { at: number; host?: string; seller?: string; amount: string; status: string; why?: string; reason?: string; url?: string };

export type PaidRow = { at: number | null; to: string; amount: string; why: string; by: string | null; receipt: string | null };
export type RefusedRow = { at: number | null; what: string; amount: string; why: string; reason: string | null };
export type AskRow = { at: number | null; text: string; why: string | null };

/** Newest first; a row with no time goes last. */
const byTimeDesc = <T extends { at: number | null }>(rows: T[]) => [...rows].sort((a, b) => (b.at ?? -1) - (a.at ?? -1));

/** What was paid, to whom (the account's own name for the payee), and why the agent said it paid. */
export function paidRows(paid: PaidIn[], nameOf: (address: string) => string): PaidRow[] {
  return byTimeDesc(
    paid.map((p) => ({
      at: num(p.at),
      to: nameOf(p.counterparty),
      amount: p.amount,
      why: p.reason?.trim() || p.bought?.trim() || "",
      by: p.by ?? null,
      receipt: p.explorer ?? null,
    })),
  );
}

/** What the checks turned away, in words: the bills the agent was told to refuse (`refused`, no time) and the x402 purchases the
 *  mandate would not let through (`purchases` with status "refused", which carry their own sentence). `plain` turns the checks'
 *  codes into words (the console's `plainWhy`). */
export function refusedRows(refused: RefusedIn[], purchases: PurchaseIn[], plain: (why?: string | null) => string, nameOf: (address: string) => string): RefusedRow[] {
  const bills = refused.map((r) => ({ at: null, what: nameOf(r.counterparty), amount: r.amount, why: plain(r.why) || "refused by the checks", reason: null }));
  const buys = purchases
    .filter((p) => p.status === "refused")
    .map((p) => ({ at: num(p.at), what: p.host || (p.seller ? nameOf(p.seller) : p.url) || "a seller", amount: p.amount, why: p.why?.trim() || "refused by the checks", reason: p.reason?.trim() || null }));
  return [...byTimeDesc(buys), ...bills];
}

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
/** What the agents' keys asked of the owner, each with the reason they gave (`agentAsks`), then what else waits (`waiting`). The
 *  words are the service's; a malformed entry is skipped, not guessed. */
export function askRows(agentAsks: unknown, waiting: { what: string; why?: string }[]): AskRow[] {
  const a = obj(agentAsks) ?? {};
  const rows: AskRow[] = [];
  const by = (v: Record<string, unknown>) => str(v.by) ?? "an agent key";
  const limits = obj(a.limits);
  if (limits) rows.push({ at: secs(limits.at), text: `${by(limits)} asked for the limits ${str(limits.perAction) ?? "?"} a payment, ${str(limits.daily) ?? "?"} a day`, why: str(limits.reason) });
  const mode = obj(a.mode);
  if (mode) rows.push({ at: secs(mode.at), text: `${by(mode)} asked to move to ${["Watch", "Propose", "Autopilot"][num(mode.mode) ?? -1] ?? "another mode"}`, why: str(mode.reason) });
  const payees = obj(a.payees);
  if (payees) {
    const n = Array.isArray(payees.payees) ? payees.payees.length : 0;
    rows.push({ at: secs(payees.at), text: `${by(payees)} asked to ${payees.allow === false ? "stop paying" : "pay"} ${n} payee${n === 1 ? "" : "s"}`, why: str(payees.reason) });
  }
  return [...byTimeDesc(rows), ...waiting.map((w) => ({ at: null, text: cap(w.what), why: w.why ?? null }))];
}

/** The part of the daily cap spent in the trailing 24 hours, 0 and up (over 1 is over the cap); null when either figure is not there. */
export function spentShare(spent: string | null | undefined, daily: string | null | undefined): number | null {
  const s = parseFloat(spent ?? "");
  const d = parseFloat(daily ?? "");
  return Number.isFinite(s) && Number.isFinite(d) && d > 0 ? Math.max(0, s / d) : null;
}

