// The /api proxy against a stand-in service on a free port: what it forwards, what it refuses, what it says when the
// service is gone; and the typed reads (src/lib/api.ts) parsing what comes back.
import assert from "node:assert/strict";
import { createServer, type IncomingMessage } from "node:http";
import type { AddressInfo } from "node:net";
import { after, before, test } from "node:test";

type Seen = { method?: string; url?: string; headers: IncomingMessage["headers"]; body: string };
let seen: Seen;
const service = createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    seen = { method: req.method, url: req.url, headers: req.headers, body };
    if (req.url?.startsWith("/api/level1/record?")) {
      res.writeHead(200, { "content-type": "application/x-ndjson" });
      return res.end('{"seq":0,"at":1,"kind":"paid","note":"a","canonical":"0x00"}\n{"seq":1,"at":2,"kind":"refused","note":"b","canonical":"0x01"}\n');
    }
    if (req.url?.startsWith("/api/level1/tenant/")) {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ tenant: { policy: "0x1", owner: "0x2", chain: "arbitrum-sepolia", name: "acme-agents-1541" } }));
    }
    if (req.url?.startsWith("/api/level1/events")) {
      res.writeHead(200, { "content-type": "text/event-stream" });
      return res.end("data: x\n\n");
    }
    if (req.url?.startsWith("/api/level1/inbox")) {
      res.writeHead(404, { "content-type": "application/json" });
      const error = { code: "not_found", by: "input", executed: "no", audience: "agent", meaning: "m", nextStep: "n", retryAfterSecs: 0, words: "no such tenant" };
      return res.end(JSON.stringify({ ok: false, contractVersion: "2026-10-18.1", error }));
    }
    res.writeHead(200, { "content-type": "application/json", "idempotent-replay": "true" });
    res.end('{"ok":true}');
  });
});

let route: typeof import("../src/app/api/[...path]/route.ts");
let api: typeof import("../src/lib/api.ts");
before(async () => {
  await new Promise<void>((r) => service.listen(0, "127.0.0.1", r));
  process.env.TAMEION_URL = `http://127.0.0.1:${(service.address() as AddressInfo).port}/`;
  route = await import("../src/app/api/[...path]/route.ts");
  api = await import("../src/lib/api.ts");
});
after(() => service.close());

const call = (method: "GET" | "POST", path: string[], search = "", headers: Record<string, string> = {}, body?: string) =>
  route[method](new Request(`http://localhost:3010/api/${path.join("/")}${search}`, { method, headers, body }), { params: Promise.resolve({ path }) });

test("forwards the path, the query, the body and only Authorization, Idempotency-Key and Content-Type", async () => {
  const r = await call(
    "POST",
    ["level1", "pay"],
    "?policy=0xabc",
    { authorization: "Bearer k", "idempotency-key": "id-1", "content-type": "application/json", cookie: "s=1", "x-tameion-device": "d", "x-forwarded-for": "1.2.3.4" },
    '{"a":1}',
  );
  assert.equal(r.status, 200);
  assert.equal(await r.text(), '{"ok":true}');
  assert.equal(r.headers.get("idempotent-replay"), "true", "the service's own reply headers come back");
  assert.equal(seen.method, "POST");
  assert.equal(seen.url, "/api/level1/pay?policy=0xabc");
  assert.equal(seen.body, '{"a":1}');
  assert.equal(seen.headers.authorization, "Bearer k");
  assert.equal(seen.headers["idempotency-key"], "id-1");
  assert.equal(seen.headers["content-type"], "application/json");
  for (const h of ["cookie", "x-tameion-device", "x-forwarded-for"]) assert.equal(seen.headers[h], undefined, `${h} is not forwarded`);
});

test("passes the service's refusal through unchanged", async () => {
  const r = await call("GET", ["level1", "inbox"], "?policy=0x1");
  assert.equal(r.status, 404);
  assert.equal(((await r.json()) as { error: { code: string } }).error.code, "not_found");
});

test("refuses a path that climbs out of /api", async () => {
  const r = await call("GET", ["level1", "..", "..", "healthz"]);
  assert.equal(r.status, 400);
  assert.equal(((await r.json()) as { error: { code: string } }).error.code, "invalid_body");
});

test("an event stream is marked no-transform so nothing holds its frames back", async () => {
  const r = await call("GET", ["level1", "events"], "?policy=0x1");
  assert.equal(r.headers.get("cache-control"), "no-cache, no-transform");
});

test("the typed reads: NDJSON records parse in order; a refusal throws ApiError with its envelope", async () => {
  const events = await api.fetchRecord("0x1", "0x2");
  assert.deepEqual(
    events.map((e) => [e.seq, e.kind]),
    [
      [0, "paid"],
      [1, "refused"],
    ],
  );
  assert.equal(seen.url, "/api/level1/record?policy=0x1&root=0x2");
  await assert.rejects(api.fetchInbox("0x1"), (e: unknown) => {
    assert.ok(e instanceof api.ApiError);
    assert.equal(e.status, 404);
    assert.equal(e.refusal?.code, "not_found");
    assert.equal(e.refusal?.executed, "no");
    assert.equal(e.message, "no such tenant");
    return true;
  });
  assert.equal((await api.fetchTenant("0x1")).name, "acme-agents-1541", "the tenant route wraps the record as { tenant }");
  await api.fetchRecall("0x1", ["evt:0", "evt:3"]);
  assert.equal(seen.url, "/api/level1/recall?policy=0x1&ids=evt%3A0%2Cevt%3A3");
});

test("when the service does not answer: a read says nothing moved; a POST says it may have", async () => {
  await new Promise<void>((r) => service.close(() => r()));
  const get = await call("GET", ["level1", "status"]);
  assert.equal(get.status, 502);
  const g = (await get.json()) as { ok: boolean; error: { code: string; executed: string } };
  assert.equal(g.ok, false);
  assert.equal(g.error.code, "service_unavailable");
  assert.equal(g.error.executed, "no");
  const post = await call("POST", ["level1", "pay"], "", { "content-type": "application/json" }, "{}");
  assert.equal(((await post.json()) as { error: { executed: string } }).error.executed, "unknown");
});
