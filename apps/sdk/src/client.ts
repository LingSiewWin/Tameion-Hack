// A client for a Tameion account's agent routes: read the account, ask for a
// check, tell the agent about bills, buy a paid API (x402) inside the mandate
// the owner signed. Zero dependencies: `fetch` only, Node 20+.
//
// The one rule that matters: a spend carries an operationId, sent as
// `Idempotency-Key`. A lost reply is `executed: "unknown"` and is resent once
// with the same id; nothing is ever sent a third time, and never under a new
// id, because the first one may have paid.

/** The reply contract this SDK was written against (docs/115 §2). */
export const EXPECTED_CONTRACT_VERSION = "2026-10-17.1";

export type Executed = "yes" | "no" | "unknown";

/** Why the service (or this client, before sending) did not deliver. */
export interface Refusal {
  /** `error.code` from the service's catalogue; absent on older replies and on this client's own refusals. */
  code?: string;
  by?: string;
  executed: Executed;
  /** `user`: a human must act (approve a payee, raise a limit, switch mode). `agent`: the caller can act. */
  audience?: "user" | "agent";
  /** What happened, in words: `error.meaning`, else the older `error` string, else the reply's text. */
  meaning: string;
  nextStep?: string;
  retryAfterSecs?: number;
  words?: string;
}

interface Base {
  /** HTTP status; 0 when no reply arrived. */
  status: number;
  /** The service answered with the first reply to this operationId (`Idempotent-Replayed: true`). */
  replayed: boolean;
  /** The parsed reply as sent, for `--json` and anything the types below leave out. */
  raw: unknown;
  operationId?: string;
}
export type Reply<T> = (Base & { ok: true; data: T }) | (Base & { ok: false; error: Refusal });

// Reply types, from apps/classic/openapi.json and the replies the service sends today.

export interface Caps {
  perAction: string;
  daily: string;
  floor: string;
  /** Spent in the trailing 24 hours, read from the contract. */
  spent24h?: string;
  /** What the contract still allows right now. */
  leftNow?: string;
}
export interface Payee {
  address: string;
  approvedAt?: number;
  /** Unix seconds when the payee's cooling-off ends. */
  payableFrom?: number;
  payableNow?: boolean;
}
export interface Waiting {
  kind: string;
  what: string;
  amount?: string;
  counterparty?: string;
  dueAt?: number;
  why?: string;
  decidedBy?: string;
}
export interface TypedBill {
  payee: string;
  amount: string;
  everySecs: number;
  nextDue: number;
}
export interface StandingOrder {
  id: number;
  payee: string;
  amount: string;
  everySecs: number;
  nextDue: number;
  active: boolean;
  due: boolean;
}
export interface PaidLine {
  amount: string;
  at: number;
  by: string;
  counterparty: string;
  tx?: string;
  explorer?: string;
  reason?: string;
}
export interface Inbox {
  policy: string;
  mode: string;
  balance: string;
  caps: Caps;
  payees: Payee[];
  /** Everything waiting for the owner: bills, sellers, invoices. Read this, not one list alone. */
  waiting: Waiting[];
  waitingCount: number;
  typedBills?: TypedBill[];
  standingOrders?: StandingOrder[];
  paid?: PaidLine[];
  paidTotal?: string;
  deployed?: boolean;
  keyless?: boolean;
  contractVersion?: string;
  [field: string]: unknown;
}
export interface Status {
  contractVersion?: string;
  recentUpdates?: { date: string; note: string }[];
  paused?: unknown;
  stoppedTokens?: unknown;
  halt?: unknown;
  [field: string]: unknown;
}
export interface RunStarted {
  id: string;
  status: string;
  /** A recent pass answered instead of a new one (one pass per account per ten minutes). */
  cached?: boolean;
  [field: string]: unknown;
}
export interface BillInput {
  payee: string;
  /** USDC, e.g. "0.05". */
  amount: string;
  everyDays?: number;
  nextDue?: number;
  discountPercent?: number;
  earlyDays?: number;
}
export interface BillsSaved {
  policy: string;
  bills: unknown[];
  refused: unknown[];
  [field: string]: unknown;
}
export interface Purchase {
  amount: string;
  seller: string;
  status: string;
  tx?: string;
  explorer?: string;
  actionHash?: string;
  reason?: string;
  [field: string]: unknown;
}
export interface Bought {
  /** false when the API answered without asking for payment. */
  paid: boolean;
  purchase?: Purchase;
  status?: number;
  contentType?: string;
  body?: unknown;
  [field: string]: unknown;
}
export interface BuyInput {
  policy: string;
  url: string;
  /** The most this one call may cost, in USDC, e.g. "0.01". */
  maxUsdc: string;
  /** Why the agent needs it; kept with the purchase and shown to the owner. */
  reason: string;
  method?: string;
  body?: unknown;
  /** `op_` + a UUID (newOperationId()). Keep it until the outcome is known; resend only with the same one. */
  operationId: string;
}

/** A spend as it was saved before it was sent. */
export interface SavedOperation {
  operationId: string;
  method: "POST";
  path: string;
  body: string;
  savedAt: number;
  /** How many times it has been sent by this client. */
  sends: number;
  executed?: Executed;
}

export interface ClientOptions {
  /** `https:` unless the host is localhost, 127.0.0.1 or [::1]. */
  baseUrl: string;
  /** The agent key the owner minted; sent as `Authorization: Bearer`. */
  token?: string;
  fetch?: typeof fetch;
  now?: () => number;
  /** Called once per client when a reply's contractVersion differs from EXPECTED_CONTRACT_VERSION. */
  onContractMismatch?: (got: string, expected: string) => void;
  /** How long one request waits before its reply counts as lost. Default 60 000. */
  timeoutMs?: number;
  /** Waits before a rate-limit resend; injectable for tests. */
  sleep?: (ms: number) => Promise<void>;
  /** Persist a spend before it is sent (awaited; if it throws, nothing is sent). */
  save?: (op: SavedOperation) => void | Promise<void>;
}

const OPERATION_ID = /^op_[A-Za-z0-9_.:-]{5,125}$/;
const LOCAL_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);
const RESEND_ON = new Set(["rate_limited", "idempotency_pending"]);

/** A fresh operationId: `op_` + a random UUID. */
export function newOperationId(): string {
  return `op_${globalThis.crypto.randomUUID()}`;
}

export function isOperationId(id: unknown): id is string {
  return typeof id === "string" && OPERATION_ID.test(id);
}

/** Throws unless the URL is https, or http on this machine. */
export function checkBaseUrl(baseUrl: string): URL {
  let u: URL;
  try {
    u = new URL(baseUrl);
  } catch {
    throw new Error("baseUrl is not a URL");
  }
  if (u.protocol === "https:" || (u.protocol === "http:" && LOCAL_HOSTS.has(u.hostname))) return u;
  throw new Error("baseUrl must be https: (plain http only for localhost, 127.0.0.1 or [::1]): the agent key travels in every request");
}

type Sent = { lost: false; status: number; replayed: boolean; json: unknown } | { lost: true; why: string };

export class TameionClient {
  readonly baseUrl: string;
  private readonly token: string | undefined;
  private readonly doFetch: typeof fetch;
  private readonly now: () => number;
  private readonly onMismatch: (got: string, expected: string) => void;
  private readonly timeoutMs: number;
  private readonly sleep: (ms: number) => Promise<void>;
  private readonly save: ((op: SavedOperation) => void | Promise<void>) | undefined;
  private warned = false;
  private readonly ops = new Map<string, { op: SavedOperation; reply?: Reply<unknown> }>();

  constructor(o: ClientOptions) {
    this.baseUrl = checkBaseUrl(o.baseUrl).href.replace(/\/+$/, "");
    this.token = o.token?.trim() || undefined;
    this.doFetch = o.fetch ?? globalThis.fetch;
    this.now = o.now ?? Date.now;
    this.onMismatch =
      o.onContractMismatch ??
      ((got, want) => console.warn(`tameion: the service speaks contract ${got}; this SDK expects ${want}`));
    this.timeoutMs = o.timeoutMs ?? 60_000;
    this.sleep = o.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
    this.save = o.save;
  }

  /** The service: contract version, recent updates, kill switches. */
  status(policy?: string): Promise<Reply<Status>> {
    const q = policy ? `?policy=${encodeURIComponent(policy)}` : "";
    return this.read<Status>(`/api/level1/status${q}`);
  }

  /** The account in one read (token scope `read`). */
  inbox(policy: string): Promise<Reply<Inbox>> {
    return this.read<Inbox>(`/api/level1/inbox?policy=${encodeURIComponent(policy)}`);
  }

  /** Ask the agent to check the account now (scope `run`); poll `/api/watch/<id>`. Moves no money in Propose mode. */
  run(policy: string): Promise<Reply<RunStarted>> {
    return this.post<RunStarted>("/api/level1/run", { policy });
  }

  /** Tell the agent about bills (scope `import`); only payees the owner approved are kept. Pays nothing by itself. */
  bills(policy: string, bills: BillInput[]): Promise<Reply<BillsSaved>> {
    return this.post<BillsSaved>("/api/level1/bills", { policy, bills });
  }

  /** Buy one call of a paid API over x402, inside the mandate (scope `buy`). Spends. */
  buy(input: BuyInput): Promise<Reply<Bought>> {
    const { operationId, ...rest } = input;
    const body = { method: "GET", ...rest };
    return this.spend<Bought>("/api/level1/buy", body, operationId);
  }

  /** The spend saved under this id, if this client sent it. */
  operation(operationId: string): SavedOperation | undefined {
    const e = this.ops.get(operationId);
    return e && { ...e.op };
  }

  private async read<T>(path: string): Promise<Reply<T>> {
    const s = await this.send("GET", path);
    return this.reply<T>(s, false, "no");
  }

  private async post<T>(path: string, body: unknown): Promise<Reply<T>> {
    const s = await this.send("POST", path, JSON.stringify(body));
    return this.reply<T>(s, false, "unknown");
  }

  private async spend<T>(path: string, body: unknown, operationId: string): Promise<Reply<T>> {
    if (!isOperationId(operationId)) {
      throw new Error("operationId must be op_ followed by 5 to 125 of [A-Za-z0-9_.:-] (use newOperationId())");
    }
    const payload = JSON.stringify(body);
    const prior = this.ops.get(operationId);
    if (prior) {
      if (prior.op.path !== path || prior.op.body !== payload) {
        return local(operationId, "no", `${operationId} was already used for a different request; this one was not sent. Use a new operationId for a new spend.`);
      }
      if (prior.op.executed === "yes" && prior.reply) return prior.reply as Reply<T>;
      if (prior.op.executed === "unknown") {
        return local(operationId, "unknown", `no answer said whether ${operationId} paid, so this client does not send it again. Read the account (inbox: paid, purchases) first; never resend it under a new operationId.`);
      }
    }
    const op: SavedOperation = { operationId, method: "POST", path, body: payload, savedAt: this.now(), sends: prior?.op.sends ?? 0 };
    const entry: { op: SavedOperation; reply?: Reply<unknown> } = { op };
    this.ops.set(operationId, entry);
    await this.save?.({ ...op });

    let lostBefore = false;
    let r: Reply<T> | undefined;
    for (let attempt = 0; attempt < 2; attempt++) {
      const s = await this.send("POST", path, payload, operationId);
      op.sends++;
      if (s.lost) {
        lostBefore = true;
        r = this.reply<T>(s, true, "unknown");
        continue; // one resend with the same id
      }
      r = this.reply<T>(s, true, "unknown");
      if (r.ok) break;
      // After a lost reply, a refusal cannot say the first send did not pay.
      if (lostBefore && r.error.executed !== "yes") r.error.executed = "unknown";
      const e = r.error;
      if (attempt === 0 && e.executed === "no" && e.code && RESEND_ON.has(e.code)) {
        await this.sleep(clampSecs(e.retryAfterSecs) * 1000);
        continue;
      }
      break;
    }
    const final = r as Reply<T>;
    final.operationId = operationId;
    op.executed = final.ok ? "yes" : final.error.executed;
    entry.reply = final;
    return final;
  }

  private async send(method: "GET" | "POST", path: string, body?: string, idempotencyKey?: string): Promise<Sent> {
    const headers: Record<string, string> = { accept: "application/json" };
    if (body !== undefined) headers["content-type"] = "application/json";
    if (this.token) headers.authorization = `Bearer ${this.token}`;
    if (idempotencyKey) headers["idempotency-key"] = idempotencyKey;
    let res: Response;
    let text: string;
    try {
      res = await this.doFetch(this.baseUrl + path, {
        method,
        headers,
        ...(body !== undefined ? { body } : {}),
        signal: AbortSignal.timeout(this.timeoutMs),
      });
      text = await res.text();
    } catch (e) {
      const name = e instanceof Error ? e.name : "";
      return { lost: true, why: name === "TimeoutError" || name === "AbortError" ? `no answer in ${this.timeoutMs} ms` : "the service could not be reached" };
    }
    let json: unknown;
    try {
      json = JSON.parse(text);
    } catch {
      // A reply that is not JSON is lost for a spend (a proxy page may follow a payment);
      // for anything else its words are kept.
      if (idempotencyKey) return { lost: true, why: `HTTP ${res.status} with a body that is not JSON` };
      json = { error: text.trim().slice(0, 300) || `HTTP ${res.status}` };
    }
    this.checkContract(json);
    return { lost: false, status: res.status, replayed: res.headers.get("idempotent-replayed") === "true", json };
  }

  private reply<T>(s: Sent, spend: boolean, lostExecuted: Executed): Reply<T> {
    if (s.lost) {
      const error: Refusal = { executed: lostExecuted, meaning: `${s.why}; the request may still complete` };
      return { ok: false, status: 0, replayed: false, raw: null, error };
    }
    const j = s.json as Record<string, unknown> | null;
    const httpOk = s.status >= 200 && s.status < 300;
    if (httpOk && !(j && j.ok === false)) return { ok: true, status: s.status, replayed: s.replayed, raw: j, data: j as T };
    return { ok: false, status: s.status, replayed: s.replayed, raw: j, error: refusalOf(s.status, j, spend) };
  }

  private checkContract(json: unknown): void {
    const v = (json as { contractVersion?: unknown } | null)?.contractVersion;
    if (typeof v === "string" && v !== EXPECTED_CONTRACT_VERSION && !this.warned) {
      this.warned = true;
      this.onMismatch(v, EXPECTED_CONTRACT_VERSION);
    }
  }
}

/** `error.code` and friends when the service sends the envelope; the older `error` string otherwise. */
export function refusalOf(status: number, j: Record<string, unknown> | null, spend: boolean): Refusal {
  // Older replies carry no `executed`: refused before sending is "no"; a 5xx on a spend may follow a payment.
  const fallback: Executed = spend && status >= 500 ? "unknown" : "no";
  const e = j?.error;
  if (e && typeof e === "object") {
    const o = e as Record<string, unknown>;
    const str = (k: string) => (typeof o[k] === "string" ? (o[k] as string) : undefined);
    const executed = str("executed");
    const audience = str("audience");
    const r: Refusal = {
      executed: executed === "yes" || executed === "no" || executed === "unknown" ? executed : fallback,
      meaning: str("meaning") ?? str("words") ?? `HTTP ${status}`,
    };
    const code = str("code");
    if (code) r.code = code;
    const by = str("by");
    if (by) r.by = by;
    if (audience === "user" || audience === "agent") r.audience = audience;
    const next = str("nextStep");
    if (next) r.nextStep = next;
    const words = str("words");
    if (words) r.words = words;
    if (typeof o.retryAfterSecs === "number") r.retryAfterSecs = o.retryAfterSecs;
    return r;
  }
  return { executed: fallback, meaning: typeof e === "string" ? e : `HTTP ${status}` };
}

function clampSecs(s: number | undefined): number {
  return Math.min(60, Math.max(1, Math.round(s ?? 1)));
}

function local<T>(operationId: string, executed: Executed, meaning: string): Reply<T> {
  return { ok: false, status: 0, replayed: false, raw: null, operationId, error: { executed, meaning } };
}
