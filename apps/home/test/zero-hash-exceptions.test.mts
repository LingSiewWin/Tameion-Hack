// The zero-hash rule's one allowed exception (founder, 2026-10-18): the receive address, shown whole with Copy beside it, in
// exactly two components — /add's AddressPlate and /receive's ReceiveAddress. Every other hash on an Owner surface is a leak;
// the Owner home's own test (owner-no-hash) allows nothing. This test holds the whitelist to those two and nothing else.
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

const SRC = new URL("../src/", import.meta.url).pathname;
const WHITELIST = { AddressPlate: "features/add-money/add-money.tsx", ReceiveAddress: "features/onboarding/receive.tsx" };

const files = (dir: string): string[] =>
  readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? files(p) : /\.(tsx?|mts)$/.test(f) ? [p] : [];
  });

test("data-hash-allowed names exactly the two receive-address components, each in its own file, once", () => {
  const found: string[] = [];
  for (const f of files(SRC)) {
    for (const m of readFileSync(f, "utf8").matchAll(/data-hash-allowed="([^"]*)"/g)) found.push(`${m[1]} ${f.slice(SRC.length)}`);
  }
  assert.deepEqual(found.sort(), Object.entries(WHITELIST).map(([who, f]) => `${who} ${f}`).sort());
});

test("the browser preflight allows the same two, each on its own route only", () => {
  const ci = readFileSync(new URL("../scripts/ci.mjs", import.meta.url), "utf8");
  assert.match(ci, /const ALLOWED = \{ AddressPlate: "add", ReceiveAddress: "receive" \};/);
});

test("the Owner home allows none", () => {
  for (const f of files(join(SRC, "features/owner"))) assert.doesNotMatch(readFileSync(f, "utf8"), /data-hash-allowed/, f);
});
