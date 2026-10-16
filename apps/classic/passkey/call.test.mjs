// node --test apps/classic/passkey/call.test.mjs
//
// The page computes what it signs, so the page's arithmetic is pinned here:
// Keccak-256 to published vectors, and challengeFor to challenges the deployed
// PasskeyPrincipal produced and then accepted signatures for
// (proof/passkey-local-2026-10-02.txt, proof/agent-passkey-loop-2026-10-02.txt).
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const C = createRequire(import.meta.url)("./call.js");
const enc = (s) => new TextEncoder().encode(s);

test("keccak256 matches published vectors", () => {
  assert.equal(C.hex(C.keccak256(enc(""))), "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470");
  assert.equal(C.hex(C.keccak256(enc("abc"))), "0x4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45");
});

test("keccak256 is right at the padding edge (135, 136, 137 bytes)", () => {
  // Values from `cast keccak` over N bytes of 0x61 ('a').
  const want = {
    135: "0x34367dc248bbd832f4e3e69dfaac2f92638bd0bbd18f2912ba4ef454919cf446",
    136: "0xa6c4d403279fe3e0af03729caada8374b5ca54d8065329a3ebcaeb4b60aa386e",
    137: "0xd869f639c7046b4929fc92a4d988a8b22c55fbadb802c0c66ebcd484f1915f39",
  };
  for (const [n, h] of Object.entries(want)) {
    assert.equal(C.hex(C.keccak256(new Uint8Array(Number(n)).fill(0x61))), h, `${n} bytes`);
  }
});

test("function selectors match cast sig", () => {
  assert.equal(C.selector("setPayee(address,bool)"), "0x34764f3d");
  assert.equal(C.selector("setPayees(address[],bool)"), "0x48a56503");
});

test("recovery calls are described in words", () => {
  const c = C.describe(C.selector("cancelRecovery()"));
  assert.equal(c.known, true);
  assert.match(c.action, /Cancel the pending replacement/);
  const s = C.describe(C.selector("setRecovery(address)") + "0".repeat(24) + "d00d".padStart(40, "0"));
  assert.equal(s.known, true);
  assert.match(s.action, /recovery address/);
  assert.deepEqual(s.payees, ["0x" + "d00d".padStart(40, "0")]);
  assert.equal(C.describe(C.selector("setRecovery(address)")).known, false, "a setRecovery without an address is not known");
});

test("cancelPending is described in words and takes no arguments", () => {
  const d = C.describe(C.selector("cancelPending()"));
  assert.equal(d.known, true);
  assert.match(d.action, /Cancel the pending raise/);
  assert.deepEqual(d.payees, []);
  // a stray argument is not a cancel
  assert.equal(C.describe(C.selector("cancelPending()") + "00".repeat(32)).known, false);
});

test("setMode and setLimits are described in words for the principal's own signature", () => {
  const word = (n) => n.toString(16).padStart(64, "0");
  const auto = C.describe(C.selector("setMode(uint8)") + word(2));
  assert.equal(auto.known, true);
  assert.match(auto.action, /Autopilot/);
  assert.match(auto.note, /waits the delay/);
  assert.match(C.describe(C.selector("setMode(uint8)") + word(0)).action, /Watch/);
  assert.equal(C.describe(C.selector("setMode(uint8)") + word(3)).known, false, "mode 3 does not exist");
  assert.equal(C.describe(C.selector("setMode(uint8)")).known, false, "no argument is not a mode");
  const lim = C.describe(C.selector("setLimits(uint256,uint256,uint256)") + word(25_000_000_000n) + word(170_000_000_000n) + word(340_000_000_000n));
  assert.equal(lim.known, true);
  assert.match(lim.action, /25000\.000000 USDC per payment/);
  assert.match(lim.action, /340000\.000000 USDC untouched/);
});

test("standing orders are described in words: payee, amount, cadence, first due", () => {
  const word = (n) => n.toString(16).padStart(64, "0");
  const payee = "0".repeat(24) + "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
  const d = C.describe(C.selector("addSchedule(address,uint256,uint64,uint64)") + payee + word(4_000n) + word(86_400n) + word(1_791_958_577n));
  assert.equal(d.known, true);
  assert.match(d.action, /0\.004000 USDC every 1 day, first on 2026-10-14/);
  assert.equal(d.payees[0].toLowerCase(), "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
  const m = C.describe(C.selector("addSchedule(address,uint256,uint64,uint64)") + payee + word(1_000_000n) + word(30n * 86_400n) + word(1_800_000_000n));
  assert.match(m.action, /every 30 days/);
  const e = C.describe(C.selector("removeSchedule(uint256)") + word(3n));
  assert.equal(e.known, true);
  assert.match(e.action, /End standing order #3/);
  assert.equal(C.describe(C.selector("payDue(uint256)") + word(0n)).known, false, "payDue is anyone's call, never the passkey's");
  const r = C.describe(C.selector("renewAgent(uint64)") + word(1_800_000_000n));
  assert.equal(r.known, true);
  assert.match(r.action, /keep paying until 2027-01-15/);
  assert.match(r.note, /your brake/);
});

test("a bound approval names its terms and deadline, and the page recomputes the contract's fingerprint", () => {
  const word = (n) => n.toString(16).padStart(64, "0");
  const payee = "0".repeat(24) + "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
  const terms = "95a4f8627252855cd26640bbeec95e9a4d924e1c6bceef775d858778547ad43c";
  const s = C.describe(C.selector("settleAsPrincipalIf(address,uint256,bytes32,bytes32,uint64)") + payee + word(4_000n) + word(7n) + terms + word(1_791_961_000n));
  assert.equal(s.known, true);
  assert.match(s.action, /Pay 0\.004000 USDC yourself to/);
  assert.equal(s.terms, "0x" + terms);
  assert.equal(s.deadline, 1_791_961_000);
  assert.match(s.note, /valid only while/);
  // setPayeesIf: offset 0x80, attested, terms, deadline, n, payee
  const p = C.describe(C.selector("setPayeesIf(address[],bool,bytes32,uint64)") + word(128n) + word(1n) + terms + word(1_791_961_000n) + word(1n) + payee);
  assert.equal(p.known, true);
  assert.equal(p.payees.length, 1);
  assert.equal(p.terms, "0x" + terms);
  // The same ten words cast encoded and hashed on 2026-10-14.
  const fp = C.termsFingerprint({ perActionCap: "5000", dailyCap: "50000", reserveFloor: "0", mode: 2, agent: "0x524511aec2d76d9422e8550b9cb0014531005100", operator: "0x6c1ce01bfe3f834a5fbfa0a9081d5d976b819a25", delay: "60", agentExpiry: 1799736841, pendingEta: 0, pendingKinds: 0 });
  assert.equal(fp, "0x" + terms);
  const moved = C.termsFingerprint({ perActionCap: "5000", dailyCap: "50000", reserveFloor: "0", mode: 1, agent: "0x524511aec2d76d9422e8550b9cb0014531005100", operator: "0x6c1ce01bfe3f834a5fbfa0a9081d5d976b819a25", delay: "60", agentExpiry: 1799736841, pendingEta: 0, pendingKinds: 0 });
  assert.notEqual(moved, fp);
});

test("naming guardians is described with the count, the threshold and the addresses", () => {
  const word = (n) => n.toString(16).padStart(64, "0");
  const a1 = "0".repeat(24) + "6001000000000000000000000000000000006001";
  const a2 = "0".repeat(24) + "6002000000000000000000000000000000006002";
  const d = C.describe(C.selector("setGuardians(address[],uint8)") + word(64n) + word(2n) + word(2n) + a1 + a2);
  assert.equal(d.known, true);
  assert.match(d.action, /Name 2 guardians; 2 of them may together propose a replacement/);
  assert.equal(d.payees.length, 2);
  assert.match(d.note, /does not let them spend/);
  const none = C.describe(C.selector("setGuardians(address[],uint8)") + word(64n) + word(0n) + word(0n));
  assert.match(none.action, /Remove all guardians/);
  assert.equal(C.describe(C.selector("setGuardians(address[],uint8)") + word(64n) + word(2n) + word(2n) + a1).known, false, "a short list is not described");
});

test("challengeFor matches what the deployed contract computed", () => {
  const owner = "0x5FC8d32690cc91D4c39d9d3abcBD16989F875707";
  const policy = "0x0165878A594ca255338adfa4d48449f69242Eb8F";
  const setPayee = (who) => "0x34764f3d" + "0".repeat(24) + who.slice(2) + "0".repeat(63) + "1";
  assert.equal(
    C.challengeFor({ chainId: 31337, owner, nonce: 0, target: policy, data: setPayee("0x1111111111111111111111111111111111111111") }),
    "0x54f32e4c3fccc9d715f02c8d04d48f8c582784dffd55f04b5dc9459b08a7d32b");
  assert.equal(
    C.challengeFor({ chainId: 31337, owner, nonce: 1, target: policy, data: setPayee("0x2222222222222222222222222222222222222222") }),
    "0xe9ff4f33afc246070f6470a88bc58baccb47826f470f32b2f4ed3fb96fca5f3f");
});

const BATCH =
  "0x48a56503000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000002000000000000000000000000111111111111111111111111111111111111111100000000000000000000000033333333333333333333333333333333333333330";

test("a batch: the contract's challenge, and the words shown for it", () => {
  const data = BATCH.slice(0, -1); // the literal above ends in a stray 0 so its length is checked
  assert.equal(
    C.challengeFor({ chainId: 31337, owner: "0x1613beB3B2C4f22Ee086B2b38C1476A3cE7f78E8", nonce: 0,
                     target: "0x851356ae760d987E095750cCeb3bC6014560891C", data }),
    "0x8278912013449540498663f98f66e049f088feeb4840504753a2abbfec8b0812");
  assert.deepEqual(C.describe(data), {
    known: true, action: "Allow the agent to pay",
    payees: ["0x1111111111111111111111111111111111111111", "0x3333333333333333333333333333333333333333"],
  });
  assert.throws(() => C.describe(BATCH), /not hex/, "odd-length calldata is refused, not guessed");
});

test("settlement and withdrawal are described with their amounts", () => {
  const settle = "0xb87f7b630000000000000000000000003333333333333333333333333333333333333333000000000000000000000000000000000000000000000000000000000016e360abababababababababababababababababababababababababababababababab";
  const d = C.describe(settle);
  assert.equal(d.action, "Pay 1.500000 USDC yourself to");
  assert.deepEqual(d.payees, ["0x3333333333333333333333333333333333333333"]);
});

test("anything unrecognised is unknown, never described", () => {
  assert.equal(C.describe("0xdeadbeef").known, false);
  // setPayee's selector with the wrong number of words.
  assert.equal(C.describe("0x34764f3d" + "00".repeat(32)).known, false);
  // a batch whose length word disagrees with its body
  const lying = "0x48a56503" + "0".repeat(62) + "40" + "0".repeat(63) + "1" + "0".repeat(63) + "5"
    + "0".repeat(24) + "11".repeat(20);
  assert.equal(C.describe(lying).known, false);
});

test("the owner's reviewed run lists every bill, the total, and the binding", () => {
  const word = (n) => n.toString(16).padStart(64, "0");
  const addr = (x) => "0".repeat(24) + x;
  const a1 = addr("1111111111111111111111111111111111111111");
  const a2 = addr("2222222222222222222222222222222222222222");
  const terms = "ab".repeat(32);
  const n = 2n;
  const body = word(160n) + word(160n + 32n * 3n) + word(160n + 64n * 3n) + terms + word(1_791_961_000n)
    + word(n) + a1 + a2 + word(n) + word(2_000n) + word(3_500n) + word(n) + "cc".repeat(32) + "dd".repeat(32);
  const d = C.describe(C.selector("settleRunIf(address[],uint256[],bytes32[],bytes32,uint64)") + body);
  assert.equal(d.known, true);
  assert.match(d.action, /^Pay 2 bills yourself, 0\.005500 USDC in all/);
  assert.equal(d.rows.length, 2);
  assert.equal(d.rows[1].amount, "0.003500 USDC");
  assert.equal(d.terms, "0x" + terms);
  assert.match(d.note, /already paid is skipped, never paid twice/);
  // A run whose arrays disagree in length is not described (and so not signed).
  const bad = word(160n) + word(160n + 32n * 3n) + word(160n + 64n * 3n) + terms + word(1n)
    + word(n) + a1 + a2 + word(1n) + word(2_000n) + word(0n) + word(n) + "cc".repeat(32) + "dd".repeat(32);
  assert.equal(C.describe(C.selector("settleRunIf(address[],uint256[],bytes32[],bytes32,uint64)") + bad).known, false);
});

test("adding a co-owner is described with the count and the threshold", () => {
  const word = (n) => n.toString(16).padStart(64, "0");
  const x = "ab".repeat(32), y = "cd".repeat(32);
  const body = word(96n) + word(96n + 64n) + word(2n) + word(1n) + x + word(1n) + y;
  const d = C.describe(C.selector("setCoOwners(bytes32[],bytes32[],uint8)") + body);
  assert.equal(d.known, true);
  assert.match(d.action, /your passkey and 1 more device; 2 of 2 must approve every change/);
  assert.equal(d.threshold, 2);
  const none = C.describe(C.selector("setCoOwners(bytes32[],bytes32[],uint8)") + word(96n) + word(128n) + word(1n) + word(0n) + word(0n));
  assert.match(none.action, /only owner again/);
  assert.equal(C.describe(C.selector("setCoOwners(bytes32[],bytes32[],uint8)") + word(96n) + word(96n + 64n) + word(2n) + word(1n) + x + word(2n) + y).known, false, "mismatched lengths are not described");
});

