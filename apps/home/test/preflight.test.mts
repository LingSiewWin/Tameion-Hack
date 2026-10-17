// The pure helpers of scripts/ci.mjs (the lane preflight): what a hash is, how DESIGN.md's table and tokens.css are read, which
// scenarios the matrix lists, and how two runs are compared. The browser part is proved by running `npm run ci` against the app.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";
// @ts-expect-error scripts/ci.mjs is plain JavaScript (allowJs is off)
import { HASH, contrast, cssTokens, designPalette, diffRuns, hexNorm, matrix, parseArgs, shotName } from "../scripts/ci.mjs";

test("a hash is 0x and eight or more hex digits, so a four-digit shorthand is not one", () => {
  assert.ok(HASH.test("0xf0ae90c66d410dc0cc3bc9e6a667af8c39fb6b85"));
  assert.ok(HASH.test("/a/0xDEADBEEF/add"));
  assert.ok(!HASH.test("0xf0ae"));
  assert.ok(!HASH.test("0.05 USDC"));
});

test("colours: hex and rgb() normalise, contrast follows WCAG", () => {
  assert.equal(hexNorm("#abc"), "#AABBCC");
  assert.equal(hexNorm("rgb(16, 20, 16)"), "#101410");
  assert.equal(hexNorm("oklch(1 0 0)"), null);
  assert.ok(contrast("#101410", "#EEF2EA") > 16 && contrast("#101410", "#EEF2EA") < 17);
  assert.ok(contrast("#1E5F46", "#0F1511") < 4.5, "DESIGN's night --up (same as day) fails 4.5:1");
});

test("DESIGN.md's palette table and tokens.css read to the same shape", () => {
  const md = new URL("../DESIGN.md", import.meta.url);
  if (existsSync(md)) {
    const p = designPalette(readFileSync(md, "utf8"));
    assert.equal(p.owner["--paper"], "#EEF2EA");
    assert.equal(p.agent["--paper"], "#0F1511");
    assert.equal(p.agent["--down"], p.owner["--down"], '"same" takes the Owner value');
  }
  const t = cssTokens(':root, :root[data-view="owner"] { --paper: #eef2ea; } /* x */ :root[data-view="agent"] { --paper: #0F1511; }');
  assert.equal(t.owner["--paper"], "#eef2ea");
  assert.equal(t.agent["--paper"], "#0F1511");
});

test("the matrix: every state of a route in both views at both widths, filtered by flags", () => {
  const all = matrix({}, { policy: true });
  assert.equal(all.filter((s: { route: string }) => s.route === "account").length, 8 * 2 * 2);
  const one = matrix(parseArgs(["--route", "account", "--state=loading,error", "--view", "owner", "--width", "390"]), { policy: true });
  assert.deepEqual(one.map(shotName), ["account-loading-owner-390.png", "account-error-owner-390.png"]);
  assert.ok(matrix({ route: "add" }, { policy: false }).every((s: { skip: boolean }) => s.skip), "no POLICY: account routes are skipped, not guessed");
});

test("two runs: what is new, what is fixed", () => {
  assert.deepEqual(diffRuns(["a", "b"], ["b", "c"]), { added: ["c"], fixed: ["a"] });
});
