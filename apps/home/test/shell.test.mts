// The shell renders: the header (name, network word, capsule) around the page, and no hash anywhere (DESIGN Rule 1).
import "../scripts/tsx.mjs";
import assert from "node:assert/strict";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

const { Shell } = await import("../src/components/shell.tsx");

test("the shell renders the header around the page", () => {
  const html = renderToStaticMarkup(createElement(Shell, null, createElement("p", null, "the page")));
  assert.match(html, /<header[^>]*>/);
  assert.match(html, /<span class="row-title[^"]*">Tameion<\/span>/, "no account: the name reads Tameion");
  assert.match(html, /<span class="pill meta[^"]*">Testnet<\/span>/, "the network word, in words and a hollow pill");
  assert.match(html, /role="group" aria-label="View"/);
  assert.match(html, /aria-pressed="true"[^>]*>Owner<\/button>/, "Owner is the view until the device remembers another");
  assert.match(html, /aria-pressed="false"[^>]*>Agent<\/button>/);
  assert.match(html, /<main[^>]*><p>the page<\/p><\/main>/);
  assert.doesNotMatch(html, /0x[0-9a-fA-F]{8,}/);
});
