// The owner's actions on the Owner home (docs/122, M1): the calls they sign are the ones the lane service's prepare-* routes
// returned for guest-demo-arc (2026-10-18), the five minutes read their step from the account, the limits say raise or cut
// before anything is signed, and every new surface (the five minutes, a change waiting with its Cancel, the three sheets)
// renders with no hash and with the controls seat A1's acceptance script finds by name.
import "../scripts/tsx.mjs";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { Demo, Inbox } from "../src/lib/types.ts";

process.env.TZ = "Asia/Kuala_Lumpur";
const { calls, limitsCall } = await import("../src/features/owner/owner-call.ts");
const { changeOf, clockText, everyWords, modeIndex, nextDue, payeeLabel, stripOf } = await import("../src/features/owner/words.ts");
const { OwnerView } = await import("../src/features/owner/owner-view.tsx");

const read = <T,>(f: string) => JSON.parse(readFileSync(new URL(`./fixtures/${f}`, import.meta.url), "utf8")) as T;
const inbox = read<Inbox>("owner-inbox.json");
const fresh = read<Inbox>("owner-inbox-fresh.json");
const VENDOR = "0xe5e2d8dcc2038fd0a010a6e7845378b646e33854";
const demo: Demo = { enabled: true, vendor: VENDOR, bill: { usdc: "0.002", everySecs: 120 }, delaySecs: 30 };
const NOW = 1792276548;
const HASH = /0x[0-9a-fA-F]{8,}/;
const noop = () => {};
const named = { [VENDOR]: { label: "Demo Vendor", kind: "tameion", verified: true, note: "", tail: "3854" } } as Inbox["parties"];

test("the calls are byte for byte the ones the service prepared (prepare-cancel, prepare-payees allow:false, prepare-limits)", () => {
  assert.equal(calls.cancelPending(), "0xdedad97d");
  assert.equal(
    calls.setPayees([VENDOR], false),
    "0x48a56503000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000e5e2d8dcc2038fd0a010a6e7845378b646e33854",
  );
  const l = limitsCall({ floor: "0.000000 USDC" }, 4000n, 50_000n);
  assert.deepEqual(l.body, { perAction: "0.004000", daily: "0.050000", floor: "0.000000" });
  assert.equal(
    l.data,
    "0x189ae5f20000000000000000000000000000000000000000000000000000000000000fa0000000000000000000000000000000000000000000000000000000000000c3500000000000000000000000000000000000000000000000000000000000000000",
  );
});

test("limits: a raise waits, a cut applies at once, both is a change; a bad figure is refused in words before any signing", () => {
  const caps = { perAction: "0.005000 USDC", daily: "0.050000 USDC" };
  assert.deepEqual(changeOf(caps, "0.01", "0.05"), { kind: "raise", perAction: 10_000n, daily: 50_000n });
  assert.deepEqual(changeOf(caps, "0.004", "0.05"), { kind: "cut", perAction: 4000n, daily: 50_000n });
  assert.equal((changeOf(caps, "0.004", "0.1") as { kind: string }).kind, "change");
  assert.equal((changeOf(caps, "0.005", "0.05") as { kind: string }).kind, "same");
  assert.deepEqual(changeOf(caps, "0.06", "0.05"), { error: "One payment cannot be more than a day." });
  assert.deepEqual(changeOf(caps, "0", "0.05"), { error: "A limit must be more than 0." });
  for (const bad of ["", "1,000", "0.0000001", "-1", "abc"]) assert.deepEqual(changeOf(caps, bad, "1"), { error: "Write each limit as a number of USDC, like 0.05." }, bad);
});

test("the five minutes read their step from the account", () => {
  assert.equal(stripOf(inbox, demo).step, 4, "funded, bill saved, Autopilot, paid");
  assert.equal(stripOf({ ...fresh, balance: "0.000000 USDC" }, demo).step, 0);
  assert.equal(stripOf(fresh, demo).step, 1);
  const billed = { ...fresh, payees: inbox.payees, typedBills: inbox.typedBills };
  assert.equal(stripOf(billed, demo).step, 2);
  assert.equal(stripOf({ ...billed, mode: "Autopilot" }, demo).step, 3);
  assert.equal(stripOf({ ...fresh, payees: inbox.payees }, demo).step, 1, "approved but no bill saved: the tap saves it");
});

test("small words: a countdown, a cadence, the next due time, a mode, a payee by name", () => {
  assert.equal(clockText(27.2), "0:28");
  assert.equal(clockText(102), "1:42");
  assert.equal(clockText(-5), "0:00");
  assert.equal(everyWords(120), "every 2 minutes");
  assert.equal(everyWords(60), "every minute");
  assert.equal(everyWords(86_400 * 30), "every 30 days");
  assert.equal(nextDue(100, 120, 50), 100);
  assert.equal(nextDue(100, 120, 101), 220);
  assert.deepEqual([modeIndex("Watch"), modeIndex("Propose"), modeIndex("Autopilot")], [0, 1, 2]);
  assert.equal(payeeLabel({ parties: named, payees: inbox.payees }, VENDOR), "Demo Vendor");
  assert.equal(payeeLabel({ parties: {}, payees: inbox.payees }, VENDOR), "the payee ending 3854");
});

// ---- rendered ---------------------------------------------------------------------------------------------------------

const act = (over: Record<string, unknown> = {}) => ({ phases: {}, sheet: null, onSheet: noop, onBill: noop, onMode: noop, onCancel: noop, onApprove: noop, onStopPayee: noop, onLimits: noop, ...over });
const render = (i: Inbox, over: Record<string, unknown> = {}) =>
  renderToStaticMarkup(createElement(OwnerView, { inbox: { ...i, parties: { ...i.parties, ...named } }, byRateDays: null, halves: new Map(), nowSecs: NOW, act: act(), ...over }));
const autopilotWaits = { ...fresh, payees: inbox.payees, typedBills: inbox.typedBills, pending: { mode: "Autopilot", kinds: 2, eta: NOW + 27 } };
const raiseWaits = { ...inbox, pending: { kinds: 1, eta: NOW + 20, perActionCap: "0.010000 USDC", dailyCap: "0.050000 USDC" } };

const cases: [string, string][] = [
  ["the five minutes: the bill", render(fresh, { demo })],
  ["the five minutes: Autopilot", render({ ...fresh, payees: inbox.payees, typedBills: inbox.typedBills }, { demo })],
  ["the five minutes: Autopilot counting down", render(autopilotWaits, { demo, clockSecs: NOW })],
  ["the five minutes: the bill due", render({ ...autopilotWaits, pending: null, mode: "Autopilot" }, { demo, clockSecs: NOW })],
  ["the five minutes: done, the first row", render(inbox, { demo })],
  ["a raise counting down", render(raiseWaits, { clockSecs: NOW })],
  ["the payees sheet", render(inbox, { act: act({ sheet: "payees" }) })],
  ["the limits sheet", render(inbox, { act: act({ sheet: "limits" }) })],
  ["the mode sheet", render(inbox, { act: act({ sheet: "mode" }) })],
  ["a guest during a raise", render(raiseWaits, { guest: true, clockSecs: NOW })],
];
for (const [name, html] of cases)
  test(`no hash on the new owner surfaces: ${name}`, () => {
    const m = HASH.exec(html);
    assert.equal(m, null, m ? `found ${m[0].slice(0, 12)}… near: ${html.slice(Math.max(0, m.index - 80), m.index + 20)}` : "");
  });

test("each new control carries the name the acceptance script finds it by", () => {
  const page = Object.fromEntries(cases);
  assert.match(page["the five minutes: the bill"], />Try it in 5 minutes</);
  assert.match(page["the five minutes: the bill"], /<button[^>]*>Let the agent pay Demo Vendor 0\.002 USDC every 2 minutes<\/button>/);
  assert.match(page["the five minutes: Autopilot"], /<button[^>]*>Turn Autopilot on<\/button>/);
  const counting = page["the five minutes: Autopilot counting down"];
  assert.match(counting, /Autopilot turns on\. It applies in 0:27 unless you cancel\./);
  assert.equal((counting.match(/>Cancel<\/button>/g) ?? []).length, 1, "one Cancel, in the five minutes, not twice");
  assert.match(page["the five minutes: the bill due"], /The bill is due/);
  const done = page["the five minutes: done, the first row"];
  assert.match(done, /Done\. The agent paid Demo Vendor by itself/);
  assert.match(done, /Done\. The agent paid[^<]*<\/p><ul[^>]*><li[^>]*><button[^>]*>.*?Paid by the agent: a bill the owner typed/, "the first row, with its why, is the ledger's own row");
  assert.match(page["a raise counting down"], /the limits rise to 0\.05 USDC a day, 0\.01 at a time\. It applies in 0:20 unless you cancel\./);
  assert.match(page["a raise counting down"], />Cancel<\/button>/);
  assert.doesNotMatch(page["a guest during a raise"], />Cancel<\/button>/, "a reader sees the change coming, never its Cancel");
  assert.doesNotMatch(page["a guest during a raise"], /Change what the agent may do/);
  for (const k of ["Payees", "Limits", "Mode"]) assert.match(page["the payees sheet"], new RegExp(`<button[^>]*>${k}</button>`));
  const payees = page["the payees sheet"];
  assert.match(payees, /<button[^>]*>Stop paying Demo Vendor<\/button>/);
  assert.match(payees, /<label for="payee-address"[^>]*>Payee address<\/label>/);
  assert.match(payees, /<label for="payee-name"[^>]*>Name<\/label>/);
  assert.match(payees, /<button type="submit"[^>]*>Approve payee<\/button>/);
  const limits = page["the limits sheet"];
  assert.match(limits, /<label for="limit-per"[^>]*>One payment, at most \(USDC\)<\/label>/);
  assert.match(limits, /<input id="limit-per"[^>]*value="0\.005"/);
  assert.match(limits, /<input id="limit-day"[^>]*value="0\.05"/);
  assert.doesNotMatch(/id="limit-per".*?<\/form>/.exec(limits)?.[0] ?? "", /type="submit"/, "nothing changed yet: no button to press");
  const modes = page["the mode sheet"];
  for (const m of ["Watch", "Propose", "Autopilot"]) assert.match(modes, new RegExp(`<button[^>]*aria-pressed="(true|false)"[^>]*><span class="row-title">${m}</span>`));
  assert.match(modes, /aria-pressed="true"[^>]*><span class="row-title">Autopilot<\/span>/, "the contract's mode is the pressed one");
});
