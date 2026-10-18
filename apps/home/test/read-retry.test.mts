// A judge on the preview (Arc's public RPC rate-limits): a read that fails is tried once more after RETRY_MS before the
// page shows its error card, and a body that carries the service's last good read (stale + asOf) is shown, marked "as of".
// The reads are src/lib/api.ts's (the Owner home and the Agent view both read through its one GET), against a stand-in.
import "../scripts/tsx.mjs";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { after, before, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { Inbox } from "../src/lib/types.ts";

process.env.TZ = "Asia/Kuala_Lumpur";
const inbox = JSON.parse(readFileSync(new URL("./fixtures/owner-inbox.json", import.meta.url), "utf8")) as Inbox;
const POLICY = inbox.policy;
let answers: { status: number; body: unknown }[] = [];
let seen = 0;
const service = createServer((_req, res) => {
  seen++;
  const a = answers.shift() ?? { status: 500, body: { error: "no answer queued" } };
  res.writeHead(a.status, { "content-type": "application/json" });
  res.end(JSON.stringify(a.body));
});
before(async () => {
  await new Promise<void>((r) => service.listen(0, "127.0.0.1", r));
  process.env.TAMEION_URL = `http://127.0.0.1:${(service.address() as AddressInfo).port}`;
});
after(() => service.close());

const api = await import("../src/lib/api.ts");
const { OwnerView } = await import("../src/features/owner/owner-view.tsx");
const { servedAsOf } = await import("../src/features/owner/words.ts");
const render = (i: Inbox, staleAt: number | null) =>
  renderToStaticMarkup(createElement(OwnerView, { inbox: i, byRateDays: 12, halves: new Map(), nowSecs: 1792276548, staleAt }));
const unanswered = { ok: false, error: { code: "chain_unavailable", by: "chain", executed: "no", audience: "agent", meaning: "m", nextStep: "n", retryAfterSecs: 1, words: "the chain did not answer" } };

test("a read that fails once is tried again after RETRY_MS, and the figures render", async () => {
  answers = [{ status: 503, body: unanswered }, { status: 200, body: inbox }];
  seen = 0;
  const t0 = Date.now();
  const got = await api.fetchInbox(POLICY);
  assert.equal(seen, 2, "one retry");
  assert.ok(Date.now() - t0 >= api.RETRY_MS - 20, "after the pause");
  const html = render(got, servedAsOf(got));
  assert.match(html, /Your agents may spend 0\.05 USDC a day, 0\.005 at a time, only on Demo Vendor\./);
  assert.match(html, />Allowance left today</);
  assert.doesNotMatch(html, /as of /, "fresh figures carry no mark");
});

test("a second failure is the error; a refusal that cannot change on a retry is not retried", async () => {
  answers = [{ status: 502, body: unanswered }, { status: 502, body: unanswered }];
  seen = 0;
  await assert.rejects(api.fetchInbox(POLICY), (e: unknown) => e instanceof api.ApiError && e.status === 502);
  assert.equal(seen, 2);
  answers = [{ status: 404, body: { ok: false, error: { ...unanswered.error, code: "not_found", words: "no such tenant" } } }];
  seen = 0;
  await assert.rejects(api.fetchInbox(POLICY), (e: unknown) => e instanceof api.ApiError && e.status === 404);
  assert.equal(seen, 1, "a 404 is asked once");
});

test("a body with the service's last good read renders at full ink, marked as of its time, with no error", async () => {
  const asOf = 1792276530; // 06:35:30 +08, unix seconds
  answers = [{ status: 200, body: { ...inbox, stale: true, asOf } }];
  seen = 0;
  const got = await api.fetchInbox(POLICY);
  assert.equal(servedAsOf(got), asOf * 1000);
  const html = render(got, servedAsOf(got));
  assert.match(html, /as of 06:35/);
  assert.match(html, />Allowance left today</);
  assert.doesNotMatch(html, /did not answer/, "the service answered: no silent-service line");
  // the same body on a refusal status is the answer too, at once
  answers = [{ status: 503, body: { ...inbox, stale: true, asOf: "2026-10-18T06:35:30+08:00" } }];
  seen = 0;
  const again = await api.fetchInbox(POLICY);
  assert.equal(seen, 1);
  assert.equal(servedAsOf(again), asOf * 1000);
});
