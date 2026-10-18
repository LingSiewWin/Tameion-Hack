// The authority receipt (H4): what Tameion can do with the account, what it can never do, and the worst case. Every
// line is built here from two things only: the mandate as the service reads it from the chain (the inbox: caps, mode,
// payees and their names, the pending change, the contract's own terms) and the rule in contracts/src/BudgetPolicy.sol
// or PasskeyPrincipal.sol that enforces it, named in `rule`. No sentence is fixed text: change the mandate and the
// receipt changes with it (test/onboarding-receipt.test.mts). The page reads; it never computes money (DESIGN Rule 3).

/** The fields of `GET /api/level1/inbox` the receipt reads (apps/console/src/lib/types.ts `Inbox`). */
export type MandateInbox = {
  mode: string;
  caps: { perAction: string; daily: string; floor: string };
  payees?: { address: string; payableFrom: number }[];
  parties?: Record<string, { label: string }>;
  pending?: { mode?: string; eta?: number; kinds?: number; perActionCap?: string; dailyCap?: string } | null;
  /** The contract's own terms as the service read them: `delay` in seconds, `agentExpiry` in unix seconds. */
  terms?: Record<string, unknown> | null;
  agentAuthority?: { lapsesAt: number } | null;
  guardians?: { list: string[]; threshold: number } | null;
};

export type Line = { text: string; rule: string };
export type Receipt = { can: Line[]; never: Line[]; worst: Line[] };

/** "0.050000 USDC" -> "0.05 USDC": six decimals trimmed to the shortest that is exact, never `$` (DESIGN §Typography). */
export function usdc(s: string): string {
  const v = s.replace(/\s*USDC\s*$/i, "").trim();
  const m = v.match(/^(-?\d+)(?:\.(\d+))?$/);
  if (!m) return s;
  const frac = (m[2] ?? "").replace(/0+$/, "");
  return `${m[1]}${frac ? `.${frac}` : ""} USDC`;
}

/** A delay in words: "30 seconds", "1 minute", "2 days" (apps/console/src/lib/tameion.ts `spanText`). */
export function spanWords(secs: number): string {
  const u = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
  if (secs >= 86400 && secs % 86400 === 0) return u(secs / 86400, "day");
  if (secs >= 3600) return u(Math.round(secs / 3600), "hour");
  if (secs >= 60) return u(Math.round(secs / 60), "minute");
  return u(secs, "second");
}

/** "12 Oct" (DESIGN §Copy: dates in words). */
export const dayWords = (unix: number) => new Date(unix * 1000).toLocaleDateString("en-GB", { day: "numeric", month: "short" });

/** "at 06:21" on the same day, else "on 12 Oct". */
export function whenWords(unix: number, now: number): string {
  const d = new Date(unix * 1000);
  if (d.toDateString() === new Date(now * 1000).toDateString()) return `at ${d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" })}`;
  return `on ${dayWords(unix)}`;
}

/** "Demo Vendor", "A and B", "A, B and C", or "no one yet". */
export function listWords(names: string[]): string {
  if (!names.length) return "no one yet";
  if (names.length === 1) return names[0];
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** The approved payees by the name the service gives them (the owner's name, a proved name, or "Payee ••3854"). */
export const payeeNames = (i: MandateInbox) => (i.payees ?? []).map((p) => i.parties?.[p.address.toLowerCase()]?.label ?? "a payee you approved");

const num = (v: unknown): number | null => {
  const n = typeof v === "string" ? Number(v) : typeof v === "number" ? v : NaN;
  return Number.isFinite(n) ? n : null;
};

export function receiptOf(i: MandateInbox, now: number): Receipt {
  const per = usdc(i.caps.perAction);
  const daily = usdc(i.caps.daily);
  const floor = usdc(i.caps.floor);
  const keepsFloor = floor !== "0 USDC";
  const payees = listWords(payeeNames(i));
  const delay = num(i.terms?.delay);
  const wait = delay !== null ? spanWords(delay) : null;
  const lapses = i.agentAuthority?.lapsesAt ?? num(i.terms?.agentExpiry);
  const auto = i.mode === "Autopilot";
  const can: Line[] = [];
  const never: Line[] = [];
  const worst: Line[] = [];

  // Tameion can: the agent's key pays (Autopilot only), the operator key proposes and cuts.
  if (auto) can.push({ text: `Pay ${payees} by itself, up to ${per} at a time and ${daily} in any 24 hours.`, rule: "BudgetPolicy.pay: mode Autopilot, perActionCap, dailyCap" });
  else if (i.mode === "Propose") can.push({ text: `Prepare each payment to ${payees}; nothing goes out until you tap.`, rule: "BudgetPolicy.pay: NotAutopilot" });
  else can.push({ text: "Read the account and tell you what is due; it pays nothing.", rule: "BudgetPolicy.pay: NotAutopilot" });
  const p = i.pending;
  if (p?.eta && (p.kinds ?? 0) & 2 && p.mode)
    can.push({ text: `Switch to ${p.mode} ${whenWords(p.eta, now)}, unless you cancel before then.`, rule: "BudgetPolicy.setMode: pending, cancelPending" });
  if (p?.eta && (p.kinds ?? 0) & 1 && p.perActionCap && p.dailyCap)
    can.push({ text: `Raise the limits to ${usdc(p.perActionCap)} at a time and ${usdc(p.dailyCap)} a day ${whenWords(p.eta, now)}, unless you cancel before then.`, rule: "BudgetPolicy.setLimits: pending, cancelPending" });
  if (wait) can.push({ text: `Ask for higher limits or more autonomy; nothing rises for ${wait}, anyone can see it coming, and you can cancel it.`, rule: "BudgetPolicy.setLimits/setMode: onlyPrincipalOrOperator, delay" });
  can.push({ text: "Cut a limit or stop the agent at once.", rule: "BudgetPolicy.setLimits/setMode: a shrinking change applies at once" });

  // Tameion can never: what the contract refuses, whoever asks.
  never.push({ text: "Pay anyone you have not approved.", rule: "BudgetPolicy._payChecked: PayeeNotAttested" });
  if (wait) never.push({ text: `Pay a payee in its first ${wait} after you approve it.`, rule: "BudgetPolicy._payChecked: PayeeCoolingOff" });
  never.push({ text: `Pay more than ${per} at a time, or more than ${daily} in any 24 hours.`, rule: "BudgetPolicy._payChecked: OverPerActionCap, OverDailyCap" });
  if (keepsFloor) never.push({ text: `Leave less than ${floor} in the account.`, rule: "BudgetPolicy._payChecked: BelowReserveFloor" });
  never.push({ text: "Pay the same bill twice.", rule: "BudgetPolicy._payChecked: AlreadyExecuted" });
  never.push({ text: "Approve a payee, move your money out or change your keys. Those take your passkey.", rule: "BudgetPolicy.setPayee/withdraw: onlyPrincipal; PasskeyPrincipal.execute" });
  if (lapses) never.push({ text: `Keep paying after ${dayWords(lapses)} unless you renew the agent.`, rule: "BudgetPolicy.pay: AgentLapsed" });

  // Worst case: a stolen agent key, a stolen Tameion key, a lost device.
  if (auto) worst.push({ text: `A stolen agent key moves at most ${daily} in any 24 hours, and only to ${payees}.`, rule: "BudgetPolicy._payChecked: dailyCap, PayeeNotAttested" });
  else worst.push({ text: `A stolen agent key moves nothing while each payment waits for your tap.`, rule: "BudgetPolicy.pay: NotAutopilot" });
  if (wait) worst.push({ text: `A stolen Tami key is held to the same limits and payees. Anything more waits ${wait}, and your passkey cancels it.`, rule: "BudgetPolicy.setAgent/setLimits: operator, delay, cancelPending" });
  const g = i.guardians;
  if (g && g.threshold > 0 && g.list.length >= g.threshold)
    worst.push({ text: `If you lose this device, ${g.threshold} of your ${g.list.length} guardians can name a new passkey.`, rule: "PasskeyPrincipal.approveRecovery: guardians" });
  else worst.push({ text: "If you lose this device before adding guardians, no one can move the money out; the agent stays inside these limits.", rule: "BudgetPolicy.withdraw: onlyPrincipal; PasskeyPrincipal.setGuardians" });

  return { can, never, worst };
}
