#!/usr/bin/env node
// Records the live owner view and agent view of the demo account for the demo video (apps/video).
// One take, one page, Playwright recordVideo at 1280x800 in a fresh browser (no shared browser, no profile):
// load, slow scroll through the statement, 2 s hold, scroll back to the header, hold for the callouts, click the
// Owner | Agent capsule, hold on the agent view, expand one record to its three slots, hold.
// Then ffmpeg cuts the webm into public/footage/owner.mp4 (everything before the click: no hash can be on it) and
// public/footage/agent.mp4 (the click, then the loaded agent view; the ~7 s skeleton between them is cut out).
// Writes apps/video/src/footage.json (clip lengths, element boxes for the callouts) and a run log in proof/video-2026-10-18/.
// Usage: node scripts/video/record.mjs     (from the repo root or anywhere)
import { createRequire } from "node:module";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync, renameSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const APP = join(ROOT, "apps", "video");
const { chromium } = createRequire(join(APP, "package.json"))("playwright-core");
const FFMPEG = "/opt/homebrew/bin/ffmpeg";
const URL = "https://tami-home.up.railway.app/a/guest-demo-arc";
const HASH = /0x[0-9a-fA-F]{8,}/;
const W = 1280, H = 800;

const raw = mkdtempSync(join(tmpdir(), "tami-rec-"));
const log = [];
const say = (s) => { log.push(s); console.log(s); };
const stamp = () => execFileSync("date", ["+%Y-%m-%d %H:%M:%S %z"]).toString().trim();

const browser = await chromium.launch({ channel: "chromium" });
const ctx = await browser.newContext({ viewport: { width: W, height: H }, deviceScaleFactor: 1, colorScheme: "light", recordVideo: { dir: raw, size: { width: W, height: H } } });
const page = await ctx.newPage();
const t0 = Date.now();
const at = () => (Date.now() - t0) / 1000;
const marks = {};
const mark = (k) => { marks[k] = at(); say(`${k.padEnd(14)} ${marks[k].toFixed(2)} s`); };

say(`recorded ${stamp()} from ${URL}`);
await page.goto(URL, { waitUntil: "load", timeout: 90_000 });
await page.getByText("Allowance left today").waitFor({ timeout: 90_000 });
await page.waitForFunction(() => document.body.innerText.includes("Runway at this burn") && document.querySelector("button span.order-3"), null, { timeout: 90_000 });
await page.waitForTimeout(500); // the skeleton's own minimum hold ends; the figures settle
// The service sometimes answers without a burn rate ("No burn yet"); the scene's caption names days, so wait for a
// read that has one (the page re-reads every 30 s; a reload asks again). Every attempt is logged.
for (let i = 1; !/Runway at this burn\s*\d+\s*days/.test(await page.evaluate(() => document.body.innerText)); i++) {
  if (i > 8) throw new Error("the runway never showed days");
  say(`attempt ${i}: runway has no days figure; reload`);
  await page.waitForTimeout(4000);
  await page.reload({ waitUntil: "load" });
  await page.getByText("Allowance left today").waitFor({ timeout: 90_000 });
  await page.waitForTimeout(800);
}
mark("ownerLoaded");

// Boxes for the callouts, at scroll 0 (the hold at the top is where the callouts sit).
const boxes = await page.evaluate(() => {
  const card = (el) => { let e = el; while (e && getComputedStyle(e).backgroundColor === "rgba(0, 0, 0, 0)") e = e.parentElement; return e; };
  const byText = (t) => [...document.querySelectorAll("body *")].filter((e) => e.children.length === 0 && e.textContent.trim() === t).sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top)[0];
  const r = (e) => { const b = e.getBoundingClientRect(); return { x: b.left, y: b.top, w: b.width, h: b.height }; };
  const span = document.querySelector("button span.order-3");
  const why = document.createRange();
  why.selectNodeContents(span);
  return {
    allowance: r(card(byText("Allowance left today"))),
    runway: r(card(byText("Runway at this burn"))),
    needsYou: r(card(byText("Needs you"))),
    why: r(why),
    whyText: span.textContent.trim(),
    table: r(card(span)),
    scrollMax: document.documentElement.scrollHeight - innerHeight,
  };
});
say(`boxes ${JSON.stringify(boxes)}`);
const ownerText = await page.evaluate(() => document.body.innerText);
if (HASH.test(ownerText)) throw new Error(`owner view shows a hash: ${ownerText.match(HASH)[0]}`);
say("owner view text: no 0x… string (checked on load)");

const scrollTo = (y, ms) => page.evaluate(([y, ms]) => new Promise((done) => {
  const y0 = scrollY, s = performance.now();
  const step = (t) => { const p = Math.min(1, (t - s) / ms); const e = p < 0.5 ? 2 * p * p : 1 - (-2 * p + 2) ** 2 / 2; scrollTo(0, y0 + (y - y0) * e); p < 1 ? requestAnimationFrame(step) : done(); };
  requestAnimationFrame(step);
}), [y, ms]);

await page.waitForTimeout(1500);
mark("scrollDown");
await scrollTo(boxes.scrollMax, 5000);
mark("holdBottom");
await page.waitForTimeout(2000);
mark("scrollUp");
await scrollTo(0, 3000);
mark("holdTop");
if (HASH.test(await page.evaluate(() => document.body.innerText))) throw new Error("owner view shows a hash at the hold");
await page.waitForTimeout(11000);

mark("click");
await page.getByRole("button", { name: "Agent", exact: true }).click();
const firstRow = page.locator("button[aria-expanded]").first();
await firstRow.waitFor({ timeout: 90_000 });
await page.waitForTimeout(300);
mark("agentLoaded");
await page.waitForTimeout(2500);
mark("expand");
await firstRow.click();
await page.getByText("Slot", { exact: true }).waitFor({ timeout: 10_000 });
const slots = await page.evaluate(() => {
  const th = [...document.querySelectorAll("body *")].find((e) => e.children.length === 0 && e.textContent.trim() === "Slot");
  let e = th; while (e && !(e.textContent.includes("witness") && e.textContent.includes("counterparty"))) e = e.parentElement;
  const b = e.getBoundingClientRect();
  return { x: b.left, y: b.top, w: b.width, h: b.height, text: e.innerText };
});
say(`slots ${JSON.stringify(slots)}`);
await page.waitForTimeout(8000);
mark("end");

const video = page.video();
await ctx.close();
await browser.close();
const webm = await video.path();

// Cut and convert. -ss after -i: frame-accurate.
const out = join(APP, "public", "footage");
mkdirSync(out, { recursive: true });
const enc = ["-r", "30", "-c:v", "libx264", "-preset", "slow", "-crf", "14", "-pix_fmt", "yuv420p", "-an", "-movflags", "+faststart"];
const ff = (args) => execFileSync(FFMPEG, ["-y", "-loglevel", "error", ...args], { stdio: "inherit" });
// The take's clock and Date.now() differ by a fraction of a second. Calibrate on the flip: the first dark frame
// (the agent view is sea) after the click. Every mark moves by the same offset; the owner clip ends before that frame.
const probe = spawnSync(FFMPEG, ["-hide_banner", "-i", webm, "-vf", `select='between(t,${marks.click - 3},${marks.click + 2})',signalstats,metadata=print:key=lavfi.signalstats.YAVG`, "-an", "-f", "null", "-"], { encoding: "utf8" });
const series = [...probe.stderr.matchAll(/pts_time:([0-9.]+)[\s\S]*?YAVG=([0-9.]+)/g)].map((m) => [+m[1], +m[2]]);
const flip = series.find(([, y]) => y < 120)?.[0];
if (flip === undefined) throw new Error("no flip frame found after the click");
const shift = marks.click - flip;
say(`flip frame at ${flip.toFixed(2)} s of the take; marks move by ${(-shift).toFixed(2)} s`);
for (const k of Object.keys(marks)) marks[k] -= shift;
const ownerFrom = marks.ownerLoaded, ownerTo = marks.click - 0.1;
ff(["-i", webm, "-ss", ownerFrom.toFixed(3), "-to", ownerTo.toFixed(3), ...enc, join(out, "owner.mp4")]);
const a1 = [marks.click - 0.4, marks.click + 1.1], a2 = [marks.agentLoaded, marks.end];
ff(["-i", webm, "-filter_complex",
  `[0:v]trim=${a1[0].toFixed(3)}:${a1[1].toFixed(3)},setpts=PTS-STARTPTS[a];[0:v]trim=${a2[0].toFixed(3)}:${a2[1].toFixed(3)},setpts=PTS-STARTPTS[b];[a][b]concat=n=2:v=1[v]`,
  "-map", "[v]", ...enc, join(out, "agent.mp4")]);
renameSync(webm, join(raw, "take.webm"));

const footage = {
  recordedAt: log[0].replace(/^recorded (.*) from .*$/, "$1"),
  url: URL,
  size: { w: W, h: H },
  owner: {
    seconds: +(ownerTo - ownerFrom).toFixed(3),
    holdBottom: +(marks.holdBottom - ownerFrom).toFixed(3),
    holdTop: +(marks.holdTop - ownerFrom).toFixed(3),
    boxes,
  },
  agent: {
    seconds: +(a1[1] - a1[0] + a2[1] - a2[0]).toFixed(3),
    cut: +(a1[1] - a1[0]).toFixed(3),
    expand: +(a1[1] - a1[0] + marks.expand - a2[0]).toFixed(3),
    slots,
  },
};
writeFileSync(join(APP, "src", "footage.json"), JSON.stringify(footage, null, 2) + "\n");
const proof = join(ROOT, "proof", "video-2026-10-18");
mkdirSync(proof, { recursive: true });
say(`raw take kept outside the tree: ${join(raw, "take.webm")}`);
say(`owner.mp4 ${footage.owner.seconds} s (take ${ownerFrom.toFixed(2)}-${ownerTo.toFixed(2)}), agent.mp4 ${footage.agent.seconds} s (take ${a1.map((x) => x.toFixed(2)).join("-")} + ${a2.map((x) => x.toFixed(2)).join("-")})`);
say(`finished ${stamp()}`);
writeFileSync(join(proof, "record.txt"), `# node scripts/video/record.mjs\n${log.join("\n")}\n`);
