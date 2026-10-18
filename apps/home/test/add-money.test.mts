// Add money (H5): the CCTP V2 shapes checked against Circle's own answers and the proof burn as it went on chain
// (test/fixtures/cctp.json, real data), the money and date words, and the attestation route's input checks.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { toFunctionSelector, type Hex } from "viem";
import { GET } from "../src/features/add-money/attestation.ts";
import { attestationOf, burnData, byDomain, byKey, parseMove, readMessage, sourcesFor, toBytes32 } from "../src/features/add-money/cctp.ts";
import { asOfText, ledgerText, parseHeld, parseUsdc, usdcText, whenText } from "../src/features/add-money/format.ts";

type Msg = { message: Hex; attestation: Hex; status: string; destinationMintTxHash?: string; decodedMessage: Record<string, unknown> & { decodedMessageBody: Record<string, string> } };
type Sample = { tx: string; sourceDomain: number; body: { messages: Msg[] } };
const fx = JSON.parse(readFileSync(new URL("./fixtures/cctp.json", import.meta.url), "utf8")) as {
  signed: Record<string, Sample>;
  ourBurn: { input: Hex; recipient: string; amount: string; pending: unknown };
};

test("readMessage reads what Circle decoded, on three real signed messages", () => {
  for (const [name, s] of Object.entries(fx.signed)) {
    const m = s.body.messages[0];
    const d = m.decodedMessage;
    const b = readMessage(m.message);
    assert.ok(b, name);
    assert.equal(b.source, Number(d.sourceDomain), `${name} source`);
    assert.equal(b.destination, Number(d.destinationDomain), `${name} destination`);
    assert.equal(b.nonce, String(d.nonce).toLowerCase(), `${name} nonce`);
    assert.equal(b.recipient.toLowerCase(), d.decodedMessageBody.mintRecipient.toLowerCase(), `${name} recipient`);
    assert.equal(b.amount, BigInt(d.decodedMessageBody.amount), `${name} amount`);
    assert.equal(b.fee, BigInt(d.decodedMessageBody.feeExecuted), `${name} fee`);
  }
  assert.equal(readMessage("0x1234"), null, "too short for a V2 burn");
  assert.equal(readMessage("nothex" as Hex), null);
});

test("attestationOf: signed, waiting, unseen and unreadable", () => {
  const s = fx.signed.arbToArc.body;
  const a = attestationOf(s);
  assert.equal(a.state, "signed");
  if (a.state === "signed") {
    assert.equal(a.message, s.messages[0].message);
    assert.equal(a.attestation, s.messages[0].attestation);
    assert.equal(a.mintTx, s.messages[0].destinationMintTxHash ?? null);
  }
  assert.deepEqual(attestationOf(fx.ourBurn.pending), { state: "waiting", delay: null }, "the proof burn while Arbitrum finalizes");
  assert.deepEqual(attestationOf({ messages: [] }), { state: "unseen" });
  assert.deepEqual(attestationOf(null), { state: "unseen" });
  assert.equal(attestationOf({ messages: [{ ...s.messages[0], message: "0x1234" }] }).state, "unreadable");
});

test("burnData is the calldata TokenMessengerV2 accepted on Arbitrum Sepolia, under Circle's documented signature", () => {
  const from = byKey("arbitrum-sepolia")!;
  const dest = byKey("arc-testnet")!;
  assert.equal(burnData(from, dest, fx.ourBurn.recipient, BigInt(fx.ourBurn.amount)), fx.ourBurn.input);
  assert.equal(fx.ourBurn.input.slice(0, 10), toFunctionSelector("depositForBurn(uint256,uint32,bytes32,address,bytes32,uint256,uint32)"));
  assert.equal(toBytes32("0xb8064c0325b0679f219665f55e24f1de1fd369fa"), "0x000000000000000000000000b8064c0325b0679f219665f55e24f1de1fd369fa");
});

test("the chains: Circle's domains, and a move never starts on the account's own chain", () => {
  assert.deepEqual([byKey("arbitrum-sepolia")?.domain, byKey("base-sepolia")?.domain, byKey("arc-testnet")?.domain], [3, 6, 26]);
  assert.equal(byDomain(7), undefined);
  assert.deepEqual(sourcesFor(byKey("arc-testnet")!).map((c) => c.name), ["Arbitrum Sepolia", "Base Sepolia"]);
  assert.deepEqual(sourcesFor(byKey("arbitrum-sepolia")!).map((c) => c.name), ["Arc testnet", "Base Sepolia"], "Arc is offered first");
});

test("money in words: six decimals trimmed to the shortest exact form, never $", () => {
  assert.equal(usdcText(0n), "0 USDC");
  assert.equal(usdcText(50_000n), "0.05 USDC");
  assert.equal(usdcText(1n), "0.000001 USDC");
  assert.equal(usdcText(1_000_000n), "1 USDC");
  assert.equal(usdcText(1_234_567n), "1.234567 USDC");
  assert.equal(ledgerText("0.050000 USDC"), "0.05 USDC");
  assert.equal(ledgerText("100.000000 USDC"), "100 USDC");
  assert.equal(ledgerText("not money"), "not money");
});

test("parseUsdc takes a positive amount with at most six decimals, and nothing else", () => {
  assert.equal(parseUsdc("1"), 1_000_000n);
  assert.equal(parseUsdc(" 0.25 "), 250_000n);
  assert.equal(parseUsdc(".5"), 500_000n);
  assert.equal(parseUsdc("1,5"), 1_500_000n);
  assert.equal(parseUsdc("0.000001"), 1n);
  for (const bad of ["", ".", "0", "0.0", "-1", "1e3", "1.0000001", "abc", "1.2.3"]) assert.equal(parseUsdc(bad), null, bad);
});

test("dates in words, in the viewer's time zone", () => {
  const now = new Date(2026, 9, 18, 9, 0).getTime();
  assert.equal(whenText(new Date(2026, 9, 18, 6, 19).getTime() / 1000, now), "Today 06:19");
  assert.equal(whenText(new Date(2026, 9, 17, 23, 5).getTime() / 1000, now), "Yesterday");
  assert.equal(whenText(new Date(2026, 9, 12, 8, 0).getTime() / 1000, now), "12 Oct");
});

test("a kept move is read back only when every field has its shape", () => {
  const ok = { from: 3, amount: "250000", burnTx: `0x${"ab".repeat(32)}`, at: 1_792_277_562 };
  assert.deepEqual(parseMove(JSON.stringify(ok)), ok);
  assert.equal(parseMove(JSON.stringify({ ...ok, from: 7 })), null, "a domain this page does not move from");
  assert.equal(parseMove(JSON.stringify({ ...ok, amount: "1.5" })), null);
  assert.equal(parseMove(JSON.stringify({ ...ok, burnTx: "0x12" })), null);
  assert.equal(parseMove(JSON.stringify({ ...ok, mintTx: "nope" })), null);
  assert.equal(parseMove("{"), null);
  assert.equal(parseMove(null), null);
});

test("the attestation route refuses what is not a burn on a known domain, before asking Circle", async () => {
  for (const q of ["domain=7&tx=0x" + "ab".repeat(32), "domain=3&tx=0x12", "domain=&tx=", "tx=0x" + "ab".repeat(32)]) {
    const r = await GET(new Request(`http://localhost/api/circle/attestation?${q}`));
    assert.equal(r.status, 400, q);
    assert.equal(((await r.json()) as { state: string }).state, "unreadable");
  }
});

test("a kept balance comes back only with a time, and is marked stale in words", () => {
  const at = new Date(2026, 9, 18, 6, 12).getTime();
  assert.deepEqual(parseHeld(JSON.stringify({ balance: "0.050000 USDC", at })), { balance: "0.050000 USDC", at });
  assert.equal(parseHeld(JSON.stringify({ balance: "0.050000 USDC" })), null, "no time, not shown as a kept figure");
  assert.equal(parseHeld(JSON.stringify({ at })), null);
  assert.equal(parseHeld("{"), null);
  assert.equal(parseHeld(null), null);
  assert.equal(asOfText(at, new Date(2026, 9, 18, 9, 0).getTime()), "as of 06:12");
  assert.equal(asOfText(at, new Date(2026, 9, 19, 9, 0).getTime()), "as of yesterday");
  assert.equal(asOfText(at, new Date(2026, 9, 25, 9, 0).getTime()), "as of 18 Oct");
});
