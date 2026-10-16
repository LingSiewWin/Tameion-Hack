// What a passkey approval actually says, computed in the page.
//
// The page must never sign a challenge it cannot explain. A link carries the
// call itself -- owner contract, chain, approval number, target, calldata --
// and this file turns it into words a person can check ("allow the agent to
// pay 0x1111…") and into the exact 32 bytes PasskeyPrincipal.challengeFor
// computes on chain. The page signs only what it showed.
//
// Keccak-256 is here because browsers do not ship it (WebCrypto has SHA-2
// only). Lanes are BigInt: slow, short and easy to check, and the inputs are a
// few hundred bytes. call.test.mjs pins it to published vectors and to
// challenges the deployed contract produced.

// One function scope: loaded as a classic <script>, top-level consts would
// share the page's global scope and collide with its own helpers (they did:
// "Identifier 'hex' has already been declared"). Only TameionCall leaves.
(function () {
const M64 = (1n << 64n) - 1n;
const RC = [
  0x0000000000000001n, 0x0000000000008082n, 0x800000000000808an, 0x8000000080008000n,
  0x000000000000808bn, 0x0000000080000001n, 0x8000000080008081n, 0x8000000000008009n,
  0x000000000000008an, 0x0000000000000088n, 0x0000000080008009n, 0x000000008000000an,
  0x000000008000808bn, 0x800000000000008bn, 0x8000000000008089n, 0x8000000000008003n,
  0x8000000000008002n, 0x8000000000000080n, 0x000000000000800an, 0x800000008000000an,
  0x8000000080008081n, 0x8000000000008080n, 0x0000000080000001n, 0x8000000080008008n,
];
// Rotation offsets r[x][y].
const ROT = [
  [0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61],
  [28, 55, 25, 21, 56], [27, 20, 39, 8, 14],
];
const rotl = (v, n) => (n === 0 ? v : ((v << BigInt(n)) | (v >> BigInt(64 - n))) & M64);

function keccakF(A) {
  for (let round = 0; round < 24; round++) {
    const C = [0, 1, 2, 3, 4].map((x) => A[x] ^ A[x + 5] ^ A[x + 10] ^ A[x + 15] ^ A[x + 20]);
    const D = [0, 1, 2, 3, 4].map((x) => C[(x + 4) % 5] ^ rotl(C[(x + 1) % 5], 1));
    for (let i = 0; i < 25; i++) A[i] ^= D[i % 5];
    const B = new Array(25);
    for (let x = 0; x < 5; x++)
      for (let y = 0; y < 5; y++) B[y + 5 * ((2 * x + 3 * y) % 5)] = rotl(A[x + 5 * y], ROT[x][y]);
    for (let x = 0; x < 5; x++)
      for (let y = 0; y < 5; y++)
        A[x + 5 * y] = B[x + 5 * y] ^ (~B[((x + 1) % 5) + 5 * y] & M64 & B[((x + 2) % 5) + 5 * y]);
    A[0] ^= RC[round];
  }
}

function keccak256(bytes) {
  const rate = 136;
  const A = new Array(25).fill(0n);
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
    for (let b = 0; b < 8; b++) { out[i * 8 + b] = Number(lane & 0xffn); lane >>= 8n; }
  }
  return out;
}

const hex = (bytes) => "0x" + [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
function bytesOf(h) {
  const s = h.trim().toLowerCase().replace(/^0x/, "");
  if (s.length % 2 || /[^0-9a-f]/.test(s)) throw new Error("not hex: " + h.slice(0, 20));
  return Uint8Array.from(s.match(/../g) || [], (b) => parseInt(b, 16));
}
const utf8 = (s) => new TextEncoder().encode(s);
const selector = (sig) => hex(keccak256(utf8(sig)).slice(0, 4));

function word(value) {
  // value: BigInt, or a 20-byte address as hex.
  const out = new Uint8Array(32);
  if (typeof value === "bigint") {
    let v = value;
    for (let i = 31; i >= 0; i--) { out[i] = Number(v & 0xffn); v >>= 8n; }
  } else {
    const a = bytesOf(value);
    if (a.length !== 20 && a.length !== 32) throw new Error("not an address or word: " + value);
    out.set(a, 32 - a.length);
  }
  return out;
}

/** keccak256(abi.encode(chainid, owner, n, target, value, keccak256(data))) -- PasskeyPrincipal.challengeFor. */
function challengeFor({ chainId, owner, nonce, target, value = 0n, data }) {
  const parts = [word(BigInt(chainId)), word(owner), word(BigInt(nonce)), word(target),
                 word(BigInt(value)), keccak256(bytesOf(data))];
  const buf = new Uint8Array(32 * parts.length);
  parts.forEach((p, i) => buf.set(p, 32 * i));
  return hex(keccak256(buf));
}

const CHAINS = { 31337: "a local test chain", 421614: "Arbitrum Sepolia", 5042002: "Arc testnet",
                 42161: "Arbitrum One" };
const asAddress = (w) => "0x" + [...w.slice(12)].map((b) => b.toString(16).padStart(2, "0")).join("");
const asUint = (w) => w.reduce((v, b) => (v << 8n) | BigInt(b), 0n);
const usdc = (units) => `${units / 1_000_000n}.${(units % 1_000_000n).toString().padStart(6, "0")} USDC`;

/**
 * Plain words for the calls a policy's principal makes, or { known: false }.
 * Anything not recognised is shown as unknown and should not be approved.
 */
function describe(data) {
  const b = bytesOf(data);
  const sel = hex(b.slice(0, 4));
  const w = (i) => b.slice(4 + 32 * i, 4 + 32 * (i + 1));
  const words = (b.length - 4) / 32;
  if (sel === selector("setPayee(address,bool)") && words === 2) {
    const yes = asUint(w(1)) === 1n;
    return { known: true, action: yes ? "Allow the agent to pay" : "Stop the agent paying", payees: [asAddress(w(0))] };
  }
  if (sel === selector("setPayees(address[],bool)") && words >= 3) {
    const yes = asUint(w(1)) === 1n;
    const n = Number(asUint(w(2)));
    if (asUint(w(0)) !== 64n || words !== 3 + n) return { known: false, selector: sel };
    const payees = [...Array(n).keys()].map((i) => asAddress(w(3 + i)));
    return { known: true, action: yes ? "Allow the agent to pay" : "Stop the agent paying", payees };
  }
  if (sel === selector("settleAsPrincipal(address,uint256,bytes32)") && words === 3) {
    return { known: true, action: `Pay ${usdc(asUint(w(1)))} yourself to`, payees: [asAddress(w(0))],
             note: "settles one bill the agent asked about, and marks it paid" };
  }
  if (sel === selector("settleAsPrincipalIf(address,uint256,bytes32,bytes32,uint64)") && words === 5) {
    const deadline = Number(asUint(w(4)));
    return { known: true, action: `Pay ${usdc(asUint(w(1)))} yourself to`, payees: [asAddress(w(0))],
             terms: hex(w(3)), deadline,
             note: `settles one bill the agent asked about; valid only while the limits, autonomy level, keys and pending changes you see stay as they are, and until ${new Date(deadline * 1000).toISOString().slice(11, 16)} UTC` };
  }
  if (sel === selector("setPayeesIf(address[],bool,bytes32,uint64)") && words >= 5) {
    const yes = asUint(w(1)) === 1n;
    const n = Number(asUint(w(4)));
    if (asUint(w(0)) !== 128n || words !== 5 + n) return { known: false, selector: sel };
    const payees = [...Array(n).keys()].map((i) => asAddress(w(5 + i)));
    const deadline = Number(asUint(w(3)));
    return { known: true, action: yes ? "Allow the agent to pay" : "Stop the agent paying", payees,
             terms: hex(w(2)), deadline,
             note: `valid only while the terms you see stay as they are, and until ${new Date(deadline * 1000).toISOString().slice(11, 16)} UTC` };
  }
  if (sel === selector("settleRunIf(address[],uint256[],bytes32[],bytes32,uint64)") && words >= 8) {
    // PLAN 5.39: the owner's reviewed run. Strict canonical layout: three
    // arrays of the same length n, at the offsets an encoder writes.
    const n = Number(asUint(w(5)));
    const a = 160n, u = a + 32n * BigInt(1 + n), h = u + 32n * BigInt(1 + n);
    if (n < 1 || words !== 5 + 3 * (1 + n) || asUint(w(0)) !== a || asUint(w(1)) !== u || asUint(w(2)) !== h
        || Number(asUint(w(6 + n))) !== n || Number(asUint(w(7 + 2 * n))) !== n) return { known: false, selector: sel };
    const payees = [...Array(n).keys()].map((i) => asAddress(w(6 + i)));
    const amounts = [...Array(n).keys()].map((i) => asUint(w(7 + n + i)));
    const total = amounts.reduce((x, y) => x + y, 0n);
    const deadline = Number(asUint(w(4)));
    const rows = payees.map((to, i) => ({ to, amount: usdc(amounts[i]) }));
    return { known: true, action: `Pay ${n} bill${n === 1 ? "" : "s"} yourself, ${usdc(total)} in all, to`, payees, rows, total: usdc(total),
             terms: hex(w(3)), deadline,
             note: `${rows.map((r) => `${r.amount} to ${r.to}`).join("; ")}. A bill already paid is skipped, never paid twice; valid only while the limits, autonomy level, keys and pending changes you see stay as they are, and until ${new Date(deadline * 1000).toISOString().slice(11, 16)} UTC` };
  }
  if (sel === selector("setCoOwners(bytes32[],bytes32[],uint8)") && words >= 5) {
    // PLAN 5.7: who else owns the account, and how many must approve.
    const n = Number(asUint(w(3)));
    const xs = 96n, ys = xs + 32n * BigInt(1 + n);
    if (asUint(w(0)) !== xs || asUint(w(1)) !== ys || words !== 3 + 2 * (1 + n) || Number(asUint(w(4 + n))) !== n) return { known: false, selector: sel };
    const m = Number(asUint(w(2)));
    const keys = [...Array(n).keys()].map((i) => hex(w(4 + i)));
    return { known: true, action: n === 0 ? "Make your passkey the only owner again" : `Owners: your passkey and ${n} more device${n === 1 ? "" : "s"}; ${m} of ${n + 1} must approve every change`,
             payees: [], keys, threshold: m,
             note: n === 0 ? "removes every co-owner; one key approves alone" : `a co-owner key can approve changes to this account (${m === 1 ? "alone" : `with ${m - 1} other${m === 2 ? "" : "s"}`}). Add only devices you and your co-owners hold.` };
  }
  if (sel === selector("withdraw(address,uint256)") && words === 2) {
    return { known: true, action: `Withdraw ${usdc(asUint(w(1)))} to`, payees: [asAddress(w(0))] };
  }
  if (sel === selector("setOperator(address)") && words === 1) {
    return { known: true, action: "Hand the limits key (operator) to", payees: [asAddress(w(0))] };
  }
  if (sel === selector("cancelRecovery()") && words === 0) {
    return { known: true, action: "Cancel the pending replacement of your passkey", payees: [],
             note: "a replacement you did not ask for means the recovery address is acting without you" };
  }
  if (sel === selector("setRecovery(address)") && words === 1) {
    return { known: true, action: "Name the recovery address that may replace your passkey after the delay", payees: [asAddress(w(0))] };
  }
  if (sel === selector("cancelPending()") && words === 0) {
    return { known: true, action: "Cancel the pending raise of the agent's limits or autonomy", payees: [],
             note: "a raise you did not ask for is the sign of a leaked operator key" };
  }
  if (sel === selector("setMode(uint8)") && words === 1) {
    const m = asUint(w(0));
    const names = ["Watch: the agent reads and reports, pays nothing",
                   "Propose: the agent prepares each payment for your signature",
                   "Autopilot: the agent pays approved payees inside the caps"];
    if (m > 2n) return { known: false, selector: sel };
    return { known: true, action: `Set the agent's autonomy to ${names[Number(m)]}`, payees: [],
             note: "lowering applies at once; raising waits the delay and you can cancel it until then" };
  }
  if (sel === selector("setLimits(uint256,uint256,uint256)") && words === 3) {
    return { known: true,
             action: `Set the agent's limits: ${usdc(asUint(w(0)))} per payment, ${usdc(asUint(w(1)))} per day, keep ${usdc(asUint(w(2)))} untouched`,
             payees: [], note: "tightening applies at once; loosening waits the delay and you can cancel it until then" };
  }
  if (sel === selector("addSchedule(address,uint256,uint64,uint64)") && words === 4) {
    const every = Number(asUint(w(2))); const first = Number(asUint(w(3)));
    const cadence = every % 86400 === 0 ? `${every / 86400} day${every === 86400 ? "" : "s"}` : `${Math.round(every / 3600)} hours`;
    return { known: true,
             action: `Set a standing order: ${usdc(asUint(w(1)))} every ${cadence}, first on ${new Date(first * 1000).toISOString().slice(0, 10)}, to`,
             payees: [asAddress(w(0))],
             note: "anyone may trigger it on its due date, inside your caps; ending it is instant" };
  }
  if (sel === selector("setGuardians(address[],uint8)") && words >= 3) {
    const t = Number(asUint(w(1)));
    const n = Number(asUint(w(2)));
    if (asUint(w(0)) !== 64n || words !== 3 + n) return { known: false, selector: sel };
    const payees = [...Array(n).keys()].map((i) => asAddress(w(3 + i)));
    if (n === 0) return { known: true, action: "Remove all guardians", payees: [], note: "only your recovery address, if any, can then propose a replacement key" };
    return { known: true, action: `Name ${n} guardian${n === 1 ? "" : "s"}; ${t} of them may together propose a replacement for your passkey`, payees,
             note: "a replacement waits the recovery delay and you can cancel it until then; naming them does not let them spend" };
  }
  if (sel === selector("renewAgent(uint64)") && words === 1) {
    const until = Number(asUint(w(0)));
    return { known: true, action: `Let the agent keep paying until ${new Date(until * 1000).toISOString().slice(0, 10)}`, payees: [],
             note: "the agent's authority lapses on that date unless you renew again; an earlier date is your brake" };
  }
  if (sel === selector("removeSchedule(uint256)") && words === 1) {
    return { known: true, action: `End standing order #${asUint(w(0))}`, payees: [], note: "stops future payments of that order at once" };
  }
  return { known: false, selector: sel };
}

// The policy's fingerprint() over the terms an approval rests on, recomputed
// here from the fields the service hands the page, so a bound approval is
// signed against what this page shows and nothing else (PLAN 5.23):
// keccak256(abi.encode(perActionCap, dailyCap, reserveFloor, uint8 mode,
// agent, operator, delay, agentExpiry, pending.eta, pending.kinds)).
function termsFingerprint(t) {
  const word = (v) => {
    const n = typeof v === "bigint" ? v : BigInt(String(v).startsWith("0x") ? v : String(v));
    return n.toString(16).padStart(64, "0");
  };
  const addr = (a) => a.toLowerCase().replace(/^0x/, "").padStart(64, "0");
  const enc = word(t.perActionCap) + word(t.dailyCap) + word(t.reserveFloor) + word(t.mode) + addr(t.agent) + addr(t.operator)
    + word(t.delay) + word(t.agentExpiry) + word(t.pendingEta) + word(t.pendingKinds);
  return hex(keccak256(bytesOf("0x" + enc)));
}

const TameionCall = { keccak256, hex, bytesOf, selector, challengeFor, describe, termsFingerprint, CHAINS };
if (typeof module !== "undefined") module.exports = TameionCall;
if (typeof window !== "undefined") window.TameionCall = TameionCall;
})();
