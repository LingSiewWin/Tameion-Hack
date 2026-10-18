#!/usr/bin/env node
// The judge's five minutes (docs/115 §3), driven end to end in a headless browser, every figure read twice: once from the
// page, once from the service's own API through the same origin (docs/122 D-RC-4, seat A1). The retire-console merge is
// measured by this script: green on tami-console first (the baseline), then green on tami-home.
//
//   node apps/home/scripts/five-minutes.mjs --origin <url> --invite-file <path> [--console]
//
// Passkeys work headless through a CDP virtual authenticator (ctap2, internal, resident key, user verified). The invite
// word is read from the file and typed into the page; it is never printed: every line this script writes is scrubbed of
// it, and every screenshot masks its field. Each run opens one demo account, which spends about 0.055 test USDC from the
// host's demo relayer. One line per step: `STEP n PASS|FAIL <what was read> | from <where>`, then `RESULT PASS|FAIL k/5`,
// exit 0 only when all five pass. Screenshots: proof/five-minutes/<host>-<time>/.

import { mkdirSync, readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";

// ---- What the script looks for, per origin: controls by accessible name, reads by the page's own words. ------------
// Ops: { fill: <label>, with: "invite" | "name", optional } · { click: <button name> } · { see: <button name> } ·
// { text: <page text> } (the text read is logged).
// Reads are regexes over the page's text; a capture group is the figure read.
const MAP = {
  // apps/console as served by the Rust service (components/start.tsx, components/account/judge.tsx).
  console: {
    open: [
      { fill: /^Your account's name$/, with: "name" },
      { fill: /^Invite word$/, with: "invite", optional: true }, // shown while the host needs the word (cfg.inviteNeeded)
      { click: /^Try it in 5 minutes$/ },
    ],
    funded: /Funded: ([\d.,]+) USDC/,
    bill: [{ click: /^Let the agent pay Demo Vendor / }],
    billDone: /Demo Vendor approved; the bill saved/,
    autopilot: [{ click: /^Turn Autopilot on$/ }],
    countdown: /Autopilot applies in (\d+:\d\d) unless you cancel/,
    cancelAutopilot: /^Cancel$/,
    paidRow: /Why, as recorded:/, // the row: amount, payee, who paid, Receipt; the why line; the book line
    paidPayee: /USDC\s+to\s+(.+?)\s+by (?:the agent|you)/,
    paidWhy: /Why, as recorded:\s*(.+)/,
    paidLink: /^Receipt$/,
    raise: /^Raise one payment to ([\d.,]+) USDC$/,
    raiseWait: /applies in (\d+:\d\d)\./,
    cancelRaise: /^Cancel the raise$/,
    // The console offers the cut only once the raise no longer waits: cancel it first (one passkey tap, instant).
    beforeCut: [{ click: /^Cancel the raise$/ }, { text: /Cancelled at once/ }],
    cut: /^Cut one payment to ([\d.,]+) USDC$/,
    cutDone: /Cut at once|the cut applied at once/, // the finish card replaces the step the moment the cut is signed
  },
  // apps/home at lane/home-actions 6447365 (names from M1's report, docs/research-data/102-team/reports/M1-report.md):
  // the guest card, the /new stepper (its own bill tap), then the strip on /a/<name>, owner device only.
  home: {
    open: [
      { fill: /^Invite word/, with: "invite" },
      { click: /^Make your own account/ },
      { fill: /^Name the account$/, with: "name" },
      { click: /^Continue$/ },
      { click: /^Make my passkey$/ },
      { see: /^Let the agent pay Demo Vendor / }, // the Allowance step: the account is open and funded
    ],
    funded: null, // "The account holds …" shows for under a second (P-406): the figure is read from the inbox, and the
    // page's own figure from the receipt in step 2 ("It holds 0.05 USDC on …")
    bill: [{ click: /^Let the agent pay Demo Vendor / }, { text: /It holds ([\d.,]+) USDC/ }, { click: /^Open the account$/ }],
    billDone: /Demo Vendor approved; the bill saved/,
    autopilot: [{ click: /^Turn Autopilot on$/ }],
    countdown: /It applies in (\d+:\d\d) unless you cancel/,
    cancelAutopilot: /^Cancel$/,
    paidRow: /Paid by the agent/, // a listitem: "<when> <payee> <amount> USDC <why> <state>"
    paidPayee: /^(?:Today \d\d:\d\d|Yesterday|\d+ \w+)\s+(.+?)\s+[\d.,]+ USDC/,
    paidWhy: /[\d.,]+ USDC\s+(.+?)\s+(?:Paid and sealed|Paid)$/,
    paidLink: null, // the Owner view shows no tx (zero-hash rule): the tx is the inbox's payment of that amount
    raise: /^Raise one payment to ([\d.,]+) USDC$/,
    raiseWait: /the limits rise to .+? a day, .+? at a time\. It applies in (\d+:\d\d) unless you cancel\./,
    cancelRaise: /^Cancel$/,
    beforeCut: [], // the cut sits beside the waiting raise
    cut: /^Cut (?:one payment|the day) to ([\d.,]+) USDC$/,
    cutDone: /Cut, at once: .+? at a time, .+? a day\./,
  },
};

// Public RPC per chain key. GET /api/level1/config names each chain's explorer but no RPC (read 2026-10-18).
const RPC = { "arbitrum-sepolia": "https://sepolia-rollup.arbitrum.io/rpc", "arc-testnet": "https://rpc.testnet.arc.io" };
const FUND = 0.05; // what the demo funds a new account with (docs/115 §3 step 1)

// ---- arguments ---------------------------------------------------------------------------------------------------------
const { values: arg } = parseArgs({ options: { origin: { type: "string" }, "invite-file": { type: "string" }, console: { type: "boolean", default: false } } });
if (!arg.origin || !arg["invite-file"]) {
  console.error("usage: node apps/home/scripts/five-minutes.mjs --origin <url> --invite-file <path> [--console]");
  process.exit(2);
}
const origin = arg.origin.replace(/\/+$/, "");
const invite = readFileSync(arg["invite-file"], "utf8").trim();
if (!invite) {
  console.error("the invite file is empty");
  process.exit(2);
}
const M = MAP[arg.console ? "console" : "home"];

const pad = (n) => String(n).padStart(2, "0");
const hms = (d = new Date()) => `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
const unixHms = (s) => (s ? hms(new Date(s * 1000)) : "?");
const start = new Date();
const stamp = `${start.getFullYear()}${pad(start.getMonth() + 1)}${pad(start.getDate())}-${pad(start.getHours())}${pad(start.getMinutes())}${pad(start.getSeconds())}`;
const name = `accept-${pad(start.getHours())}${pad(start.getMinutes())}`;
const shots = new URL(`../../../proof/five-minutes/${new URL(origin).host.replace(":", "-")}-${stamp}/`, import.meta.url).pathname;
mkdirSync(shots, { recursive: true });

const say = (line) => console.log(String(line).split(invite).join("[invite word]"));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const num = (s) => parseFloat(String(s ?? "").replace(/,/g, ""));
const short = (a) => (a ? `${a.slice(0, 6)}…${a.slice(-4)}` : "?");
const need = (key) => {
  if (M[key] == null) throw new Error(`home map: "${key}" not filled yet (M1)`);
  return M[key];
};
/** Polls `fn` until it returns something truthy; null after `ms`. */
async function until(fn, ms, every = 3000) {
  const end = Date.now() + ms;
  for (;;) {
    const v = await fn().catch(() => null);
    if (v) return v;
    if (Date.now() > end) return null;
    await sleep(every);
  }
}

// ---- browser -----------------------------------------------------------------------------------------------------------
const browser = await chromium.launch({ headless: true, channel: "chromium" }); // Playwright's own Chromium, a fresh profile
const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const page = await ctx.newPage();
page.setDefaultTimeout(60_000);
const cdp = await ctx.newCDPSession(page);
await cdp.send("WebAuthn.enable");
await cdp.send("WebAuthn.addVirtualAuthenticator", {
  options: { protocol: "ctap2", transport: "internal", hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
});
// Every API answer above 399 the page got, so a flaky run says where (paths only; bodies never logged).
page.on("response", (r) => {
  const u = new URL(r.url());
  if (u.pathname.startsWith("/api/") && r.status() >= 400) say(`  ${hms()} http ${r.status()} ${r.request().method()} ${u.pathname}`);
});

const inviteField = () => {
  const op = M.open.find((o) => o.with === "invite");
  return op ? [page.getByLabel(op.fill)] : [];
};
const pageText = async () => (await page.locator("body").innerText()).replace(/\s+/g, " ").trim();
const waitText = (re, ms) => until(async () => (await pageText()).match(re), ms, 1000);
const shown = (re) => page.getByRole("button", { name: re }).filter({ visible: true });
const tap = async (re) => {
  say(`  ${hms()} tap "${re.source}"`);
  await shown(re).first().click({ timeout: 90_000 });
};

async function run(ops) {
  for (const op of ops) {
    if (op.fill) {
      const field = page.getByLabel(op.fill);
      if (op.optional && !(await field.waitFor({ state: "visible", timeout: 10_000 }).then(() => true, () => false))) continue;
      await field.fill(op.with === "invite" ? invite : name);
      say(`  ${hms()} typed the ${op.with === "invite" ? "invite word (from the file)" : `name ${name}`}`);
    } else if (op.click) await tap(op.click);
    else if (op.see) await shown(op.see).first().waitFor({ timeout: 240_000 });
    else if (op.text) {
      const m = await waitText(op.text, 120_000);
      if (!m) throw new Error(`no "${op.text.source}" on the page within 120 s`);
      say(`  ${hms()} page: "${m[0]}"`);
    }
  }
}

/** GET through the origin's own API path, with the page's cookies; retried while the host or its RPC is busy. */
async function api(path) {
  let last;
  for (let i = 0; i < 4; i++) {
    try {
      const r = await ctx.request.get(origin + path, { timeout: 45_000 });
      if (r.ok()) return await r.json();
      last = `${r.status()} ${(await r.text()).slice(0, 200)}`;
      if (r.status() < 500 && r.status() !== 429) break;
    } catch (e) {
      last = e.message;
    }
    await sleep(3000 * (i + 1));
  }
  throw new Error(`GET ${path}: ${last}`);
}

async function receipt(chain, tx) {
  const url = RPC[chain];
  if (!url) return { error: `no public RPC known for chain "${chain}"` };
  for (let i = 0; i < 10; i++) {
    try {
      const r = await fetch(url, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "eth_getTransactionReceipt", params: [tx] }),
        signal: AbortSignal.timeout(20_000),
      });
      if (r.ok) {
        const j = await r.json();
        if (j.result) return j.result;
      }
    } catch {
      // the public RPC rate-limits (P-12, P-14): wait and ask again
    }
    await sleep(Math.min(20_000, 4000 * (i + 1)));
  }
  return { error: `no receipt from ${url} after 10 tries` };
}

// ---- the five steps (each returns { ok, read, from }) -------------------------------------------------------------------
let demo, policy, chain;
const inbox = () => api(`/api/level1/inbox?policy=${policy}`);
const vendorIs = (a) => a?.toLowerCase() === demo.vendor.toLowerCase();

async function open() {
  await page.goto(`${origin}/`, { waitUntil: "domcontentloaded" });
  await run(M.open);
  const m = M.funded ? await waitText(M.funded, 240_000) : null;
  if (M.funded && !m) return { ok: false, read: `no "${M.funded.source}" on the page within 240 s`, from: "page text" };
  const rec = await api(`/api/level1/name/${name}`);
  policy = rec.account;
  chain = rec.chains?.[0]?.key;
  const b = await until(async () => {
    const x = await inbox();
    return num(x.balance) > 0 && x;
  }, 180_000, 5000);
  const ok = (!m || num(m[1]) === FUND) && !!b && num(b.balance) === FUND;
  return {
    ok,
    read: `funded page ${m ? `"${m[0]}"` : "(open; the figure is read from the inbox)"} · inbox balance ${b?.balance ?? "none within 180 s"} · account ${name} ${policy} on ${chain}`,
    from: `page text + GET /api/level1/name/${name} + GET /api/level1/inbox`,
  };
}

async function bill() {
  await run(need("bill"));
  const m = await waitText(need("billDone"), 180_000);
  const got = await until(async () => {
    const x = await inbox();
    const p = x.payees?.find((y) => vendorIs(y.address));
    const t = x.typedBills?.find((y) => vendorIs(y.payee));
    return p && t && { p, t };
  }, 180_000, 4000);
  const ok = !!m && !!got && num(got.t.amount) === num(demo.bill.usdc) && got.t.everySecs === demo.bill.everySecs;
  return {
    ok,
    read:
      `page "${m?.[0] ?? `no "${M.billDone.source}"`}" · inbox ` +
      (got
        ? `payee ${short(demo.vendor)} approved, payable from ${unixHms(got.p.payableFrom)}; bill ${got.t.amount} every ${got.t.everySecs} s, first due ${unixHms(got.t.nextDue)}`
        : "no approved payee + bill for the demo vendor within 180 s"),
    from: "page text + GET /api/level1/inbox (payees, typedBills)",
  };
}

async function autopilot() {
  const t0 = Date.now();
  await run(need("autopilot"));
  const c = await waitText(need("countdown"), 120_000);
  const cancel = c ? (await shown(need("cancelAutopilot")).count()) > 0 : false;
  const pend = await until(async () => {
    const x = await inbox();
    return (x.pending?.mode === "Autopilot" || x.mode === "Autopilot") && x;
  }, 90_000);
  const on = await until(async () => {
    const x = await inbox();
    return x.mode === "Autopilot" && x;
  }, 240_000, 5000);
  const ok = !!c && cancel && !!on;
  return {
    ok,
    read:
      `page "${c?.[0] ?? `no "${M.countdown.source}"`}" with Cancel ${cancel ? "shown" : "MISSING"} · inbox ` +
      (pend?.pending ? `pending mode ${pend.pending.mode} eta ${unixHms(pend.pending.eta)}; ` : "") +
      (on ? `mode Autopilot by ${hms()} (${Math.round((Date.now() - t0) / 1000)} s after the tap)` : `mode still ${pend?.mode ?? "?"} after 240 s`),
    from: "page text + GET /api/level1/inbox (pending, mode)",
  };
}

async function watch() {
  const row = page.getByRole("listitem").filter({ hasText: need("paidRow") }).first();
  const seen = await row.waitFor({ state: "visible", timeout: 300_000 }).then(() => true, () => false);
  if (!seen) return { ok: false, read: `no paid row ("${M.paidRow.source}") on the page within 5 min`, from: "page" };
  const text = (await row.innerText()).replace(/\s+/g, " ").trim();
  const amount = text.match(/([\d.,]+)\s*USDC/)?.[1];
  const payee = text.match(need("paidPayee"))?.[1];
  const why = text.match(need("paidWhy"))?.[1]?.replace(/\s*In the book:.*$/, "");
  // The tx: the row's link where the page shows one; otherwise (home's Owner view) the inbox's latest agent payment of
  // that amount to the demo vendor.
  const href = M.paidLink ? await row.getByRole("link", { name: M.paidLink }).first().getAttribute("href").catch(() => null) : null;
  say(`  ${hms()} row: ${text.slice(0, 400)}`);
  const p = await until(async () => {
    const paid = (await inbox()).paid ?? [];
    if (M.paidLink) return href && paid.find((y) => y.tx && href.toLowerCase().includes(y.tx.toLowerCase()));
    return paid.filter((y) => vendorIs(y.counterparty) && y.by !== "you" && num(y.amount) === num(amount)).sort((x, y) => (y.at ?? 0) - (x.at ?? 0))[0];
  }, 90_000, 5000);
  const tx = p?.tx;
  const rc = tx ? await receipt(chain, tx) : { error: "no tx link in the row" };
  const ok = !!amount && !!payee && !!why && !!p && num(p.amount) === num(amount) && vendorIs(p.counterparty) && p.by !== "you" && rc.status === "0x1";
  return {
    ok,
    read:
      `page ${amount ?? "?"} USDC to ${payee ?? "?"}, why "${why ?? "?"}", tx ${href ?? "not on the page"} · inbox ` +
      (p ? `${p.amount} to ${short(p.counterparty)} at ${unixHms(p.at)} (due ${unixHms(p.dueAt)}), by ${p.by ?? "the agent"}, reason "${p.reason ?? ""}", tx ${tx}` : "no matching paid entry") +
      ` · chain ${chain} receipt ${rc.status ? `status ${parseInt(rc.status, 16)} block ${parseInt(rc.blockNumber, 16)}` : rc.error}`,
    from: `page row + GET /api/level1/inbox (paid) + eth_getTransactionReceipt on ${RPC[chain] ?? "?"}`,
  };
}

async function raiseCut() {
  const before = (await inbox()).caps;
  const to = (await shown(need("raise")).first().innerText({ timeout: 90_000 })).trim().match(M.raise)?.[1];
  await tap(M.raise);
  const w = await waitText(need("raiseWait"), 180_000);
  const cancel = w ? (await shown(need("cancelRaise")).count()) > 0 : false;
  const pend = await until(async () => {
    const x = await inbox();
    return x.pending && (x.pending.kinds & 1) && x.pending;
  }, 90_000);
  await run(M.beforeCut);
  const cutLabel = (await shown(need("cut")).first().innerText({ timeout: 120_000 })).trim();
  const cutTo = cutLabel.match(M.cut)?.[1];
  const cap = /the day/.test(cutLabel) ? "daily" : "perAction"; // "Cut the day to …" once one payment equals the bill
  await tap(M.cut);
  const done = await waitText(need("cutDone"), 180_000);
  const tDone = Date.now();
  const after = await until(async () => {
    const x = await inbox();
    return num(x.caps[cap]) === num(cutTo) && x;
  }, 90_000, 2000);
  const lag = after ? Math.round((Date.now() - tDone) / 1000) : null;
  const ok = !!w && cancel && !!pend && num(pend.perActionCap) === num(to) && !!done && !!after && lag <= demo.delaySecs;
  return {
    ok,
    read:
      `cap ${before.perAction} → raise to ${to} USDC: page "${w?.[0] ?? `no "${M.raiseWait.source}"`}" with Cancel ${cancel ? "shown" : "MISSING"}, inbox ` +
      (pend ? `pending perActionCap ${pend.perActionCap} eta ${unixHms(pend.eta)}` : "no pending raise within 90 s") +
      ` · cut ${cap} to ${cutTo} USDC: page "${done?.[0] ?? `no "${M.cutDone.source}"`}", inbox ` +
      (after ? `${cap} ${after.caps[cap]}, ${lag} s after the page's confirmation (delay ${demo.delaySecs} s)` : "cap not lowered within 90 s"),
    from: "page text + GET /api/level1/inbox (caps, pending)",
  };
}

// ---- run ---------------------------------------------------------------------------------------------------------------
say(`five-minutes ${origin} (${arg.console ? "console" : "home"} map) started ${start.toString()}`);
say(`screenshots ${shots}`);
let k = 0;
let broken = 0;
try {
  demo = await api("/api/level1/demo");
  say(`demo: enabled ${demo.enabled}, vendor ${demo.vendor}, bill ${demo.bill?.usdc} every ${demo.bill?.everySecs} s, delay ${demo.delaySecs} s | from GET /api/level1/demo`);
  const steps = [open, bill, autopilot, watch, raiseCut];
  for (const [i, step] of steps.entries()) {
    const n = i + 1;
    if (broken) {
      say(`STEP ${n} FAIL not reached (step ${broken} failed) | from -`);
      continue;
    }
    say(`  ${hms()} step ${n} ${step.name}`);
    let r;
    try {
      r = await step();
    } catch (e) {
      r = { ok: false, read: `error: ${e.message.split("\n")[0]}`, from: "script" };
    }
    say(`STEP ${n} ${r.ok ? "PASS" : "FAIL"} ${r.read} | from ${r.from}`);
    if (r.ok) k++;
    else {
      broken = n;
      say(`  page text at the failure: ${(await pageText().catch(() => "unreadable")).slice(0, 1200)}`);
    }
    await page.screenshot({ path: `${shots}step-${n}-${r.ok ? "pass" : "fail"}.png`, fullPage: true, mask: inviteField() }).catch(() => undefined);
  }
} catch (e) {
  say(`error before the steps: ${e.message.split("\n")[0]}`);
}
say(`  account ${name} ${policy ?? "-"} on ${chain ?? "-"}; its passkey lived only in this run's virtual authenticator`);
say(`RESULT ${k === 5 ? "PASS" : "FAIL"} ${k}/5 (ended ${new Date().toString()})`);
await browser.close();
process.exitCode = k === 5 ? 0 : 1;
