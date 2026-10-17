// The /api proxy against a stand-in service on a free port: what it forwards, what it refuses, what it says when the
// service is gone; and the typed reads (src/lib/api.ts) parsing what comes back.
import assert from "node:assert/strict";
import { createServer, type IncomingMessage } from "node:http";
import type { AddressInfo } from "node:net";
import { after, before, test } from "node:test";

type Seen = { method?: string; url?: string; headers: IncomingMessage["headers"]; body: string };
let seen: Seen;
let names = 0;
const ACME = "0xb1da17cf2b327cacf2acd7418f3debb24ff4a795";
const service = createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    seen = { method: req.method, url: req.url, headers: req.headers, body };
    if (req.url?.startsWith("/api/level1/record?")) {
      res.writeHead(200, { "content-type": "application/x-ndjson" });
      return res.end('{"seq":0,"at":1,"kind":"paid","note":"a","canonical":"0x00"}\n{"seq":1,"at":2,"kind":"refused","note":"b","canonical":"0x01"}\n');
    }
    if (req.url === "/api/level1/name/acme-agents-1541") {
      names++;
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ name: "acme-agents-1541", account: ACME, owner: "0x2", chains: [] }));
    }
    if (req.url?.startsWith("/api/level1/name/")) {
      res.writeHead(404, { "content-type": "application/json" });
      return res.end('{"error":"no account has that name"}');
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
    `?policy=${ACME}`,
    { authorization: "Bearer k", "idempotency-key": "id-1", "content-type": "application/json", cookie: "s=1", "x-tameion-device": "d", "x-forwarded-for": "1.2.3.4" },
    '{"a":1}',
  );
  assert.equal(r.status, 200);
  assert.equal(await r.text(), '{"ok":true}');
  assert.equal(r.headers.get("idempotent-replay"), "true", "the service's own reply headers come back");
  assert.equal(seen.method, "POST");
  assert.equal(seen.url, `/api/level1/pay?policy=${ACME}`);
  assert.equal(seen.body, '{"a":1}');
  assert.equal(seen.headers.authorization, "Bearer k");
  assert.equal(seen.headers["idempotency-key"], "id-1");
  assert.equal(seen.headers["content-type"], "application/json");
  for (const h of ["cookie", "x-tameion-device", "x-forwarded-for"]) assert.equal(seen.headers[h], undefined, `${h} is not forwarded`);
});

test("passes the service's refusal through unchanged", async () => {
  const r = await call("GET", ["level1", "inbox"], `?policy=${ACME}`);
  assert.equal(r.status, 404);
  assert.equal(((await r.json()) as { error: { code: string } }).error.code, "not_found");
});

test("a name in the query, the tenant path or a JSON body reaches the service as the address; other bytes untouched", async () => {
  await call("GET", ["level1", "inbox"], "?policy=acme-agents-1541");
  assert.equal(seen.url, `/api/level1/inbox?policy=${ACME}`);
  await call("GET", ["level1", "tenant", "acme-agents-1541"]);
  assert.equal(seen.url, `/api/level1/tenant/${ACME}`);
  const body = '{"policy":"acme-agents-1541","nonce":123456789012345678901234567890,"inner":{"policy": "acme-agents-1541"}}';
  await call("POST", ["level1", "prepare-mode"], "", { "content-type": "application/json" }, body);
  assert.equal(seen.body, body.replaceAll("acme-agents-1541", ACME), "big numbers stay as sent: the swap is textual");
  const before = names;
  await call("GET", ["level1", "records"], "?policy=acme-agents-1541");
  assert.equal(names, before, "a name found once is not asked again");
  await call("GET", ["level1", "status"], `?policy=${ACME}`);
  assert.equal(seen.url, `/api/level1/status?policy=${ACME}`, "an address passes as it is");
});

test("an unknown name is refused before anything reaches the service", async () => {
  seen = { headers: {}, body: "" };
  const r = await call("GET", ["level1", "inbox"], "?policy=nobody-0000");
  assert.equal(r.status, 404);
  const e = ((await r.json()) as { error: { code: string; executed: string } }).error;
  assert.equal(e.code, "not_found");
  assert.equal(e.executed, "no");
  assert.equal(seen.url, "/api/level1/name/nobody-0000", "only the name lookup went out");
});

test("refuses a path that climbs out of /api", async () => {
  const r = await call("GET", ["level1", "..", "..", "healthz"]);
  assert.equal(r.status, 400);
  assert.equal(((await r.json()) as { error: { code: string } }).error.code, "invalid_body");
});

test("an event stream is marked no-transform so nothing holds its frames back", async () => {
  const r = await call("GET", ["level1", "events"], `?policy=${ACME}`);
  assert.equal(r.headers.get("cache-control"), "no-cache, no-transform");
});

test("the typed reads: NDJSON records parse in order; a refusal throws ApiError with its envelope", async () => {
  const events = await api.fetchRecord(ACME, "0x2");
  assert.deepEqual(
    events.map((e) => [e.seq, e.kind]),
    [
      [0, "paid"],
      [1, "refused"],
    ],
  );
  assert.equal(seen.url, `/api/level1/record?policy=${ACME}&root=0x2`);
  await assert.rejects(api.fetchInbox(ACME), (e: unknown) => {
    assert.ok(e instanceof api.ApiError);
    assert.equal(e.status, 404);
    assert.equal(e.refusal?.code, "not_found");
    assert.equal(e.refusal?.executed, "no");
    assert.equal(e.message, "no such tenant");
    return true;
  });
  assert.equal((await api.fetchTenant(ACME)).name, "acme-agents-1541", "the tenant route wraps the record as { tenant }");
  assert.equal((await api.resolveAccount("acme-agents-1541"))?.policy, ACME, "a name resolves to its account");
  assert.equal(await api.resolveAccount("nobody-0000"), null, "no account has that name: null, not an error");
  await api.fetchStatus("acme-agents-1541");
  assert.equal(seen.url, `/api/level1/status?policy=${ACME}`, "on the server a fetcher sends the address, never the name");
  await api.fetchRecall(ACME, ["evt:0", "evt:3"]);
  assert.equal(seen.url, `/api/level1/recall?policy=${ACME}&ids=evt%3A0%2Cevt%3A3`);
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
