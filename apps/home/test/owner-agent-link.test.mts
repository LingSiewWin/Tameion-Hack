// "Open in Agent view" (Owner home) sets #pass-<unix time> and flips the capsule; the Agent view's stream marks the newest
// record of each agent check with that id (once: two records of one check share their time) and scrolls to it.
import "../scripts/tsx.mjs";
import assert from "node:assert/strict";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { Symbolon } from "../src/lib/types.ts";

const { Stream } = await import("../src/features/agent/stream.tsx");

test("each agent check is reachable as pass-<at>, exactly once", () => {
  const rec = (seq: number, at: number) => ({ root: `0x${"a1".repeat(32)}`, seq, leaf: `0x${"f1".repeat(32)}`, kind: "DECISION", at, agent: null, counterpartySlot: null, witness: null });
  const data: Symbolon = {
    ok: true,
    contractVersion: "2026-10-18.1",
    policy: `0x${"cf".repeat(20)}`,
    chain: "arbitrum-sepolia",
    explorer: "https://sepolia.arbiscan.io",
    locked: false,
    counts: { unsigned: 3, single: 0, double: 0, triple: 0 },
    chainReadFailed: [],
    records: [rec(0, 1792279953), rec(1, 1792279953), rec(0, 1792279833)],
  };
  const html = renderToStaticMarkup(createElement(Stream, { data }));
  assert.equal((html.match(/id="pass-1792279953"/g) ?? []).length, 1);
  assert.equal((html.match(/id="pass-1792279833"/g) ?? []).length, 1);
});
