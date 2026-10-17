// The judge's five minutes (docs/115 §3): which step a demo account is on, when its bill is next due, and the raise and
// the cut step 5 signs. Pure (viem only), so `node --test` runs it.
import { formatUnits, parseUnits } from "viem";

export const STEPS = ["Open", "Bill", "Autopilot", "Watch it pay", "Raise and cut"] as const;

/** 0…4 is the step the judge is on; 5 once all five are done. Each step needs the one before it. `paid` counts the bill's
 *  payments, the agent's own or the owner's tap when the agent asked instead (the page says which). */
export function stepOf(s: { funded: boolean; billSaved: boolean; autopilot: boolean; paid: number; cut: boolean }): number {
  const done = [s.funded, s.billSaved, s.autopilot, s.paid > 0, s.cut];
  const i = done.indexOf(false);
  return i < 0 ? 5 : i;
}

/** The next due time (unix s) of a bill first due at `first`, every `every` s: `first` itself until it passes, then the
 *  cycle after now. */
export const nextDue = (first: number, every: number, now: number) =>
  every <= 0 || now <= first ? first : first + every * Math.ceil((now - first) / every);

/** "2:05", "0:09"; never negative. */
export const clock = (secs: number) => {
  const s = Math.max(0, Math.ceil(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

const u = (s: string) => parseUnits(s.replace(" USDC", "").trim() || "0", 6);
const f = (n: bigint) => formatUnits(n, 6);
export type Caps = { perAction: string; daily: string };

/** Step 5's raise: twice the cap on one payment (the daily cap rises with it only when it would be passed). It waits
 *  the delay; the contract refuses a per-payment cap above the daily one. */
export function raiseOf(c: Caps): Caps {
  const per = u(c.perAction) * 2n;
  const daily = u(c.daily);
  return { perAction: f(per), daily: f(per > daily ? per : daily) };
}

/** Step 5's cut, applied at once: the cap on one payment down to the bill itself, so the agent still pays the bill and
 *  nothing bigger; already there, the daily cap halves (never below one payment). null when nothing is left to cut. */
export function cutOf(c: Caps, bill: string): Caps | null {
  const per = u(c.perAction);
  const daily = u(c.daily);
  const b = u(bill);
  if (per > b) return { perAction: f(b), daily: f(daily) };
  const half = daily / 2n;
  const next = half > per ? half : per;
  return next < daily ? { perAction: f(per), daily: f(next) } : null;
}
