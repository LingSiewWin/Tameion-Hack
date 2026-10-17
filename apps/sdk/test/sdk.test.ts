import { test } from "node:test";
import assert from "node:assert/strict";
import { TameionClient, EXPECTED_CONTRACT_VERSION, newOperationId, isOperationId, type SavedOperation } from "../src/client.js";
import { run, type Io } from "../src/cli.js";

const POLICY = "0xdaf055fff0ad86f112a5f7f9055eee4e5a9ad4c1";
const TOKEN = "s3cr3t-token-value-0123456789abcdef";

type Step = Response | Error;
function fakeFetch(steps: Step[]) {
  const calls: { url: string; method: string; headers: Record<string, string>; body?: string }[] = [];
  const f = async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({
      url: String(url),
      method: init?.method ?? "GET",
      headers: (init?.headers ?? {}) as Record<string, string>,
      ...(typeof init?.body === "string" ? { body: init.body } : {}),
    });
    const s = steps.shift();
    if (!s) throw new Error("the test gave no more replies");
    if (s instanceof Error) throw s;
    return s;
  };
  return { fetch: f as typeof fetch, calls };
}
const json = (status: number, body: unknown, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json", ...headers } });
const refusal = (status: number, code: string, executed: string, extra: Record<string, unknown> = {}) =>
  json(status, { ok: false, contractVersion: EXPECTED_CONTRACT_VERSION, error: { code, by: "service", executed, audience: "agent", meaning: code, ...extra } });
const paid = () => json(200, { paid: true, purchase: { amount: "0.001000 USDC", seller: "0xee80", status: "paid", tx: "0xabc" } });
const lost = () => Object.assign(new Error("fetch failed"), { name: "TypeError" });

function client(steps: Step[], extra: Partial<ConstructorParameters<typeof TameionClient>[0]> = {}) {
  const ff = fakeFetch(steps);
  const sleeps: number[] = [];
  const c = new TameionClient({
    baseUrl: "http://127.0.0.1:7880",
    token: TOKEN,
    fetch: ff.fetch,
    sleep: async (ms) => void sleeps.push(ms),
    onContractMismatch: () => {},
    ...extra,
  });
  return { c, calls: ff.calls, sleeps };
}
const buyInput = (operationId: string) => ({ policy: POLICY, url: "http://127.0.0.1:7918/v1/mid?coin=ETH", maxUsdc: "0.001", reason: "price", operationId });

test("https rule: plain http only on this machine", () => {
  for (const ok of ["https://tameion.example", "http://localhost:7880", "http://127.0.0.1:7880", "http://[::1]:7880"]) {
    assert.doesNotThrow(() => new TameionClient({ baseUrl: ok }), ok);
  }
  for (const bad of ["http://tameion.example", "http://10.0.0.2:7880", "ftp://localhost", "not a url"]) {
    assert.throws(() => new TameionClient({ baseUrl: bad }), bad);
  }
});

test("operationId: op_ + UUID, checked before anything is sent", async () => {
  const id = newOperationId();
  assert.ok(isOperationId(id) && id.startsWith("op_") && id.length === 39);
  for (const bad of ["", "op_", "op_ab", "xx_12345678", "op_has space", `op_${"a".repeat(126)}`]) assert.equal(isOperationId(bad), false, bad);
  const { c, calls } = client([]);
  await assert.rejects(() => c.buy(buyInput("pay-1")));
  assert.equal(calls.length, 0);
});

test("a spend sends the operationId as Idempotency-Key, the token as Bearer, and saves before sending", async () => {
  const order: string[] = [];
  const ff = fakeFetch([paid()]);
  const c = new TameionClient({
    baseUrl: "http://localhost:7880/",
    token: TOKEN,
    fetch: (async (u: string, i: RequestInit) => (order.push("send"), ff.fetch(u, i))) as typeof fetch,
    save: (op: SavedOperation) => void order.push(`save ${op.operationId} ${op.sends}`),
  });
  const id = newOperationId();
  const r = await c.buy(buyInput(id));
  assert.ok(r.ok);
  assert.deepEqual(order, [`save ${id} 0`, "send"]);
  const call = ff.calls[0]!;
  assert.equal(call.url, "http://localhost:7880/api/level1/buy");
  assert.equal(call.headers["idempotency-key"], id);
  assert.equal(call.headers.authorization, `Bearer ${TOKEN}`);
  assert.deepEqual(JSON.parse(call.body!), { method: "GET", policy: POLICY, url: buyInput(id).url, maxUsdc: "0.001", reason: "price" });
  assert.equal(c.operation(id)?.executed, "yes");
});

test("a lost reply is resent once with the same id and body; the replay is delivered", async () => {
  const { c, calls } = client([lost(), json(200, { paid: true }, { "idempotent-replayed": "true" })]);
  const id = newOperationId();
  const r = await c.buy(buyInput(id));
  assert.ok(r.ok && r.replayed);
  assert.equal(calls.length, 2);
  assert.equal(calls[0]!.headers["idempotency-key"], calls[1]!.headers["idempotency-key"]);
  assert.equal(calls[0]!.body, calls[1]!.body);
});

test("lost twice: executed unknown, never a third send, not even on a later call", async () => {
  const { c, calls } = client([lost(), new Response("<html>bad gateway</html>", { status: 502 })]);
  const id = newOperationId();
  const r = await c.buy(buyInput(id));
  assert.ok(!r.ok);
  assert.equal(r.error.executed, "unknown");
  assert.equal(calls.length, 2, "bad JSON counts as a lost reply");
  const again = await c.buy(buyInput(id));
  assert.ok(!again.ok && again.error.executed === "unknown");
  assert.equal(calls.length, 2);
});

test("executed yes: the same id is never sent again", async () => {
  const { c, calls } = client([paid()]);
  const id = newOperationId();
  const first = await c.buy(buyInput(id));
  const second = await c.buy(buyInput(id));
  assert.ok(first.ok && second.ok);
  assert.equal(calls.length, 1);
});

test("the same id with a different body is refused here, unsent", async () => {
  const { c, calls } = client([paid()]);
  const id = newOperationId();
  await c.buy(buyInput(id));
  const r = await c.buy({ ...buyInput(id), maxUsdc: "0.002" });
  assert.ok(!r.ok && r.error.executed === "no");
  assert.equal(calls.length, 1);
});

test("rate_limited with executed no: one resend after retryAfterSecs, clamped to 1-60 s", async () => {
  for (const [after, ms] of [[120, 60_000], [0, 1_000], [5, 5_000]] as const) {
    const { c, calls, sleeps } = client([refusal(429, "rate_limited", "no", { retryAfterSecs: after }), refusal(429, "rate_limited", "no", { retryAfterSecs: after })]);
    const r = await c.buy(buyInput(newOperationId()));
    assert.ok(!r.ok && r.error.code === "rate_limited");
    assert.equal(calls.length, 2, "once, not again");
    assert.deepEqual(sleeps, [ms]);
  }
  const ok = client([refusal(409, "idempotency_pending", "no", { retryAfterSecs: 2 }), paid()]);
  assert.ok((await ok.c.buy(buyInput(newOperationId()))).ok);
  assert.equal(ok.calls.length, 2);
});

test("no resend for other refusals, for executed unknown, or for a reply without a code", async () => {
  for (const reply of [
    refusal(409, "payee_not_approved", "no"),
    refusal(429, "rate_limited", "unknown", { retryAfterSecs: 1 }),
    json(409, { error: "a request with this Idempotency-Key is still running" }),
  ]) {
    const { c, calls } = client([reply]);
    const r = await c.buy(buyInput(newOperationId()));
    assert.ok(!r.ok);
    assert.equal(calls.length, 1);
  }
});

test("old string errors: the string is the meaning; a 5xx on a spend is unknown, on a read it is no", async () => {
  const { c } = client([json(409, { error: "the seller is not one of your approved payees" }), json(502, { error: "the chain did not answer" }), json(502, { error: "the chain did not answer" })]);
  const a = await c.buy(buyInput(newOperationId()));
  assert.ok(!a.ok && a.error.code === undefined && a.error.meaning === "the seller is not one of your approved payees" && a.error.executed === "no");
  const b = await c.buy(buyInput(newOperationId()));
  assert.ok(!b.ok && b.error.executed === "unknown");
  const i = await c.inbox(POLICY);
  assert.ok(!i.ok && i.error.executed === "no");
});

test("the envelope: error.code and its fields are read", async () => {
  const { c } = client([refusal(409, "payee_not_approved", "no", { by: "mandate", audience: "user", nextStep: "Ask the owner.", words: "w" })]);
  const r = await c.buy(buyInput(newOperationId()));
  assert.ok(!r.ok);
  assert.deepEqual(r.error, { code: "payee_not_approved", by: "mandate", executed: "no", audience: "user", meaning: "payee_not_approved", nextStep: "Ask the owner.", words: "w" });
});

test("after a lost reply a refusal cannot say no", async () => {
  const { c } = client([lost(), refusal(409, "idempotency_pending", "no", { retryAfterSecs: 1 })]);
  const r = await c.buy(buyInput(newOperationId()));
  assert.ok(!r.ok && r.error.executed === "unknown");
});

test("contractVersion: warned once when it differs, never when equal or absent", async () => {
  const seen: string[] = [];
  const { c } = client(
    [json(200, { contractVersion: EXPECTED_CONTRACT_VERSION }), json(200, {}), json(200, { contractVersion: "2027-01-01.1" }), json(200, { contractVersion: "2027-01-01.1" })],
    { onContractMismatch: (got, want) => void seen.push(`${got}/${want}`) },
  );
  for (let i = 0; i < 4; i++) await c.status();
  assert.deepEqual(seen, [`2027-01-01.1/${EXPECTED_CONTRACT_VERSION}`]);
});

// ---- the CLI ----

function io(steps: Step[], over: Partial<Io> = {}) {
  const out: string[] = [];
  const err: string[] = [];
  const ff = fakeFetch(steps);
  const value: Io = {
    env: { TAMEION_POLICY: POLICY },
    out: (s) => void out.push(s),
    err: (s) => void err.push(s),
    readFile: async () => `${TOKEN}\n`,
    fileMode: async () => 0o600,
    fetch: ff.fetch,
    now: () => 1_792_224_000_000,
    ...over,
  };
  return { io: value, out, err, calls: ff.calls, all: () => [...out, ...err].join("\n") };
}
const INBOX = {
  policy: POLICY,
  mode: "Autopilot",
  balance: "0.095000 USDC",
  caps: { perAction: "0.010000 USDC", daily: "0.048000 USDC", floor: "0.000000 USDC", spent24h: "0.000000 USDC", leftNow: "0.010000 USDC" },
  payees: [{ address: "0xee80b9f69d80a04d4b2e803c426715aa8c1bb1f3", payableNow: true }],
  waiting: [{ kind: "bill", what: "approve a payment the agent prepared", amount: "0.001000 USDC", counterparty: "0xee80b9f69d80a04d4b2e803c426715aa8c1bb1f3" }],
  waitingCount: 1,
  typedBills: [{ payee: "0xee80b9f69d80a04d4b2e803c426715aa8c1bb1f3", amount: "0.001 USDC", everySecs: 2_592_000, nextDue: 1_792_219_285 }],
  standingOrders: [],
};

test("cli: exit 0 delivered, 1 refused, 2 usage", async () => {
  const a = io([json(200, INBOX)], { env: { TAMEION_POLICY: POLICY, TAMEION_TOKEN: TOKEN } });
  assert.equal(await run(["inbox"], a.io), 0);
  assert.match(a.out.join("\n"), /Autopilot · balance 0\.095000 USDC/);
  assert.equal(a.calls[0]!.headers.authorization, `Bearer ${TOKEN}`);

  const b = io([json(200, INBOX)]);
  assert.equal(await run(["bills"], b.io), 0);
  assert.match(b.out.join("\n"), /0\.001 USDC every 30 days to 0xee80/);

  const c = io([refusal(403, "token_scope", "no")]);
  assert.equal(await run(["inbox"], c.io), 1);
  assert.match(c.err.join("\n"), /refused \(token_scope, by service, executed: no\)/);

  const d = io([]);
  assert.equal(await run(["buy", "http://127.0.0.1:7918/x", "--max", "0.001"], d.io), 2, "--why is required");
  assert.equal(await run(["inbox", "--policy", "0x12"], d.io), 2);
  assert.equal(await run([], d.io), 2);
  assert.equal(d.calls.length, 0);
});

test("cli: top quotes the service's burn rate and runways, and needs a policy", async () => {
  const status = { policy: POLICY, rate: "0.001428 USDC", windowDays: 7, runway: { byRateDays: 28, firstShortfallDays: null }, forces: [{ at: 1_792_254_326, kind: "bill_added", counterparty: "0xe5e2d8dcc2038fd0a010a6e7845378b646e33854", deltaPerDay: "0.001428 USDC" }] };
  const a = io([json(200, status)]);
  assert.equal(await run(["top"], a.io), 0);
  assert.match(a.calls[0]!.url, /\/api\/level1\/status\?policy=0x/);
  assert.match(a.out.join("\n"), /at this burn \(0\.001428 USDC a day over the last 7 days\), the floor is 28 days away/);
  assert.match(a.out.join("\n"), /every bill in the next 90 days is covered/);
  assert.match(a.out.join("\n"), /bill_added 0xe5e2.* 0\.001428 USDC a day/);
  const quiet = io([json(200, { ...status, rate: "0.000000 USDC", runway: { byRateDays: null, firstShortfallDays: 0 }, forces: [] })]);
  assert.equal(await run(["top"], quiet.io), 0);
  assert.match(quiet.out.join("\n"), /no burn yet: nothing was paid in the last 7 days\n.*due today/);
  const none = io([], { env: {} });
  assert.equal(await run(["top"], none.io), 2);
});

test("cli: unknown command gets a did-you-mean and never echoes the input", async () => {
  const a = io([]);
  assert.equal(await run(["inbx"], a.io), 2);
  assert.equal(a.err.join("\n"), "unknown command; did you mean `tameion inbox`?");
  const b = io([]);
  assert.equal(await run(["zzzzzzzzqq"], b.io), 2);
  assert.ok(!b.all().includes("zzzzzzzzqq"));
  const c = io([]);
  assert.equal(await run(["inbox", "--frobnicate-xyz"], c.io), 2);
  assert.ok(!c.all().includes("frobnicate"));
});

test("cli: the token is never taken from argv and never printed", async () => {
  const a = io([]);
  assert.equal(await run(["inbox", "--token", TOKEN], a.io), 2);
  assert.equal(await run(["inbox", `--token=${TOKEN}`], a.io), 2);
  assert.ok(!a.all().includes(TOKEN));
  assert.equal(a.calls.length, 0);

  // A reply that carries the token back (a misbehaving proxy) is scrubbed, in prose and in --json.
  for (const flags of [[], ["--json"]]) {
    const b = io([json(401, { error: `bad key ${TOKEN}` })], { env: { TAMEION_POLICY: POLICY, TAMEION_TOKEN: TOKEN } });
    assert.equal(await run(["inbox", ...flags], b.io), 1);
    assert.ok(!b.all().includes(TOKEN), b.all());
    assert.ok(b.all().includes("[token]"));
  }
});

test("cli: --token-file warns when group or others can read it", async () => {
  const a = io([json(200, INBOX)], { fileMode: async () => 0o644 });
  assert.equal(await run(["inbox", "--token-file", "/k/agent.token"], a.io), 0);
  assert.match(a.err.join("\n"), /readable by group or others \(mode 644\)/);
  assert.equal(a.calls[0]!.headers.authorization, `Bearer ${TOKEN}`, "the file's newline is trimmed");
  const b = io([json(200, INBOX)]);
  assert.equal(await run(["inbox", "--token-file", "/k/agent.token"], b.io), 0);
  assert.equal(b.err.length, 0);
});

test("cli: https rule, --json raw reply, buy prints its operationId before sending", async () => {
  const a = io([]);
  assert.equal(await run(["inbox", "--url", "http://tameion.example"], a.io), 2);
  assert.equal(a.calls.length, 0);

  const b = io([json(200, INBOX)]);
  assert.equal(await run(["inbox", "--json"], b.io), 0);
  assert.deepEqual(JSON.parse(b.out.join("\n")), INBOX);

  const c = io([lost(), lost()]);
  assert.equal(await run(["buy", "http://127.0.0.1:7918/x", "--max", "0.001", "--why", "price"], c.io), 1);
  const id = /operation (op_[0-9a-f-]{36})/.exec(c.err[0]!)?.[1];
  assert.ok(id, c.err[0]);
  assert.equal(c.calls.length, 2);
  assert.ok(c.calls.every((x) => x.headers["idempotency-key"] === id));
  assert.match(c.err.join("\n"), new RegExp(`executed: unknown.*\\n.*resend with --op ${id}`, "s"));

  const d = io([json(200, { paid: true, purchase: { amount: "0.001000 USDC", seller: "0xee80", status: "paid", explorer: "https://sepolia.arbiscan.io/tx/0xabc" } }, { "idempotent-replayed": "true" })]);
  assert.equal(await run(["buy", "http://127.0.0.1:7918/x", "--max", "0.001", "--why", "price", "--op", id], d.io), 0);
  assert.equal(d.calls[0]!.headers["idempotency-key"], id);
  assert.match(d.out.join("\n"), /paid 0\.001000 USDC to 0xee80 · https:\/\/sepolia\.arbiscan\.io\/tx\/0xabc\nreplayed: /);
});

// ---- pay: an approved payee, now, under the same operationId rules as buy ----

const PAYEE = "0xee80b9f69d80a04d4b2e803c426715aa8c1bb1f3";
const payInput = (operationId: string) => ({ policy: POLICY, payee: PAYEE, amount: "0.001", why: "monthly hosting", operationId });
const payment = (extra: Record<string, unknown> = {}) =>
  json(200, {
    paid: true,
    payment: { at: 1, operationId: "op_x", actionHash: "0xah", policy: POLICY, payee: PAYEE, amount: "0.001000 USDC", minor: "1000", why: "monthly hosting", status: "paid", tx: "0xt", explorer: "https://sepolia.arbiscan.io/tx/0xt" },
    ...extra,
  });

test("pay: POST /api/level1/pay with {policy, payee, amount, why}, the operationId as Idempotency-Key; no id, nothing sent", async () => {
  const { c, calls } = client([payment()]);
  await assert.rejects(() => c.pay(payInput("")));
  assert.equal(calls.length, 0);
  const id = newOperationId();
  const r = await c.pay(payInput(id));
  assert.ok(r.ok && r.data.paid && !r.replayed);
  assert.equal(calls[0]!.url, "http://127.0.0.1:7880/api/level1/pay");
  assert.equal(calls[0]!.method, "POST");
  assert.equal(calls[0]!.headers["idempotency-key"], id);
  assert.equal(calls[0]!.headers.authorization, `Bearer ${TOKEN}`);
  assert.deepEqual(JSON.parse(calls[0]!.body!), { policy: POLICY, payee: PAYEE, amount: "0.001", why: "monthly hosting" });
  assert.ok((await c.pay(payInput(id))).ok);
  assert.equal(calls.length, 1, "executed yes: never sent again");
});

test("pay: the route's own replayed: true counts as a replay; lost replies follow the buy rules", async () => {
  const a = client([lost(), payment({ replayed: true })]);
  const r = await a.c.pay(payInput(newOperationId()));
  assert.ok(r.ok && r.replayed);
  assert.equal(a.calls.length, 2);
  assert.equal(a.calls[0]!.headers["idempotency-key"], a.calls[1]!.headers["idempotency-key"]);

  const b = client([lost(), lost()]);
  const id = newOperationId();
  const u = await b.c.pay(payInput(id));
  assert.ok(!u.ok && u.error.executed === "unknown");
  assert.ok(!(await b.c.pay(payInput(id))).ok);
  assert.equal(b.calls.length, 2, "never a third send");

  const d = client([refusal(502, "chain_unavailable", "unknown")]);
  const e = await d.c.pay(payInput(newOperationId()));
  assert.ok(!e.ok && e.error.executed === "unknown");
  assert.equal(d.calls.length, 1, "executed unknown from the service: not resent");
});

test("cli pay: prose, replay, --op, refusals, usage", async () => {
  const a = io([payment()]);
  assert.equal(await run(["pay", PAYEE, "0.001", "--why", "monthly hosting"], a.io), 0);
  const id = /operation (op_[0-9a-f-]{36})/.exec(a.err[0]!)?.[1];
  assert.ok(id);
  assert.equal(a.calls[0]!.headers["idempotency-key"], id);
  assert.deepEqual(a.out, [`paid 0.001000 USDC to ${PAYEE} · https://sepolia.arbiscan.io/tx/0xt`, `operation ${id}`]);

  const b = io([payment({ replayed: true })]);
  assert.equal(await run(["pay", PAYEE, "0.001", "--why", "monthly hosting", "--op", id], b.io), 0);
  assert.equal(b.calls[0]!.headers["idempotency-key"], id);
  assert.match(b.out.join("\n"), /replayed: this operationId was already paid; nothing was paid again/);

  const c = io([json(200, { paid: false, payment: { payee: PAYEE, amount: "0.001000 USDC", status: "unconfirmed", tx: "0xt", note: "the receipt does not match" } })]);
  assert.equal(await run(["pay", PAYEE, "0.001", "--why", "x"], c.io), 0);
  assert.match(c.out[0]!, /^sent 0\.001000 USDC .* did not match .*do not pay again/);

  const d = io([refusal(409, "payee_cooling_off", "no", { by: "payee", audience: "agent", nextStep: "Wait until it can be paid." })]);
  assert.equal(await run(["pay", PAYEE, "0.001", "--why", "x"], d.io), 1);
  assert.equal(d.calls.length, 1);
  assert.match(d.err.join("\n"), /refused \(payee_cooling_off, by payee, executed: no\): payee_cooling_off\nnext: Wait until it can be paid\./);

  const e = io([]);
  assert.equal(await run(["pay", "0x12", "0.001", "--why", "x"], e.io), 2);
  assert.equal(await run(["pay", PAYEE, "0", "--why", "x"], e.io), 2);
  assert.equal(await run(["pay", PAYEE, "1.1234567", "--why", "x"], e.io), 2);
  assert.equal(await run(["pay", PAYEE, "0.001"], e.io), 2);
  assert.equal(await run(["pay", PAYEE, "0.001", "--why", "x", "--op", "not-an-op"], e.io), 2);
  assert.equal(e.calls.length, 0);
  const f = io([]);
  assert.equal(await run(["py"], f.io), 2);
  assert.equal(f.err.join("\n"), "unknown command; did you mean `tameion pay`?");
});

// ---- recall (docs/117 §8) ----

const ROOT = `0x${"ab".repeat(32)}`;
type Result = { id: string; status: string; words: string; [k: string]: unknown };
const recalled = (results: Result[]) =>
  json(200, {
    ok: true,
    contractVersion: EXPECTED_CONTRACT_VERSION,
    policy: POLICY,
    chain: "arc-testnet",
    record: { root: ROOT, latest: true, events: 3, anchorTx: `0x${"cd".repeat(32)}`, at: 1 },
    chainRead: { how: "rootOf", chain: "arc-testnet" },
    results,
    verified: results.filter((r) => r.status === "verified").length,
    unverified: results.filter((r) => r.status === "unverified").length,
    unknown: results.filter((r) => r.status === "unknown").length,
  });
const settled: Result = {
  id: "evt:1",
  status: "verified",
  words: "the event's bytes hash into the root the chain holds",
  root: ROOT,
  event: { kind: "SETTLEMENT", seq: 1, at: 1, payee: PAYEE, amount: "0.050000 USDC", note: "Paid the hosting bill." },
};

test("recall: one GET with escaped ids and the root; a read is never resent", async () => {
  const { c, calls } = client([recalled([settled])]);
  const r = await c.recall(POLICY, ["evt:1", " evt:2"], ROOT);
  assert.ok(r.ok);
  assert.equal(r.data.results[0]!.event?.kind, "SETTLEMENT");
  assert.equal(calls.length, 1);
  assert.equal(calls[0]!.method, "GET");
  assert.equal(calls[0]!.url, `http://127.0.0.1:7880/api/level1/recall?policy=${POLICY}&ids=evt%3A1,evt%3A2&root=${ROOT}`);
  const { c: c2, calls: calls2 } = client([recalled([settled])]);
  await c2.recall(POLICY, ["evt:1"]);
  assert.ok(!calls2[0]!.url.includes("root="), "no root: the latest record");
});

test("cli recall: exit 0 only when every id verified; ids checked before sending", async () => {
  const a = io([recalled([settled])]);
  assert.equal(await run(["recall", "evt:1"], a.io), 0);
  assert.match(a.out.join("\n"), /record 0xabab…abab \(latest, 3 events\) · chain read: rootOf on arc-testnet/);
  assert.match(a.out.join("\n"), /evt:1 verified SETTLEMENT 0\.050000 USDC to 0x.*— Paid the hosting bill\./);

  const b = io([recalled([settled, { id: "evt:9", status: "unknown", by: "record", code: "recall_unknown", words: "the record holds 3 events; evt:9 is not one of them" }])]);
  assert.equal(await run(["recall", "evt:1,evt:9"], b.io), 1, "delivered is not verified");
  assert.match(b.out.join("\n"), /evt:9 unknown \(recall_unknown, by record\): the record holds 3 events/);
  assert.match(b.out.join("\n"), /verified 1 · unverified 0 · unknown 1/);

  const c = io([recalled([settled])]);
  assert.equal(await run(["recall", "evt:1", "--json"], c.io), 0);
  assert.equal(JSON.parse(c.out.join("\n")).results[0].status, "verified");

  const d = io([]);
  assert.equal(await run(["recall"], d.io), 2);
  assert.equal(await run(["recall", "evt:01"], d.io), 2);
  assert.equal(await run(["recall", "3"], d.io), 2);
  assert.equal(await run(["recall", "evt:1", "--root", "0x12"], d.io), 2);
  assert.equal(d.calls.length, 0);
});
