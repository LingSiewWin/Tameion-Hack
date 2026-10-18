// lib/mark.ts: a face for a name, drawn in ink only. Same id, same cells; two payees differ; agents and payees wear
// different masks; the seal is symmetric both ways; the SVG carries no colour and no hash.
import assert from "node:assert/strict";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { avatarPixels, GRID, HUMANOID_MASK, Mark, markPath, SEAL_MASK } from "../src/lib/mark.ts";

const VENDOR = "0xe5e2d8dcc2038fd0a010a6e7845378b646e33854";
const AGENT = "0x270d9f049e75b5eead616975196edf80b756b3cb";
const cells = (id: string, kind: "agent" | "payee") => markPath(avatarPixels(id, kind));

test("the same id draws the same cells, every time", () => {
  assert.equal(cells(VENDOR, "payee"), cells(VENDOR, "payee"));
  assert.equal(cells(AGENT, "agent"), cells(AGENT, "agent"));
  assert.ok(avatarPixels(VENDOR, "payee").length > 0);
});

test("two payees differ, and one id wears a different face as agent and as payee", () => {
  assert.notEqual(cells(VENDOR, "payee"), cells("0x" + "ab".repeat(20), "payee"));
  assert.notEqual(cells(VENDOR, "agent"), cells(VENDOR, "payee"));
});

test("agents and payees have different masks; every cell sits on its mask and is mirrored", () => {
  const half = (m: ReadonlyArray<ReadonlyArray<0 | 1>>) => m.map((r) => r.slice(0, GRID / 2).join("")).join("/");
  assert.notEqual(half(HUMANOID_MASK), half(SEAL_MASK));
  for (const [kind, mask] of [["agent", HUMANOID_MASK], ["payee", SEAL_MASK]] as const) {
    const px = avatarPixels(VENDOR, kind);
    const set = new Set(px.map((p) => `${p.x},${p.y}`));
    for (const p of px) {
      const x = p.x < GRID / 2 ? p.x : GRID - 1 - p.x;
      assert.equal(mask[p.y][x], 1, `${kind} cell ${p.x},${p.y} is on the mask`);
      assert.ok(set.has(`${GRID - 1 - p.x},${p.y}`), `${kind} cell ${p.x},${p.y} has its mirror`);
    }
  }
});

test("the seal is 16 x 16, symmetric left-right and top-bottom: a ring with a 4 x 4 core", () => {
  assert.equal(SEAL_MASK.length, GRID);
  for (let y = 0; y < GRID; y++) {
    assert.equal(SEAL_MASK[y].length, GRID);
    for (let x = 0; x < GRID; x++) {
      assert.equal(SEAL_MASK[y][x], SEAL_MASK[y][GRID - 1 - x], `left-right at ${x},${y}`);
      assert.equal(SEAL_MASK[y][x], SEAL_MASK[GRID - 1 - y][x], `top-bottom at ${x},${y}`);
    }
  }
  for (let y = 6; y <= 9; y++) for (let x = 6; x <= 9; x++) assert.equal(SEAL_MASK[y][x], 1, "the 4 x 4 core");
  for (const [x, y] of [[5, 7], [10, 7], [7, 5], [7, 10]]) assert.equal(SEAL_MASK[y][x], 0, "a gap between core and ring");
  for (const [x, y] of [[1, 7], [14, 7], [7, 1], [7, 14]]) assert.equal(SEAL_MASK[y][x], 1, "the ring");
});

test("<Mark> is one hidden SVG in the ink token at 24 and 40 px, with no colour literal and no hash", () => {
  for (const size of [24, 40] as const) {
    const html = renderToStaticMarkup(createElement(Mark, { id: VENDOR, kind: "payee", size }));
    assert.match(html, new RegExp(`^<svg width="${size}" height="${size}" viewBox="0 0 16 16" aria-hidden="true"`));
    assert.equal((html.match(/<path /g) ?? []).length, 1, "one path");
    assert.match(html, /fill="currentColor"/);
    assert.match(html, /text-\(--ink\)/);
    assert.doesNotMatch(html, /#[0-9a-fA-F]{3,8}\b|rgb\(|hsl\(/, "no colour literal");
    assert.doesNotMatch(html, /0x[0-9a-fA-F]{8,}/, "the id is never drawn");
  }
});
