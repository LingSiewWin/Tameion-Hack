// The Owner home's words and the owner calls it signs. Call vectors are the lane service's own: calldata its prepare-*
// routes returned and a challenge its /api/level1/challenge computed for the demo account northwind-agents (2026-10-18).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { calls, challengeFor, checkSettle, hex, invoiceDecideChallenge, keccak256 } from "../src/features/owner/owner-call.ts";
import { amount, authority, dateWords, facts, halvesWords, lines, minor, money, needs, payeesWords, plain } from "../src/features/owner/words.ts";
import type { Inbox, Status } from "../src/lib/types.ts";

process.env.TZ = "Asia/Kuala_Lumpur";
const inbox = JSON.parse(readFileSync(new URL("./fixtures/owner-inbox.json", import.meta.url), "utf8")) as Inbox;
const status = JSON.parse(readFileSync(new URL("./fixtures/owner-status.json", import.meta.url), "utf8")) as Status;
const POLICY = "0x907a9f4fe960017ee9c31550d2f1956e07e2df4f";
const VENDOR = "0xe5e2d8dcc2038fd0a010a6e7845378b646e33854";

test("money is the service's figure, trimmed to the shortest exact form", () => {
  assert.equal(money("0.050000 USDC"), "0.05 USDC");
  assert.equal(money("100.000000 USDC"), "100 USDC");
  assert.equal(money("1234.500000"), "1,234.5 USDC");
  assert.equal(money("0.000285 USDC"), "0.000285 USDC");
  assert.equal(money("not money"), "");
  assert.equal(minor("0.002 USDC"), 2000n);
  assert.equal(amount(50_000n), "0.05");
});

test("the authority sentence is generated from the mandate", () => {
  assert.equal(authority(inbox), "Your agents may spend 0.05 USDC a day, 0.005 at a time, only on Demo Vendor.");
  const two = { ...inbox, payees: [...inbox.payees!, { address: "0x" + "ab".repeat(20), approvedAt: 0, payableFrom: 0, payableNow: true }] };
  assert.equal(payeesWords(two), "only on Demo Vendor and a payee you approved");
  assert.equal(payeesWords({ ...inbox, payees: [] }), "on no one yet");
  assert.equal(authority({ ...inbox, mode: "Propose" }), "Your agents may spend 0.05 USDC a day, 0.005 at a time, only on Demo Vendor, and each payment waits for your tap.");
  assert.equal(authority({ ...inbox, mode: "Watch" }), "The agent is stopped. It pays nothing until you start it again.");
});

test("dates in words, in the viewer's time", () => {
  const now = 1792276548; // 2026-10-18 06:35:48 +08
  assert.equal(dateWords(1792276488, now), "Today 06:34");
  assert.equal(dateWords(now - 86_400, now), "Yesterday");
  assert.equal(dateWords(now - 5 * 86_400, now), "13 Oct");
});

test("the four facts read the service; only the two named combinations are computed", () => {
  const now = 1792276548;
  const f = facts(inbox, status.runway?.byRateDays, now);
  assert.deepEqual(f.map((x) => x.label), ["Allowance left today", "Runway at this burn", "Needs you", "Paid this week"]);
  assert.deepEqual(f[0], { label: "Allowance left today", figure: "0.032", unit: "USDC" }); // 0.05 daily − 0.018 spent (contract)
  assert.deepEqual(f[1], { label: "Runway at this burn", figure: "12", unit: "days" });
  assert.deepEqual(f[2], { label: "Needs you", figure: "0", attention: false });
  assert.deepEqual(f[3], { label: "Paid this week", figure: "0.016", unit: "USDC" }); // eight 0.002 payments in paid[]
  assert.deepEqual(facts(inbox, null, now)[1], { label: "Runway at this burn", words: "No burn yet" });
});

test("rows name the payee, never an address, and say the state in words", () => {
  const rows = lines(inbox, 1792276548);
  assert.equal(rows.length, 8);
  assert.equal(rows[0].payee, "Demo Vendor");
  assert.equal(rows[0].amount, "0.002 USDC");
  assert.equal(rows[0].why, "Paid by the agent: a bill the owner typed: 0.002 USDC");
  assert.equal(rows[0].state, "Paid");
  assert.equal(rows[0].sealed, false, "no projection on this service: never sealed");
  const unnamed = { ...inbox, parties: {} };
  assert.equal(lines(unnamed, 1792276548)[0].payee, "a payee you approved");
  assert.equal(plain(unnamed, `paid ${VENDOR} tx 0x${"9".repeat(64)}`), "Paid ••3854 tx a reference");
});

test("sealed only when all three halves verify; the sheet says which are there", () => {
  const p = inbox.paid[0];
  const k = `${p.at}|${p.counterparty}|${minor(p.amount)}`;
  const h = { agent: true, payee: true, witness: true, root: "r", seq: 2 };
  assert.equal(lines(inbox, 1792276548, new Map([[k, h]]))[0].state, "Paid and sealed");
  assert.equal(lines(inbox, 1792276548, new Map([[k, { ...h, witness: false }]]))[0].sealed, false);
  assert.equal(halvesWords({ ...h, payee: false, witness: false }, "Demo Vendor"), "Signed by the agent. Not yet signed by Demo Vendor or the witness.");
  assert.equal(halvesWords(h, "Demo Vendor"), "Sealed: the agent, Demo Vendor and the witness each signed this record.");
});

test("needs you: one sentence and one action per item", () => {
  const n = needs({
    ...inbox,
    sellersToApprove: [{ seller: VENDOR, amount: "0.002 USDC", to: POLICY, data: "0x" }],
    needsYou: [{ counterparty: VENDOR, amount: "0.010000 USDC", why: "per-action-cap: over", to: POLICY, data: "0x" }],
  });
  assert.deepEqual(n.map((x) => [x.sentence, x.action]), [
    ["Let the agent pay Demo Vendor.", "Approve"],
    ["0.01 USDC to Demo Vendor is more than one payment may be.", "Raise the cap"],
  ]);
});

test("keccak-256 matches the published vectors", () => {
  assert.equal(hex(keccak256(new Uint8Array())), "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470");
  assert.equal(hex(keccak256(new TextEncoder().encode("abc"))), "0x4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45");
});

test("the calls built here are the ones the service prepared", () => {
  assert.equal(calls.setMode(2), "0x21175b4a0000000000000000000000000000000000000000000000000000000000000002");
  assert.equal(
    calls.setPayees([VENDOR], true),
    "0x48a56503000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000001000000000000000000000000e5e2d8dcc2038fd0a010a6e7845378b646e33854",
  );
  assert.equal(
    calls.setLimits(5000n, 50_000n, 0n),
    "0x189ae5f20000000000000000000000000000000000000000000000000000000000001388000000000000000000000000000000000000000000000000000000000000c3500000000000000000000000000000000000000000000000000000000000000000",
  );
});

test("the challenge is the one the service computed (PasskeyPrincipal.challengeFor)", () => {
  const c = challengeFor({ chainId: 421614, owner: "0xf31d2ab18b605eb85337154e0229d076dca33144", nonce: "3", target: POLICY, data: calls.setMode(0) });
  assert.equal(c, "0x6e773878fa730a44f85cb8d30219a5cc3d32af54492a47777cfe2b145f0824b8");
  assert.notEqual(challengeFor({ chainId: 421614, owner: "0xf31d2ab18b605eb85337154e0229d076dca33144", nonce: "3", target: POLICY, data: calls.setMode(2) }), c);
});

test("a prepared payment must pay the payee and amount on the page", () => {
  const settle = "0x" + "c0ffee00".slice(0, 8);
  assert.throws(() => checkSettle(settle + "00".repeat(96), VENDOR, 2000n), /not a payment/);
  assert.match(invoiceDecideChallenge(421614, POLICY, "0x" + "11".repeat(32), true, 1792276548), /^0x[0-9a-f]{64}$/);
});
