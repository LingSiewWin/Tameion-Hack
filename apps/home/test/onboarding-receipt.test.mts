// The authority receipt (H4): every line comes from the mandate, so changing the mandate changes the receipt, and no
// number appears that the mandate does not hold. The fixture is a real inbox read from the lane service on :7890.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { listWords, receiptOf, spanWords, usdc, type MandateInbox } from "../src/features/onboarding/receipt.ts";

const inbox = JSON.parse(readFileSync(new URL("./fixtures/onboarding-inbox.json", import.meta.url), "utf8")) as MandateInbox & { pending: { eta: number } };
const now = inbox.pending.eta - 20;
const all = (r: ReturnType<typeof receiptOf>) => [...r.can, ...r.never, ...r.worst];

test("money is the shortest exact amount, never a dollar sign", () => {
  assert.equal(usdc("0.050000 USDC"), "0.05 USDC");
  assert.equal(usdc("0.005000 USDC"), "0.005 USDC");
  assert.equal(usdc("0.000000 USDC"), "0 USDC");
  assert.equal(usdc("12.000000 USDC"), "12 USDC");
  assert.equal(spanWords(30), "30 seconds");
  assert.equal(spanWords(172800), "2 days");
  assert.equal(listWords(["A", "B", "C"]), "A, B and C");
  assert.equal(listWords([]), "no one yet");
});

test("the real demo inbox: Propose, a pending Autopilot, one unnamed payee, a 30 s delay, no guardians", () => {
  const r = receiptOf(inbox, now);
  assert.equal(r.can[0].text, "Prepare each payment to Payee ••3854; nothing goes out until you tap.");
  assert.match(r.can[1].text, /^Switch to Autopilot at \d\d:\d\d, unless you cancel before then\.$/);
  assert.ok(r.never.some((l) => l.text === "Pay more than 0.005 USDC at a time, or more than 0.05 USDC in any 24 hours."));
  assert.ok(r.never.some((l) => l.text === "Pay a payee in its first 30 seconds after you approve it."));
  assert.ok(!r.never.some((l) => /Leave less than/.test(l.text)), "a zero floor says nothing about a floor");
  assert.equal(r.worst[0].text, "A stolen agent key moves nothing while each payment waits for your tap.");
  assert.match(r.worst.at(-1)!.text, /before adding guardians/);
  for (const l of all(r)) {
    assert.ok(l.rule.length > 0, `"${l.text}" names the rule that enforces it`);
    assert.doesNotMatch(l.text, /0x[0-9a-fA-F]{8,}/, "a receipt line never shows a hash");
    assert.doesNotMatch(l.text, /\$|guarantee|locked|yield/i);
  }
});

test("a different mandate is a different receipt", () => {
  const changed: MandateInbox = {
    ...inbox,
    mode: "Autopilot",
    pending: null,
    caps: { perAction: "1.250000 USDC", daily: "10.000000 USDC", floor: "2.500000 USDC" },
    terms: { ...inbox.terms, delay: "172800" },
    parties: { ...inbox.parties, [inbox.payees![0].address]: { label: "Demo Vendor" } },
    guardians: { list: ["0x1", "0x2", "0x3"], threshold: 2 },
  };
  const r = receiptOf(changed, now);
  assert.equal(r.can[0].text, "Pay Demo Vendor by itself, up to 1.25 USDC at a time and 10 USDC in any 24 hours.");
  assert.ok(!r.can.some((l) => /Switch to/.test(l.text)), "no pending change, no pending line");
  assert.ok(r.can.some((l) => l.text.includes("nothing rises for 2 days")));
  assert.ok(r.never.some((l) => l.text === "Leave less than 2.5 USDC in the account."));
  assert.equal(r.worst[0].text, "A stolen agent key moves at most 10 USDC in any 24 hours, and only to Demo Vendor.");
  assert.equal(r.worst.at(-1)!.text, "If you lose this device, 2 of your 3 guardians can name a new passkey.");
});

test("every number on the receipt is one the mandate holds", () => {
  const r = receiptOf(inbox, now);
  const allowed = new Set(["0.005", "0.05", "30", "24", "3854"]);
  for (const l of all(r)) {
    const nums = l.text.replace(/\b\d\d:\d\d\b/g, "").replace(/\b\d{1,2} [A-Z][a-z]{2}\b/g, "").match(/\d+(?:\.\d+)?/g) ?? [];
    for (const n of nums) assert.ok(allowed.has(n), `"${n}" in "${l.text}" is not a value of the mandate`);
  }
});
