// The lender's checks in the browser rebuild what the Rust desk signed and anchored.
// Fixture: the kit of deal GOLD-1792092934 on desk-gold-2329 (Arbitrum Sepolia), steps signed for
// that desk (EIP-712 v2), counterparties with roles, the short and the desk's holding in the statement (docs/92 T4),
// issued 2026-10-16 03:36 and anchored in 0x1c947bac… (proof/desk-api-third-review-2026-10-16-0335.txt).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { bookError, canonicalJson, checkAge, checkHistory, checkOffline, checkTogether, fundingOk, milestoneCanonical, minorUnits, parseParties, stagesFrom, type DeskStatement, type LenderKit } from "../src/lib/desk-verify.ts";

const kit = JSON.parse(readFileSync(new URL("./fixtures/desk-kit.json", import.meta.url), "utf8")) as LenderKit;
const SIGNER = "0x62f82e3494eb9c446c161a9f52f0a90b986e5062";
const PARTIES = Object.fromEntries((kit.parties ?? []).map((p) => [p.name, p.address]));
const pins = { signer: SIGNER, parties: PARTIES };

test("decimals become minor units exactly", () => {
  assert.equal(minorUnits("10.000", 3), 10000n);
  assert.equal(minorUnits("10", 3), 10000n);
  assert.equal(minorUnits("1334.45", 6), 1334450000n);
  assert.throws(() => minorUnits("0.0001", 3));
});

test("canonical JSON sorts keys at every level", () => {
  assert.equal(canonicalJson({ b: 1, a: { d: "x", c: [2, { f: 1, e: 0 }] } }), '{"a":{"c":[2,{"e":0,"f":1}],"d":"x"},"b":1}');
});

test("each signed step re-encodes to the disclosed line", () => {
  for (const [i, s] of (kit.milestones ?? []).entries()) assert.equal(milestoneCanonical(s.milestone), kit.disclosure.items[i].canonical);
});

test("the steps add up to the statement's grams (check h)", () => {
  const by = stagesFrom(kit.disclosure.items.map((i) => i.canonical));
  assert.equal(by.in_stock, 8000n);
  assert.equal(by.in_fabrication, 2000n);
  assert.deepEqual(parseParties("Supplier=0xab\n\nVault = 0xcd"), { Supplier: "0xab", Vault: "0xcd" });
});

test("the real kit passes every offline check when pinned", async () => {
  const checks = await checkOffline(kit, pins);
  for (const c of checks) assert.ok(c.ok, `${c.id}: ${c.lines.join(" ")}`);
  assert.deepEqual(checks.map((c) => c.id), ["a", "b", "c", "f", "h", "m"]);
});

test("a changed number, a changed step, a wrong signer, another desk, an unapproved party each fail", async () => {
  const k1 = structuredClone(kit);
  k1.signed.statement.eligibleUsdc = "1000000";
  assert.equal((await checkOffline(k1, pins)).find((c) => c.id === "a")?.ok, false);
  const k2 = structuredClone(kit);
  k2.disclosure.items[0].canonical = k2.disclosure.items[0].canonical.replace("PO-", "PX-");
  const c2 = await checkOffline(k2, pins);
  assert.equal(c2.find((c) => c.id === "c")?.ok, false);
  assert.equal(c2.find((c) => c.id === "f")?.ok, false);
  assert.equal((await checkOffline(kit, { ...pins, signer: "0x85e1c3924ed413a7e24e3388ed2c8697fd6b8904" })).find((c) => c.id === "b")?.ok, false);
  const k3 = structuredClone(kit);
  k3.desk = "0x000000000000000000000000000000000000dEaD";
  assert.equal((await checkOffline(k3, pins)).find((c) => c.id === "f")?.ok, false, "steps signed for another desk");
  assert.equal((await checkOffline(kit, { ...pins, parties: { ...PARTIES, Supplier: "0x000000000000000000000000000000000000dEaD" } })).find((c) => c.id === "f")?.ok, false);
  const k4 = structuredClone(kit);
  k4.signed.statement.byStage = (k4.signed.statement.byStage ?? []).map((l) => (l.stage === "in_fabrication" ? { ...l, grams: "0.000 g gold" } : l));
  assert.equal((await checkOffline(k4, pins)).find((c) => c.id === "h")?.ok, false, "grams that the steps do not add up to");
  const k5 = structuredClone(kit);
  (k5.signed.statement.milestones as unknown[]).pop();
  assert.equal((await checkOffline(k5, pins)).find((c) => c.id === "h")?.ok, false, "a statement listing fewer steps than disclosed");
});

// docs/91: kits forged from the live kit of GOLD-1792085266 by the second reviewer, re-signed with a throwaway
// statement key (0xc876…84cd) that stands in for the trader's own; before R1/R2 both passed every offline check.
const forged = (f: string) => JSON.parse(readFileSync(new URL(`./fixtures/${f}`, import.meta.url), "utf8")) as LenderKit;
const FORGER = "0xc876312d40df412c467e0eeca812d80b5d4884cd";
const failed = async (k: LenderKit) =>
  (await checkOffline(k, { signer: FORGER, parties: Object.fromEntries((k.parties ?? []).map((p) => [p.name, p.address])) }))
    .filter((c) => !c.ok)
    .map((c) => c.id);

test("R1: invented money fails m (price x10, max loss 0, eligible 1,000,000)", async () => {
  assert.deepEqual(await failed(forged("desk-kit-forged-money.json")), ["m"]);
});

test("R2: a step disclosed twice, reordered, or shown at another position fails c", async () => {
  assert.ok((await failed(forged("desk-kit-forged-twice.json"))).includes("c"));
  const k = forged("desk-kit-forged-money.json");
  const r = structuredClone(k); // reordered: the vault receipt before the purchase
  [r.disclosure.items[0], r.disclosure.items[1]] = [r.disclosure.items[1], r.disclosure.items[0]];
  [r.milestones![0], r.milestones![1]] = [r.milestones![1], r.milestones![0]];
  assert.ok((await failed(r)).includes("c"));
  const l = structuredClone(k); // the same steps, each claiming the other's position
  [l.disclosure.items[1].index, l.disclosure.items[2].index] = [l.disclosure.items[2].index, l.disclosure.items[1].index];
  [l.disclosure.items[1], l.disclosure.items[2]] = [l.disclosure.items[2], l.disclosure.items[1]];
  [l.milestones![1], l.milestones![2]] = [l.milestones![2], l.milestones![1]];
  assert.ok((await failed(l)).includes("c"));
  assert.throws(() => stagesFrom([k.disclosure.items[0].canonical, k.disclosure.items[0].canonical]), /one purchase/);
});

test("R10: grams that came in on the trader's own word are said", async () => {
  const k = structuredClone(kit);
  const arrived = (k.milestones ?? []).find((s) => s.milestone.kind === "arrived")!;
  arrived.signature = "";
  const f = (await checkOffline(k, pins)).find((c) => c.id === "f")!;
  assert.ok(f.lines.some((l) => /came into stock or transit on steps no counterparty signed/.test(l)), f.lines.join(" | "));
});

test("R7: a role pinned by the lender refuses a step of another kind", async () => {
  const supplier = PARTIES.Supplier;
  const f = async (role: string) => (await checkOffline(kit, { ...pins, parties: { ...PARTIES, Supplier: `${supplier}:${role}` } })).find((c) => c.id === "f")!;
  assert.equal((await f("bought")).ok, true);
  const wrong = await f("sold");
  assert.equal(wrong.ok, false);
  assert.ok(wrong.lines.some((l) => l.includes("Supplier may sign sold, not bought")));
});

// Statement A2 of HOLD-1792085890 (20 steps, 02:19) and the earlier statement of GOLD-1792085266 (12 steps, 01:34) on the
// same desk, plus one from before the 00:28 rewrite (v1 book, 18 steps): proof/desk-append-only-2026-10-16.txt.
test("n: the book extends an earlier statement, and a rewritten one does not", () => {
  const now = forged("desk-kit-hold-a2.json");
  assert.equal(checkHistory(now, forged("desk-kit-v2-3steps.json")).ok, true, "the first v2 statement (3 steps)");
  assert.equal(checkHistory(now, forged("desk-kit-v1-18.json")).ok, false, "the v1 book was rewritten");
  const t = structuredClone(now);
  t.history!.find((h) => h.leaves === 3)!.consistency[0] = `0x${"11".repeat(32)}`;
  assert.equal(checkHistory(t, forged("desk-kit-v2-3steps.json")).ok, false, "a changed path");
});

// docs/92: kits forged by the third reviewer, re-signed with a throwaway statement key (0xae04…6b41); each passed every pin before.
const FORGER3 = "0xae04d8a8cb1c2c0e021ac41e058d0c74a78d6b41";
const failed3 = async (f: string) => {
  const k = forged(f);
  return (await checkOffline(k, { signer: FORGER3, parties: Object.fromEntries((k.parties ?? []).map((p) => [p.name, p.address])) }))
    .filter((c) => !c.ok)
    .map((c) => c.id);
};

test("T2, T3: gold priced on another market, stress and basis at 0, an invented cost: each fails m", async () => {
  assert.deepEqual(await failed3("desk-kit-forged-market.json"), ["m"]);
  assert.deepEqual(await failed3("desk-kit-forged-stress.json"), ["m"]);
  assert.deepEqual(await failed3("desk-kit-forged-cost.json"), ["m"]);
});

test("T1: a step the lender's earlier statement showed cannot disappear from the same deal", () => {
  const n = checkHistory(forged("desk-kit-forged-hidden.json"), forged("desk-kit-gold-1792085266.json"));
  assert.equal(n.ok, false);
  assert.ok(n.lines.some((l) => /Step 11, shown in your earlier statement, is missing/.test(l)), n.lines.join(" | "));
});

test("T8: a statement older than the lender allows fails t", () => {
  const k = forged("desk-kit-gold-1792085266.json");
  assert.equal(checkAge(k, 24, k.signed.statement.at + 3600).ok, true);
  assert.equal(checkAge(k, 24, k.signed.statement.at + 25 * 3600).ok, false);
});

test("T4: a statement must carry the short it read and the desk's holding; a deal cannot claim more than its share", async () => {
  const m = async (k: LenderKit) => (await checkOffline(k, pins)).find((c) => c.id === "m")!;
  assert.equal((await m(kit)).ok, true);
  const shared = structuredClone(kit);
  shared.signed.statement.deskHeldContracts = "0.643014"; // a second deal of the same size shares the short
  const r = await m(shared);
  assert.equal(r.ok, false, "0.321 claimed where the share is 0.1605");
  assert.ok(r.lines.some((l) => /this deal's share of the 0.321000 short/.test(l)), r.lines.join(" | "));
  assert.equal((await m(forged("desk-kit-gold-1792085266.json"))).ok, false, "issued before statements carried the short");
});

// docs/94: kits forged by the fourth reviewer (throwaway statement key 0xae04…6b41, as in docs/92).
test("U3: positions written as text are refused before any check; out of order as numbers fails c", async () => {
  const k = forged("desk-kit-forged-order.json");
  const parties = Object.fromEntries((k.parties ?? []).map((p) => [p.name, p.address]));
  const text = await checkOffline(k, { signer: FORGER3, parties });
  assert.equal(text.length, 1);
  assert.match(text[0].lines[0], /not a whole number/);
  const n = forged("desk-kit-forged-order-numeric.json");
  const num = await checkOffline(n, { signer: FORGER3, parties: Object.fromEntries((n.parties ?? []).map((p) => [p.name, p.address])) });
  assert.equal(num.find((c) => c.id === "c")?.ok, false);
});

test("U4: eligible grams the steps do not give fail h", async () => {
  const k = forged("desk-kit-forged-words.json");
  const h = (await checkOffline(k, { signer: FORGER3, parties: PARTIES })).find((c) => c.id === "h")!;
  assert.equal(h.ok, false);
  assert.ok(h.lines.some((l) => /grams by class/.test(l)), h.lines.join(" | "));
});

test("U1: two statements at one root that each claim the whole short fail w together", () => {
  const two = structuredClone(kit);
  two.signed.statement.deal = "OTHER";
  assert.equal(checkTogether(kit, []).ok, true);
  assert.equal(checkTogether(kit, [two]).ok, false);
});

// docs/96: the morning file of 2026-10-16 04:55 (STOCK-A 4 g, STOCK-B 6 g, short 0.321) and the reviewer's forgeries.
const book = (f: string) => (forged(f) as unknown as { kits: LenderKit[] }).kits;
const morning = book("desk-book-morning.json");

test("V2: a whole-stock file is one moment of one desk, each deal once", () => {
  assert.equal(bookError(morning), null);
  assert.match(bookError(book("desk-book-mixed.json")) ?? "", /not one statement/);
  assert.match(bookError(book("desk-book-dup.json")) ?? "", /not one statement/);
  assert.match(bookError([]) ?? "", /no deals/);
});

test("V1: the morning file fails w once the short falls below what it counts (04:59:13, short 0.193)", () => {
  const [a, b] = morning;
  assert.equal(checkTogether(a, [b], 0.321).ok, true);
  assert.equal(checkTogether(b, [a], 0.321).ok, true);
  const later = checkTogether(a, [b], 0.193);
  assert.equal(later.ok, false);
  assert.ok(later.lines.some((l) => /a deal was sold/.test(l)), later.lines.join(" | "));
});

test("V3: a kit of another desk fails f under the set-up of the desk it is presented as", async () => {
  const parties = {
    Supplier: "0x85e1c3924ed413a7e24e3388ed2c8697fd6b8904:bought",
    Vault: "0x8b95fdedb647236da0f266231f71440d2714d542:arrived,returned",
    Buyer: "0x328fb78926cee49691851ca84f226d1c08edacb4:sold",
  };
  const f = (cs: { id: string; ok: boolean; lines: string[] }[]) => cs.find((c) => c.id === "f")!;
  const other = f(await checkOffline(forged("desk-kit-v3-other-desk.json"), { signer: "0xae04d8a8cb1c2c0e021ac41e058d0c74a78d6b41", desk: "0x048a9a549f19be4aa04a84dbcc51007f57d0160a", parties }));
  assert.equal(other.ok, false);
  assert.ok(other.lines.some((l) => /not the one you set up/.test(l)), other.lines.join(" | "));
  const own = f(await checkOffline(morning[0], { signer: "0x62f82e3494eb9c446c161a9f52f0a90b986e5062", desk: "0xcd8d71d97c3a946a14ac3fb92db4fb86e2974308", parties }));
  assert.equal(own.ok, true, own.lines.join(" | "));
});

test("V5: the service's own statements pass g at any funding; a doubled share still fails", () => {
  const sts = JSON.parse(readFileSync(new URL("./fixtures/desk-statements-funding.json", import.meta.url), "utf8")) as Record<string, DeskStatement>;
  const micro = (s: string) => Math.round(Number(s) * 1e6);
  for (const [name, st] of Object.entries(sts)) {
    const acct = micro(name.split("acct ")[1]);
    assert.equal(fundingOk(st, acct, micro(st.fundingUsdc as string)), true, name);
    if (Math.abs(acct) >= 1e6) assert.equal(fundingOk(st, acct, 2 * micro(st.fundingUsdc as string)), false, `${name} doubled`);
  }
});

// docs/99: the sixth review's whole-stock forgeries.
test("W1: a whole-stock file that leaves out a deal is refused, even with one deal left", () => {
  const omitted = book("desk-book-omitted.json");
  assert.equal(omitted.length, 1);
  assert.match(bookError(omitted) ?? "", /leaves out stock/);
  assert.equal(bookError(morning), null);
});

test("W8: forty honest deals at one root pass w despite 6-place rounding", () => {
  // 40 deals of 0.25 g each: each heldContracts rounds up by ~4.6e-7 oz; the desk total rounds once.
  const each = (0.25 / 31.1034768).toFixed(6);
  const total = ((40 * 0.25) / 31.1034768).toFixed(6);
  const deals = Array.from({ length: 40 }, (_, i) => {
    const k = structuredClone(morning[0]);
    Object.assign(k.signed.statement, { deal: `D${i}`, heldContracts: each, offsetContracts: each, deskHeldContracts: total, shortContracts: total });
    return k;
  });
  assert.ok(40 * Number(each) > Number(total) + 2e-6, "the old fixed tolerance would have failed this");
  assert.equal(checkTogether(deals[0], deals.slice(1)).ok, true);
});

// docs/100 X1: STOCK-B deleted and STOCK-A re-signed (throwaway key as the trader's) with the desk total lowered to its own.
test("X1: a desk total lowered below the account's short fails m", async () => {
  const lowered = book("desk-book-total-lowered.json");
  assert.equal(bookError(lowered), null, "it adds up to its own stated total");
  const m = (await checkOffline(lowered[0], { signer: "0xae04d8a8cb1c2c0e021ac41e058d0c74a78d6b41" })).find((c) => c.id === "m")!;
  assert.equal(m.ok, false);
  assert.ok(m.lines.some((l) => /more than the 0.128603 oz the desk says it holds/.test(l)), m.lines.join(" | "));
  const honest = (await checkOffline(morning[0])).find((c) => c.id === "m")!;
  assert.equal(honest.ok, true, honest.lines.join(" | "));
});

// docs/101 Y1, Y6: the lender's replay applies the desk's own rules.
test("Y1, Y6: a step twice under one document number, or a look-alike number, fails h", () => {
  const twice = forged("desk-kit-double-return.json");
  assert.throws(() => stagesFrom(twice.disclosure.items.map((i) => i.canonical)), /twice/);
  const lookalike = kit.disclosure.items.map((i) => i.canonical.replace(/document=([^\n]*)/, "document=$1⁠"));
  assert.throws(() => stagesFrom(lookalike), /printable ASCII/);
  assert.doesNotThrow(() => stagesFrom(kit.disclosure.items.map((i) => i.canonical)));
});
