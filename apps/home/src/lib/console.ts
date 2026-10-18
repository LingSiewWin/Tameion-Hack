// The one link judges need (founder, 2026-10-18): from the account page to the console, where every owner action lives.
// The console's origin comes from NEXT_PUBLIC_CONSOLE_URL, read on the server (src/app/layout.tsx) and handed down as a prop,
// so no client code reads the environment; unset, the link is not drawn. The account's address goes only into the href,
// marked as the zero-hash checks' one attribute exception, "console-link" (test/zero-hash-exceptions.test.mts).

export const CONSOLE = { label: "Console", title: "Every owner action: payees, bills, Autopilot, limits" } as const;

/** `${base}/?policy=<address>`, or null when there is no console to link to or no address yet. */
export function consoleHref(base: string | null | undefined, policy: string | null | undefined): string | null {
  const b = (base ?? "").trim().replace(/\/+$/, "");
  if (!b || !/^https?:\/\//i.test(b) || !policy || !/^0x[0-9a-fA-F]{40}$/.test(policy)) return null;
  return `${b}/?policy=${policy}`;
}
