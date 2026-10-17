// The Owner home in words (DESIGN §Copy, Rules 1 and 3). Every figure is the service's string shown as it is, trimmed
// to the shortest exact form; figures are combined here only where a ruling names the combination (allowance left today
// = min(caps.daily − caps.spent24h, balance − caps.floor); paid this week = paid[] of the last 7 days). No hash ever
// leaves this file.
import type { Inbox, Paid, SymbolonRecord } from "../../lib/types.ts";

const UNIT = 1_000_000n;

/** "0.050000 USDC" → 50000n; null when it is not a USDC figure. */
export function minor(s?: string | null): bigint | null {
  const m = /^\s*(-?)(\d+)(?:\.(\d{1,6}))?\s*(?:USDC)?\s*$/.exec(s ?? "");
  if (!m) return null;
  const v = BigInt(m[2]) * UNIT + BigInt((m[3] ?? "").padEnd(6, "0"));
  return m[1] ? -v : v;
}

/** 50000n → "0.05": six decimals trimmed to the shortest that is exact, thousands grouped, never `$`. */
export function amount(u: bigint): string {
  const a = u < 0n ? -u : u;
  const frac = (a % UNIT).toString().padStart(6, "0").replace(/0+$/, "");
  return (u < 0n ? "-" : "") + (a / UNIT).toLocaleString("en-US") + (frac ? `.${frac}` : "");
}

/** The service's figure as a person reads it: "0.050000 USDC" → "0.05 USDC". */
export function money(s?: string | null): string {
  const u = minor(s);
  return u === null ? "" : `${amount(u)} USDC`;
}

const HASH = /0x[0-9a-fA-F]{8,}/g;
const HAS_HASH = /0x[0-9a-fA-F]{8,}/;
const isAddress = (s: string) => /^0x[0-9a-fA-F]{40}$/.test(s);

/** A party by name, never an address (DESIGN Rule 1): the owner's name for it, or a plain description. */
export function payeeName(inbox: Pick<Inbox, "parties" | "payees">, address: string): string {
  const p = inbox.parties?.[address.toLowerCase()];
  // kind "new" is the service's placeholder ("Payee ••3854"): no name yet.
  if (p && p.kind !== "new" && p.label && !HAS_HASH.test(p.label)) return p.label;
  const approved = inbox.payees?.some((x) => x.address.toLowerCase() === address.toLowerCase());
  return approved ? "a payee you approved" : "a payee you have not approved";
}

/** A shortened address in a service sentence ("0xe5e2d8…e33854"), as the party it names, else its last four. */
function shortAddress(inbox: Pick<Inbox, "parties" | "payees">, head: string, tail: string): string {
  const hit = Object.keys(inbox.parties ?? {}).find((a) => a.startsWith(`0x${head.toLowerCase()}`) && a.endsWith(tail.toLowerCase()));
  const name = hit ? payeeName(inbox, hit) : "";
  return name && !name.startsWith("a payee") ? name : `••${tail.slice(-4).toUpperCase()}`;
}

/** A service sentence for the Owner view: amounts trimmed, every address named (or its last four), no other hash, and
 *  no machine markers (the agent's "[evt:1]" citations). */
export function plain(inbox: Pick<Inbox, "parties" | "payees">, text?: string | null): string {
  const t = String(text ?? "")
    .replace(/\s*\[evt:\d+\]/g, "")
    .replace(/0x([0-9a-fA-F]{4,})(?:…|\.\.\.)([0-9a-fA-F]{4,})/g, (_m, head: string, tail: string) => shortAddress(inbox, head, tail))
    .replace(/(\d+\.\d{6}) USDC/g, (_m, v: string) => money(v))
    .replace(HASH, (h) => {
      if (!isAddress(h)) return "a reference";
      const name = payeeName(inbox, h);
      return name.startsWith("a payee") ? `••${h.slice(-4).toUpperCase()}` : name;
    })
    .trim();
  return t.charAt(0).toUpperCase() + t.slice(1);
}

/** "A, B and C". */
const list = (xs: string[]) => (xs.length < 2 ? xs.join("") : `${xs.slice(0, -1).join(", ")} and ${xs[xs.length - 1]}`);

/** Whom the agent may pay, by name: "only on Demo Vendor", "only on A and B", "on no one yet". */
export function payeesWords(inbox: Pick<Inbox, "parties" | "payees">): string {
  const all = inbox.payees ?? [];
  if (!all.length) return "on no one yet";
  const names = all.map((p) => payeeName(inbox, p.address));
  const named = names.filter((n) => !n.startsWith("a payee"));
  const unnamed = names.length - named.length;
  const rest = unnamed === 0 ? [] : unnamed === 1 ? ["a payee you approved"] : [`${unnamed} payees you approved`];
  return `only on ${list([...named, ...rest])}`;
}

/** The hero (DESIGN §Direction): the authority sentence generated from the mandate the contract holds. */
export function authority(inbox: Pick<Inbox, "mode" | "caps" | "parties" | "payees">): string {
  if (inbox.mode === "Watch") return "The agent is stopped. It pays nothing until you start it again.";
  const day = minor(inbox.caps.daily);
  const once = minor(inbox.caps.perAction);
  if (day === null || once === null) return "The account's limits could not be read.";
  const base = `Your agents may spend ${amount(day)} USDC a day, ${amount(once)} at a time, ${payeesWords(inbox)}`;
  return inbox.mode === "Propose" ? `${base}, and each payment waits for your tap.` : `${base}.`;
}

/** A trust-raising change waiting out the delay, in words, with when it applies (anyone can see it coming). */
export function pendingWords(inbox: Pick<Inbox, "pending">): string | null {
  const p = inbox.pending;
  if (!p?.eta) return null;
  const what: string[] = [];
  if ((p.kinds ?? 0) & 2)
    what.push(p.mode === "Autopilot" ? "Autopilot turns on" : p.mode === "Propose" ? "the agent starts again, asking before each payment" : `the agent's mode becomes ${p.mode}`);
  const day = minor(p.dailyCap);
  const once = minor(p.perActionCap);
  if ((p.kinds ?? 0) & 1 && day !== null && once !== null) what.push(`the limits rise to ${amount(day)} USDC a day, ${amount(once)} at a time`);
  return what.length ? `At ${hhmm(new Date(p.eta * 1000))} ${what.join(" and ")}.` : null;
}

const DAY = 86_400;
const hhmm = (d: Date) => `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** "Today 05:41", "Yesterday", "12 Oct" (DESIGN §Copy), in the viewer's time. */
export function dateWords(unix: number, nowSecs: number): string {
  const d = new Date(unix * 1000);
  const start = new Date(nowSecs * 1000);
  start.setHours(0, 0, 0, 0);
  const today = start.getTime() / 1000;
  if (unix >= today) return `Today ${hhmm(d)}`;
  if (unix >= today - DAY) return "Yesterday";
  const year = d.getFullYear() === start.getFullYear() ? "" : ` ${d.getFullYear()}`;
  return `${d.getDate()} ${MONTHS[d.getMonth()]}${year}`;
}

/** "as of 06:12" for a value kept from an earlier read (DESIGN §States, offline / stale). */
export const asOf = (ms: number) => `as of ${hhmm(new Date(ms))}`;

export type Fact = { label: string; figure?: string; unit?: string; words?: string; attention?: boolean };

/** The four facts at rest (DESIGN §Page grammar). `byRateDays` is status.runway.byRateDays. */
export function facts(inbox: Pick<Inbox, "caps" | "waitingCount" | "paid" | "balance">, byRateDays: number | null | undefined, nowSecs: number): Fact[] {
  // What the agents can still spend today from money the account holds (ruled 2026-10-18 after H4's finding: 100 USDC
  // read on an account holding 0.05). Not caps.leftNow: that one is capped by the per-payment limit.
  const day = minor(inbox.caps.daily);
  const spent = minor(inbox.caps.spent24h ?? "0") ?? 0n;
  const held = minor(inbox.balance);
  const floor = minor(inbox.caps.floor) ?? 0n;
  const min = (a: bigint, b: bigint) => (a < b ? a : b);
  const left = day === null || held === null ? null : min(day - spent, held - floor) > 0n ? min(day - spent, held - floor) : 0n;
  const days = byRateDays == null ? null : Math.floor(byRateDays);
  const week = (inbox.paid ?? []).filter((p) => (p.at ?? 0) >= nowSecs - 7 * DAY).reduce((s, p) => s + (minor(p.amount) ?? 0n), 0n);
  const waiting = inbox.waitingCount ?? 0;
  return [
    left === null ? { label: "Allowance left today", words: "Not read yet" } : { label: "Allowance left today", figure: amount(left), unit: "USDC" },
    days === null
      ? { label: "Runway at this burn", words: "No burn yet" }
      : days < 1
        ? { label: "Runway at this burn", words: "Under a day" }
        : { label: "Runway at this burn", figure: String(days), unit: days === 1 ? "day" : "days" },
    { label: "Needs you", figure: String(waiting), attention: waiting > 0 },
    { label: "Paid this week", figure: amount(week), unit: "USDC" },
  ];
}

export type Line = {
  key: string;
  payee: string;
  amount: string;
  why: string;
  when: string;
  state: "Paid" | "Paid and sealed" | "Refused";
  /** the three halves present (agent, counterparty, witness), from the symbolon projection; never guessed */
  sealed: boolean;
  /** for the disclosure: which halves are present (absent when the service shows none) */
  halves?: Halves;
  at?: number;
  by: "the agent" | "you";
};

/** Which halves of a payment's record are present, from H3's projection (`GET /api/level1/symbolon`): the settlement
 *  record that names the payment's transaction. Two records naming one transaction are AND-ed; none, and nothing is shown. */
export type Halves = { agent: boolean; payee: boolean; witness: boolean; at?: number };
export function halvesByPayment(records: readonly SymbolonRecord[] | null | undefined): Map<string, Halves> {
  const out = new Map<string, Halves>();
  for (const r of records ?? []) {
    if (!r.tx) continue;
    const k = r.tx.toLowerCase();
    const h = { agent: !!r.agent?.verified, payee: !!r.counterpartySlot?.verified, witness: !!r.witness?.verified, at: r.at };
    const was = out.get(k);
    out.set(k, was ? { ...was, agent: was.agent && h.agent, payee: was.payee && h.payee, witness: was.witness && h.witness } : h);
  }
  return out;
}
const sealedOf = (h?: Halves) => !!h && h.agent && h.payee && h.witness;

/** The rows of "What the agent paid, and why": refusals first (they are the last check's), then payments newest first. */
export function lines(inbox: Pick<Inbox, "paid" | "refused" | "parties" | "payees">, nowSecs: number, halves: ReadonlyMap<string, Halves> = new Map()): Line[] {
  const refused: Line[] = (inbox.refused ?? []).map((r, i) => ({
    key: `refused-${i}`,
    payee: payeeName(inbox, r.counterparty),
    amount: money(r.amount),
    why: plainWhy(inbox, r.why),
    when: "In the last check",
    state: "Refused",
    sealed: false,
    by: "the agent",
  }));
  const paid: Line[] = (inbox.paid ?? []).map((p: Paid, i) => {
    const h = p.tx ? halves.get(p.tx.toLowerCase()) : undefined;
    const isSealed = sealedOf(h);
    return {
      key: p.tx ?? `paid-${i}`,
      payee: payeeName(inbox, p.counterparty),
      amount: money(p.amount),
      why: plain(inbox, p.reason) || (p.by === "you" ? "Paid by you with your passkey." : "Paid by the agent inside your limits."),
      when: p.at ? dateWords(p.at, nowSecs) : "",
      state: isSealed ? "Paid and sealed" : "Paid",
      sealed: isSealed,
      halves: h,
      at: p.at,
      by: p.by === "you" ? "you" : "the agent",
    };
  });
  return [...refused, ...paid];
}

// The checks' reasons in the owner's words, as apps/console says them (src/lib/tameion.ts PLAIN).
const WHY: Record<string, string> = {
  "payee-attested": "Not one of your approved payees yet.",
  "cycles-behind": "Nobody has paid this bill for a while: it may have ended.",
  solvency: "The account holds less than this bill.",
  "pre:balance": "The account holds less than this bill.",
  "pre:reserve": "Paying it would go below the amount you keep untouched.",
  "per-action-cap": "More than one payment may be on its own.",
  "daily-cap": "Over what may go out in a day.",
  "amount-range": "More than this payee has been paid before.",
  "payee-lookalike": "This address looks like another payee's.",
  "payee-unchanged": "The payee's details changed since the last payment.",
  "pre:payee": "The payee's details changed since the last payment.",
  "pre:confidence": "The agent is not sure enough that this is a real recurring bill.",
  "pre:evidence": "Not enough past payments to be sure.",
  "pre:known": "Never paid before.",
  "pre:provenance": "The evidence for this bill is weak.",
  "issuer-blocklist": "Circle has frozen this address; it cannot be paid.",
  idempotency: "Already paid.",
  "pre:not-settled": "Already paid.",
};
export function plainWhy(inbox: Pick<Inbox, "parties" | "payees">, why?: string | null): string {
  const out = new Set<string>();
  for (const part of String(why ?? "").split(" · ")) {
    if (!part.trim()) continue;
    const k = part.split(":")[0].trim();
    const k2 = part.split(":").slice(0, 2).join(":").trim();
    out.add(WHY[k2] ?? WHY[k] ?? plain(inbox, part));
  }
  return [...out].join(" ");
}

/** Whether a check's reasons say the payment is over the per-payment cap (then the one action is "Raise the cap"). */
export const overCap = (why?: string | null) => /per-action-cap/.test(why ?? "");

/** Where "Open in Agent view" lands: the agent check's own time, from the settlement record matched by transaction (a
 *  payment's chain time runs seconds after it); the payment's time only when the service shows no record. Never a hash. */
export const agentAnchor = (l: Pick<Line, "at" | "halves">): string | null => {
  const at = l.halves?.at ?? l.at;
  return at ? `pass-${at}` : null;
};

/** The disclosure's proof in words: which halves of the record are present (DESIGN §States, sealed). */
export function halvesWords(h: Halves | undefined, payee: string): string {
  if (!h) return "This service does not show the record's halves yet, so the payment is not marked sealed.";
  const who = payee.startsWith("a payee") ? "the payee" : payee;
  if (h.agent && h.payee && h.witness) return `Sealed: the agent, ${who} and the witness each signed this record.`;
  const has = [h.agent && "the agent", h.payee && who, h.witness && "the witness"].filter((x): x is string => !!x);
  const not = [!h.agent && "the agent", !h.payee && who, !h.witness && "the witness"].filter((x): x is string => !!x);
  const or = (xs: string[]) => (xs.length < 2 ? xs.join("") : `${xs.slice(0, -1).join(", ")} or ${xs[xs.length - 1]}`);
  return has.length ? `Signed by ${list(has)}. Not yet signed by ${or(not)}.` : "No half of this record is signed yet.";
}

/** One thing that waits for the owner: one sentence, one action (DESIGN §States, needs you). */
export type Need = {
  key: string;
  sentence: string;
  /** null: the owner already acted and the account waits (a raise waiting out its delay) */
  action: "Approve" | "Raise the cap" | null;
  kind: "seller" | "bill" | "invoice";
  counterparty: string;
  amount: string;
  /** the call the service prepared, when the item carries one (a payment the agent prepared, a one-off invoice) */
  call?: { to: string; data: string } | null;
  invoice?: { id: string; everyDays?: number; approvedPayee?: boolean };
};

/** Needs you, from the lists the service counts in `waitingCount` (sellers, invoices, prepared payments; in its order).
 *  A payment over the per-payment cap asks to raise the cap only on Autopilot, where the agent then pays it itself; on
 *  the other modes the owner's tap pays it (Approve). */
export function needs(inbox: Pick<Inbox, "sellersToApprove" | "invoices" | "needsYou" | "parties" | "payees" | "mode" | "pending">): Need[] {
  const out: Need[] = [];
  for (const s of inbox.sellersToApprove ?? [])
    out.push({ key: `seller-${s.seller}-${s.host ?? ""}`, sentence: `Let the agent pay ${payeeName(inbox, s.seller)}.`, action: "Approve", kind: "seller", counterparty: s.seller, amount: s.amount });
  for (const v of inbox.invoices ?? [])
    out.push({
      key: `invoice-${v.id}`,
      sentence: `${cap(payeeName(inbox, v.payee))} sent an invoice for ${money(v.amountText)}${v.everyDays ? `, every ${v.everyDays} day${v.everyDays === 1 ? "" : "s"}` : ""}.`,
      action: "Approve",
      kind: "invoice",
      counterparty: v.payee,
      amount: v.amountText,
      call: v.pay ?? null,
      invoice: { id: v.id, everyDays: v.everyDays, approvedPayee: v.approvedPayee },
    });
  for (const n of inbox.needsYou ?? []) {
    const over = overCap(n.why) && inbox.mode === "Autopilot";
    // A raise the owner already asked that covers this payment: the item says when, and offers no second ask.
    const p = inbox.pending;
    const raising = over && !!p?.eta && ((p.kinds ?? 0) & 1) !== 0 && (minor(p.perActionCap) ?? 0n) >= (minor(n.amount) ?? 0n);
    const who = payeeName(inbox, n.counterparty);
    out.push({
      key: `bill-${n.actionHash ?? n.counterparty + n.amount}`,
      sentence: raising
        ? `${money(n.amount)} to ${who} waits for the cap to rise at ${hhmm(new Date(p!.eta! * 1000))}.`
        : over
          ? `${money(n.amount)} to ${who} is more than one payment may be.`
          : `The agent prepared ${money(n.amount)} to ${who}.`,
      action: raising ? null : over ? "Raise the cap" : "Approve",
      kind: "bill",
      counterparty: n.counterparty,
      amount: n.amount,
      call: { to: n.to, data: n.data },
    });
  }
  return out;
}
const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** Every sentence the Owner home's components show that is not generated above (DESIGN: no hard-coded strings in
 *  components). One word per thing through the whole flow: the account, the agent, a payee by its name. */
export const COPY = {
  stop: "Stop the agent",
  start: "Start the agent",
  stopAsk: "Stop the agent now? Payments wait for your tap until you start it again.",
  startAsk: "Start the agent again? It asks before each payment, once the wait has passed.",
  keepRunning: "Keep it running",
  keepStopped: "Keep it stopped",
  stopped: "Stopped",
  started: (at: string) => `Started. It asks before each payment from ${at}.`,
  factsLabel: "The account at rest",
  paidTitle: "What the agent paid, and why",
  paidColumns: ["Date", "Who", "Amount", "Why", "State"],
  paidEmptyApproved: "Nothing paid yet. The agent pays your approved payees when their bills fall due.",
  paidEmptyNoPayee: "Nothing paid yet. Approve a payee under Needs you and the agent can pay it.",
  needsTitle: "Needs you",
  needsEmpty: "Nothing needs you.",
  refusedSheet: (when: string) => `Refused ${when.toLowerCase()}. Nothing was sent.`,
  paidSheet: (state: string, by: string, when: string) => `${state} by ${by}. ${when}.`,
  agentView: "Open in Agent view",
  stale: (asOf: string) => `The service did not answer. The figures below are ${asOf}.`,
  tryAgain: "Try again",
  noFigures: "The service did not answer, so there are no figures to show yet. Nothing was sent or changed.",
  noAccount: "No account has this name on this service.",
  serviceSilent: "The service did not answer.",
  whereUnknown: "This page could not read where the account lives.",
  promptClosed: "The passkey prompt was closed. Nothing was sent.",
  nothingSent: "Nothing was sent.",
  lostAnswer: "It was sent and the answer was lost. Still checking: it will not be sent twice.",
  sentWriting: "Sent; the record is still being written.",
  stillChecking: "Still checking. It will not be sent twice.",
  whatHappened: "What the service said",
  notReady: "The account is not ready to pay this yet; try again after the agent's next check.",
  notThisAccount: "Not signing: the payment goes to a contract that is not this account's.",
  approved: (who: string) => `Approved. The agent may pay ${who} once the wait has passed.`,
  capAsked: (amt: string) => `Asked. One payment may be up to ${amt} once the wait has passed; anyone can see it coming.`,
  paidTo: (amt: string, who: string) => `Paid ${amt} to ${who}.`,
  accepted: (who: string, amt: string) => `Accepted. The agent pays ${who} ${amt} when it falls due.`,
  demoVendor: "Demo Vendor",
  demoVendorNote: "the demo's vendor",
  phase: { signing: "Signing", sent: "Sent", confirmed: "Confirmed" },
} as const;

/** A control in flight names its phase; after 3 s it counts the seconds, and a lost answer reads "still checking"
 *  after 15 s (DESIGN §States, waits). */
export function phaseText(p: { label: string; since: number }, nowMs: number): string {
  const s = Math.max(0, Math.floor((nowMs - p.since) / 1000));
  if (p.label === COPY.phase.sent && s >= 15) return COPY.stillChecking;
  return s >= 3 ? `${p.label} ${s} s` : p.label;
}
