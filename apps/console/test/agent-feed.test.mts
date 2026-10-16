// The Agent view's pure core (src/app/agent/feed.ts): the frame, the event stream it arrives in, the margin
// bands and the number formats. A network read cuts a stream anywhere, so the parser is checked at every
// cut; a number the feed did not carry must come out as a dash, never as a guess.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  age,
  askRows,
  bandOf,
  clock,
  createSseParser,
  describeEvent,
  fillOf,
  fixed,
  MINUS,
  money,
  paidRows,
  places,
  ratioText,
  readFrame,
  refusedRows,
  shortTx,
  signed,
  signedAt,
  spentShare,
  STALE_AFTER_MS,
  stamp,
  VENUE_STALE_AFTER_MS,
  venueSeen,
  type SseEvent,
} from "../src/app/agent/feed.ts";

const parse = (chunks: string[]) => {
  const out: SseEvent[] = [];
  const push = createSseParser((e) => out.push(e));
  chunks.forEach(push);
  return out;
};

const STREAM =
  ": ping\n\nretry: 3000\n\nevent: hello\ndata: {\"at\":1}\n\ndata: {\"a\":1,\ndata: \"b\":2}\n\nid: 7\nevent: pass\ndata: x\n\nevent: noData\n\n";
const WANT: SseEvent[] = [
  { event: "hello", data: '{"at":1}' },
  { event: "message", data: '{"a":1,\n"b":2}' },
  { event: "pass", data: "x" },
];

test("the parser delivers the same events however the network cut the stream", () => {
  assert.deepEqual(parse([STREAM]), WANT);
  assert.deepEqual(parse([...STREAM]), WANT, "one character at a time");
  for (let cut = 1; cut < STREAM.length; cut++) assert.deepEqual(parse([STREAM.slice(0, cut), STREAM.slice(cut)]), WANT, `cut at ${cut}`);
});

test("CRLF and lone CR end lines too, even when the CR and LF arrive apart", () => {
  const crlf = STREAM.replace(/\n/g, "\r\n");
  assert.deepEqual(parse([crlf]), WANT);
  assert.deepEqual(parse([...crlf]), WANT);
  assert.deepEqual(parse([STREAM.replace(/\n/g, "\r")]), WANT, "a lone CR is a line end");
  assert.deepEqual(parse(["data: a\r", "\ndata: b\r\n\r\n"]), [{ event: "message", data: "a\nb" }]);
});

test("an event cut off by the end of the stream is never delivered", () => {
  assert.deepEqual(parse(["data: whole\n\ndata: half"]), [{ event: "message", data: "whole" }]);
});

const FRAME = {
  at: 1_792_100_000,
  venueTime: 1_792_099_999_500,
  fundingReadAt: 1_792_099_800,
  venue: "0x557e1e07652b75abaa667223b11704165fc94d09",
  positions: [{ coin: "PAXG", size: "-0.3210", entry: "4181.2", mark: "4180.9", pnl: "0.0963" }, { nope: 1 }],
  marginUsed: "670.97",
  maintenanceMarginUsed: "67.1",
  accountValue: "1234.5",
  kRatio: "9.99",
  kPolicy: 4,
  fundingLastHour: "0.0013",
  funding24h: "-0.0551",
  lastOrders: [
    { at: 1_792_099_990, coin: "PAXG", side: "sell", size: "0.321", oid: 123456789012, readBack: "match" },
    { at: 1_792_099_000_000, coin: "PAXG", side: "buy", size: "0.1", oid: "9", readBack: "other" },
  ],
  halt: null,
  passLatencyMs: 812,
  lastAnchor: { tx: "0xabcdef0123456789abcdef", at: 1_792_099_800 },
  lastPassAt: 1_792_099_999,
};

test("a frame is read as sent: decimals stay strings, odd rows are dropped, nothing is filled in", () => {
  const r = readFrame(FRAME);
  assert.ok("frame" in r);
  const f = r.frame;
  assert.equal(f.positions?.length, 1, "a position with no coin is not a position");
  assert.deepEqual(f.positions?.[0], { coin: "PAXG", size: "-0.3210", entry: "4181.2", mark: "4180.9", pnl: "0.0963" });
  assert.equal(f.venueTime, 1_792_099_999_500, "the venue's own clock, in milliseconds");
  assert.equal(f.fundingReadAt, 1_792_099_800, "when the funding payments were read, in seconds");
  assert.equal(f.kRatio, 9.99, "a ratio sent as a string is read as a number");
  assert.equal(f.kPolicy, 4);
  assert.equal(f.orders[0].oid, "123456789012");
  assert.equal(f.orders[1].at, 1_792_099_000, "milliseconds are brought down to seconds");
  assert.equal(f.orders[1].readBack, null, "a read-back that is neither match nor mismatch is not a match");
  assert.deepEqual(f.anchor, { tx: "0xabcdef0123456789abcdef", at: 1_792_099_800 });
  const bare = readFrame({ at: 5, positions: [], halt: null });
  assert.ok("frame" in bare);
  assert.deepEqual(bare.frame.positions, [], "a flat account is an empty list");
  assert.equal(bare.frame.venueError, null);
  assert.equal(bare.frame.venueTime, null);
  assert.equal(bare.frame.fundingReadAt, null);
  assert.equal(bare.frame.accountValue, null);
  assert.equal(bare.frame.kRatio, null);
  assert.equal(bare.frame.anchor, null);
});

test("a frame that cannot be trusted is refused with its reason, a halt is never assumed away", () => {
  assert.deepEqual(readFrame("x"), { error: "the feed sent something that is not a frame" });
  assert.ok("error" in readFrame({ positions: [], halt: null }), "no time");
  assert.ok("error" in readFrame({ at: 1, positions: "x", halt: null }), "positions that are not a list");
  const nohalt = readFrame({ at: 1, positions: [] });
  assert.ok("error" in nohalt && /halt/.test(nohalt.error), "a frame silent on halt is not 'no halt'");
  const stopped = readFrame({ at: 1, positions: [], halt: { reason: "read-back mismatch", at: 1_792_100_001 } });
  assert.ok("frame" in stopped);
  assert.deepEqual(stopped.frame.halt, { reason: "read-back mismatch", at: 1_792_100_001 });
  for (const odd of ["stop", true, {}, 1]) {
    const r = readFrame({ at: 1, positions: [], halt: odd });
    assert.ok("frame" in r && r.frame.halt !== null, `halt ${JSON.stringify(odd)} still shows a halt`);
  }
  const no = readFrame({ at: 1, positions: [], halt: false });
  assert.ok("frame" in no && no.frame.halt === null);
});

test("a venue that did not answer is unknown, not flat", () => {
  const r = readFrame({ at: 9, positions: null, accountValue: null, kRatio: null, venueError: "the venue answered 2.4 s ago", halt: null });
  assert.ok("frame" in r);
  assert.equal(r.frame.positions, null, "null is not an empty account");
  assert.equal(r.frame.accountValue, null);
  assert.equal(r.frame.venueError, "the venue answered 2.4 s ago");
  const silent = readFrame({ at: 9, halt: null });
  assert.ok("frame" in silent && silent.frame.positions === null, "a frame with no positions says nothing about them");
});

test("the venue's reading is old when its own clock stops moving, never by subtracting one machine's time from another's", () => {
  assert.equal(venueSeen(null, 1000, 50), 50, "first sight of a reading");
  assert.equal(venueSeen({ venueTime: 1000, venueSeenAt: 50 }, 1000, 5050), 50, "the same reading is as old as it was");
  assert.equal(venueSeen({ venueTime: 1000, venueSeenAt: 50 }, 2000, 6000), 6000, "a new reading is new");
  assert.equal(venueSeen({ venueTime: 1000, venueSeenAt: 50 }, null, 7000), null, "no venue numbers, no age");
  assert.equal(venueSeen({ venueTime: null, venueSeenAt: null }, 3000, 8000), 8000);
  assert.ok(VENUE_STALE_AFTER_MS < STALE_AFTER_MS, "a venue reading goes stale before the connection does");
  const seconds = readFrame({ at: 5, positions: [], halt: null, venueTime: 1_792_100_000 });
  assert.ok("frame" in seconds && seconds.frame.venueTime === 1_792_100_000_000, "a stamp in seconds is taken up to milliseconds");
});

test("the margin meter turns red under 1.0x, amber under the rule, green at it", () => {
  assert.equal(bandOf(0.99, 4), "critical");
  assert.equal(bandOf(1, 4), "below-rule");
  assert.equal(bandOf(3.999, 4), "below-rule");
  assert.equal(bandOf(4, 4), "ok");
  assert.equal(bandOf(37, 4), "ok");
  assert.equal(fillOf(2, 4), 0.5);
  assert.equal(fillOf(9, 4), 1, "past the rule the meter is full, not wider");
  assert.equal(fillOf(-1, 4), 0);
  assert.equal(fillOf(1, 0), 0, "no rule, no fill");
  assert.equal(ratioText(5.8312), "5.83×");
  assert.equal(ratioText(212.4), "212×");
});

test("numbers line up on the point and are never rounded in a column", () => {
  const col = ["4181.2", "4,000", "12345.125", null];
  const dp = places(col.map((c) => (c === "4,000" ? null : c)));
  assert.equal(dp, 3);
  assert.equal(fixed("4181.2", dp), "4,181.200");
  assert.equal(fixed("-0.3210", 4), `${MINUS}0.3210`);
  assert.equal(fixed("12345.125", dp), "12,345.125");
  assert.equal(fixed(null), "—");
  assert.equal(fixed("n/a"), "n/a");
});

test("money prints to the cent, keeps digits below a cent, and signs gains and losses", () => {
  assert.equal(money("1234.5"), "1,234.50");
  assert.equal(money("0.0963"), "0.10");
  assert.equal(money("0.0013"), "0.0013");
  assert.equal(money("-0.0040"), `${MINUS}0.004`);
  assert.equal(money("-5.126"), `${MINUS}5.13`);
  assert.equal(money("0.000000"), "0.00");
  assert.equal(money(null), "—");
  assert.deepEqual(signed("0.19"), { text: "+0.19", sign: 1 });
  assert.deepEqual(signed("-12.345"), { text: `${MINUS}12.35`, sign: -1 });
  assert.deepEqual(signed("0"), { text: "0.00", sign: 0 });
  assert.deepEqual(signed("-0.0551"), { text: `${MINUS}0.06`, sign: -1 });
  assert.deepEqual(signed("0.0013"), { text: "+0.0013", sign: 1 });
  assert.deepEqual(signed(null), { text: "—", sign: 0 });
});

test("a pair of small figures is printed at the same places, signed, never rounded to a cent", () => {
  assert.deepEqual(signedAt("0.0013", 4), { text: "+0.0013", sign: 1 });
  assert.deepEqual(signedAt("0.0551", 4), { text: "+0.0551", sign: 1 });
  assert.deepEqual(signedAt("-0.0210", 4), { text: `${MINUS}0.0210`, sign: -1 });
  assert.deepEqual(signedAt("0.00004", 4), { text: "0.0000", sign: 0 }, "rounds to zero: no sign");
  assert.deepEqual(signedAt("-0.00004", 4), { text: "0.0000", sign: 0 });
  assert.deepEqual(signedAt("1234.5", 2), { text: "+1,234.50", sign: 1 });
  assert.deepEqual(signedAt(null, 4), { text: "—", sign: 0 });
});

test("time: a clock, the date once it is not today, an age", () => {
  const now = Date.UTC(2026, 9, 16, 12, 0, 0);
  const sec = now / 1000;
  assert.match(clock(sec), /^\d\d:\d\d:\d\d$/);
  assert.equal(stamp(sec, now), clock(sec));
  assert.match(stamp(sec - 3 * 86_400, now), /^\d{1,2} \w{3} \d\d:\d\d$/);
  assert.equal(age(3_000), "3 s");
  assert.equal(age(120_000), "2 min");
  assert.equal(age(3 * 3_600_000), "3 h");
  assert.equal(age(5 * 86_400_000), "5 d");
  assert.equal(age(-5), "0 s");
  assert.equal(shortTx("0xabcdef0123456789abcdef"), "0xabcdef…cdef");
  assert.equal(shortTx("0xabc"), "0xabc");
});

test("account events in words: known ones read like a log, unknown ones are shown, not dropped", () => {
  assert.deepEqual(describeEvent("hello", { policy: "0x1", at: 1 }), { source: "Stream", text: "Connected", tone: "info" });
  assert.equal(describeEvent("pass", { status: "queued" }).text, "Check queued");
  assert.equal(describeEvent("pass", { status: "running", progress: { scannedBlocks: 41, totalBlocks: 100 } }).text, "Check running, 41 % read");
  assert.equal(describeEvent("pass", { status: "running", progress: { phase: "reading your bills, then deciding" } }).text, "Check running: reading your bills, then deciding");
  assert.deepEqual(describeEvent("pass", { status: "done", paid: 2, needsYou: 1, anchorTx: "0xabcdef0123456789abcdef" }), {
    source: "Check",
    text: "Check done: paid 2, needs you 1, recorded on chain 0xabcdef…cdef",
    tone: "ok",
  });
  assert.deepEqual(describeEvent("pass", { status: "failed", error: "rpc timed out" }), { source: "Check", text: "Check failed: rpc timed out", tone: "bad" });
  assert.equal(describeEvent("inbox", { cause: "resync" }).text, "The stream caught up after a gap");
  assert.equal(describeEvent("inbox", { cause: "invoice" }).text, "Account changed: invoice");
  assert.deepEqual(describeEvent("desk", { offset: "0.3", nested: { a: 1 } }), { source: "desk", text: 'offset: 0.3, nested: {"a":1}', tone: "info" });
  assert.equal(describeEvent("pass", "not an object").text, "Check update");
});

const NAMES: Record<string, string> = { "0xaa": "Weather API", "0xbb": "Supplier" };
const nameOf = (a: string) => NAMES[a] ?? `••${a.slice(-4)}`;

test("an account's payments: newest first, the payee by the account's own name, the reason the agent gave", () => {
  const rows = paidRows(
    [
      { counterparty: "0xaa", amount: "0.002000 USDC", at: 100, reason: "ETH price for the weekly report", by: "agent", explorer: "https://x/tx/1" },
      { counterparty: "0xcc00000000000000000000000000000000000c0d", amount: "1.000000 USDC", at: 300, bought: "api.example.com" },
      { counterparty: "0xbb", amount: "0.500000 USDC", reason: "  " },
    ],
    nameOf,
  );
  assert.deepEqual(rows.map((r) => r.to), ["••0c0d", "Weather API", "Supplier"], "newest first, no time last; an unnamed payee is the last four, never the address");
  assert.equal(rows[0].why, "api.example.com", "no reason given: what was bought");
  assert.equal(rows[1].why, "ETH price for the weekly report");
  assert.equal(rows[2].why, "", "nothing is made up");
  assert.equal(rows[1].receipt, "https://x/tx/1");
});

test("what was refused is in words: the checks' reason, the purchase's own sentence, and why the agent wanted it", () => {
  const plain = (w?: string | null) => (w === "per-action-cap" ? "More than one payment may be on its own." : (w ?? ""));
  const rows = refusedRows(
    [{ counterparty: "0xbb", amount: "9.000000 USDC", why: "per-action-cap" }],
    [
      { at: 50, host: "127.0.0.1", amount: "0.002000 USDC", status: "refused", why: "the price 0.002000 USDC is above the most you allowed for one call (0.001000 USDC)", reason: "ETH mid price, cheap only" },
      { at: 60, host: "paid.example", amount: "0.001000 USDC", status: "paid" },
      { at: 70, seller: "0xaa", amount: "0.004000 USDC", status: "refused" },
    ],
    plain,
    nameOf,
  );
  assert.equal(rows.length, 3, "a paid purchase is not a refusal");
  assert.deepEqual(rows.map((r) => r.what), ["Weather API", "127.0.0.1", "Supplier"], "purchases newest first, then the bills that carry no time");
  assert.equal(rows[0].why, "refused by the checks", "no sentence from the service: say only what is known");
  assert.match(rows[1].why, /above the most you allowed/);
  assert.equal(rows[1].reason, "ETH mid price, cheap only");
  assert.equal(rows[2].why, "More than one payment may be on its own.");
});

test("what the agents asked of the owner, each with its reason; a malformed ask is skipped, not guessed", () => {
  const rows = askRows(
    {
      limits: { at: 200, by: "uat-u3-ops", perAction: "0.004000 USDC", daily: "0.008000 USDC", reason: "the weather API now costs 0.004 a call" },
      mode: { at: 100, by: "uat-u3-ops", mode: 2, reason: "lower the risk" },
      payees: { at: 150, by: "uat-u3-ops", payees: ["0x1", "0x2"], allow: true },
      broken: 7,
    },
    [{ what: "approve 127.0.0.1 so the agent may pay it", why: "SOL mid price" }],
  );
  assert.deepEqual(rows.map((r) => r.text), [
    "uat-u3-ops asked for the limits 0.004000 USDC a payment, 0.008000 USDC a day",
    "uat-u3-ops asked to pay 2 payees",
    "uat-u3-ops asked to move to Autopilot",
    "Approve 127.0.0.1 so the agent may pay it",
  ]);
  assert.equal(rows[0].why, "the weather API now costs 0.004 a call");
  assert.equal(rows[1].why, null);
  assert.deepEqual(askRows(null, []), [], "no asks, no rows");
  assert.deepEqual(askRows("junk", []), []);
});

test("the spent share of the daily cap comes from the two figures the service sent, or from nothing", () => {
  assert.equal(spentShare("0.002000 USDC", "0.008000 USDC"), 0.25);
  assert.equal(spentShare("0.008000 USDC", "0.008000 USDC"), 1);
  assert.equal(spentShare("0.010000 USDC", "0.008000 USDC"), 1.25, "over the cap is over 1");
  assert.equal(spentShare(undefined, "0.008000 USDC"), null);
  assert.equal(spentShare("0.002000 USDC", "0.000000 USDC"), null, "a daily cap of zero has no share");
});

