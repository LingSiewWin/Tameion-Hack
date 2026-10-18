// Money and dates in the page's words (DESIGN §Typography, §Copy): "0.05 USDC", six decimals trimmed to the shortest
// that is exact, never "$"; "Today 05:41", "Yesterday", "12 Oct". Strings and bigints only: no floating point near money.
import type { Inbox } from "../../lib/types.ts";

/** USDC minor units (6 decimals) as "0.05 USDC". */
export function usdcText(minor: bigint): string {
  const sign = minor < 0n ? "-" : "";
  const abs = minor < 0n ? -minor : minor;
  const frac = (abs % 1_000_000n).toString().padStart(6, "0").replace(/0+$/, "");
  return `${sign}${abs / 1_000_000n}${frac ? `.${frac}` : ""} USDC`;
}

/** The ledger's string ("0.050000 USDC") trimmed the same way; anything else comes back as it was. */
export function ledgerText(s: string): string {
  const m = /^(-?\d+)(?:\.(\d{1,6}))?\s*USDC$/.exec(s.trim());
  if (!m) return s;
  const frac = (m[2] ?? "").replace(/0+$/, "");
  return `${m[1]}${frac ? `.${frac}` : ""} USDC`;
}

/** What the person typed, as USDC minor units; null unless it is a positive amount with at most 6 decimals. */
export function parseUsdc(input: string): bigint | null {
  const m = /^(\d{0,12})(?:[.,](\d{0,6}))?$/.exec(input.trim());
  if (!m || (!m[1] && !m[2])) return null;
  const minor = BigInt(m[1] || "0") * 1_000_000n + BigInt((m[2] ?? "").padEnd(6, "0"));
  return minor > 0n ? minor : null;
}

const pad2 = (n: number) => String(n).padStart(2, "0");
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** A moment in words, in the viewer's time zone: "Today 05:41", "Yesterday", "12 Oct". */
export function whenText(unixSecs: number, nowMs = Date.now()): string {
  const d = new Date(unixSecs * 1000);
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((day(new Date(nowMs)) - day(d)) / 86_400_000);
  if (days === 0) return `Today ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
  if (days === 1) return "Yesterday";
  return `${d.getDate()} ${MONTHS[d.getMonth()]}`;
}


/** What the page last read of the account and when (ms), kept on this device so a service that stops answering leaves
 *  the figures at full ink, marked stale with their time (DESIGN §States, Offline). */
export type Held = { balance: string; holdingsElsewhere?: Inbox["holdingsElsewhere"]; at: number };

/** A kept read back from storage; null unless it has a balance and a time (storage is the device's, not ours). */
export function parseHeld(raw: string | null): Held | null {
  try {
    const h = JSON.parse(raw ?? "null") as Partial<Held> | null;
    return h && typeof h.balance === "string" && typeof h.at === "number" ? (h as Held) : null;
  } catch {
    return null;
  }
}

/** The stale mark: "as of 06:12" today, "as of yesterday", "as of 12 Oct". */
export function asOfText(ms: number, nowMs = Date.now()): string {
  const w = whenText(ms / 1000, nowMs);
  return `as of ${w.startsWith("Today ") ? w.slice(6) : w.charAt(0).toLowerCase() + w.slice(1)}`;
}
