// GET /api/circle/attestation?domain=<source domain>&tx=<burn tx hash>: Circle's sandbox attestation for one burn, asked
// from the server (src/app/api/circle/attestation/route.ts re-exports this), answered as the page's `Attestation`.
// Circle: GET https://iris-api-sandbox.circle.com/v2/messages/{sourceDomainId}?transactionHash= ; 404 until it has seen
// the burn; 40 requests a second, then every request is refused for five minutes (HTTP 429).
import { attestationOf, byDomain, IRIS, type Attestation } from "./cctp.ts";

const reply = (a: Attestation, status = 200, retryAfterSecs?: number) =>
  Response.json({ ...a, ...(retryAfterSecs ? { retryAfterSecs } : {}) }, { status, headers: { "cache-control": "no-store" } });

export async function GET(req: Request): Promise<Response> {
  const u = new URL(req.url);
  const domain = Number(u.searchParams.get("domain"));
  const tx = u.searchParams.get("tx") ?? "";
  if (!byDomain(domain) || !/^0x[0-9a-fA-F]{64}$/.test(tx)) return reply({ state: "unreadable", words: "That is not a burn this page can look up." }, 400);
  let r: Response;
  try {
    r = await fetch(`${IRIS}/v2/messages/${domain}?transactionHash=${tx}`, { cache: "no-store", signal: AbortSignal.timeout(10_000) });
  } catch {
    return reply({ state: "unreadable", words: "Circle did not answer." }, 502, 15);
  }
  if (r.status === 404) return reply({ state: "unseen" });
  if (r.status === 429) return reply({ state: "unreadable", words: "Circle is limiting requests for five minutes." }, 503, 300);
  if (!r.ok) return reply({ state: "unreadable", words: `Circle answered ${r.status}.` }, 502, 15);
  return reply(attestationOf(await r.json().catch(() => null)));
}
