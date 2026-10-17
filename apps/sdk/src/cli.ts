#!/usr/bin/env node
// `tameion`: the account from a terminal or an agent's shell.
// Exit 0 delivered, 1 the service refused (or the outcome is unknown), 2 usage.
// The token comes only from TAMEION_TOKEN or --token-file, never argv, and
// every printed string has it replaced by [token].

import { readFile, stat } from "node:fs/promises";
import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";
import {
  TameionClient,
  isOperationId,
  newOperationId,
  type Inbox,
  type Refusal,
  type Reply,
  type Status,
  type Bought,
  type PaymentMade,
  type Recall,
} from "./client.js";

export interface Io {
  env: Record<string, string | undefined>;
  out: (line: string) => void;
  err: (line: string) => void;
  readFile: (path: string) => Promise<string>;
  /** Permission bits of a file (st_mode & 0o777). */
  fileMode: (path: string) => Promise<number>;
  fetch: typeof fetch;
  now: () => number;
}

const COMMANDS = ["status", "top", "inbox", "bills", "pay", "buy", "recall", "help"] as const;
const VALUE_FLAGS = new Set(["--policy", "--url", "--token-file", "--why", "--max", "--op", "--method", "--root"]);
const EVENT_ID = /^evt:(0|[1-9]\d*)$/;
const BOOL_FLAGS = new Set(["--json", "--help"]);
const USDC = /^\d+(\.\d{1,6})?$/;

const USAGE = `tameion — a Tameion account from the terminal

  tameion status                         the service: contract version, recent updates, kill switches
  tameion top                            the burn rate, how far the floor is at that rate, the first bill
                                         the money cannot cover, and what moved the rate
  tameion inbox                          mode, balance, limits, payees, what waits for the owner
  tameion bills                          the bills the agent knows and the ones waiting for the owner
  tameion pay <payee> <usdc> --why <text> [--op <operationId>]
                                         pay an approved payee now, inside the mandate (Autopilot only)
  tameion buy <url> --max <usdc> --why <text> [--op <operationId>] [--method GET]
                                         buy one call of a paid API (x402) inside the mandate
  tameion recall <evt:N> [<evt:N>…] [--root <0x…>]
                                         check remembered events against the anchored record and the
                                         chain (latest record unless --root); exit 0 only if all verified

  --policy <0x…>     the account (or TAMEION_POLICY)
  --url <base>       the service (or TAMEION_URL; default http://127.0.0.1:7880; https unless localhost)
  --token-file <p>   the agent key, from a file (or TAMEION_TOKEN); never on the command line
  --json             print the service's reply as JSON
  --op <id>          resend a pay or buy with the operationId it printed; never invent a new one for the same payment

exit 0 delivered · 1 refused or outcome unknown · 2 usage`;

class Usage extends Error {}

function parse(argv: string[]): { pos: string[]; flags: Map<string, string | true> } {
  const pos: string[] = [];
  const flags = new Map<string, string | true>();
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i] as string;
    if (!a.startsWith("--")) {
      pos.push(a);
      continue;
    }
    const eq = a.indexOf("=");
    const name = eq > 0 ? a.slice(0, eq) : a;
    if (name === "--token") throw new Usage("the token is never taken from the command line (it would stay in shell history); set TAMEION_TOKEN or use --token-file");
    if (BOOL_FLAGS.has(name)) {
      flags.set(name, true);
    } else if (VALUE_FLAGS.has(name)) {
      const v = eq > 0 ? a.slice(eq + 1) : argv[++i];
      if (v === undefined) throw new Usage(`${name} needs a value`);
      flags.set(name, v);
    } else {
      throw new Usage("unknown option; run `tameion help`");
    }
  }
  return { pos, flags };
}

/** Levenshtein distance, for did-you-mean. */
function distance(a: string, b: string): number {
  let prev = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i++) {
    const cur = [i];
    for (let j = 1; j <= b.length; j++) {
      cur[j] = Math.min((prev[j] as number) + 1, (cur[j - 1] as number) + 1, (prev[j - 1] as number) + (a[i - 1] === b[j - 1] ? 0 : 1));
    }
    prev = cur;
  }
  return prev[b.length] as number;
}

function didYouMean(input: string): string {
  const best = COMMANDS.map((c) => [c, distance(input.toLowerCase(), c)] as const).sort((x, y) => x[1] - y[1])[0];
  return best && best[1] <= 2 ? `unknown command; did you mean \`tameion ${best[0]}\`?` : "unknown command; run `tameion help`";
}

const short = (a: unknown) => (typeof a === "string" && a.length > 14 ? `${a.slice(0, 6)}…${a.slice(-4)}` : String(a ?? ""));
const when = (unix: unknown) => (typeof unix === "number" ? new Date(unix * 1000).toISOString().replace(/:\d\d\.\d+Z$/, " UTC").replace("T", " ") : "");

function refusalLines(e: Refusal, status: number, operationId?: string): string[] {
  const tag = [e.code ?? (status ? `HTTP ${status}` : "no answer"), e.by && `by ${e.by}`, `executed: ${e.executed}`].filter(Boolean).join(", ");
  const lines = [`refused (${tag}): ${e.meaning}`];
  if (e.nextStep) lines.push(`next: ${e.nextStep}`);
  if (e.audience === "user") lines.push("a person must act on the account page; do not retry");
  if (e.executed === "unknown" && operationId) {
    lines.push(
      `it may have paid. Do not pay again under a new id. Read \`tameion inbox\` (paid) first; if nothing went out, resend with --op ${operationId}`,
    );
  }
  return lines;
}

function inboxLines(d: Inbox): string[] {
  const c = d.caps ?? ({} as Inbox["caps"]);
  const payees = d.payees ?? [];
  const lines = [
    `${d.mode} · balance ${d.balance}`,
    `per payment ${c.perAction} · per 24 h ${c.daily}${c.spent24h ? ` (spent ${c.spent24h}, left now ${c.leftNow})` : ""} · floor ${c.floor}`,
    `payees: ${payees.length} approved, ${payees.filter((p) => p.payableNow).length} payable now`,
    `waiting for the owner: ${d.waitingCount ?? (d.waiting ?? []).length}`,
  ];
  for (const w of d.waiting ?? []) lines.push(`  - ${w.kind} ${w.amount ?? ""} ${short(w.counterparty)}: ${w.what}${w.why ? ` — ${w.why}` : ""}`);
  if (d.paidTotal) lines.push(`paid: ${d.paidTotal} in ${(d.paid ?? []).length} payments`);
  return lines;
}

function billLines(d: Inbox): string[] {
  const typed = d.typedBills ?? [];
  const orders = d.standingOrders ?? [];
  const asks = (d.waiting ?? []).filter((w) => w.kind === "bill");
  const lines = [`bills the agent knows: ${typed.length}`];
  for (const b of typed) lines.push(`  - ${b.amount} every ${Math.round(b.everySecs / 86400)} days to ${b.payee}, next due ${when(b.nextDue)}`);
  lines.push(`standing orders: ${orders.length}`);
  for (const o of orders) lines.push(`  - #${o.id} ${o.amount} every ${Math.round(o.everySecs / 86400)} days to ${o.payee}, next due ${when(o.nextDue)}${o.due ? " (due now)" : ""}`);
  lines.push(`bill payments waiting for the owner: ${asks.length}`);
  for (const w of asks) lines.push(`  - ${w.amount} to ${w.counterparty}${w.why ? ` — ${w.why}` : ""}`);
  return lines;
}

function statusLines(d: Status): string[] {
  const lines = [`contract ${d.contractVersion ?? "(not stated)"}`];
  for (const k of ["paused", "halt", "stoppedTokens"] as const) if (k in d) lines.push(`${k}: ${JSON.stringify(d[k])}`);
  for (const u of d.recentUpdates ?? []) lines.push(`  ${u.date} ${u.note}`);
  return lines;
}

function recallLines(d: Recall): string[] {
  const how = (d.chainRead as { how?: string } | null)?.how;
  const lines = [
    `record ${short(d.record.root)} (${d.record.latest ? "latest" : "earlier"}, ${d.record.events} events) · chain read: ${how ?? "none"} on ${d.chain}`,
  ];
  for (const r of d.results) {
    const e = r.event;
    const what = e ? ` ${e.kind}${e.amount ? ` ${e.amount}` : ""}${e.payee ? ` to ${short(e.payee)}` : ""}${e.note ? ` — ${e.note}` : ""}` : "";
    const tag = r.status === "verified" ? "verified" : `${r.status} (${[r.code, r.by && `by ${r.by}`].filter(Boolean).join(", ")})`;
    lines.push(`  ${r.id} ${tag}${what}${r.status === "verified" ? "" : `: ${r.words}`}`);
  }
  lines.push(`verified ${d.verified} · unverified ${d.unverified} · unknown ${d.unknown}`);
  return lines;
}

/** The account's momentum as the service folds it (never computed here): quote, do not recompute. */
function topLines(d: Status): string[] {
  const r = d.runway;
  if (d.rate == null || !r) return ["the service could not read the account's balance just now; try again"];
  const days = (n: number) => `${n} day${n === 1 ? "" : "s"}`;
  const lines = [
    r.byRateDays == null
      ? `no burn yet: nothing was paid in the last ${d.windowDays} days`
      : `at this burn (${d.rate} a day over the last ${d.windowDays} days), the floor is ${days(r.byRateDays)} away`,
    r.firstShortfallDays == null
      ? "every bill in the next 90 days is covered"
      : `the first bill the money above the floor cannot cover is ${r.firstShortfallDays === 0 ? "due today" : `in ${days(r.firstShortfallDays)}`}`,
  ];
  for (const f of d.forces ?? []) lines.push(`  ${new Date(f.at * 1000).toISOString().slice(0, 16)}Z ${f.kind} ${f.counterparty} ${f.deltaPerDay} a day`);
  return lines;
}

function boughtLines(d: Bought, replayed: boolean, operationId?: string): string[] {
  const p = d.purchase;
  const lines = d.paid && p
    ? [`paid ${p.amount} to ${p.seller}${p.explorer ? ` · ${p.explorer}` : p.tx ? ` · tx ${p.tx}` : ""}`]
    : [`delivered without a payment (the API answered ${d.status ?? "without asking to be paid"})`];
  if (replayed) lines.push("replayed: this is the first answer to this operationId; nothing was paid again");
  if (operationId) lines.push(`operation ${operationId}`);
  return lines;
}

function paymentLines(d: PaymentMade, replayed: boolean, operationId: string): string[] {
  const p = d.payment;
  const link = p.explorer ? ` · ${p.explorer}` : p.tx ? ` · tx ${p.tx}` : "";
  const lines = d.paid
    ? [`paid ${p.amount} to ${p.payee}${link}`]
    : [`sent ${p.amount} to ${p.payee}${link}, but the receipt did not match (${p.note ?? p.status}); do not pay again, read \`tameion inbox\``];
  if (replayed) lines.push("replayed: this operationId was already paid; nothing was paid again");
  lines.push(`operation ${operationId}`);
  return lines;
}

/** The operationId for a spend: --op when resending, else a new one, printed before anything is sent. */
function operationFor(flags: Map<string, string | true>, err: (s: string) => void): string {
  const given = flags.get("--op");
  if (given !== undefined && !isOperationId(given)) throw new Usage("--op must be an operationId this CLI printed (op_ + UUID)");
  const operationId = typeof given === "string" ? given : newOperationId();
  // Saved before sending: the terminal holds the id the resend needs.
  err(`operation ${operationId} — if no answer comes back, resend with --op ${operationId}, never a new one`);
  return operationId;
}

export async function run(argv: string[], io: Io): Promise<number> {
  let token = "";
  const scrub = (s: string) => (token ? s.split(token).join("[token]") : s);
  const out = (s: string) => io.out(scrub(s));
  const err = (s: string) => io.err(scrub(s));
  try {
    const { pos, flags } = parse(argv);
    const cmd = pos[0];
    if (!cmd || cmd === "help" || flags.has("--help")) {
      out(USAGE);
      return cmd || flags.has("--help") ? 0 : 2;
    }
    if (!(COMMANDS as readonly string[]).includes(cmd)) throw new Usage(didYouMean(cmd));

    const file = flags.get("--token-file");
    if (typeof file === "string") {
      try {
        token = (await io.readFile(file)).trim();
        const mode = await io.fileMode(file);
        if (mode & 0o077) err(`warning: the token file is readable by group or others (mode ${mode.toString(8)}); run chmod 600 on it`);
      } catch {
        throw new Usage("the token file could not be read");
      }
    } else {
      token = (io.env.TAMEION_TOKEN ?? "").trim();
    }

    const baseUrl = String(flags.get("--url") ?? io.env.TAMEION_URL ?? "http://127.0.0.1:7880");
    const policy = String(flags.get("--policy") ?? io.env.TAMEION_POLICY ?? "");
    if (cmd !== "status" && !/^0x[0-9a-fA-F]{40}$/.test(policy)) throw new Usage("--policy <0x… account address> is required (or TAMEION_POLICY)");
    const json = flags.has("--json");

    let client: TameionClient;
    try {
      client = new TameionClient({
        baseUrl,
        token,
        fetch: io.fetch,
        now: io.now,
        onContractMismatch: (got, want) => err(`warning: the service speaks contract ${got}; this CLI expects ${want}`),
      });
    } catch (e) {
      throw new Usage((e as Error).message);
    }

    let reply: Reply<unknown>;
    let render: () => string[] = () => [];
    let allVerified = true;
    if (cmd === "status" || cmd === "top") {
      const r = await client.status(policy || undefined);
      reply = r;
      if (r.ok) render = () => (cmd === "top" ? topLines(r.data) : statusLines(r.data));
    } else if (cmd === "inbox" || cmd === "bills") {
      const r = await client.inbox(policy);
      reply = r;
      if (r.ok) render = () => (cmd === "inbox" ? inboxLines(r.data) : billLines(r.data));
    } else if (cmd === "recall") {
      const ids = pos.slice(1).flatMap((p) => p.split(",")).map((p) => p.trim()).filter(Boolean);
      if (!ids.length || ids.length > 32) throw new Usage("usage: tameion recall <evt:N> [<evt:N>…] [--root <0x…>] (1 to 32 ids)");
      if (!ids.every((i) => EVENT_ID.test(i))) throw new Usage("event ids are evt:<digits>, as record_events numbers them (seq)");
      const root = flags.get("--root");
      if (root !== undefined && (typeof root !== "string" || !/^0x[0-9a-fA-F]{64}$/.test(root))) throw new Usage("--root is a 0x… 32-byte root from records");
      const r = await client.recall(policy, ids, typeof root === "string" ? root : undefined);
      reply = r;
      // Delivered is not verified: the exit says whether every id verified.
      if (r.ok) {
        render = () => recallLines(r.data);
        allVerified = r.data.verified === ids.length;
      }
    } else if (cmd === "pay") {
      const [, payee, amount] = pos;
      const why = flags.get("--why");
      if (!payee || !amount || pos.length > 3) throw new Usage("usage: tameion pay <payee> <usdc> --why <text>");
      if (!/^0x[0-9a-fA-F]{40}$/.test(payee)) throw new Usage("<payee> must be the 0x… address of a payee the owner approved");
      if (!USDC.test(amount) || Number(amount) <= 0) throw new Usage("<usdc> must be an amount above zero, e.g. 0.25");
      if (typeof why !== "string" || !why.trim()) throw new Usage("--why <text> is required: every payment is written to the book with what it was for");
      const operationId = operationFor(flags, err);
      const r = await client.pay({ policy, payee, amount, why: why.trim(), operationId });
      reply = r;
      if (r.ok) render = () => paymentLines(r.data, r.replayed, operationId);
    } else {
      const url = pos[1];
      const max = flags.get("--max");
      const why = flags.get("--why");
      if (!url || pos.length > 2) throw new Usage("usage: tameion buy <url> --max <usdc> --why <text>");
      if (typeof max !== "string" || !USDC.test(max)) throw new Usage("--max <usdc> is required, e.g. --max 0.01");
      if (typeof why !== "string" || !why.trim()) throw new Usage("--why <text> is required: every purchase is written down with its reason");
      const operationId = operationFor(flags, err);
      const method = flags.get("--method");
      const r = await client.buy({ policy, url, maxUsdc: max, reason: why.trim(), operationId, ...(typeof method === "string" ? { method } : {}) });
      reply = r;
      if (r.ok) render = () => boughtLines(r.data, r.replayed, operationId);
    }

    if (json) {
      out(JSON.stringify(reply.ok || reply.raw ? reply.raw : { ok: false, error: reply.error, operationId: reply.operationId }, null, 2));
    } else if (reply.ok) {
      for (const l of render()) out(l);
    } else {
      for (const l of refusalLines(reply.error, reply.status, reply.operationId)) err(l);
    }
    return reply.ok && allVerified ? 0 : 1;
  } catch (e) {
    if (e instanceof Usage) {
      err(e.message);
      return 2;
    }
    err(`tameion: ${(e as Error).message}`);
    return 1;
  }
}

/** The real process's io. */
export const nodeIo: Io = {
  env: process.env,
  out: (s) => void process.stdout.write(s + "\n"),
  err: (s) => void process.stderr.write(s + "\n"),
  readFile: (p) => readFile(p, "utf8"),
  fileMode: async (p) => (await stat(p)).mode & 0o777,
  fetch: (...a) => globalThis.fetch(...a),
  now: () => Date.now(),
};

function invokedDirectly(): boolean {
  try {
    const entry = process.argv[1];
    return !!entry && import.meta.url === pathToFileURL(realpathSync(entry)).href;
  } catch {
    return false;
  }
}

if (invokedDirectly()) {
  run(process.argv.slice(2), nodeIo).then((code) => {
    process.exitCode = code;
  });
}
