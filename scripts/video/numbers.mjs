#!/usr/bin/env node
// Reads the proof files the demo video quotes and writes apps/video/src/numbers.json. Every figure, hash and log line
// on screen comes from that JSON; each entry carries the file and line it was read from. Fails loudly when a line is
// not where the pattern says, so a changed proof file cannot slip a stale number into the video.
// Usage: node scripts/video/numbers.mjs
import { readFileSync, writeFileSync, existsSync, readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const notes = [];
const read = (rel) => readFileSync(join(ROOT, rel), "utf8").split("\n");
const find = (rel, lines, re, from = 0) => {
  const i = lines.findIndex((l, k) => k >= from && re.test(l));
  if (i < 0) throw new Error(`${rel}: no line matches ${re}`);
  return { line: i + 1, text: lines[i], m: lines[i].match(re) };
};
// Scene 3 sits outside the agent view and the transaction scene, so its log lines show no hash: every 32-byte hash and
// 20-byte address becomes "0x…". The full line stays in `text`.
const noHash = (s) => s.replace(/0x[0-9a-fA-F]{40,64}/g, "0x…");

// The organiser's sentence and the three tests.
const ORG = "proof/organiser-sentences-2026-10-17.txt";
const org = read(ORG);
const quote = (org[2] + " " + org[3]).match(/"(A ledger checks.*twice\.)"/);
if (!quote) throw new Error(`${ORG}:3-4: the organiser's sentence is not there`);
const cmd = find(ORG, org, /^\$ cargo test --locked -p operator --test organiser/);
const run = find(ORG, org, /^date: /);
const tests = ["wrong_vendor", "phantom_invoice", "double_pay_on_retry"].map((name) => {
  const t = find(ORG, org, new RegExp(`^test ${name} \\.\\.\\. ${name}: `));
  const ok = org[t.line]; // the next line
  if (ok.trim() !== "ok") throw new Error(`${ORG}:${t.line + 1}: expected "ok" after ${name}`);
  const body = t.text.replace(/^test \S+ \.\.\. /, "");
  return { name, line: t.line, text: t.text, shown: noHash(body), ok: "ok" };
});
const result = find(ORG, org, /^test result: ok\. 3 passed; 0 failed/);

// The signature count.
const SYM = "proof/symbolon-count-2026-10-18-4ce997c.txt";
const sym = read(SYM);
const head = find(SYM, sym, /^account\s+records\s+unsigned\s+single\s+double\s+triple\s+chain$/);
const total = find(SYM, sym, /^total\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s*$/, head.line);
const [records, unsigned, single, double, triple] = total.m.slice(1).map(Number);
if (unsigned + single + double + triple !== records) throw new Error(`${SYM}:${total.line}: the columns do not add up`);

// Metrics: which reasoner decided each finished pass.
let MET = "proof/metrics-2026-10-16.txt";
if (!existsSync(join(ROOT, MET))) {
  const near = readdirSync(join(ROOT, "proof")).filter((f) => /^metrics-.*\.txt$/.test(f)).sort().pop();
  notes.push(`${MET} is absent; used proof/${near}`);
  MET = `proof/${near}`;
}
const met = read(MET);
const passes = find(MET, met, /passes: (\d+) \((\d+) done, (\d+) failed/);
const rules = find(MET, met, /^\s+(\d+) "reasoner":"rule-based"$/);
const fallback = find(MET, met, /^\s+(\d+) "reasoner":"rule-based \(fallback from [^)]+\)"$/);
const model = find(MET, met, /a model has decided (\d+) passes; the rule baseline decided all of them/);
const finished = Number(passes.m[2]);
if (Number(rules.m[1]) + Number(fallback.m[1]) !== finished) throw new Error(`${MET}: the reasoner lines do not add up to ${finished}`);

// The four credit transactions.
const CR = "proof/credit-line-2026-10-18.txt";
const cr = read(CR);
const txs = ["lock", "draw", "repay", "slash"].map((step, k) => {
  const t = find(CR, cr, new RegExp(`^\\s+${k + 1}\\. ${step}\\s+([0-9.]+)\\s+(https://explorer\\.testnet\\.arc\\.io/tx/(0x[0-9a-f]{64}))$`));
  return { step, amount: t.m[1], url: t.m[2], hash: t.m[3], line: t.line, text: t.text.trim() };
});
const crHead = find(CR, cr, /^No external lender; testnet; not a lending protocol\.$/);
const crChain = find(CR, cr, /^Chain: Arc testnet, chain id (\d+)/);

// The Gateway line of the audit pack (the second balance the book is checked against).
const AP = "proof/audit-pack-2026-10-18.txt";
const ap = read(AP);
const gw = find(AP, ap, /^\s+gateway\s+[0-9.]+ USDC at Circle for the bill wallet 0x[0-9a-fA-F]{40} \(second balance line\)$/);
const apOk = find(AP, ap, /^audit pack: every check held$/, gw.line);

const out = {
  generated: "node scripts/video/numbers.mjs",
  notes,
  organiser: {
    file: ORG,
    sentence: { lines: "3-4", text: quote[1] },
    run: { line: run.line, text: run.text },
    command: { line: cmd.line, text: cmd.text },
    tests,
    result: { line: result.line, text: result.text },
  },
  symbolon: {
    file: SYM,
    header: { line: head.line, text: head.text },
    total: { line: total.line, text: total.text },
    records, unsigned, single, double, triple,
  },
  metrics: {
    file: MET,
    passes: { line: passes.line, text: passes.text.trim(), all: Number(passes.m[1]), finished },
    rules: { line: rules.line, text: rules.text.trim(), n: Number(rules.m[1]) },
    fallback: { line: fallback.line, text: fallback.text.trim(), n: Number(fallback.m[1]) },
    model: { line: model.line, n: Number(model.m[1]) },
  },
  credit: {
    file: CR,
    headline: { line: crHead.line, text: crHead.text },
    chainId: Number(crChain.m[1]),
    explorer: "https://explorer.testnet.arc.io/tx/",
    txs,
  },
  gateway: { file: AP, line: gw.line, text: gw.text.trim(), verdict: { line: apOk.line, text: apOk.text } },
};
writeFileSync(join(ROOT, "apps", "video", "src", "numbers.json"), JSON.stringify(out, null, 2) + "\n");
console.log(`numbers.json: ${tests.length} tests, ${records} records (${single} single, ${double} double, ${triple} triple), ` +
  `rules ${rules.m[1]} + fallback ${fallback.m[1]} of ${finished}, model ${model.m[1]}, ${txs.length} credit txs, gateway line ${AP}:${gw.line}${notes.length ? `; notes: ${notes.join("; ")}` : ""}`);
