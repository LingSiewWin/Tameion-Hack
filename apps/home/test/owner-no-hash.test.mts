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
const { OwnerView } = await import("../src/features/owner/owner-view.tsx");
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
  return halvesByPayment([{ root: "0x" + "cd".repeat(32), seq: 2, leaf: "0x" + "ef".repeat(32), kind: "SETTLEMENT", at: p.at, tx: p.tx, agent: slot, counterparty_slot: slot, witness: slot }]);
};

const cases: [string, string][] = [
  ["at rest", render()],
  ["the Stop question", render({ asking: true })],
  ["a row's sheet open", render({ openKey: lines(inbox, NOW)[0].key })],
  ["sealed, its sheet open", render({ halves: sealedHalves(), openKey: lines(inbox, NOW)[0].key })],
  ["stale", render({ staleAt: NOW * 1000 - 360_000, onRetry: () => {} })],
  ["empty (a fresh account)", render({ inbox: fresh, byRateDays: null })],
  ["guest", render({ guest: true })],
];

for (const [name, html] of cases)
  test(`no hash in the Owner view: ${name}`, () => {
    const m = HASH.exec(html);
    assert.equal(m, null, m ? `found ${m[0].slice(0, 12)}… near: ${html.slice(Math.max(0, m.index - 80), m.index + 20)}` : "");
  });

test("the seeded page shows what it is for (so the empty check above is not vacuous)", () => {
  const html = cases[0][1];
  assert.match(html, /Your agents may spend 0\.05 USDC a day, 0\.005 at a time, only on Demo Vendor\./);
  assert.match(html, />Stop the agent</);
  assert.match(html, /What the agent paid, and why/);
  assert.equal((html.match(/>Demo Vendor</g) ?? []).length, 8, "eight paid rows, each by name");
  assert.match(cases[3][1], /Paid and sealed/);
  assert.doesNotMatch(cases[6][1], /Stop the agent/, "a guest has no Stop");
  assert.match(cases[5][1], /Nothing paid yet/);
});

test("the test catches a hash when one leaks (a row whose payee has no name falls back to words, never the address)", () => {
  const leaky = renderToStaticMarkup(createElement("a", { href: `/a/${inbox.policy}` }, "x"));
  assert.match(leaky, HASH);
  const unnamed = render({ inbox: { ...inbox, parties: {} } });
  assert.doesNotMatch(unnamed, HASH);
  assert.match(unnamed, /a payee you approved/);
});
