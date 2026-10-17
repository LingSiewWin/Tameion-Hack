"use server";
// The server half of onboarding (H4). Two secrets never reach the page: the invite word (HOME_INVITE_WORD, compared
// here; the browser keeps only an HMAC of it in an httpOnly cookie) and the device token the service mints when an
// account opens (DESIGN §Data: "the device token never leaves the server session"; it lives in an httpOnly cookie per
// account and is added here to the three owner routes that take it). Request bodies are the console's
// (apps/console/src/components/start.tsx `open`, components/account/judge.tsx `BillStep`, lib/tameion.ts `saveName`).
// Each action returns `{ error }` in words instead of throwing: a thrown message is hidden from the page in production.
import { createHmac, timingSafeEqual } from "node:crypto";
import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import { guest, server } from "./copy.ts";

const SERVICE = () => (process.env.TAMEION_URL ?? "http://127.0.0.1:7890").replace(/\/+$/, "");
const INVITE = "tameion.home.invite";
// The owner's device token for the account this browser opened (ruled 2026-10-18): httpOnly, so page code never reads
// it; the /api proxy forwards it as `x-tameion-device` (src/app/api/[...path]/route.ts reads the same name).
const DEVICE = "tameion.home.device";
const COOKIE = { httpOnly: true, sameSite: "lax", secure: process.env.NODE_ENV === "production", path: "/" } as const;
const isAddr = (s: unknown): s is string => typeof s === "string" && /^0x[0-9a-fA-F]{40}$/.test(s);
const isWord32 = (s: unknown): s is string => typeof s === "string" && /^0x[0-9a-fA-F]{64}$/.test(s);
const isHandle = (s: unknown): s is string => typeof s === "string" && /^[a-z0-9][a-z0-9-]{1,30}[a-z0-9]$/.test(s);

/** An HMAC of the word, so the cookie proves the word was typed without holding it. Fixed length, compared in constant time. */
const proof = (word: string) => createHmac("sha256", word).update("tameion.home.invite.v1").digest();
const sameWord = (typed: string, word: string) => timingSafeEqual(proof(typed), proof(word));

async function invited(): Promise<string | null> {
  const word = process.env.HOME_INVITE_WORD;
  const held = (await cookies()).get(INVITE)?.value;
  if (!word || !held) return null;
  const b = Buffer.from(held, "base64url");
  const want = proof(word);
  return b.length === want.length && timingSafeEqual(b, want) ? word : null;
}

/** The service's sentence: a page route's `error` string, or the refusal envelope's `words` (docs/115 §2). */
const wordsOf = (body: unknown, fallback: string): string => {
  const e = (body as { error?: unknown } | null)?.error;
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && typeof (e as { words?: unknown }).words === "string") return (e as { words: string }).words;
  return fallback;
};

async function call(path: string, init?: RequestInit): Promise<{ ok: boolean; status: number; body: unknown }> {
  try {
    const r = await fetch(`${SERVICE()}${path}`, { ...init, cache: "no-store" });
    return { ok: r.ok, status: r.status, body: await r.json().catch(() => null) };
  } catch {
    return { ok: false, status: 502, body: null };
  }
}

/** Whether this browser typed the word (the stepper's page asks before it renders). Says yes or no, never the word. */
export async function hasInvite(): Promise<boolean> {
  return (await invited()) !== null;
}

export type GuestState = { error?: string };

/** `/`: the word from the judges' form opens the seeded demo account read-only, or starts the stepper. */
export async function enterGuest(_prev: GuestState, form: FormData): Promise<GuestState> {
  const word = process.env.HOME_INVITE_WORD;
  if (!word) return { error: guest.noWord };
  const typed = String(form.get("word") ?? "").trim();
  if (!typed || !sameWord(typed, word)) return { error: guest.wrongWord };
  (await cookies()).set(INVITE, proof(word).toString("base64url"), { ...COOKIE, maxAge: 7 * 86400 });
  if (form.get("intent") === "new") redirect("/new");
  // The account is opened by its name (DESIGN §Routes, ruled 06:32): the address never rides in an href.
  const demo = process.env.HOME_DEMO_NAME?.trim().toLowerCase();
  if (!isHandle(demo)) return { error: guest.noDemo };
  redirect(`/a/${demo}`);
}

export type Opened = {
  policy: string;
  owner: string;
  chain: string;
  name: string;
  /** what the demo sent at open: its amount and that it was sent, or why nothing was sent */
  funding: { usdc?: string; sent: boolean; error?: string };
};

/** Step 3: open the account on chain with the passkey made in step 2; the demo funds it (`POST /api/level1/deploy`). */
export async function openAccount(input: { name: string; qx: string; qy: string; chain: string }): Promise<Opened | { error: string; taken?: boolean }> {
  const word = await invited();
  if (!word) return { error: server.noInvite };
  if (!isHandle(input.name)) return { error: server.badName };
  if (!isWord32(input.qx) || !isWord32(input.qy)) return { error: server.badKey };
  const r = await call("/api/level1/deploy", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ chain: input.chain, qx: input.qx, qy: input.qy, name: input.name, keyless: false, days: 90, invite: word, lazy: true, demo: true }),
  });
  if (!r.ok) return { error: wordsOf(r.body, r.status === 502 ? server.downOpen : server.refused(r.status)), taken: r.status === 409 };
  const d = r.body as { tenant: { policy: string; owner: string; chain: string; name?: string }; reserveToken?: string | null; demo?: { usdc?: string; tx?: string; error?: string } };
  // Shown once by the service; kept only here, as an httpOnly cookie this account's owner routes read.
  if (d.reserveToken) (await cookies()).set(DEVICE, d.reserveToken, { ...COOKIE, maxAge: 90 * 86400 });
  return {
    policy: d.tenant.policy,
    owner: d.tenant.owner,
    chain: d.tenant.chain,
    name: d.tenant.name ?? input.name,
    funding: { usdc: d.demo?.usdc, sent: !!d.demo?.tx, error: d.demo?.error },
  };
}

const OWNER = new Set(["bills", "prepare-payees", "contacts"]);

/** An owner route that takes this device's token, as the console sends it: the header on `bills` and `prepare-*`
 *  (lib/tameion.ts `ownerDevice`), `reserveToken` in the body on `bills` and `contacts`. */
export async function ownerPost(policy: string, route: "bills" | "prepare-payees" | "contacts", body: Record<string, unknown>): Promise<{ ok: true; body: unknown } | { error: string; executed: "no" | "unknown" }> {
  if (!isAddr(policy) || !OWNER.has(route)) return { error: server.notOwnerRoute, executed: "no" };
  const tok = (await cookies()).get(DEVICE)?.value;
  const withToken = route === "prepare-payees" ? body : { ...body, reserveToken: tok };
  const r = await call(`/api/level1/${route}`, {
    method: "POST",
    headers: { "content-type": "application/json", ...(tok && route !== "contacts" ? { "x-tameion-device": tok } : {}) },
    body: JSON.stringify({ ...withToken, policy }),
  });
  if (!r.ok) return { error: wordsOf(r.body, r.status === 502 ? server.down : server.refused(r.status)), executed: r.status === 502 ? "unknown" : "no" };
  return { ok: true, body: r.body };
}

/** The inbox with this device's token, so the owner's own names come back (docs/81). */
export async function readInbox(policy: string): Promise<{ ok: true; inbox: unknown } | { error: string }> {
  if (!isAddr(policy)) return { error: server.notAddress };
  const tok = (await cookies()).get(DEVICE)?.value;
  const r = await call(`/api/level1/inbox?policy=${policy.toLowerCase()}`, { headers: tok ? { "x-tameion-device": tok } : {} });
  return r.ok ? { ok: true, inbox: r.body } : { error: wordsOf(r.body, server.down) };
}
