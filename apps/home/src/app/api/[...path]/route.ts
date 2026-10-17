// The page's only way to the service (DESIGN §Data): /api/* here is /api/* on TAMEION_URL, GET and POST, so the
// browser never needs CORS. Forwards only Authorization, Idempotency-Key and Content-Type; never logs a body. The owner's
// device token travels only in the httpOnly cookie `tameion.home.device` (set by this app after the stepper opens an
// account) and reaches the service here as X-Tameion-Device (the owner's names, docs/81; the prepare-* gate); a page
// script can neither read it nor send its own (ruled 2026-10-18).
// The page names accounts by NAME; a `policy` that is a name (query, `/level1/tenant/<x>`, or `"policy":"<name>"` in a
// JSON body) becomes the address here, on the server, so the page never needs it (DESIGN §Page grammar, routes).
import { isAddress, resolveAccount } from "../../../lib/api.ts";
import type { Refusal } from "../../../lib/types.ts";

const SERVICE = (process.env.TAMEION_URL ?? "http://127.0.0.1:7890").replace(/\/+$/, "");
const FORWARD = ["authorization", "idempotency-key", "content-type"] as const;
const DEVICE_COOKIE = "tameion.home.device";
// Node's fetch has already decoded and framed the body; these would describe the wrong bytes.
const DROP = new Set(["content-encoding", "content-length", "transfer-encoding", "connection", "keep-alive"]);
const BODY_POLICY = /("policy"\s*:\s*")([^"\\]+)(")/g;

type Ctx = { params: Promise<{ path: string[] }> };

async function proxy(req: Request, { params }: Ctx): Promise<Response> {
  const path = [...(await params).path];
  if (path.some((s) => s === "." || s === "..")) return refusal(400, "invalid_body", "input", "no", "That path is not one this service has.", "Use a path the service lists in /llms.txt.");
  const search = new URL(req.url).searchParams;
  let body = req.method === "POST" ? await req.text() : undefined;

  // name -> address; an unknown name is refused here, before anything reaches the service
  const names = new Set<string>();
  const policy = search.get("policy");
  if (policy && !isAddress(policy)) names.add(policy);
  const tenantAt = path[0] === "level1" && path[1] === "tenant" && path.length === 3 ? 2 : -1;
  if (tenantAt > 0 && !isAddress(path[tenantAt])) names.add(path[tenantAt]);
  if (body && req.headers.get("content-type")?.includes("json")) for (const m of body.matchAll(BODY_POLICY)) if (!isAddress(m[2])) names.add(m[2]);
  const to = new Map<string, string>();
  try {
    for (const name of names) {
      const account = await resolveAccount(name);
      if (!account) return refusal(404, "not_found", "input", "no", "No account has that name.", "Check the name; an account lives on the service that opened it.");
      to.set(name, account.policy);
    }
  } catch {
    return refusal(502, "service_unavailable", "service", "no", "The service did not answer.");
  }
  if (policy && to.has(policy)) search.set("policy", to.get(policy)!);
  if (tenantAt > 0 && to.has(path[tenantAt])) path[tenantAt] = to.get(path[tenantAt])!;
  // textual, so every other byte of the body (large numbers included) reaches the service exactly as sent
  if (body && to.size) body = body.replace(BODY_POLICY, (all, a: string, name: string, z: string) => (to.has(name) ? `${a}${to.get(name)}${z}` : all));

  const query = search.toString();
  const target = `${SERVICE}/api/${path.map(encodeURIComponent).join("/")}${query ? `?${query}` : ""}`;
  const headers = new Headers();
  for (const h of FORWARD) {
    const v = req.headers.get(h);
    if (v) headers.set(h, v);
  }
  const device = deviceToken(req.headers.get("cookie"));
  if (device) headers.set("x-tameion-device", device);
  let r: Response;
  try {
    r = await fetch(target, {
      method: req.method,
      headers,
      body,
      cache: "no-store",
      redirect: "manual",
      // A reader who leaves cancels a read; a POST is left to finish, so a payment is never cut off half-way by a
      // closed tab (its idempotent reply is there to read again).
      signal: req.method === "GET" ? req.signal : undefined,
    });
  } catch {
    // A read moved nothing; a POST may have reached the service before the answer was lost: say so, never re-send.
    return refusal(502, "service_unavailable", "service", req.method === "GET" ? "no" : "unknown", "The service did not answer.");
  }
  const out = new Headers();
  r.headers.forEach((v, k) => {
    if (!DROP.has(k)) out.set(k, v);
  });
  // A proxied event stream must not be compressed and held back (docs/60 P-152): no-transform keeps it frame by frame.
  if (out.get("content-type")?.startsWith("text/event-stream")) out.set("cache-control", "no-cache, no-transform");
  return new Response(r.body, { status: r.status, statusText: r.statusText, headers: out });
}

/** The owner's device token from the `tameion.home.device` cookie, or null. */
function deviceToken(cookie: string | null): string | null {
  for (const part of (cookie ?? "").split(";")) {
    const at = part.indexOf("=");
    if (at > 0 && part.slice(0, at).trim() === DEVICE_COOKIE) {
      try {
        return decodeURIComponent(part.slice(at + 1).trim()) || null;
      } catch {
        return null;
      }
    }
  }
  return null;
}

function refusal(
  status: number,
  code: string,
  by: Refusal["error"]["by"],
  executed: Refusal["error"]["executed"],
  words: string,
  nextStep = executed === "unknown" ? "Check the account before trying again; the answer may still arrive." : "Try again in a moment.",
) {
  const body: Refusal = {
    ok: false,
    error: {
      code,
      by,
      executed,
      audience: "user",
      meaning: words,
      nextStep,
      retryAfterSecs: 5,
      words,
    },
  };
  return Response.json(body, { status });
}

export { proxy as GET, proxy as POST };
