// What the owner's passkey signs, decided in the page and never taken from the
// service (console audit 2026-10-15, docs/60 P-72). A hostile or buggy service
// answer must not make the owner sign something other than what the button
// says: every off-chain challenge is computed here from values the page chose,
// and every on-chain call is decoded and held against the button's expectation.
// Out of scope: a host that serves different JavaScript can change this file.
// Pure (viem only), so `node --test` runs it.
import { decodeFunctionData, encodeFunctionData, encodePacked, hashTypedData, keccak256, parseAbi, parseUnits, type Address, type Hex } from "viem";

// ---- off-chain challenges: byte for byte the Rust in crates/operator ----------
const len = (s: string) => new TextEncoder().encode(s).length;
const addr = (a: string) => a.toLowerCase() as Address;

/** level1::token_challenge — mint an agent key. */
export const tokenChallenge = (chainId: number, policy: string, scopes: string[], label: string, expires: number, issued: number) => {
  const j = scopes.join(",");
  return keccak256(
    encodePacked(
      ["string", "uint64", "address", "uint32", "string", "uint32", "string", "uint64", "uint64"],
      ["tameion.token.v1", BigInt(chainId), addr(policy), len(j), j, len(label), label, BigInt(expires), BigInt(issued)],
    ),
  );
};

/** desk::party_challenge — approve a counterparty on the desk (docs/89 M5). */
export const deskPartyChallenge = (chainId: number, policy: string, name: string, address: string, issued: number) =>
  keccak256(
    encodePacked(
      ["string", "uint64", "address", "uint32", "string", "address", "uint64"],
      ["tameion.desk.party.v1", BigInt(chainId), addr(policy), len(name), name, addr(address), BigInt(issued)],
    ),
  );

/** The kinds of step in the desk's order (desk::kinds_mask: bought = 1 … sold = 64). */
export const DESK_KINDS = ["bought", "shipped", "arrived", "to_fabrication", "consigned", "returned", "sold"] as const;
export type DeskKind = (typeof DESK_KINDS)[number];

/** desk::party_challenge_v2 — approve a counterparty for some kinds of step only (docs/91 R7). */
export const deskPartyChallengeV2 = (chainId: number, policy: string, name: string, address: string, kinds: readonly DeskKind[], issued: number) =>
  keccak256(
    encodePacked(
      ["string", "uint64", "address", "uint32", "string", "address", "uint8", "uint64"],
      [
        "tameion.desk.party.v2",
        BigInt(chainId),
        addr(policy),
        len(name),
        name,
        addr(address),
        DESK_KINDS.reduce((m, k, i) => (kinds.includes(k) ? m | (1 << i) : m), 0),
        BigInt(issued),
      ],
    ),
  );

/** desk::base_challenge — the stock kept unhedged (docs/89 M5). */
export const deskBaseChallenge = (chainId: number, policy: string, goods: string, grams: string, issued: number) =>
  keccak256(
    encodePacked(
      ["string", "uint64", "address", "uint32", "string", "uint32", "string", "uint64"],
      ["tameion.desk.base.v1", BigInt(chainId), addr(policy), len(goods), goods, len(grams), grams, BigInt(issued)],
    ),
  );

/** desk::control_challenge — who holds the venue's master key (docs/89 M5). */
export const deskControlChallenge = (chainId: number, policy: string, text: string, issued: number) =>
  keccak256(encodePacked(["string", "uint64", "address", "uint32", "string", "uint64"], ["tameion.desk.control.v1", BigInt(chainId), addr(policy), len(text), text, BigInt(issued)]));

/** desk::offset_challenge — release an offset larger than the desk may move by itself (docs/91 R6): the target short, as shown. */
export const deskOffsetChallenge = (chainId: number, policy: string, target: string, issued: number) =>
  keccak256(encodePacked(["string", "uint64", "address", "uint32", "string", "uint64"], ["tameion.desk.offset.v1", BigInt(chainId), addr(policy), len(target), target, BigInt(issued)]));

/** desk::venue_challenge — let the desk trade on the exchange account that approved this Mac's trade-only key. */
export const deskVenueChallenge = (chainId: number, policy: string, account: string, issued: number) =>
  keccak256(encodePacked(["string", "uint64", "address", "address", "uint64"], ["tameion.desk.venue.v1", BigInt(chainId), addr(policy), addr(account), BigInt(issued)]));

/** A step as the desk books it (desk::Milestone), for its EIP-712 digest. */
export type DeskStep = { deal: string; kind: string; goods: string; quantity: string; usdc?: string | null; counterparty: string; document: string; at: number };

/** desk::event_digest — the step's EIP-712 ShipmentEvent digest, bound to this desk (domain v2). */
export const deskStepDigest = (chainId: number, policy: string, m: DeskStep) =>
  hashTypedData({
    domain: { name: "Tameion Desk", version: "2", chainId, verifyingContract: addr(policy) },
    types: {
      ShipmentEvent: [
        { name: "deal", type: "string" },
        { name: "kind", type: "string" },
        { name: "goods", type: "string" },
        { name: "quantity", type: "string" },
        { name: "usdc", type: "string" },
        { name: "counterparty", type: "string" },
        { name: "document", type: "string" },
        { name: "at", type: "uint64" },
      ],
    },
    primaryType: "ShipmentEvent",
    message: { ...m, usdc: m.usdc ?? "", at: BigInt(m.at) },
  });

/** desk::step_challenge — a step of the owner's own (docs/89 M5). */
export const deskStepChallenge = (chainId: number, policy: string, m: DeskStep, issued: number) =>
  keccak256(encodePacked(["string", "uint64", "address", "bytes32", "uint64"], ["tameion.desk.step.v1", BigInt(chainId), addr(policy), deskStepDigest(chainId, policy, m), BigInt(issued)]));

/** level1::token_revoke_challenge — revoke agent key `id` (no length prefix on the id). */
export const tokenRevokeChallenge = (chainId: number, policy: string, id: string, issued: number) =>
  keccak256(encodePacked(["string", "uint64", "address", "string", "uint64"], ["tameion.token-revoke.v1", BigInt(chainId), addr(policy), id, BigInt(issued)]));

/** level1::notify_challenge — where the agent's reports go. */
export const notifyChallenge = (chainId: number, policy: string, url: string, chat: string, issued: number) =>
  keccak256(
    encodePacked(
      ["string", "uint64", "address", "uint32", "string", "uint32", "string", "uint64"],
      ["tameion.notify.v1", BigInt(chainId), addr(policy), len(url), url, len(chat), chat, BigInt(issued)],
    ),
  );

/** level1::reserve_token_challenge — a new reserve token for a reserved account
 *  on a device that holds its passkey but not the token (docs/60 P-87). */
export const reserveTokenChallenge = (chainId: number, policy: string, issued: number) =>
  keccak256(encodePacked(["string", "uint64", "address", "uint64"], ["tameion.reserve-token.v1", BigInt(chainId), addr(policy), BigInt(issued)]));

/** push::challenge — add this browser's push subscription. */
export const pushChallenge = (chainId: number, policy: string, endpoint: string, p256dh: string, auth: string, issued: number) =>
  keccak256(
    encodePacked(
      ["string", "uint64", "address", "uint32", "string", "uint32", "string", "uint32", "string", "uint64"],
      ["tameion.push.v1", BigInt(chainId), addr(policy), len(endpoint), endpoint, len(p256dh), p256dh, len(auth), auth, BigInt(issued)],
    ),
  );

/** level1::invoice_decide_challenge — accept (1) or decline (0) invoice `id`. */
export const invoiceDecideChallenge = (chainId: number, policy: string, id: string, accept: boolean, issued: number) =>
  keccak256(
    encodePacked(["string", "uint64", "address", "bytes32", "uint8", "uint64"], ["tameion.invoice-decide.v1", BigInt(chainId), addr(policy), id as Hex, accept ? 1 : 0, BigInt(issued)]),
  );

// ---- on-chain calls: what each button may sign ----------------------------------
/** Every call the owner's passkey makes (the same set /level1/call.js describes). */
export const CALLS = parseAbi([
  "function setPayee(address payee, bool attested)",
  "function setPayees(address[] payees, bool attested)",
  "function setPayeesIf(address[] payees, bool attested, bytes32 terms, uint64 deadline)",
  "function settleAsPrincipal(address to, uint256 usdcUnits, bytes32 actionHash)",
  "function settleAsPrincipalIf(address to, uint256 usdcUnits, bytes32 actionHash, bytes32 terms, uint64 deadline)",
  "function settleRunIf(address[] to, uint256[] usdcUnits, bytes32[] actionHashes, bytes32 terms, uint64 deadline)",
  "function withdraw(address to, uint256 usdcUnits)",
  "function cancelPending()",
  "function setMode(uint8 next)",
  "function setLimits(uint256 perActionCap, uint256 dailyCap, uint256 reserveFloor)",
  "function addSchedule(address payee, uint256 amount, uint64 every, uint64 firstDue)",
  "function removeSchedule(uint256 id)",
  "function renewAgent(uint64 until)",
  "function setCoOwners(bytes32[] xs, bytes32[] ys, uint8 threshold)",
  "function setGuardians(address[] list, uint8 threshold)",
  "function setRecovery(address next)",
  "function cancelRecovery()",
]);
export type CallName = (typeof CALLS)[number]["name"];
/** Calls on the owner's passkey contract; every other one goes to the policy. */
const ON_OWNER: CallName[] = ["setCoOwners", "setGuardians", "setRecovery", "cancelRecovery"];
export const PAY: CallName[] = ["settleAsPrincipal", "settleAsPrincipalIf"];
export const PAYEES: CallName[] = ["setPayee", "setPayees", "setPayeesIf"];
export const ANY: CallName[] = CALLS.map((c) => c.name);

/** Calldata the page builds itself (viem), to compare byte for byte. */
export const build = <N extends CallName>(functionName: N, args: readonly unknown[]) =>
  encodeFunctionData({ abi: CALLS, functionName, args } as Parameters<typeof encodeFunctionData>[0]);

/** "0.200000 USDC" -> 200000n (USDC has 6 decimals). */
export const minor = (s?: string | null) => parseUnits((s || "").replace(" USDC", "").trim() || "0", 6);

export type Row = { to: string; amount?: bigint };
export type Expect = {
  /** the function(s) this button may sign */
  fn: readonly CallName[];
  /** the calldata, when the page knows every argument: must match byte for byte */
  data?: Hex;
  /** the payees (and amounts) the call must name, in any order, no more, no fewer */
  rows?: readonly Row[];
  /** or: every payee row in the call must be one of these (a reviewed run) */
  within?: readonly Row[];
  /** setPayee*: true approves the payees, false stops them */
  attested?: boolean;
};

export function decodeCall(data: string) {
  try {
    return decodeFunctionData({ abi: CALLS, data: data as Hex });
  } catch {
    return null;
  }
}

/** The (payee, amount) rows a call names. */
export function rowsOf(c: NonNullable<ReturnType<typeof decodeCall>>): Row[] {
  const a = (c.args ?? []) as readonly unknown[];
  switch (c.functionName) {
    case "setPayee":
      return [{ to: a[0] as string }];
    case "setPayees":
    case "setPayeesIf":
      return (a[0] as string[]).map((to) => ({ to }));
    case "settleAsPrincipal":
    case "settleAsPrincipalIf":
    case "withdraw":
    case "addSchedule":
      return [{ to: a[0] as string, amount: a[1] as bigint }];
    case "settleRunIf":
      return (a[0] as string[]).map((to, i) => ({ to, amount: (a[1] as bigint[])[i] }));
    default:
      return [];
  }
}

const key = (r: Row, amounts: boolean) => r.to.toLowerCase() + (amounts ? `:${r.amount ?? ""}` : "");

/** Throws, in words, unless the call is what the button says. */
export function checkCall(acct: { policy: string; owner: string }, target: string, data: string, x: Expect) {
  const c = decodeCall(data);
  if (!c) throw new Error("Not signing: this page cannot read the call it was handed.");
  if (!x.fn.includes(c.functionName)) throw new Error(`Not signing: this button signs ${x.fn.join(" or ")}, but the call handed to it is ${c.functionName}.`);
  const want = ON_OWNER.includes(c.functionName) ? acct.owner : acct.policy;
  if (target.toLowerCase() !== want.toLowerCase()) throw new Error("Not signing: the call goes to a contract that is not this account's.");
  if (x.data && x.data.toLowerCase() !== data.toLowerCase()) throw new Error("Not signing: the call differs from the one this page built from what it shows.");
  if (x.attested !== undefined && PAYEES.includes(c.functionName) && (c.args as readonly unknown[])[1] !== x.attested)
    throw new Error(`Not signing: the call would ${x.attested ? "stop" : "approve"} the payee, not ${x.attested ? "approve" : "stop"} it.`);
  const got = rowsOf(c);
  if (x.rows) {
    const amounts = x.rows.some((r) => r.amount !== undefined);
    const a = got.map((r) => key(r, amounts)).sort();
    const b = x.rows.map((r) => key(r, amounts)).sort();
    if (a.length !== b.length || a.some((k, i) => k !== b[i])) throw new Error("Not signing: the call pays or approves a different payee or amount than the one on this page.");
  }
  if (x.within) {
    const ok = new Set(x.within.map((r) => key(r, true)));
    if (!got.length || got.some((r) => !ok.has(key(r, true)))) throw new Error("Not signing: the call pays a bill that is not on this page.");
  }
  return c;
}

/** A pairing code for a passkey: 48 bits of keccak256(qx ‖ qy) in three groups.
 *  Same function in apps/classic/home.html, so both pages show the same code. */
export const pairCode = (qx: string, qy: string) => {
  const x = keccak256(`0x${qx.replace(/^0x/, "")}${qy.replace(/^0x/, "")}` as Hex).slice(2, 14).toUpperCase();
  return `${x.slice(0, 4)}-${x.slice(4, 8)}-${x.slice(8, 12)}`;
};

/** Only an https link opens from a button (a service-supplied `javascript:` must not). */
export const safeHref = (h?: string | null) => (h && /^https:\/\//i.test(h) ? h : undefined);
