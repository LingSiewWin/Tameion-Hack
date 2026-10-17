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
