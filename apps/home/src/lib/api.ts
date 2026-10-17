// Typed reads of the service (DESIGN §Data). In the browser they go through this app's own /api proxy
// (src/app/api/[...path]/route.ts), so no CORS and no service URL in the page; on the server they ask the service directly.
import type { Demo, Inbox, Recall, RecordEvent, Records, Refusal, RefusalError, Status, Tenant } from "./types.ts";

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

export const fetchStatus = (policy: string, headers?: HeadersInit) => json<Status>(`/api/level1/status?${q({ policy })}`, headers);
export const fetchInbox = (policy: string, headers?: HeadersInit) => json<Inbox>(`/api/level1/inbox?${q({ policy })}`, headers);
export const fetchRecords = (policy: string, headers?: HeadersInit) => json<Records>(`/api/level1/records?${q({ policy })}`, headers);
export const fetchRecall = (policy: string, ids: string[], root?: string, headers?: HeadersInit) =>
  json<Recall>(`/api/level1/recall?${q({ policy, ids: ids.join(","), root })}`, headers);
export const fetchDemo = () => json<Demo>("/api/level1/demo");
/** The tenant as the service keeps it, without keys (the header reads `name`); the route wraps it as `{ tenant }`. */
export const fetchTenant = async (policy: string) => (await json<{ tenant: Tenant }>(`/api/level1/tenant/${encodeURIComponent(policy)}`)).tenant;

/** The events behind one anchored root, in order (the route answers NDJSON). */
export async function fetchRecord(policy: string, root: string, headers?: HeadersInit): Promise<RecordEvent[]> {
  const text = await (await get(`/api/level1/record?${q({ policy, root })}`, headers)).text();
  return parseNdjson<RecordEvent>(text);
}

export const parseNdjson = <T>(text: string): T[] =>
  text
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l) as T);
