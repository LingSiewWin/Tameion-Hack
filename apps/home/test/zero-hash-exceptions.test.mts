// The zero-hash rule's allowed exceptions (founder, 2026-10-18): the receive address, shown whole with Copy beside it, in
// exactly two components — /add's AddressPlate and /receive's ReceiveAddress — and (docs/122, M1) the payee address field on
// the Owner home, where the owner types the address of a payee to approve; it holds only what the owner typed. Every other
// hash on an Owner surface is a leak; the Owner home's own test (owner-no-hash) renders it with that field empty and allows
// nothing. This test holds the whitelist to these and nothing else.
import "../scripts/tsx.mjs";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

const SRC = new URL("../src/", import.meta.url).pathname;
const WHITELIST = {
  AddressPlate: "features/add-money/add-money.tsx",
  ReceiveAddress: "features/onboarding/receive.tsx",
  // the header's Console link: the account's address in its href only (founder, 2026-10-18: judges get one link)
  "console-link": "components/header.tsx",
  // the payee's address, as the owner types it to approve that payee (docs/122): an input, never a drawn address
  "payee-address": "features/owner/actions-view.tsx",
};

const files = (dir: string): string[] =>
  readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? files(p) : /\.(tsx?|mts)$/.test(f) ? [p] : [];
  });

test("data-hash-allowed names exactly the two receive-address components, the Console link and the payee field, each in its own file, once", () => {
  const found: string[] = [];
  for (const f of files(SRC)) {
    for (const m of readFileSync(f, "utf8").matchAll(/data-hash-allowed="([^"]*)"/g)) found.push(`${m[1]} ${f.slice(SRC.length)}`);
  }
  assert.deepEqual(found.sort(), Object.entries(WHITELIST).map(([who, f]) => `${who} ${f}`).sort());
});

test("the browser preflight allows the same, each on its own route only", () => {
  const ci = readFileSync(new URL("../scripts/ci.mjs", import.meta.url), "utf8");
  assert.match(ci, /const ALLOWED = \{ AddressPlate: "add", ReceiveAddress: "receive" \};/);
  assert.match(ci, /const FIELD = \{ "payee-address": "account" \};/);
  assert.match(ci, /FIELD\[who\] === route && e\.tagName === "INPUT"/, "the payee field is allowed only as an input, only on the account page");
  assert.match(ci, /field && a\.name === "value"/, "the field's address is allowed in its value, no other attribute");
  assert.match(ci, /const LINK = "console-link";/);
  assert.match(ci, /!\(link && a\.name === "href"\)/, "the link's address is allowed in its href, no other attribute");
});

test("the Owner home allows only the payee field, and only on an input", () => {
  for (const f of files(join(SRC, "features/owner"))) {
    const marks = [...readFileSync(f, "utf8").matchAll(/<(\w+)[^<>]*?data-hash-allowed="([^"]*)"/g)].map((m) => `${m[1]} ${m[2]}`);
    assert.deepEqual(marks, f.endsWith("actions-view.tsx") ? ["input payee-address"] : [], f);
  }
});

test("the payee field renders empty: no address reaches the Owner DOM until the owner types one", async () => {
  const { createElement } = await import("react");
  const { renderToStaticMarkup } = await import("react-dom/server");
  const { Mandate } = await import("../src/features/owner/actions-view.tsx");
  const inbox = JSON.parse(readFileSync(new URL("./fixtures/owner-inbox.json", import.meta.url), "utf8"));
  const noop = () => {};
  const act = { phases: {}, sheet: "payees" as const, onSheet: noop, onBill: noop, onMode: noop, onCancel: noop, onApprove: noop, onStopPayee: noop, onLimits: noop };
  const html = renderToStaticMarkup(createElement(Mandate, { inbox, act }));
  const field = /<input[^>]*data-hash-allowed="payee-address"[^>]*>/.exec(html)?.[0] ?? "";
  assert.match(field, /value=""/);
  assert.doesNotMatch(html, /0x[0-9a-fA-F]{8,}/);
});

test("the Console link: the console's origin and the account's address, or nothing", async () => {
  const { CONSOLE, consoleHref } = await import("../src/lib/console.ts");
  const policy = "0x8e388dc7a4098b0e7837b6607a70114238655623";
  assert.equal(consoleHref("https://console.example/", policy), `https://console.example/?policy=${policy}`);
  assert.equal(consoleHref(null, policy), null, "unset: not drawn");
  assert.equal(consoleHref("", policy), null);
  assert.equal(consoleHref("javascript:alert(1)", policy), null, "only an http(s) origin");
  assert.equal(consoleHref("https://console.example", undefined), null, "no address yet: not drawn");
  assert.equal(consoleHref("https://console.example", "guest-demo-arc"), null, "an address, not a name");
  assert.equal(CONSOLE.label, "Console");
  assert.equal(CONSOLE.title, "Every owner action: payees, bills, Autopilot, limits");
});
