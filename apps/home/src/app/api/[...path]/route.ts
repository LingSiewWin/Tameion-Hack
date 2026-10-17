// The page's only way to the service (DESIGN §Data): /api/* here is /api/* on TAMEION_URL, GET and POST, so the
// browser never needs CORS. Forwards only Authorization, Idempotency-Key and Content-Type; never logs a body.
import type { Refusal } from "../../../lib/types.ts";

const SERVICE = (process.env.TAMEION_URL ?? "http://127.0.0.1:7890").replace(/\/+$/, "");
const FORWARD = ["authorization", "idempotency-key", "content-type"] as const;
// Node's fetch has already decoded and framed the body; these would describe the wrong bytes.
const DROP = new Set(["content-encoding", "content-length", "transfer-encoding", "connection", "keep-alive"]);

type Ctx = { params: Promise<{ path: string[] }> };

async function proxy(req: Request, { params }: Ctx): Promise<Response> {
  const { path } = await params;
  if (path.some((s) => s === "." || s === "..")) return refusal(400, "invalid_body", "input", "no", "That path is not one this service has.");
  const target = `${SERVICE}/api/${path.map(encodeURIComponent).join("/")}${new URL(req.url).search}`;
  const headers = new Headers();
  for (const h of FORWARD) {
    const v = req.headers.get(h);
    if (v) headers.set(h, v);
  }
  let r: Response;
  try {
    r = await fetch(target, {
      method: req.method,
      headers,
      body: req.method === "POST" ? await req.arrayBuffer() : undefined,
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

function refusal(status: number, code: string, by: Refusal["error"]["by"], executed: Refusal["error"]["executed"], words: string) {
  const body: Refusal = {
    ok: false,
    error: {
      code,
      by,
      executed,
      audience: "user",
      meaning: words,
      nextStep: executed === "unknown" ? "Check the account before trying again; the answer may still arrive." : "Try again in a moment.",
      retryAfterSecs: 5,
      words,
    },
  };
  return Response.json(body, { status });
}

export { proxy as GET, proxy as POST };
