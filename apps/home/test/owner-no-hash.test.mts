// DESIGN Rule 1: zero hashes in the Owner view. Renders the Owner home from real inboxes captured on the lane service
// (northwind-agents, 2026-10-18: one after eight payments, one freshly opened) in every state it draws — at rest, the
// Stop question, a row's sheet open, sealed, stale, empty, guest — and fails on any 0x… of 8+ hex digits, hrefs included.
import "../scripts/tsx.mjs";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { Inbox, Status } from "../src/lib/types.ts";

process.env.TZ = "Asia/Kuala_Lumpur";
const { OwnerError, OwnerView } = await import("../src/features/owner/owner-view.tsx");
const { lines, halvesByPayment } = await import("../src/features/owner/words.ts");

const read = <T,>(f: string) => JSON.parse(readFileSync(new URL(`./fixtures/${f}`, import.meta.url), "utf8")) as T;
const inbox = read<Inbox>("owner-inbox.json");
const fresh = read<Inbox>("owner-inbox-fresh.json");
const status = read<Status>("owner-status.json");
const NOW = 1792276548;
const HASH = /0x[0-9a-fA-F]{8,}/;

const render = (over: Record<string, unknown> = {}) =>
  renderToStaticMarkup(createElement(OwnerView, { inbox, byRateDays: status.runway?.byRateDays, halves: new Map(), nowSecs: NOW, ...over }));

// A settlement record as H3's route gives it, all three slots verified (real accounts have at most two today).
const sealedHalves = () => {
  const p = inbox.paid[0];
  const slot = { alg: "secp256k1", keyId: "0x" + "ab".repeat(20), verified: true };
  return halvesByPayment([{ root: "0x" + "cd".repeat(32), seq: 2, leaf: "0x" + "ef".repeat(32), kind: "SETTLEMENT", at: p.at, tx: p.tx, agent: slot, counterpartySlot: slot, witness: slot }]);
};

const cases: [string, string][] = [
  ["at rest", render()],
  ["the Stop question", render({ asking: true })],
  ["a row's sheet open, the agent's mark beside it", render({ openKey: lines(inbox, NOW)[0].key, agentId: "0x270d9f049e75b5eead616975196edf80b756b3cb" })],
  ["a folded run's sheet open", render({ openKey: `run-${lines(inbox, NOW)[0].key}` })],
  ["sealed, its sheet open", render({ halves: sealedHalves(), openKey: lines(inbox, NOW)[0].key })],
  ["stale, the service silent", render({ staleAt: NOW * 1000 - 360_000, silent: true, onRetry: () => {} })],
  ["stale, served by the service", render({ staleAt: NOW * 1000 - 20_000 })],
  ["empty (a fresh account)", render({ inbox: fresh, byRateDays: null })],
  ["guest", render({ guest: true })],
  ["the cold error", renderToStaticMarkup(createElement(OwnerError, { missing: false }))],
  ["no such account", renderToStaticMarkup(createElement(OwnerError, { missing: true }))],
];

for (const [name, html] of cases)
  test(`no hash in the Owner view: ${name}`, () => {
    const m = HASH.exec(html);
    assert.equal(m, null, m ? `found ${m[0].slice(0, 12)}… near: ${html.slice(Math.max(0, m.index - 80), m.index + 20)}` : "");
  });

test("the seeded page shows what it is for (so the empty check above is not vacuous)", () => {
  const page = Object.fromEntries(cases);
  const html = page["at rest"];
  assert.match(html, /Your agents may spend 0\.05 USDC a day, 0\.005 at a time, only on Demo Vendor\./);
  assert.match(html, />Stop the agent</);
  assert.match(html, /What the agent paid, and why/);
  // eight identical payments in a row fold into one row in the founder's words (never more than three identical rows)
  assert.match(html, /Demo Vendor · paid 8 times today · 0\.016 USDC/);
  assert.equal((html.match(/<li class="border-b/g) ?? []).length, 1, "one row stands for the run");
  assert.equal((html.match(/<svg width="24" height="24" viewBox="0 0 16 16"/g) ?? []).length, 2, "the run's payee mark, and the payee in Who is who");
  assert.match(page["a folded run's sheet open"], /0\.002 USDC<\/span><\/li>/, "the run's sheet lists its payments");
  assert.equal((page["a folded run's sheet open"].match(/>Today 06:\d\d</g) ?? []).length >= 8, true, "all eight, by time");
  const sheet = page["a row's sheet open, the agent's mark beside it"];
  assert.match(sheet, /<svg width="40" height="40" viewBox="0 0 16 16"/, "the sheet: the payee's seal at 40 px");
  assert.equal((sheet.match(/<svg width="24" height="24" viewBox="0 0 16 16"/g) ?? []).length, 4, "the run row, the agent and payee in Who is who, and the agent beside its line in the sheet");
  assert.match(page["sealed, its sheet open"], /Paid and sealed/);
  assert.doesNotMatch(page["guest"], /Stop the agent/, "a guest has no Stop");
  const empty = page["empty (a fresh account)"];
  assert.match(empty, /Nothing paid yet: the first payment goes to a payee you approve/);
  assert.doesNotMatch(empty, /Approve a payee under Needs you/, "an empty line never names a button that is not there");
  for (const k of ["the cold error", "no such account"]) assert.match(page[k], /<h1 class="authority[^"]*">/, "the cold error has the page's one heading");
  assert.match(page["stale, the service silent"], /The service did not answer\. The figures below are as of/);
  assert.match(page["stale, served by the service"], /as of /);
  assert.doesNotMatch(page["stale, served by the service"], /did not answer/, "a last good read the service served is not a silence");
});

test("the test catches a hash when one leaks (a row whose payee has no name falls back to words, never the address)", () => {
  const leaky = renderToStaticMarkup(createElement("a", { href: `/a/${inbox.policy}` }, "x"));
  assert.match(leaky, HASH);
  const unnamed = render({ inbox: { ...inbox, parties: {} } });
  assert.doesNotMatch(unnamed, HASH);
  assert.match(unnamed, /a payee you approved/);
});
