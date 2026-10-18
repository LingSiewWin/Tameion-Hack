// What the owner's passkey signs from the Owner home, and nothing else: the call is built here from what the button
// says, the service's prepared call must equal it byte for byte, and the challenge the passkey signs is recomputed here
// (PasskeyPrincipal.challengeFor) — the passkey never signs a challenge this page did not compute. The flow is the
// service's own: prepare-* → POST /api/level1/challenge → passkey → POST /api/level1/execute (as apps/console does).
// Keccak-256 is ported from the service's describer (/level1/call.js, pinned there to published vectors): browsers ship
// SHA-2 only, and this app carries no chain library.

const M64 = (1n << 64n) - 1n;
const RC = [
  0x0000000000000001n, 0x0000000000008082n, 0x800000000000808an, 0x8000000080008000n,
  0x000000000000808bn, 0x0000000080000001n, 0x8000000080008081n, 0x8000000000008009n,
  0x000000000000008an, 0x0000000000000088n, 0x0000000080008009n, 0x000000008000000an,
  0x000000008000808bn, 0x800000000000008bn, 0x8000000000008089n, 0x8000000000008003n,
  0x8000000000008002n, 0x8000000000000080n, 0x000000000000800an, 0x800000008000000an,
  0x8000000080008081n, 0x8000000000008080n, 0x0000000080000001n, 0x8000000080008008n,
];
const ROT = [
  [0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61],
  [28, 55, 25, 21, 56], [27, 20, 39, 8, 14],
];
const rotl = (v: bigint, n: number) => (n === 0 ? v : ((v << BigInt(n)) | (v >> BigInt(64 - n))) & M64);

function keccakF(A: bigint[]) {
  for (let round = 0; round < 24; round++) {
    const C = [0, 1, 2, 3, 4].map((x) => A[x] ^ A[x + 5] ^ A[x + 10] ^ A[x + 15] ^ A[x + 20]);
    const D = [0, 1, 2, 3, 4].map((x) => C[(x + 4) % 5] ^ rotl(C[(x + 1) % 5], 1));
    for (let i = 0; i < 25; i++) A[i] ^= D[i % 5];
    const B = new Array<bigint>(25);
    for (let x = 0; x < 5; x++) for (let y = 0; y < 5; y++) B[y + 5 * ((2 * x + 3 * y) % 5)] = rotl(A[x + 5 * y], ROT[x][y]);
    for (let x = 0; x < 5; x++)
      for (let y = 0; y < 5; y++) A[x + 5 * y] = B[x + 5 * y] ^ (~B[((x + 1) % 5) + 5 * y] & M64 & B[((x + 2) % 5) + 5 * y]);
    A[0] ^= RC[round];
  }
}

export function keccak256(bytes: Uint8Array): Uint8Array {
  const rate = 136;
  const A = new Array<bigint>(25).fill(0n);
  const padded = new Uint8Array(Math.ceil((bytes.length + 1) / rate) * rate);
  padded.set(bytes);
  padded[bytes.length] ^= 0x01; // Keccak's padding, not SHA-3's 0x06
  padded[padded.length - 1] ^= 0x80;
  for (let off = 0; off < padded.length; off += rate) {
    for (let i = 0; i < rate / 8; i++) {
      let lane = 0n;
      for (let b = 7; b >= 0; b--) lane = (lane << 8n) | BigInt(padded[off + i * 8 + b]);
      A[i] ^= lane;
    }
    keccakF(A);
  }
  const out = new Uint8Array(32);
  for (let i = 0; i < 4; i++) {
    let lane = A[i];
    for (let b = 0; b < 8; b++) {
      out[i * 8 + b] = Number(lane & 0xffn);
      lane >>= 8n;
    }
  }
  return out;
}

export const hex = (bytes: Uint8Array) => "0x" + [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
export function bytesOf(h: string): Uint8Array {
  const s = h.trim().toLowerCase().replace(/^0x/, "");
  if (s.length % 2 || /[^0-9a-f]/.test(s)) throw new Error("not hex");
  return Uint8Array.from(s.match(/../g) ?? [], (b) => parseInt(b, 16));
}
const selector = (sig: string) => hex(keccak256(new TextEncoder().encode(sig)).slice(0, 4)).slice(2);
/** One ABI word: a uint, or a 20-byte address left-padded. */
function word(v: bigint | string): string {
  if (typeof v === "bigint") return v.toString(16).padStart(64, "0");
  const a = v.toLowerCase().replace(/^0x/, "");
  if (!/^[0-9a-f]{40}$/.test(a)) throw new Error("not an address");
  return a.padStart(64, "0");
}

/** keccak256(abi.encode(chainid, owner, nonce, target, value = 0, keccak256(data))): PasskeyPrincipal.challengeFor. */
export function challengeFor(a: { chainId: number; owner: string; nonce: string | number; target: string; data: string }) {
  const words = word(BigInt(a.chainId)) + word(a.owner) + word(BigInt(a.nonce)) + word(a.target) + word(0n) + hex(keccak256(bytesOf(a.data))).slice(2);
  return hex(keccak256(bytesOf(words)));
}

/** USDC has six decimals: "0.005" or "0.005000 USDC" → 5000n. */
export function units(s: string): bigint {
  const m = /^\s*(\d+)(?:\.(\d{1,6}))?\s*(?:USDC)?\s*$/.exec(s);
  if (!m) throw new Error(`not a USDC amount: ${s}`);
  return BigInt(m[1]) * 1_000_000n + BigInt((m[2] ?? "").padEnd(6, "0"));
}

/** The owner calls the Owner home makes, built here from what the button says. */
export const calls = {
  /** 0 Watch (stopped), 1 Propose (asks before paying), 2 Autopilot */
  setMode: (mode: 0 | 1 | 2) => "0x" + selector("setMode(uint8)") + word(BigInt(mode)),
  setPayees: (payees: string[], attested: boolean) =>
    "0x" + selector("setPayees(address[],bool)") + word(64n) + word(attested ? 1n : 0n) + word(BigInt(payees.length)) + payees.map(word).join(""),
  setLimits: (perAction: bigint, daily: bigint, floor: bigint) =>
    "0x" + selector("setLimits(uint256,uint256,uint256)") + word(perAction) + word(daily) + word(floor),
  /** drops the change waiting out the delay (a raised limit or mode) */
  cancelPending: () => "0x" + selector("cancelPending()"),
};

const text = (u: bigint) => `${u / 1_000_000n}.${(u % 1_000_000n).toString().padStart(6, "0")}`;
/** The prepare-limits body and the call the page will compare it with: these two caps, the floor kept. */
export function limitsCall(caps: { floor: string }, perAction: bigint, daily: bigint) {
  const floor = units(caps.floor);
  return { body: { perAction: text(perAction), daily: text(daily), floor: text(floor) }, data: calls.setLimits(perAction, daily, floor) };
}

/** "Raise the cap" for a payment over the per-payment cap: one payment may be that payment's amount, and the day's cap
 *  at least as much; the floor stays. */
export function raiseCap(caps: { daily: string; floor: string }, amountText: string) {
  const want = units(amountText);
  const daily = units(caps.daily);
  return limitsCall(caps, want, daily > want ? daily : want);
}

const SETTLE = [selector("settleAsPrincipal(address,uint256,bytes32)"), selector("settleAsPrincipalIf(address,uint256,bytes32,bytes32,uint64)")];
/** A payment the agent prepared for the owner's tap pays exactly `to` exactly `amount`, or this throws in words. */
export function checkSettle(data: string, to: string, amount: bigint) {
  const h = data.toLowerCase().replace(/^0x/, "");
  if (!SETTLE.includes(h.slice(0, 8))) throw new Error("Not signing: the call handed to this button is not a payment.");
  if (h.slice(8, 72) !== word(to)) throw new Error("Not signing: the call pays someone other than the payee on this page.");
  if (BigInt("0x" + h.slice(72, 136)) !== amount) throw new Error("Not signing: the call pays another amount than the one on this page.");
}

// ---- the flow in the browser ------------------------------------------------------------------------------------

/** Where the owner signs: the account (policy) and its owner contract, by address — in memory and API calls only, never
 *  rendered (DESIGN §Page grammar) — and the chain. */
export type Account = { policy: string; owner: string; chain: string; chainId: number };
/** The owner's passkey as apps/console keeps it (localStorage `tameion.pub`): its id narrows the browser's prompt. */
type Pub = { qx: string; qy: string; credentialId: string };
export type Executed = { explorer?: string; tx?: string; pending?: boolean; need?: number; have?: number };

const N = 0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551n;
/** A WebAuthn DER signature as the low-s (r, s) the contract checks. */
function derToRS(der: ArrayBuffer) {
  const b = new Uint8Array(der);
  let i = 2;
  const read = () => {
    if (b[i++] !== 0x02) throw new Error("The passkey's signature could not be read.");
    const len = b[i++];
    let v = 0n;
    for (let k = 0; k < len; k++) v = (v << 8n) | BigInt(b[i++]);
    return v;
  };
  const r = read();
  let s = read();
  if (s > N / 2n) s = N - s;
  return { r: "0x" + word(r), s: "0x" + word(s) };
}
const unb64u = (s: string) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));

export function savedPub(): Pub | null {
  try {
    return JSON.parse(localStorage.getItem("tameion.pub") ?? "null") as Pub | null;
  } catch {
    return null;
  }
}
/** The service answered with a refusal. `executed` is the envelope's word where it has one (docs/115 §2; this app's proxy
 *  says "unknown" when the service did not answer a POST); a refusal without it executed nothing. */
export class Refused extends Error {
  readonly status: number;
  readonly executed: "yes" | "no" | "unknown";
  constructor(message: string, status: number, executed: "yes" | "no" | "unknown") {
    super(message);
    this.name = "Refused";
    this.status = status;
    this.executed = executed;
  }
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const r = await fetch(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const d = (await r.json().catch(() => ({}))) as { error?: string | { words?: string; executed?: "yes" | "no" | "unknown" } };
  if (!r.ok) {
    const e = typeof d.error === "object" && d.error ? d.error : null;
    throw new Refused(typeof d.error === "string" ? d.error : (e?.words ?? `The service answered ${r.status}.`), r.status, e?.executed ?? "no");
  }
  return d as T;
}

export async function passkey(challenge: string) {
  const id = savedPub()?.credentialId;
  const a = (await navigator.credentials.get({
    publicKey: {
      challenge: bytesOf(challenge) as BufferSource,
      rpId: location.hostname,
      userVerification: "required",
      allowCredentials: id ? [{ type: "public-key", id: unb64u(id) as BufferSource }] : [],
    },
    signal: AbortSignal.timeout(60_000),
  })) as PublicKeyCredential;
  const res = a.response as AuthenticatorAssertionResponse;
  const clientDataJSON = new TextDecoder().decode(res.clientDataJSON);
  return {
    ...derToRS(res.signature),
    challengeIndex: clientDataJSON.indexOf('"challenge":"'),
    typeIndex: clientDataJSON.indexOf('"type":"webauthn.get"'),
    authenticatorData: hex(new Uint8Array(res.authenticatorData)),
    clientDataJSON,
  };
}

/** Sign `data` on the account with the owner's passkey. `prepared` is the service's call when it built one: it must be
 *  this call exactly. `keyIndex`: the owner's key among the account's owner keys (0 on a one-key account). */
export async function ownerCall(
  acct: Account,
  data: string,
  opts: { keyIndex?: number; prepared?: { to: string; data: string }; phase?: (p: "Signing" | "Sent") => void } = {},
): Promise<Executed> {
  const target = acct.policy;
  if (opts.prepared && (opts.prepared.to.toLowerCase() !== target.toLowerCase() || opts.prepared.data.toLowerCase() !== data.toLowerCase()))
    throw new Error("Not signing: the service prepared a different call than the one this button makes. Nothing was sent.");
  const ch = await post<{ nonce: string; challenge: string }>("/api/level1/challenge", { chain: acct.chain, owner: acct.owner, target, data });
  const mine = challengeFor({ chainId: acct.chainId, owner: acct.owner, nonce: ch.nonce, target, data });
  if (mine.toLowerCase() !== String(ch.challenge).toLowerCase())
    throw new Error("Not signing: the service's challenge differs from the one this page computed. Nothing was sent.");
  opts.phase?.("Signing");
  const auth = await passkey(mine);
  opts.phase?.("Sent");
  return post<Executed>("/api/level1/execute", { chain: acct.chain, owner: acct.owner, target, data, auth, keyIndex: opts.keyIndex ?? 0 });
}

/** level1::invoice_decide_challenge: keccak256(abi.encodePacked("tameion.invoice-decide.v1", uint64 chainId, address policy,
 *  bytes32 id, uint8 accept, uint64 issued)), the bytes apps/console's guard.ts signs for an invoice decision. */
export function invoiceDecideChallenge(chainId: number, policy: string, id: string, accept: boolean, issued: number) {
  const u64 = (n: bigint) => n.toString(16).padStart(16, "0");
  const tag = hex(new TextEncoder().encode("tameion.invoice-decide.v1")).slice(2);
  const idHex = id.toLowerCase().replace(/^0x/, "");
  if (!/^[0-9a-f]{64}$/.test(idHex)) throw new Error("This invoice's id is not one this page can sign.");
  const packed = tag + u64(BigInt(chainId)) + word(policy).slice(24) + idHex + (accept ? "01" : "00") + u64(BigInt(issued));
  return hex(keccak256(bytesOf(packed)));
}

/** Accept (or decline) an invoice: the owner's passkey over the decision's own challenge (no chain call). */
export async function decideInvoice(acct: Account, id: string, accept: boolean) {
  const issued = Math.floor(Date.now() / 1000);
  const auth = await passkey(invoiceDecideChallenge(acct.chainId, acct.policy, id, accept, issued));
  return post("/api/level1/invoice-decide", { policy: acct.policy, id, accept, issued, auth });
}

/** The owner key this browser holds, as an index among the account's owner keys (0 when it cannot tell). */
export function keyIndexOf(keys?: { qx: string }[] | null): number {
  const pub = savedPub();
  const i = keys?.findIndex((k) => pub && k.qx.toLowerCase() === pub.qx.toLowerCase()) ?? -1;
  return Math.max(0, i);
}

/** prepare-* on the service, for the calls it builds (it reads the account's current state); the result is checked. */
export const prepare = (acct: Account, route: string, body: Record<string, unknown>) =>
  post<{ to: string; data: string }>(`/api/level1/${route}`, { policy: acct.policy, ...body });
