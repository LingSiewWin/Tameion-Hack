// What the passkey signs is decided in the page (docs/60 P-72). The challenge
// vectors were printed by a verbatim copy of the Rust functions
// (crates/operator/src/level1.rs token_challenge, token_revoke_challenge,
// notify_challenge, invoice_decide_challenge; push.rs challenge) built against
// alloy-primitives 1.7.3; the invoice pair also comes from the Rust test
// `the_decision_challenge_matches_its_documented_bytes` and `cast keccak`.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";
import {
  ANY,
  build,
  checkCall,
  invoiceDecideChallenge,
  minor,
  notifyChallenge,
  pairCode,
  PAY,
  PAYEES,
  pushChallenge,
  reserveTokenChallenge,
  safeHref,
  tokenChallenge,
  tokenRevokeChallenge,
} from "../src/lib/guard.ts";

const P = "0x1645951793f7ca2ae36d7a3eeae90ef93e1932ab";
const T = 1_791_900_000;

test("off-chain challenges match the Rust byte for byte", () => {
  assert.equal(tokenChallenge(5_042_002, P, ["read", "run"], "ops bot é", 1_799_676_000, T), "0xd8017bd895d3a5d21bed3b968a69f7c9d015a776dc67418ad949346a162acdd3");
  assert.equal(tokenRevokeChallenge(5_042_002, P, "tok-1a2b3c4d", T), "0xd3e4c5106e4f36ce171263c09cb31ccea93f36eb14941952215b225fb21e2815");
  assert.equal(notifyChallenge(5_042_002, P, "https://x.example/hook", "42", T), "0xbaab3ba069398fdba73a44b5ee8aa689114617555de19c5e51ecdb28917f586f");
  assert.equal(pushChallenge(421_614, P, "https://fcm.googleapis.com/fcm/send/abc", "BPk3", "c2VjcmV0", T), "0xf468dffbf13f38935d28241c196f58c257f4d83f0ed46b1e36a017c9892d23df");
  const id = "0x5c1b6c8b6b2f1a1e0a4d4f1c4e1b5a7d3c2b1a0f9e8d7c6b5a4938271605f4e3";
  assert.equal(invoiceDecideChallenge(421_614, P, id, true, T), "0x97c47bbf8be24e6fac64a5c8b9238953cc9d295e9cec9a2f7e077d3dc7f1adf5");
  assert.equal(invoiceDecideChallenge(421_614, P, id, false, T), "0x424f4be72865e4eaeedf54c254d6e18724710c9944ab063ff69c76257638fd34");
  const P2 = "0x35a2e2701d4d72be257bf7708c67d1433c60342f";
  const id2 = "0x8b9badaf6d88b27f468085fc9e1558d02152496bf86113f2909554ba95a9543a";
  assert.equal(invoiceDecideChallenge(421_614, P2, id2, true, 1_792_003_905), "0x724f3c25f4d9271c46dfff4adc4b5713e5d7ec53e2fb735a87481c1991ce0350");
  assert.equal(invoiceDecideChallenge(421_614, P2, id2, false, 1_792_003_905), "0x9ae991b8cc4f74a7dc47299af29c5885d7e2e6f67487f70240db00b52b7bf63a");
  // Rust `the_reserve_token_challenge_matches_its_documented_bytes` and `cast keccak` (docs/60 P-87).
  assert.equal(reserveTokenChallenge(421_614, "0x00000000000000000000000000000000000000d1", 0x6700_0000), "0x44f87f27ab1211749325aa73f4c89335569eeaa17fb35672b066790ebf5192e5");
});

const acct = { policy: P, owner: "0xf6731ff8b53c6a2cd808b592dab898a530257ddf" };
const A = "0x1111111111111111111111111111111111111111";
const B = "0x2222222222222222222222222222222222222222";
const H = `0x${"ab".repeat(32)}` as const;

test("a button signs only its own function, on its own contract", () => {
  const pause = build("setMode", [0]);
  assert.doesNotThrow(() => checkCall(acct, P, pause, { fn: ["setMode"], data: pause }));
  // the service hands a withdraw to the Pause button
  const w = build("withdraw", [B, 5_000_000n]);
  assert.throws(() => checkCall(acct, P, w, { fn: ["setMode"] }), /signs setMode, but the call handed to it is withdraw/);
  // same function, different bytes (Autopilot instead of Pause)
  assert.throws(() => checkCall(acct, P, build("setMode", [2]), { fn: ["setMode"], data: pause }), /differs/);
  // right call, wrong contract
  assert.throws(() => checkCall(acct, B, pause, { fn: ["setMode"] }), /not this account's/);
  const g = build("setGuardians", [[A], 1]);
  assert.doesNotThrow(() => checkCall(acct, acct.owner, g, { fn: ["setGuardians"], data: g }));
  assert.throws(() => checkCall(acct, P, g, { fn: ["setGuardians"] }), /not this account's/);
  // the spare key is set on the owner contract, never the policy
  const r = build("setRecovery", [A]);
  assert.doesNotThrow(() => checkCall(acct, acct.owner, r, { fn: ["setRecovery"], data: r }));
  assert.throws(() => checkCall(acct, P, r, { fn: ["setRecovery"] }), /not this account's/);
  assert.throws(() => checkCall(acct, acct.owner, build("setRecovery", [B]), { fn: ["setRecovery"], data: r }), /differs/);
  // cancelling a pending replacement key (docs/104) goes to the owner contract, and the Cancel button signs nothing else
  const c = build("cancelRecovery", []);
  assert.doesNotThrow(() => checkCall(acct, acct.owner, c, { fn: ["cancelRecovery"], data: c }));
  assert.throws(() => checkCall(acct, P, c, { fn: ["cancelRecovery"] }), /not this account's/);
  assert.throws(() => checkCall(acct, acct.owner, g, { fn: ["cancelRecovery"] }), /signs cancelRecovery, but the call handed to it is setGuardians/);
  assert.throws(() => checkCall(acct, P, "0xdeadbeef", { fn: ANY }), /cannot read/);
});

test("a pay row signs exactly its payee and amount", () => {
  const pay = build("settleAsPrincipalIf", [A, minor("1.500000 USDC"), H, H, 1n]);
  assert.doesNotThrow(() => checkCall(acct, P, pay, { fn: PAY, rows: [{ to: A, amount: 1_500_000n }] }));
  assert.throws(() => checkCall(acct, P, pay, { fn: PAY, rows: [{ to: B, amount: 1_500_000n }] }), /different payee or amount/);
  assert.throws(() => checkCall(acct, P, pay, { fn: PAY, rows: [{ to: A, amount: 1_000_000n }] }), /different payee or amount/);
  // approve vs stop
  const stop = build("setPayeesIf", [[A], false, H, 1n]);
  assert.doesNotThrow(() => checkCall(acct, P, stop, { fn: PAYEES, attested: false, rows: [{ to: A }] }));
  assert.throws(() => checkCall(acct, P, stop, { fn: PAYEES, attested: true, rows: [{ to: A }] }), /stop the payee, not approve/);
  // an extra payee slipped into an approval
  const two = build("setPayees", [[A, B], true]);
  assert.throws(() => checkCall(acct, P, two, { fn: PAYEES, attested: true, rows: [{ to: A }] }), /different payee/);
  // a reviewed run: every row must be one the page shows
  const run = build("settleRunIf", [[A, B], [1n, 2n], [H, H], H, 1n]);
  assert.doesNotThrow(() => checkCall(acct, P, run, { fn: ["settleRunIf"], within: [{ to: A, amount: 1n }, { to: B, amount: 2n }] }));
  assert.throws(() => checkCall(acct, P, run, { fn: ["settleRunIf"], within: [{ to: A, amount: 1n }] }), /not on this page/);
});

test("the pairing code is 48 bits and the same as the classic page's", () => {
  const qx = "0x5ceabc7d690b369ff43d5df67ec4f1f069d55c4bcab5d0f836fed60a3400ef1e";
  const qy = "0xff8b64a10678280334a9e215c712db44838248a6f02a6a690c6d2f4720228101";
  const code = pairCode(qx, qy);
  assert.match(code, /^[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}$/);
  // apps/classic/home.html computes it with /level1/call.js's own keccak
  const TC = createRequire(import.meta.url)("../../classic/passkey/call.js");
  const x = TC.hex(TC.keccak256(TC.bytesOf(qx.slice(2) + qy.slice(2)))).slice(2, 14).toUpperCase();
  assert.equal(code, `${x.slice(0, 4)}-${x.slice(4, 8)}-${x.slice(8, 12)}`);
});

test("only https receipts open", () => {
  assert.equal(safeHref("https://sepolia.arbiscan.io/tx/0x1"), "https://sepolia.arbiscan.io/tx/0x1");
  assert.equal(safeHref("javascript:alert(1)"), undefined);
  assert.equal(safeHref("http://x"), undefined);
});

test("the desk's party challenge is the Rust bytes (desk::party_challenge)", async () => {
  const { deskPartyChallenge } = await import("../src/lib/guard.ts");
  assert.equal(
    deskPartyChallenge(421614, "0xcd8d71d97c3a946a14ac3fb92db4fb86e2974308", "Supplier", "0x85e1c3924ed413a7e24e3388ed2c8697fd6b8904", 1792080000),
    "0x65caa0468d12f8c25cf6907f40e52abda88fcb8518185efde9fd105f328aa883",
  );
});

test("the desk's owner challenges are the Rust bytes (desk::{base,control,step}_challenge, event_digest)", async () => {
  const g = await import("../src/lib/guard.ts");
  const P = "0xcd8d71d97c3a946a14ac3fb92db4fb86e2974308";
  const m = { deal: "G-1", kind: "bought", goods: "gold", quantity: "10.000", usdc: "1334.45", counterparty: "Workshop", document: "RCPT-1", at: 1792080000 };
  assert.equal(g.deskBaseChallenge(421614, P, "gold", "100.000", 1792080000), "0x905cd68699780501fcdb2cf2398368f9ebe2b63be1a3cc732f0a17413ce40770");
  assert.equal(g.deskControlChallenge(421614, P, "held by the lender", 1792080000), "0x45958132365bccdd313e0a5dae8b8e9d1398e73180bdfef5efae171f6511ff62");
  assert.equal(g.deskStepDigest(421614, P, m), "0x2449d514c067381b192aff76f298745acf6fae3b0396c6c73678ac62caf35575");
  assert.equal(g.deskStepChallenge(421614, P, m, 1792080000), "0x2ad9d672ace341b3a1895860b3085bc92f02e97dd1cc2116ba6481d12ebfdfeb");
  assert.equal(g.deskOffsetChallenge(421614, P, "0.321507", 1792080000), "0xad7c937d74cea31d279418f85c0efd4660834ee9609af502dad3c8356f5b88cd");
  assert.equal(g.deskVenueChallenge(421614, P, "0x557e1e07652b75abaa667223b11704165fc94d09", 1792080000), "0x882ca155c72b418b1a99c79b7e4a0fc7040677d090f58082ea1a73807bff85d1");
  assert.equal(
    g.deskPartyChallengeV2(421614, P, "Vault", "0x8b95fdedb647236da0f266231f71440d2714d542", ["arrived", "returned"], 1792080000),
    "0xa4e74a38febef2dfe5e250cc6b23b498f0a89e5167df205e2ff27a8b993629b1",
  );
});

// docs/60 P-226: the bytes "Save bill" signs are the bytes the service checks (level1::bills_challenge, the same vector).
test("the bills challenge matches the service's", async () => {
  const { billsChallenge } = await import("../src/lib/guard.ts");
  const bill = { payee: "0x179dCA89F69FAEF5765D2E7445aa5452B8159520", amount: "0.05", everyDays: 30, nextDue: 1792156029 };
  assert.equal(
    billsChallenge(421614, "0x42463b314d392fe34abe1854b925f0f6e55fa555", [bill], 1792216000),
    "0x6d37a6d579dd8a3f43b1a55f3735445329134b69101a8efc5ac1719f1e7a9838",
  );
  assert.notEqual(
    billsChallenge(421614, "0x42463b314d392fe34abe1854b925f0f6e55fa555", [{ ...bill, amount: "0.06" }], 1792216000),
    billsChallenge(421614, "0x42463b314d392fe34abe1854b925f0f6e55fa555", [bill], 1792216000),
  );
});
