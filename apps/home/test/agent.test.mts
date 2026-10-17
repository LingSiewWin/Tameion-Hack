// The Agent view: how it writes the service's figures, and what it renders from a real-shaped read (the e1-seller-signed
// account as GET /api/level1/symbolon, /status and /inbox answered on the lane service, 2026-10-18 06:32 +08).
import "../scripts/tsx.mjs";
import assert from "node:assert/strict";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { clock, money, perMinute, short, when } from "../src/features/agent/format.ts";
import type { Inbox, Status, Symbolon } from "../src/lib/types.ts";

const { AgentScreen } = await import("../src/features/agent/agent-view.tsx");

test("money keeps the service's digits, trimmed to the shortest exact form; absent is null, never 0", () => {
  assert.deepEqual(money("0.003000 USDC"), { qty: "0.003", unit: "USDC" });
  assert.deepEqual(money("100.000000 USDC"), { qty: "100", unit: "USDC" });
  assert.deepEqual(money("-0.000428 USDC"), { qty: "-0.000428", unit: "USDC" });
  assert.deepEqual(money("0.000000 USDC"), { qty: "0", unit: "USDC" });
  assert.equal(money(undefined), null);
  assert.equal(money(""), null);
  assert.equal(money("about 3"), null);
});

test("the rate per minute is the day's rate over 1440 in integers, marked ≈ when inexact", () => {
  assert.equal(perMinute("0.001428 USDC"), "≈ 0.00000099");
  assert.equal(perMinute("0.000428 USDC"), "≈ 0.00000029");
  assert.equal(perMinute("1.440000 USDC"), "0.001", "1.44 a day is exactly 0.001 a minute");
  assert.equal(perMinute("0.000000 USDC"), "0");
  assert.equal(perMinute("0.000001 USDC"), "< 0.00000001");
  assert.equal(perMinute(null), null);
});

test("hashes shorten, dates read in words, the clock reads hh:mm:ss", () => {
  assert.equal(short("0x508d927c137efd782a16b3b22f78b14ce6383e24c33546b51c24f37ce793a968"), "0x508d…a968");
  assert.equal(short("chain:421614"), "chain:421614");
  const now = new Date(2026, 9, 18, 6, 40).getTime();
  const at = (d: number, h: number, m: number) => new Date(2026, 9, d, h, m).getTime() / 1000;
  assert.equal(when(at(18, 5, 56), now), "Today 05:56");
  assert.equal(when(at(17, 23, 10), now), "Yesterday 23:10");
  assert.equal(when(at(12, 17, 37), now), "12 Oct 17:37");
  assert.equal(clock(new Date(2026, 9, 18, 6, 41, 2).getTime()), "06:41:02");
});

const ROOT_A = "0x5033622a28d6caf728a1517e1b80608dbcb1315760877b005a2e6128718c861a";
const ROOT_B = "0xed2a76a76a246228672c9dc1f938d40dfd0f7108f272607ccb286399e20db567";
const VENDOR = "0xe5e2d8dcc2038fd0a010a6e7845378b646e33854";
const witness = { alg: "chain-receipt", keyId: "chain:421614", verified: true };
const symbolon: Symbolon = {
  ok: true,
  contractVersion: "2026-10-18.1",
  policy: "0xcf57c48b69104c89a462cdb68ddc3f4f0933e2e1",
  chain: "arbitrum-sepolia",
  explorer: "https://sepolia.arbiscan.io",
  locked: false,
  counts: { unsigned: 0, single: 2, double: 1, triple: 0 },
  chainReadFailed: [ROOT_B],
  records: [
    { root: ROOT_A, seq: 0, leaf: `0x${"f1".repeat(32)}`, kind: "FLOWCHECK", at: 1792274100, anchorTx: `0x${"a0".repeat(32)}`, agent: null, counterpartySlot: null, witness },
    {
      root: ROOT_A,
      seq: 2,
      leaf: "0x508d927c137efd782a16b3b22f78b14ce6383e24c33546b51c24f37ce793a968",
      kind: "SETTLEMENT",
      at: 1792274196,
      counterparty: VENDOR,
      amount: "0.003000 USDC",
      minor: "3000",
      tx: `0x${"b8".repeat(32)}`,
      note: "paid",
      anchorTx: `0x${"66".repeat(32)}`,
      agent: null,
      counterpartySlot: { alg: "secp256k1-eip712", keyId: VENDOR, verified: true },
      witness,
    },
    { root: ROOT_B, seq: 1, leaf: `0x${"7b".repeat(32)}`, kind: "DECISION", at: 1792274300, counterparty: VENDOR, amount: "0.500000 USDC", decision: "BLOCK", anchorTx: `0x${"ed".repeat(32)}`, agent: null, counterpartySlot: null, witness: null },
  ],
};
const status = { ok: true, contractVersion: "2026-10-18.1", recentUpdates: [], rate: "0.000428 USDC", windowDays: 7, runway: { byRateDays: 109, firstShortfallDays: null } } as Status;
const inbox = { policy: symbolon.policy, mode: "Autopilot", balance: "0.047000 USDC", paidTotal: "0.003000 USDC", runway: null } as unknown as Inbox;
const src = <T,>(value: T) => ({ value, at: new Date(2026, 9, 18, 6, 42, 5).getTime(), error: null });
const render = (s: Symbolon, extra: Partial<Parameters<typeof AgentScreen>[0]["reads"]> = {}) =>
  renderToStaticMarkup(
    createElement(AgentScreen, { reads: { status: src(status), inbox: src(inbox), symbolon: src(s), ...extra }, now: new Date(2026, 9, 18, 6, 50).getTime() }),
  );

test("top: each figure from its field, labelled by it; a field the service left empty is a dash and named, never 0", () => {
  const html = render(symbolon);
  const text = html.replace(/<wbr\/>/g, ""); // field names may break after their dots
  for (const label of ["inbox.balance", "inbox.runway.horizonTotal", "inbox.paidTotal", "status.rate (7 d) ÷ 1440", "status.runway.byRateDays"])
    assert.ok(text.includes(label), label);
  assert.match(html, /status\.<wbr\/>runway\.<wbr\/>firstShortfallDays/);
  assert.match(html, />0\.047<\/span><\/td>/, "a live figure sits in its rolling wrapper");
  assert.match(html, />0\.003<\/span><\/td>/);
  assert.match(html, />≈ 0\.00000029<\/span><\/td>/);
  assert.match(html, />109 d<\/span><\/td>/);
  assert.match(html, /the service gave no figure: inbox\.runway\.horizonTotal, status\.runway\.firstShortfallDays/);
  assert.doesNotMatch(html.slice(0, html.indexOf("Records ")), />0<\/span><\/td>/, "no fake zero in the top table");
  assert.match(html, /0xcf57c48b69104c89a462cdb68ddc3f4f0933e2e1/, "the Agent view names the address");
  assert.match(html, /as of 06:42:05/);
});

test("stream: newest first, three slots per line, counts in the header, a refusal with its code, links to the anchor", () => {
  const html = render(symbolon);
  assert.match(html, /Records 3/);
  assert.match(html, /Single 2.*Double 1.*Triple 0.*Unsigned 0/s);
  const order = ["Today 05:58", "Today 05:56", "Today 05:55"].map((t) => html.indexOf(t));
  assert.ok(order.every((i, k) => i > 0 && (k === 0 || i > order[k - 1])), `newest first: ${order}`);
  assert.match(html, /Seller: verified, secp256k1-eip712, 0xe5e2d8dcc2038fd0a010a6e7845378b646e33854/);
  assert.match(html, /Agent: none/);
  assert.match(html, /Witness: the anchoring could not be read this time/, "a failed chain read is ?, not none");
  assert.match(html, /<span class="text-\(--attention\)">refused<\/span> <span class="meta">block<\/span>/);
  assert.equal((html.match(/text-\(--attention\)/g) ?? []).length, 1, "attention only on the refusal");
  assert.match(html, /href="https:\/\/sepolia\.arbiscan\.io\/tx\/0x(66){32}"/);
  assert.match(html, /0x508d…a968/);
  assert.match(html, /Anchoring not read this time for 1 root/);
});

test("locked: an outsider's view lists seq, leaf and root only, no counts and no slots", () => {
  const locked: Symbolon = {
    ...symbolon,
    locked: true,
    counts: null,
    explorer: null,
    chainReadFailed: [],
    records: symbolon.records.map(({ root, seq, leaf }) => ({ root, seq, leaf })),
  };
  const html = render(locked);
  assert.match(html, /read with its owner&#x27;s device or a read key/);
  assert.doesNotMatch(html, /Single|Seller|Witness|refused/);
  assert.match(html, />0x508d927c137efd782a16b3b22f78b14ce6383e24c33546b51c24f37ce793a968</, "the whole leaf, to check");
});

test("a read that failed keeps the last answer, says since when, and offers one action", () => {
  const html = renderToStaticMarkup(
    createElement(AgentScreen, {
      reads: { status: src(status), inbox: src(inbox), symbolon: { value: null, at: null, error: "The service did not answer." } },
      onRetry: () => {},
    }),
  );
  assert.match(html, /role="alert"/);
  assert.match(html, /The records did not answer\./, "the source in words, the technical text behind a disclosure");
  assert.match(html, /<details[^>]*><summary[^>]*>What the service said<\/summary><p[^>]*>symbolon: The service did not answer\.<\/p><\/details>/);
  assert.match(html, />Try again<\/button>/);
  assert.match(html, /The records did not load; the figures above still stand\./);
  const silent = { value: null, at: null, error: "The service did not answer." };
  const none = renderToStaticMarkup(createElement(AgentScreen, { reads: { status: silent, inbox: silent, symbolon: silent } }));
  assert.match(none, /The records did not load\.<\/p>/, "no figure stands when none was read");
});
