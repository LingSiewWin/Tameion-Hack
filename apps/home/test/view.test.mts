// The capsule's pure parts: the view a stored value names, the `v` key, and the boot script that runs before paint.
import assert from "node:assert/strict";
import { test } from "node:test";
import { isViewKey, otherView, parseView } from "../src/lib/view.ts";
import { VIEW_BOOT, VIEW_KEY } from "../src/tokens.ts";

test("only 'agent' names the Agent view; anything else is Owner", () => {
  assert.equal(parseView("agent"), "agent");
  for (const v of ["owner", null, undefined, "", "Agent", 1]) assert.equal(parseView(v), "owner");
  assert.equal(otherView("owner"), "agent");
  assert.equal(otherView("agent"), "owner");
});

test("v flips the view, except while typing or with a modifier", () => {
  const key = (over: Partial<Parameters<typeof isViewKey>[0]> = {}) =>
    isViewKey({ key: "v", metaKey: false, ctrlKey: false, altKey: false, target: { tagName: "BODY", isContentEditable: false } as unknown as EventTarget, ...over });
  assert.equal(key(), true);
  assert.equal(key({ key: "V" }), false);
  assert.equal(key({ metaKey: true }), false, "cmd-v pastes");
  assert.equal(key({ ctrlKey: true }), false);
  assert.equal(key({ altKey: true }), false);
  for (const tagName of ["INPUT", "TEXTAREA", "SELECT"]) assert.equal(key({ target: { tagName, isContentEditable: false } as unknown as EventTarget }), false, tagName);
  assert.equal(key({ target: { tagName: "DIV", isContentEditable: true } as unknown as EventTarget }), false);
});

test("the boot script sets Agent before paint only when the device remembered Agent, and survives blocked storage", () => {
  const boot = (getItem: (k: string) => string | null) => {
    const html = { dataset: {} as Record<string, string> };
    new Function("localStorage", "document", VIEW_BOOT)({ getItem }, { documentElement: html });
    return html.dataset.view;
  };
  assert.equal(boot((k) => (k === VIEW_KEY ? "agent" : null)), "agent");
  assert.equal(boot(() => "owner"), undefined, "Owner is the server's markup already");
  assert.equal(boot(() => null), undefined);
  assert.equal(
    boot(() => {
      throw new Error("SecurityError");
    }),
    undefined,
  );
});
