// Screenshots of the hero's keyframes (p = 0, .2, .4, .6, .8, 1) for each treatment at 1280 and 390, plus the
// reduced-motion page, from a private headless Chrome (never the shared MCP browser).
//   node scripts/hero-shots.mjs <outDir> [base=http://localhost:3017] [treatments=ABC]
// PW_EXECUTABLE=<path> uses that Chromium instead of the installed Chrome.
import { mkdirSync, readFileSync } from "node:fs";
import { AxeBuilder } from "@axe-core/playwright";
import { chromium } from "playwright-core";

const [out, base = "http://localhost:3017", which = "ABC"] = process.argv.slice(2);
if (!out) throw new Error("usage: hero-shots.mjs <outDir> [base] [treatments]");
mkdirSync(out, { recursive: true });
const browser = await chromium.launch({ headless: true, ...(process.env.PW_EXECUTABLE ? { executablePath: process.env.PW_EXECUTABLE } : { channel: "chrome" }) });
const sizes = [[1280, 800], [390, 844]];
const scrollTo = (page, p) =>
  page.evaluate((p) => {
    const s = document.querySelector("section[data-hero]");
    const top = s.getBoundingClientRect().top + window.scrollY;
    window.scrollTo(0, top + p * (s.offsetHeight - window.innerHeight));
  }, p);

for (const t of which) {
  for (const [width, height] of sizes) {
    const ctx = await browser.newContext({ viewport: { width, height } }); // axe needs its own context
    const page = await ctx.newPage();
    await page.goto(`${base}/?treatment=${t}`, { waitUntil: "networkidle" });
    await page.waitForSelector('[data-ready="1"]', { timeout: 20_000 });
    for (const p of [0, 0.2, 0.4, 0.6, 0.8, 1]) {
      await scrollTo(page, p);
      await page.waitForTimeout(300);
      // the book: wait until the Owner home has read the account (its skeleton says aria-busy while it loads)
      if (p === 1) await page.waitForFunction(() => !document.querySelector("[data-hero] [aria-busy='true']"), null, { timeout: 20_000 }).catch(() => console.log(`WARN ${t} ${width}: the book was still loading`));
      const path = `${out}/hero-${t}-p${String(Math.round(p * 100)).padStart(3, "0")}-${width}.png`;
      await page.screenshot({ path });
      console.log(path);
    }
    // axe on the page at rest (p = 0) and at the book (p = 1)
    for (const p of [0, 1]) {
      await scrollTo(page, p);
      await page.waitForTimeout(300);
      const { violations } = await new AxeBuilder({ page }).analyze();
      console.log(`axe ${t} ${width} p=${p}: ${violations.length ? violations.map((v) => `${v.id} (${v.nodes.length}) ${v.nodes[0]?.target}`).join("; ") : "clean"}`);
    }
    const sw = await page.evaluate(() => document.documentElement.scrollWidth);
    if (sw > width) console.log(`WARN ${t} ${width}: the page scrolls sideways (scrollWidth ${sw})`);
    await ctx.close();
  }
}
// reduced motion: three stills instead of the scroll sequence
for (const [width, height] of sizes) {
  const page = await browser.newPage({ viewport: { width, height }, reducedMotion: "reduce" });
  await page.goto(`${base}/`, { waitUntil: "networkidle" });
  await page.waitForTimeout(1200);
  const path = `${out}/hero-reduced-motion-${width}.png`;
  await page.screenshot({ path, fullPage: true });
  console.log(path);
  await page.close();
}
// one contact sheet per treatment and width: the six keyframes in order, for the founder's pick
const sheet = await browser.newPage({ viewport: { width: 1320, height: 900 } });
for (const t of which) {
  for (const [width] of sizes) {
    const imgs = [0, 20, 40, 60, 80, 100].map((p) => `${out}/hero-${t}-p${String(p).padStart(3, "0")}-${width}.png`);
    const cell = width === 1280 ? "width:420px" : "width:200px";
    const html = `<body style="margin:0;padding:12px;display:flex;flex-wrap:wrap;gap:12px;background:white;font:12px sans-serif">${imgs
      .map((f, i) => `<figure style="margin:0"><img style="${cell};display:block" src="data:image/png;base64,${readFileSync(f).toString("base64")}"><figcaption>${t} p=${(i * 0.2).toFixed(1)} ${width}</figcaption></figure>`)
      .join("")}</body>`;
    await sheet.setContent(html);
    const path = `${out}/sheet-${t}-${width}.png`;
    await sheet.screenshot({ path, fullPage: true });
    console.log(path);
  }
}
await browser.close();
