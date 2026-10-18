// How the Agent view writes what the service said. Nothing here adds, nets or converts money (DESIGN Rule 3); the one
// division is the brief's rate per minute (status.rate, a day's USDC, ÷ 1440), done in integers and marked "≈" when inexact.

/** "0.003000 USDC" → { qty: "0.003", unit: "USDC" }: six decimals trimmed to the shortest that is exact. Null when absent. */
export function money(s: string | null | undefined): { qty: string; unit: string } | null {
  const m = /^\s*(-?\d+)(?:\.(\d+))?\s*([A-Za-z]*)\s*$/.exec(s ?? "");
  if (!m) return null;
  const frac = (m[2] ?? "").replace(/0+$/, "");
  return { qty: frac ? `${m[1]}.${frac}` : m[1], unit: m[3] || "USDC" };
}

/** A day's rate ("0.001428 USDC") per minute, to eight decimals: "≈ 0.00000099". Null when the service gave no rate. */
export function perMinute(rate: string | null | undefined): string | null {
  const m = /^\s*(-?)(\d+)(?:\.(\d{1,6}))?/.exec(rate ?? "");
  if (!m) return null;
  const micro = BigInt(m[2]) * 1_000_000n + BigInt((m[3] ?? "").padEnd(6, "0"));
  const pico = micro * 1_000_000n; // 1e-12 USDC
  const q = pico / 1440n;
  const kept = q / 10_000n; // 1e-8 USDC: eight decimals
  const exact = pico % 1440n === 0n && q % 10_000n === 0n;
  if (kept === 0n && q !== 0n) return `${m[1]}< 0.00000001`;
  const whole = kept / 100_000_000n;
  const frac = (kept % 100_000_000n).toString().padStart(8, "0").replace(/0+$/, "");
  const s = `${m[1] && kept !== 0n ? "-" : ""}${whole}${frac ? `.${frac}` : ""}`;
  return exact ? s : `≈ ${s}`;
}

/** 0x + hex → "0x508d…a968": a hash or an address, short; the full value goes in the title and the expanded line. */
export const short = (h: string | null | undefined): string => (h && h.length > 14 ? `${h.slice(0, 6)}…${h.slice(-4)}` : (h ?? ""));

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const two = (n: number) => String(n).padStart(2, "0");

/** Dates in words (DESIGN §Copy): "Today 05:41", "Yesterday 23:10", "12 Oct 17:37"; local time. */
export function when(atSecs: number, now: number = Date.now()): string {
  const d = new Date(atSecs * 1000);
  const hm = `${two(d.getHours())}:${two(d.getMinutes())}`;
  const day = (t: Date) => new Date(t.getFullYear(), t.getMonth(), t.getDate()).getTime();
  const days = Math.round((day(new Date(now)) - day(d)) / 86_400_000);
  if (days === 0) return `Today ${hm}`;
  if (days === 1) return `Yesterday ${hm}`;
  return `${d.getDate()} ${MONTHS[d.getMonth()]} ${hm}`;
}

/** The clock a read was taken at, "06:41:12", for "as of". */
export const clock = (ms: number): string => {
  const d = new Date(ms);
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
};
