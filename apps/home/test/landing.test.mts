// `/` behind HOME_HERO (founder: the hero ships behind a flag until the five-second test passes). Unset: the statement is
// the page's one h1 with the guest card under it, and no hero markup, scene or tracks. "1": the hero, for A, B and C.
import "../scripts/tsx.mjs";
import assert from "node:assert/strict";
import { test } from "node:test";
import { Children, createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

const { default: GuestPage } = await import("../src/app/page.tsx");
const { Hero } = await import("../src/features/hero/hero.tsx");
const { HeroSlot } = await import("../src/features/hero/slot.tsx");

async function landing(flag: string | undefined, treatment?: string): Promise<ReactElement<{ children: ReactElement[] }>> {
  const before = process.env.HOME_HERO;
  if (flag === undefined) delete process.env.HOME_HERO;
  else process.env.HOME_HERO = flag;
  try {
    return (await GuestPage({ searchParams: Promise.resolve({ treatment }) })) as ReactElement<{ children: ReactElement[] }>;
  } finally {
    if (before === undefined) delete process.env.HOME_HERO;
    else process.env.HOME_HERO = before;
  }
}
const h1s = (html: string) => (html.match(/<h1[\s>]/g) ?? []).length;

test("HOME_HERO unset: the statement is the one h1, the guest card follows, and nothing of the hero", async () => {
  for (const flag of [undefined, "", "0", "true"]) {
    for (const treatment of [undefined, "B", "C"]) {
      const html = renderToStaticMarkup(await landing(flag, treatment));
      assert.doesNotMatch(html, /data-hero|hero-statement/, `flag ${flag} treatment ${treatment}`);
      assert.doesNotMatch(html, /scene(-828)?\.avif|ais\.json|coastline/, "no scene, no tracks");
      assert.equal(h1s(html), 1);
      assert.match(html, /<h1[^>]*>Trade moves before money does\.<\/h1>/);
      assert.match(html, /id="guest"/);
    }
  }
});

// next/dynamic renders the hero only inside Next's server (it preloads the chunk), so the page is checked for the slot and
// its props, and the hero is rendered from exactly those props; the production HTML is checked end to end in the report.
test('HOME_HERO="1": the hero above the guest card, for A, B and C, still one h1', async () => {
  for (const treatment of [undefined, "B", "C"]) {
    const [slot, rest] = Children.toArray((await landing("1", treatment)).props.children) as ReactElement<Record<string, unknown>>[];
    assert.equal(slot.type, HeroSlot, "the hero comes through its own chunk");
    assert.equal(slot.props.treatment, treatment ?? "A");
    const html = renderToStaticMarkup(createElement(Hero, slot.props as Parameters<typeof Hero>[0])) + renderToStaticMarkup(rest);
    assert.match(html, /data-hero/, `treatment ${treatment}`);
    assert.match(html, /hero-statement/);
    assert.match(html, new RegExp(`data-treatment="${treatment ?? "A"}"`));
    assert.equal(h1s(html), 1);
    assert.match(html, /id="guest"/);
  }
});
