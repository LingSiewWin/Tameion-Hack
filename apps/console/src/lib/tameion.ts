// The console's whole contract with the service, ported from the classic page
// (apps/classic/home.html, served at /classic): same routes, same localStorage keys, same
// passkey signing. A change here is a change to what the owner signs.

import { encodeFunctionData, erc20Abi, getAddress } from "viem";
import { checkCall, type Expect } from "@/lib/guard";
import type { Cfg, DeskBook, DeskStatementRow, Demo, Inbox, NameRecord, Pub, Tenant } from "@/lib/types";

// ---- bytes ------------------------------------------------------------------
export const hex = (buf: ArrayBuffer | Uint8Array) =>
  "0x" + [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
export const fromHex = (h: string) =>
  new Uint8Array((h.replace(/^0x/, "").match(/../g) ?? []).map((x) => parseInt(x, 16)));
export const b64u = (buf: ArrayBuffer) =>
  btoa(String.fromCharCode(...new Uint8Array(buf))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
export const unb64u = (s: string) =>
  Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));

const N = 0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551n;
/** A WebAuthn DER signature as the low-s (r, s) the contract checks. */
export function derToRS(der: ArrayBuffer) {
  const b = new Uint8Array(der);
  let i = 2;
  const read = () => {
    if (b[i++] !== 0x02) throw new Error("bad DER");
    const len = b[i++];
    let v = 0n;
    for (let k = 0; k < len; k++) v = (v << 8n) | BigInt(b[i++]);
    return v;
  };
  const r = read();
  let s = read();
  if (s > N / 2n) s = N - s;
  const to32 = (n: bigint) => "0x" + n.toString(16).padStart(64, "0");
  return { r: to32(r), s: to32(s) };
}

// ---- storage (same keys as the classic page: accounts open in both) ---------
export const store = {
  get: (k: string): string | null => {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  },
  set: (k: string, v: string) => {
    try {
      localStorage.setItem(k, v);
    } catch {}
  },
  del: (k: string) => {
    try {
      localStorage.removeItem(k);
    } catch {}
  },
};

export function tenants(): Tenant[] {
  try {
    return JSON.parse(store.get("tameion.tenants") || "[]");
  } catch {
    return [];
  }
}
export function remember(t: Tenant) {
  const all = tenants().filter((x) => x.policy !== t.policy);
  all.unshift(t);
  store.set("tameion.tenants", JSON.stringify(all));
  store.set("tameion.tenant", JSON.stringify(t));
}
export function currentTenant(): Tenant | null {
  const t = store.get("tameion.tenant");
  if (!t) return null;
  try {
    return JSON.parse(t);
  } catch {
    return null;
  }
}
export function savedPub(k = "tameion.pub"): Pub | null {
  const s = store.get(k);
  if (!s) return null;
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
}
/** The passkey this browser holds for an account, and whether it is one of the
 *  account's owner keys: the key it was made with (`tenant.qx/qy`) or, once read,
 *  one of the contract's owner keys. A device that asked to join keeps its own key
 *  under `tameion.pub.<policy>` so the shared `tameion.pub` is never replaced. */
export function ownerPub(t: Tenant, ownerKeys?: { qx: string; qy: string }[]): { pub: Pub | null; owns: boolean } {
  const eq = (a: unknown, b: unknown) => typeof a === "string" && typeof b === "string" && a.toLowerCase() === b.toLowerCase();
  const mine = [savedPub(`tameion.pub.${t.policy.toLowerCase()}`), savedPub()].filter((p): p is Pub => !!p);
  const holds = (p: Pub) => (ownerKeys?.length ? ownerKeys.some((k) => eq(k.qx, p.qx) && eq(k.qy, p.qy)) : eq(t.qx, p.qx) && eq(t.qy, p.qy));
  const pub = mine.find(holds);
  return { pub: pub ?? mine[0] ?? null, owns: !!pub };
}
/** The reserve token of an account this browser opened (docs/60 P-87): the
 *  service shows it once at reserve; a reserved account's wallets and first
 *  bills take it. The passkey's public key gates nothing (the service serves it). */
export const reserveToken = {
  get: (policy: string) => store.get(`tameion.reserve.${policy.toLowerCase()}`),
  set: (policy: string, tok?: string | null) => {
    if (tok) store.set(`tameion.reserve.${policy.toLowerCase()}`, tok);
  },
};
export const labelOf = (t: Tenant) => store.get(`tameion.label.${t.policy}`) || "";
/** Names this device kept before names moved to the service (docs/81); read
 *  once to move them, then forgotten. */
const localNames = (policy: string): [string, string][] => {
  const pre = `tameion.payee.${policy}.`;
  const out: [string, string][] = [];
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k?.startsWith(pre)) out.push([k.slice(pre.length), localStorage.getItem(k) || ""]);
    }
  } catch {}
  return out;
};
/** Name a party for the whole account (docs/81). Only the device that opened
 *  the account holds the token the service asks for; an empty name forgets it. */
export async function saveName(policy: string, address: string, name: string) {
  const tok = reserveToken.get(policy);
  if (!tok) throw new Error("Only the device that opened this account can name payees. On this one, use your passkey first (Settings, \"Use my passkey here\").");
  await post("/api/level1/contacts", { policy, address, name, reserveToken: tok });
}
/** Move this device's old names to the service, once. */
export async function moveLocalNames(policy: string): Promise<number> {
  if (!reserveToken.get(policy)) return 0;
  let moved = 0;
  for (const [addr, name] of localNames(policy)) {
    try {
      if (name.trim()) await saveName(policy, addr, name);
      store.del(`tameion.payee.${policy}.${addr}`);
      moved++;
    } catch {}
  }
  return moved;
}
/** What a person typed for a party — a Tameion name, an ENS name, a web domain
 *  that lists one address, or an address — resolved by the service (docs/81). */
export type Resolved = { address: string; label: string; kind: "tameion" | "ens" | "domain" | "address"; tail: string };
export const resolveName = (q: string, chain: string) =>
  getJson<Resolved>(`/api/level1/resolve?chain=${encodeURIComponent(chain)}&q=${encodeURIComponent(q.trim())}`);
/** The last four hex digits, the way a card shows its last four. */
export const tail = (a?: string | null) => (a ? a.slice(-4).toUpperCase() : "");

export const isAddr = (s?: string | null) => /^0x[0-9a-fA-F]{40}$/.test((s ?? "").trim());
export const isZero = (s?: string | null) => /^0x0{40}$/i.test((s ?? "").trim());
/** An account's handle (docs/79 §6a): a–z, 0–9 and `-`, 3 to 32, no `-` at either end. */
export const isHandle = (s: string) => /^[a-z0-9][a-z0-9-]{1,30}[a-z0-9]$/.test(s);

// ---- words and numbers -------------------------------------------------------
export const short = (a?: string | null) => (a ? a.slice(0, 6) + "…" + a.slice(-4) : "");
/** What the service takes back. */
export const raw = (s?: string | null) => (s || "").replace(" USDC", "").trim();
/** What a person reads: no trailing zeros past the cents, thousands grouped. */
export const usd = (s?: string | null) => {
  const v = raw(s);
  const m = v.match(/^(-?)(\d+)(?:\.(\d+))?$/);
  if (!m) return v;
  const frac = (m[3] || "").replace(/0+$/, "");
  return m[1] + Number(m[2]).toLocaleString("en-US") + "." + (frac.length < 2 ? frac.padEnd(2, "0") : frac);
};
export const num = (s?: string | null) => parseFloat(raw(s)) || 0;
/** A total or estimate, bank style: cents, more digits only under a cent. Never for an
 *  amount that is paid exactly (0.012 USDC stays 0.012). */
export const usdRound = (s?: string | null) => (num(s) >= 0.01 ? usd(num(s).toFixed(2)) : usd(s));
export const pretty = (t?: string | null) =>
  String(t || "").replace(/(-?\d+\.\d{6}) USDC/g, (_m, v) => usd(v) + " USDC");
/** A service phrase shown as its own sentence ("twice your largest bill…" → "Twice your largest bill…"). */
export const sentence = (t?: string | null) => {
  const p = pretty(t);
  return p.charAt(0).toUpperCase() + p.slice(1);
};
export const ago = (t: number) => {
  const d = Math.max(0, Math.floor(Date.now() / 1000 - t));
  return d < 90 ? `${d} s ago` : d < 5400 ? `${Math.round(d / 60)} min ago` : d < 172800 ? `${Math.round(d / 3600)} h ago` : `${Math.round(d / 86400)} days ago`;
};
/** "every 30 days", "every week", "every 3 minutes" — never "3.0m". */
export const everyText = (secs?: number | null, fallback?: string) => {
  if (!secs) return `every ${fallback ?? "cycle"}`;
  const plural = (n: number, u: string) => (n === 1 ? `every ${u}` : `every ${n} ${u}s`);
  const d = secs / 86400;
  const w = Math.round(d / 7);
  if (d >= 2 || Math.abs(d - 1) < 0.05)
    return w >= 1 && Math.abs(d - 7 * w) < 0.5 ? plural(w, "week") : plural(Math.round(d), "day");
  if (secs >= 7200) return plural(Math.round(secs / 3600), "hour");
  if (secs >= 120) return plural(Math.round(secs / 60), "minute");
  return plural(Math.round(secs), "second");
};
/** A delay in plain words: "2 days", "48 hours", "10 minutes", "60 seconds". */
export const spanText = (secs: number) => {
  const u = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
  if (secs >= 86400 && secs % 86400 === 0) return u(secs / 86400, "day");
  if (secs >= 3600) return u(Math.round(secs / 3600), "hour");
  if (secs >= 60) return u(Math.round(secs / 60), "minute");
  return u(secs, "second");
};
/** "in about 45 s", "in about 3 min", or "now" once the moment has passed. */
export const inText = (unix?: number | null) => {
  const d = unix ? Math.round(unix - Date.now() / 1000) : 0;
  return d > 0 ? `in about ${d < 90 ? `${d} s` : `${Math.round(d / 60)} min`}` : "now";
};
export const dateText = (unix?: number | null) =>
  unix ? new Date(unix * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" }) : "";
/** "next 14 Nov", or "due since 2 Oct" when the date has passed. A bill more
 *  than two cycles overdue has likely ended (a 3-minute test bill last paid
 *  12 days ago is not "due"), so it reads "last paid 2 Oct". */
export const looksEnded = (unix?: number | null, intervalSecs?: number | null) =>
  !!unix && !!intervalSecs && Date.now() / 1000 - unix > 2 * intervalSecs;
export const dueText = (unix?: number | null, intervalSecs?: number | null) => {
  if (!unix) return "";
  if (Date.now() / 1000 - unix <= 0) return `next ${dateText(unix)}`;
  if (intervalSecs && looksEnded(unix, intervalSecs)) return `last paid ${dateText(unix - intervalSecs)}`;
  return `due since ${dateText(unix)}`;
};

// The verifier's reasons in the owner's words (no mechanism words on `/`).
const PLAIN: Record<string, string> = {
  "payee-attested": "Not one of your approved payees yet. “Approve & pay” approves it and pays.",
  "cycles-behind": "Nobody has paid this bill for a while: it may have ended. If it has, tap “Bill ended”.",
  solvency: "The account holds less than this bill.",
  "pre:balance": "The account holds less than this bill.",
  "pre:reserve": "Paying it would go below the amount you keep untouched.",
  "per-action-cap": "More than one payment may be on its own.",
  "daily-cap": "Over what may go out in a day.",
  "amount-range": "More than this payee has been paid before.",
  "payee-lookalike": "This address looks like another payee's. Check it before paying.",
  "payee-unchanged": "The payee's details changed since the last payment.",
  "pre:payee": "The payee's details changed since the last payment.",
  "pre:confidence": "The agent is not sure enough that this is a real recurring bill.",
  "pre:evidence": "Not enough past payments to be sure.",
  "pre:known": "Never paid before.",
  "pre:provenance": "The evidence for this bill is weak.",
  "issuer-blocklist": "Circle has frozen this address; it cannot be paid.",
  idempotency: "Already paid.",
  "pre:not-settled": "Already paid.",
};
export const plainWhy = (why?: string | null) => {
  if (!why) return "";
  const seen = new Set<string>();
  const out: string[] = [];
  for (const part of String(why).split(" · ")) {
    const k = part.split(":")[0].trim();
    const k2 = part.split(":").slice(0, 2).join(":").trim();
    const t = PLAIN[k2] || PLAIN[k] || part;
    if (!seen.has(t)) {
      seen.add(t);
      out.push(t);
    }
  }
  return pretty(out.join(" "));
};

// ---- calldata (viem; the console's chain calls are typed, never hand-built) ------
/** USDC per chain id, pinned in the page (the same addresses as the service's chain presets,
 *  crates/operator/src/watch.rs): an approval or a transfer never goes to a token the service names. */
export const USDC_BY_CHAIN_ID: Record<number, string> = {
  1: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
  8453: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
  42161: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
  421614: "0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d",
  // Circle Gateway's testnet sources (watch.rs `gateway_sources`) and Arc's USDC.
  11155111: "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238",
  11155420: "0x5fd84259d66Cd46123540766Be93DFE6D43130D7",
  84532: "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
  5042002: "0x3600000000000000000000000000000000000000",
};
/** Circle Gateway's wallet on the testnets (watch.rs `GATEWAY_WALLET_TESTNET`): a deposit
 *  approves only this spender, whatever the service's config names. */
export const GATEWAY_WALLET_TESTNET = "0x0077777d7EBA4688BDeF3E311b846F25870A19B9";
/** The pinned USDC for `chainId`, or an error naming the mismatch (audit 2026-10-15). */
export const pinnedUsdc = (chainId: number, named?: string | null) => {
  const usdc = USDC_BY_CHAIN_ID[chainId];
  if (!usdc || !named || usdc.toLowerCase() !== named.toLowerCase())
    throw new Error("The token named for this is not USDC on this chain, so nothing was sent.");
  return usdc;
};

export const erc20 = {
  transfer: (to: string, amount: bigint) =>
    encodeFunctionData({ abi: erc20Abi, functionName: "transfer", args: [getAddress(to), amount] }),
  approve: (spender: string, amount: bigint) =>
    encodeFunctionData({ abi: erc20Abi, functionName: "approve", args: [getAddress(spender), amount] }),
};
const gatewayAbi = [
  { type: "function", name: "deposit", stateMutability: "nonpayable", inputs: [{ name: "token", type: "address" }, { name: "value", type: "uint256" }], outputs: [] },
] as const;
/** Circle Gateway's `deposit(token, value)` on the source chain. */
export const gatewayDeposit = (token: string, amount: bigint) =>
  encodeFunctionData({ abi: gatewayAbi, functionName: "deposit", args: [getAddress(token), amount] });

// ---- HTTP ----------------------------------------------------------------------
/** The agent key this browser holds for an account (sent once the account is gated). */
export const agentKey = (policy?: string) => (policy ? store.get(`tameion.token.${policy}`) : null);
/** Forget the agent key this browser holds for an account (revoked, or refused with 401). */
export const forgetKey = (policy: string) => {
  store.del(`tameion.token.${policy}`);
  store.del(`tameion.tokenid.${policy}`);
};
export const authHeaders = (policy?: string): Record<string, string> => {
  const k = agentKey(policy);
  return k ? { authorization: `Bearer ${k}` } : {};
};

export class ApiError extends Error {
  status: number;
  /** The service's whole answer, for the fields beside `error` (e.g. a 409's `nextUnpaid`). */
  data?: Record<string, unknown>;
  constructor(message: string, status: number, data?: Record<string, unknown>) {
    super(message);
    this.status = status;
    this.data = data;
  }
}

/** The owner's device token on the routes the service gates as `prepare` (`prepare-*`, `fund`; docs/60 P-170), and on
 *  `purchase-approve`, which only the owner's device may call, and the owner's own check, bills and invoice decisions
 *  (`run`, `bills`, `invoice-decide`; P-225; never `buy`): once an agent key exists the gate asks for a credential,
 *  and the page that holds the owner's passkey has this one, not a key. */
const ownerDevice = (path: string, policy?: string): Record<string, string> => {
  const tok = policy && /^\/api\/level1\/(prepare-|fund$|purchase-approve$|run$|bills$|invoice-decide$)/.test(path) ? reserveToken.get(policy) : null;
  return tok ? { "x-tameion-device": tok } : {};
};

export async function post<T = Record<string, unknown>>(path: string, body: unknown, policy?: string): Promise<T> {
  const r = await fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json", ...authHeaders(policy), ...ownerDevice(path, policy) },
    body: JSON.stringify(body),
  });
  const d = await r.json().catch(() => ({}));
  if (r.status === 401 && policy && agentKey(policy)) forgetKey(policy);
  if (!r.ok) throw new ApiError((d as { error?: string }).error || r.statusText, r.status, d as Record<string, unknown>);
  return d as T;
}

export async function getJson<T>(path: string, policy?: string, extra?: Record<string, string>): Promise<T> {
  const r = await fetch(path, { headers: { ...authHeaders(policy), ...extra } });
  const d = await r.json().catch(() => ({}));
  if (r.status === 401 && policy && agentKey(policy)) forgetKey(policy);
  if (!r.ok) throw new ApiError((d as { error?: string }).error || r.statusText, r.status);
  return d as T;
}

export const fetchConfig = () => getJson<Cfg>("/api/level1/config");
/** Demo mode on this host, or null where the host has no such route (docs/115 §3). */
export const fetchDemo = () => getJson<Demo>("/api/level1/demo").catch((e) => (e instanceof ApiError && e.status === 404 ? null : Promise.reject(e)));
/** The account a handle names, or null when no account has it. */
export async function fetchName(handle: string): Promise<NameRecord | null> {
  try {
    return await getJson<NameRecord>(`/api/level1/name/${encodeURIComponent(handle)}`);
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return null;
    throw e;
  }
}
/** The owner's names come back only with this device's token (docs/81). */
export const fetchInbox = (policy: string) => {
  const tok = reserveToken.get(policy);
  return getJson<Inbox>(`/api/level1/inbox?policy=${policy}`, policy, tok ? { "x-tameion-device": tok } : undefined);
};

// ---- the describer the service serves (/level1/call.js) -----------------------
export type Described = {
  known: boolean;
  selector?: string;
  action?: string;
  payees?: string[];
  rows?: { to: string; amount: string }[];
  total?: string;
  terms?: string;
  deadline?: number;
  note?: string;
};
type TameionCallT = {
  describe: (data: string) => Described;
  termsFingerprint: (t: unknown) => string;
  challengeFor: (a: { chainId: number; owner: string; nonce: unknown; target: string; data: string }) => string;
  keccak256: (b: Uint8Array) => Uint8Array | string;
  bytesOf: (h: string) => Uint8Array;
  hex: (b: Uint8Array) => string;
};
export function TC(): TameionCallT {
  const t = (globalThis as unknown as { TameionCall?: TameionCallT }).TameionCall;
  if (!t) throw new Error("the page's signing describer did not load; reload the page");
  return t;
}

// ---- passkeys ------------------------------------------------------------------
/** Never an open-ended wait (docs/60 P-100): the browser's prompt gets a minute — `timeout` is only a hint
 *  a browser may stretch, so the page's own timer aborts it too — and `signal` lets the page cancel it.
 *  A cancel rejects with `AbortError`, the minute with `TimeoutError`, a closed prompt with `NotAllowedError`. */
export async function createKey(k = "tameion.pub", signal?: AbortSignal): Promise<Pub> {
  const saved = savedPub(k);
  if (saved) return saved;
  const minute = AbortSignal.timeout(60_000);
  const cred = (await navigator.credentials.create({
    publicKey: {
      rp: { name: "Tami", id: location.hostname },
      user: { id: crypto.getRandomValues(new Uint8Array(16)), name: "treasurer", displayName: "Treasurer" },
      challenge: crypto.getRandomValues(new Uint8Array(32)),
      pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      authenticatorSelection: { userVerification: "required", residentKey: "preferred" },
      attestation: "none",
      timeout: 60_000,
    },
    signal: signal ? AbortSignal.any([signal, minute]) : minute,
  })) as PublicKeyCredential;
  const resp = cred.response as AuthenticatorAttestationResponse;
  const spki = new Uint8Array(resp.getPublicKey() as ArrayBuffer);
  const point = spki.slice(spki.length - 65);
  if (point[0] !== 0x04) throw new Error("expected an uncompressed P-256 point");
  const pub = { qx: hex(point.slice(1, 33)), qy: hex(point.slice(33, 65)), credentialId: b64u(cred.rawId) };
  store.set(k, JSON.stringify(pub));
  return pub;
}

/** A passkey assertion over a 32-byte challenge, in the shape the contract and
 *  the service check. */
export async function passkeyAssert(challengeHex: string, pub: Pub | null, decision?: Record<string, unknown>) {
  // An owner key made inside this Mac's Secure Enclave (scripts/desk_init.sh) is
  // not a browser passkey: the local bridge asks the chip, which asks for Touch ID
  // (scripts/desk_se_bridge.py, 127.0.0.1 only, the service's own origin only).
  if (pub?.credentialId === "secure-enclave") {
    let r: Response;
    try {
      r = await fetch("http://127.0.0.1:7898/sign", {
        method: "POST",
        headers: { "content-type": "application/json" },
        // A desk decision goes as its fields: the bridge computes the challenge itself and the
        // Touch ID prompt says the decision in words (docs/91 R18).
        body: JSON.stringify(decision ? { decision } : { challenge: challengeHex }),
      });
    } catch {
      throw new Error("This account's key is in this Mac's security chip: start its bridge (python3 scripts/desk_se_bridge.py) and try again.");
    }
    const body = (await r.json()) as { error?: string; challenge?: string; r: string; s: string; challengeIndex: number; typeIndex: number; authenticatorData: string; clientDataJSON: string };
    if (!r.ok) throw new Error(body.error ?? "the chip did not sign");
    if (decision && body.challenge?.toLowerCase() !== challengeHex.toLowerCase())
      throw new Error("The chip signed something other than what this page computed; nothing was sent.");
    return { r: body.r, s: body.s, challengeIndex: body.challengeIndex, typeIndex: body.typeIndex, authenticatorData: body.authenticatorData, clientDataJSON: body.clientDataJSON };
  }
  const id = pub?.credentialId;
  const assertion = (await navigator.credentials.get({
    publicKey: {
      challenge: fromHex(challengeHex),
      rpId: location.hostname,
      userVerification: "required",
      allowCredentials: id ? [{ type: "public-key", id: unb64u(id) }] : [],
    },
  })) as PublicKeyCredential;
  const r = assertion.response as AuthenticatorAssertionResponse;
  const clientDataJSON = new TextDecoder().decode(r.clientDataJSON);
  const { r: rr, s } = derToRS(r.signature);
  return {
    r: rr,
    s,
    challengeIndex: clientDataJSON.indexOf('"challenge":"'),
    typeIndex: clientDataJSON.indexOf('"type":"webauthn.get"'),
    authenticatorData: hex(r.authenticatorData),
    clientDataJSON,
  };
}

export type SignCtx = { tenant: Tenant; cfg: Cfg; inbox: Inbox | undefined; pub: Pub | null };
export type Executed = { explorer: string; tx?: string; pending?: boolean; need?: number; have?: number };

/** Clear signing: hold the call against what the button says (`expect`), describe
 *  it, check the terms it is bound to against what this page shows, recompute the
 *  challenge, then let the passkey sign. */
export async function sign(ctx: SignCtx, target: string, data: string, expect: Expect): Promise<Executed> {
  checkCall(ctx.tenant, target, data, expect);
  const t = TC();
  const d = t.describe(data);
  if (!d.known) throw new Error(`Not signing: this page cannot describe the call (${d.selector}).`);
  if (d.terms) {
    const terms = ctx.inbox?.terms;
    if (!terms) throw new Error("Not signing: this approval is bound to terms this page has not read.");
    if (t.termsFingerprint(terms).toLowerCase() !== d.terms.toLowerCase())
      throw new Error("Not signing: the call is bound to different terms than the ones on this page.");
    if (d.deadline && d.deadline * 1000 < Date.now()) throw new Error("This approval expired. Refresh and tap again.");
  }
  const { tenant, cfg } = ctx;
  const ch = await post<{ nonce: string; challenge: string }>(
    "/api/level1/challenge",
    { chain: tenant.chain, owner: tenant.owner, target, data },
    tenant.policy,
  );
  const chainId = cfg.chains.find((c) => c.key === tenant.chain)?.chainId ?? 0;
  const mine = t.challengeFor({ chainId, owner: tenant.owner, nonce: ch.nonce, target, data });
  if (mine.toLowerCase() !== String(ch.challenge).toLowerCase())
    throw new Error("Not signing: the service's challenge differs from the one this page computed.");
  // The call's own fields go too: a key in this Mac's chip signs an owner call only when its bridge can recompute this
  // challenge and say the call in words (docs/104 §E2); a browser passkey ignores them.
  const auth = await passkeyAssert(ch.challenge, ctx.pub, { kind: "call", chainId, owner: tenant.owner, nonce: ch.nonce, target, data });
  const keys = ctx.inbox?.owners?.keys ?? [];
  const mineIx = keys.findIndex((k) => ctx.pub && k.qx.toLowerCase() === ctx.pub.qx.toLowerCase());
  const res = await post<Executed>(
    "/api/level1/execute",
    { chain: tenant.chain, owner: tenant.owner, target, data, auth, keyIndex: Math.max(0, mineIx) },
    tenant.policy,
  );
  if (res.pending) {
    const more = (res.need ?? 0) - (res.have ?? 0);
    throw new Error(`Approved by you. ${more} more owner approval${more === 1 ? "" : "s"} needed: it waits on their account page.`);
  }
  return res;
}

// ---- the bill wallet in this browser -------------------------------------------
type Eth = { request: (a: { method: string; params?: unknown[] }) => Promise<unknown> };
export const eth = (): Eth | undefined => (globalThis as unknown as { ethereum?: Eth }).ethereum;

export async function switchChain(chainId: number, name?: string) {
  const w = eth();
  if (!w) throw new Error("Needs your bill wallet in this browser (MetaMask or similar).");
  try {
    await w.request({ method: "wallet_switchEthereumChain", params: [{ chainId: "0x" + chainId.toString(16) }] });
  } catch (e) {
    const err = e as { code?: number; message?: string };
    throw new Error(err.code === 4902 ? `Your wallet does not know ${name}; add it there first.` : err.message);
  }
}

/** Send from the browser's wallet. With `chain`, the wallet is switched to it and
 *  checked to be on it first (an approve on another chain changes nothing here),
 *  and the call returns once the receipt says it succeeded. */
export async function walletSend(to: string, data: string, chain?: { chainId: number; name?: string }): Promise<string> {
  const w = eth();
  if (!w) throw new Error("Needs your bill wallet in this browser (MetaMask or similar).");
  const [from] = (await w.request({ method: "eth_requestAccounts" })) as string[];
  if (chain) {
    await switchChain(chain.chainId, chain.name);
    if (parseInt(String(await w.request({ method: "eth_chainId" })), 16) !== chain.chainId)
      throw new Error(`Your wallet is not on ${chain.name ?? "the account's chain"}; switch it there and tap again.`);
  }
  const tx = (await w.request({ method: "eth_sendTransaction", params: [{ from, to, data }] })) as string;
  if (chain) {
    // Up to a minute for the receipt; a wallet that cannot say goes on.
    for (let i = 0; i < 60; i++) {
      const r = (await w.request({ method: "eth_getTransactionReceipt", params: [tx] }).catch(() => undefined)) as { status?: string } | null | undefined;
      if (r === undefined) break;
      if (r) {
        if (r.status === "0x0") throw new Error("The wallet's transaction failed on chain; nothing changed.");
        break;
      }
      await new Promise((ok) => setTimeout(ok, 1000));
    }
  }
  return tx;
}

export async function walletAccount(): Promise<string | undefined> {
  const w = eth();
  if (!w) return undefined;
  const [from] = (await w.request({ method: "eth_requestAccounts" })) as string[];
  return from;
}

// ---- the desk (docs/83 W8) ---------------------------------------------------------
/** The desk is the owner's: every desk call carries this device's token (docs/60 P-87). */
const deskHeaders = (policy: string): Record<string, string> => {
  const tok = reserveToken.get(policy);
  return tok ? { "x-tameion-device": tok } : {};
};
export const fetchDesk = (policy: string) => getJson<DeskBook>(`/api/desk?policy=${policy}`, policy, deskHeaders(policy));
/** This Mac's trade-only key, the exchange account that approved it, and whether this desk trades on it. */
export const fetchDeskVenue = (policy: string) =>
  getJson<{ agent?: string; account?: string | null; connected?: string | null; byEnvironment: boolean; error?: string }>(
    `/api/desk/venue?policy=${policy}`,
    policy,
    deskHeaders(policy),
  );
/** A desk POST; `text` is the reply exactly as the service sent it (the lender's
 *  kit is downloaded byte for byte, never re-serialised). */
export async function deskPost<T>(path: string, policy: string, body: Record<string, unknown>): Promise<{ data: T; text: string }> {
  const r = await fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json", ...deskHeaders(policy) },
    body: JSON.stringify({ policy, ...body }),
  });
  const text = await r.text();
  let data: unknown = {};
  try {
    data = JSON.parse(text);
  } catch {}
  if (!r.ok) throw new ApiError((data as { error?: string }).error || r.statusText, r.status);
  return { data: data as T, text };
}
/** The lender kits already issued for this desk, newest first. */
export const fetchStatements = (policy: string) =>
  getJson<{ statements: DeskStatementRow[]; books?: { file: string; at: number }[] }>(`/api/desk/statements?policy=${policy}`, policy, deskHeaders(policy));
/** One saved kit, byte for byte as the service signed and stored it. */
export async function fetchKitText(policy: string, file: string): Promise<string> {
  // A whole-stock file is named book-<at>.json (docs/94 U1); a deal's kit <deal>-<at>.lender.json.
  const key = file.startsWith("book-") ? "book" : "file";
  const r = await fetch(`/api/desk/statements?policy=${policy}&${key}=${encodeURIComponent(file)}`, { headers: { ...authHeaders(policy), ...deskHeaders(policy) } });
  const text = await r.text();
  if (!r.ok) {
    let msg = r.statusText;
    try {
      msg = (JSON.parse(text) as { error?: string }).error || msg;
    } catch {}
    throw new ApiError(msg, r.status);
  }
  return text;
}
/** Grams as the book prints them ("10.000 g gold") to a number. */
export const grams = (s?: string | null) => parseFloat(String(s ?? "")) || 0;
export const gramsText = (n: number) => `${n.toLocaleString("en-US", { maximumFractionDigits: 3 })} g`;
export const GRAMS_PER_OZ = 31.1034768;
export const ozText = (oz: number) => `${oz.toLocaleString("en-US", { maximumFractionDigits: 3 })} oz`;
/** A service sentence with every address cut to its last four ("account ••4D09"):
 *  addresses belong in a details fold, never in a sentence (docs/81). */
export const noAddr = (s?: string | null) => String(s ?? "").replace(/0x[0-9a-fA-F]{40}\b/g, (a) => `••${tail(a)}`);
