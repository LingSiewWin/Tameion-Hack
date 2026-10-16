// The browser's "Check it yourself" rebuilds the same root the Rust ledger anchored.
// Fixture: a real record of test account 0x7ebc…9273 (Arbitrum Sepolia), anchored in
// 0x6ac221401aacbfa961ea83186efb7c7dcd7e9f83ecdf211dffbf51f64f528efb.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { merkleRoot, rootOfEvents } from "../src/lib/verify.ts";

const jsonl = readFileSync(new URL("./fixtures/record-0xaf187d11.events.jsonl", import.meta.url), "utf8");

test("the root rebuilt from the events is the one the agent anchored", () => {
  const { root, count } = rootOfEvents(jsonl);
  assert.equal(count, 5);
  assert.equal(root, "0xaf187d11cf4816d307d30104b6bdadc604d7d48f584134ce4e80fca7be4d97cb");
});

test("one changed byte changes the root", () => {
  const lines = jsonl.trim().split("\n");
  const e = JSON.parse(lines[2]);
  e.canonical = e.canonical.slice(0, -2) + (e.canonical.endsWith("00") ? "01" : "00");
  lines[2] = JSON.stringify(e);
  assert.notEqual(rootOfEvents(lines.join("\n")).root, rootOfEvents(jsonl).root);
});

test("RFC 6962 shape: empty, one leaf, uneven split", () => {
  assert.equal(merkleRoot([]), `0x${"0".repeat(64)}`);
  const a = `0x${"11".repeat(32)}` as const;
  assert.equal(merkleRoot([a]), a);
  // three leaves split 2 + 1, so the third is not paired with a copy of itself
  const b = `0x${"22".repeat(32)}` as const;
  const c = `0x${"33".repeat(32)}` as const;
  assert.notEqual(merkleRoot([a, b, c]), merkleRoot([a, b, c, c]));
});
