// Typed reads of the service (DESIGN §Data). In the browser they go through this app's own /api proxy
// (src/app/api/[...path]/route.ts), so no CORS and no service URL in the page; on the server they ask the service directly.
//
// Accounts are addressed by NAME (`/a/<name>`, DESIGN §Page grammar): every fetcher takes the account's name (an address
// works too). The name becomes the address only on the server — here when running there, in the proxy for the browser —
// so the page never needs the address and it never reaches an href or the Owner view's DOM.
import type { Demo, Inbox, NameRecord, Recall, RecordEvent, Records, Refusal, RefusalError, Status, Symbolon, Tenant } from "./types.ts";

const base = () => (typeof window === "undefined" ? (process.env.TAMEION_URL ?? "http://127.0.0.1:7890").replace(/\/+$/, "") : "");

/** A refusal from the service (or from the proxy when the service did not answer), with whether money may have moved. */
export class ApiError extends Error {
  readonly status: number;
  readonly refusal: RefusalError | null;
  constructor(status: number, refusal: RefusalError | null, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.refusal = refusal;
  }
}

const refusalOf = (body: unknown): RefusalError | null => {
  const e = (body as Partial<Refusal> | null)?.error;
  return e && typeof e === "object" && typeof e.code === "string" ? e : null;
};

async function get(path: string, headers?: HeadersInit): Promise<Response> {
  const r = await fetch(`${base()}${path}`, { headers, cache: "no-store" });
  if (r.ok) return r;
  const body: unknown = await r.json().catch(() => null);
  const refusal = refusalOf(body);
  const words = refusal?.words ?? (typeof (body as { error?: unknown } | null)?.error === "string" ? (body as { error: string }).error : r.statusText);
  throw new ApiError(r.status, refusal, words || `HTTP ${r.status}`);
}
const json = async <T>(path: string, headers?: HeadersInit) => (await (await get(path, headers)).json()) as T;
const q = (params: Record<string, string | undefined>) =>
  new URLSearchParams(Object.entries(params).filter((e): e is [string, string] => e[1] !== undefined)).toString();

// ---- names --------------------------------------------------------------------------------------------------------

export const isAddress = (s: string) => /^0x[0-9a-fA-F]{40}$/.test(s);

/** An account found by its name: `policy` is its address, for server-side API calls only. */
export type Account = { name: string; policy: string; chains: NameRecord["chains"] };

// ponytail: an unbounded map of names found; a name never moves to another account, and only found names are kept
// (a name not taken yet may be taken by the stepper a minute later)
const found = new Map<string, Account>();

/** The account a name points at (`GET /api/level1/name/<name>`), or null when no account has that name. */
export async function resolveAccount(name: string): Promise<Account | null> {
  const hit = found.get(name);
  if (hit) return hit;
  try {
    const r = await json<NameRecord>(`/api/level1/name/${encodeURIComponent(name)}`);
    const account = { name: r.name, policy: r.account, chains: r.chains };
    found.set(name, account);
    return account;
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return null;
    throw e;
  }
}

/** The `policy` a call carries: in the browser the name itself (the proxy resolves it), on the server the address. */
async function policyOf(account: string): Promise<string> {
  if (typeof window !== "undefined" || isAddress(account)) return account;
  return (await resolveAccount(account))?.policy ?? account;
}

// ---- reads --------------------------------------------------------------------------------------------------------

export const fetchStatus = async (account: string, headers?: HeadersInit) =>
  json<Status>(`/api/level1/status?${q({ policy: await policyOf(account) })}`, headers);
export const fetchInbox = async (account: string, headers?: HeadersInit) =>
  json<Inbox>(`/api/level1/inbox?${q({ policy: await policyOf(account) })}`, headers);
export const fetchRecords = async (account: string, headers?: HeadersInit) =>
  json<Records>(`/api/level1/records?${q({ policy: await policyOf(account) })}`, headers);
export const fetchRecall = async (account: string, ids: string[], root?: string, headers?: HeadersInit) =>
  json<Recall>(`/api/level1/recall?${q({ policy: await policyOf(account), ids: ids.join(","), root })}`, headers);
/** Each record with its three slots (agent, counterparty, witness), every root or one: this lane's read-only route. */
export const fetchSymbolon = async (account: string, root?: string, headers?: HeadersInit) =>
  json<Symbolon>(`/api/level1/symbolon?${q({ policy: await policyOf(account), root })}`, headers);
export const fetchDemo = () => json<Demo>("/api/level1/demo");
/** The tenant as the service keeps it, without keys; the route wraps it as `{ tenant }`. */
export const fetchTenant = async (account: string) =>
  (await json<{ tenant: Tenant }>(`/api/level1/tenant/${encodeURIComponent(await policyOf(account))}`)).tenant;

/** The events behind one anchored root, in order (the route answers NDJSON). */
export async function fetchRecord(account: string, root: string, headers?: HeadersInit): Promise<RecordEvent[]> {
  const text = await (await get(`/api/level1/record?${q({ policy: await policyOf(account), root })}`, headers)).text();
  return parseNdjson<RecordEvent>(text);
}

export const parseNdjson = <T>(text: string): T[] =>
  text
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l) as T);
