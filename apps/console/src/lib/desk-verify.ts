// A lender checks a desk's kit in the browser (docs/83 W7): the same checks as
// `scripts/verify_desk.py`, in viem, against two public Arbitrum Sepolia nodes and
// Hyperliquid's public API — never against the desk's own service. The kit is a file
// the trader sends; this page only reads it.
// Limit: this code is served by the same site; `scripts/verify_desk.py` from a clone
// of the repository is the check that trusts nothing here.
import { concat, createPublicClient, http, keccak256, recoverTypedDataAddress, toHex, type Hex } from "viem";
import { ANCHORS, NODES } from "./verify.ts";

const HL = "https://api.hyperliquid-testnet.xyz/info";
/** The venue market that prices each good, pinned here, never taken from the statement (docs/92 T2). */
const MARKET: Record<string, string> = { gold: "PAXG" };
const marketOf = (st: DeskStatement) => MARKET[st.goods] ?? "";

export type DeskMilestone = {
  deal: string;
  kind: string;
  goods: string;
  quantity: string;
  usdc?: string | null;
  counterparty: string;
  document: string;
  signer?: string;
  at: number;
};

export type ClassLine = { grams: string; valueUsdc: string };

export type DeskStatement = {
  deal: string;
  goods: string;
  market: string;
  stage: string;
  held: string;
  heldContracts: string;
  byStage?: { stage: string; words: string; class: string; grams: string }[];
  eligible?: ClassLine;
  consigned?: ClassLine;
  ineligible?: ClassLine;
  unhedgedBase?: string;
  costUsdc: string;
  markUsdc: string;
  valueUsdc: string;
  offsetContracts: string;
  offsetShareBps: string;
  stressBps: number;
  basisBps: number;
  maxLossUsdc: string;
  eligibleUsdc: string;
  venueAccount: string;
  hedgeControl?: string;
  at: number;
  root: Hex;
  assumptions: string;
  [k: string]: unknown;
};

export type LenderKit = {
  signed: { statement: DeskStatement; statementHash: Hex; signer: Hex; signature: Hex; domain: { chainId: number } };
  disclosure: { root: Hex; leaves: number; deal: string; items: { index: number; canonical: string; path: { sibling: Hex; left: boolean }[] }[] };
  milestones?: { milestone: DeskMilestone; signature: string }[];
  parties?: { name: string; address: string }[];
  anchor: { chainId: number; contract: Hex; tx: Hex; from: Hex };
  /** The desk every counterparty step is bound to (EIP-712 verifyingContract, version 2). */
  desk?: Hex;
  /** For each earlier statement of this desk: its book size, root, and the RFC 9162 path to this root. */
  history?: { leaves: number; root: Hex; consistency: Hex[] }[];
};

export type Check = {
  id: string;
  title: string;
  ok: boolean;
  lines: string[];
  /** check e: the account's short now, oz */
  shortNow?: number;
  /** Failed only because a sale or a smaller offset came after the statement (docs/101 Y2): out of date, not forged. */
  outdated?: boolean;
};

/** RFC 9162 §2.1.4.2: the tree of `n` leaves (root `second`) extends the tree of its first `m` (root `first`). */
export function extendsTree(first: Hex, m: number, second: Hex, n: number, path: Hex[]): boolean {
  const eq = (a: Hex, b: Hex) => a.toLowerCase() === b.toLowerCase();
  if (m === n) return path.length === 0 && eq(first, second);
  if (!(m > 0 && m < n) || path.length === 0) return false;
  const p = (m & (m - 1)) === 0 ? [first, ...path] : path;
  let [fn, sn] = [m - 1, n - 1];
  while (fn & 1) [fn, sn] = [fn >>> 1, sn >>> 1];
  let [fr, sr] = [p[0], p[0]];
  for (const c of p.slice(1)) {
    if (sn === 0) return false;
    if (fn & 1 || fn === sn) {
      [fr, sr] = [nodeOf(c, fr), nodeOf(c, sr)];
      while (fn !== 0 && !(fn & 1)) [fn, sn] = [fn >>> 1, sn >>> 1];
    } else sr = nodeOf(sr, c);
    [fn, sn] = [fn >>> 1, sn >>> 1];
  }
  return eq(fr, first) && eq(sr, second) && sn === 0;
}

/** Check n (docs/89 H5 (3)): the book now extends the book at an earlier statement the lender already holds. */
export function checkHistory(kit: LenderKit, previous: LenderKit): Check {
  const m = previous.disclosure.leaves;
  const first = previous.signed.statement.root;
  const entry = (kit.history ?? []).find((h) => h.leaves === m && h.root.toLowerCase() === first.toLowerCase());
  // The same deal: every step the earlier statement showed must be shown again (docs/92 T1).
  const shownNow = new Set(kit.disclosure.items.map((i) => `${i.index}\n${i.canonical}`));
  const missing =
    previous.signed.statement.deal === kit.signed.statement.deal ? previous.disclosure.items.filter((i) => !shownNow.has(`${i.index}\n${i.canonical}`)) : [];
  const ok =
    !!entry &&
    missing.length === 0 &&
    (previous.desk ?? "").toLowerCase() === (kit.desk ?? "").toLowerCase() &&
    extendsTree(first, m, kit.signed.statement.root, kit.disclosure.leaves, entry.consistency);
  return {
    id: "n",
    title: "Nothing was removed since your earlier statement",
    ok,
    lines: [
      `The book of ${kit.disclosure.leaves} steps ${ok ? "extends" : "does NOT extend"} the book of ${m} steps at your statement on ${previous.signed.statement.deal}.`,
      ...(entry ? [] : ["The new file carries no proof from that statement."]),
      ...missing.map((i) => `Step ${i.index}, shown in your earlier statement, is missing from this one.`),
    ],
  };
}

/** Keys sorted at every level, no whitespace: what `statement::canonical_json` (Rust) and
 *  `json.dumps(sort_keys=True, separators=(",", ":"), ensure_ascii=False)` (Python) write. */
export function canonicalJson(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonicalJson).join(",")}]`;
  if (v && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonicalJson(o[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(v);
}

const SCALE: Record<string, [number, string]> = { gold: [3, "g gold"] };

/** "10.000" at scale 3 → 10000n, exactly; refuses more places than the unit has. */
export function minorUnits(dec: string, scale: number): bigint {
  const m = /^(\d+)(?:\.(\d+))?$/.exec(dec.trim());
  if (!m) throw new Error(`not a decimal: ${dec}`);
  const frac = (m[2] ?? "").replace(/0+$/, "");
  if (frac.length > scale) throw new Error(`${dec} is finer than the unit`);
  return BigInt(m[1]) * 10n ** BigInt(scale) + BigInt((frac + "0".repeat(scale)).slice(0, scale) || "0");
}

/** The bytes a desk leaf commits to (`desk::Milestone::canonical`). */
export function milestoneCanonical(m: DeskMilestone): string {
  const [scale, unit] = SCALE[m.goods] ?? (() => { throw new Error(`unknown goods ${m.goods}`); })();
  const usdc = m.usdc == null ? "-" : minorUnits(m.usdc, 6).toString();
  return `tameion-desk/v1\ndeal=${m.deal}\nkind=${m.kind}\ngoods=${m.goods}\nquantity=${minorUnits(m.quantity, scale)} ${unit}\nusdc=${usdc}\ncounterparty=${m.counterparty}\ndocument=${m.document}\nsigner=${m.signer ?? ""}\nat=${m.at}\n`;
}

const leafOf = (canonical: string) => keccak256(concat(["0x00", toHex(canonical)]));
const nodeOf = (l: Hex, r: Hex) => keccak256(concat(["0x01", l, r]));

/** Walk a disclosed leaf's path to the root from its own position (RFC 9162 §2.1.3.2): the side of each
 *  sibling comes from the index and the tree size, never from the kit's `left` flags, so a step cannot be
 *  shown at a position it does not hold (docs/91 R2). */
export function reachesRoot(canonical: string, index: number, size: number, path: { sibling: Hex }[], root: Hex): boolean {
  if (!(index >= 0 && index < size)) return false;
  let [fn, sn, r] = [index, size - 1, leafOf(canonical)];
  for (const { sibling } of path) {
    if (sn === 0) return false;
    if (fn % 2 === 1 || fn === sn) {
      r = nodeOf(sibling, r);
      while (fn !== 0 && fn % 2 === 0) [fn, sn] = [fn >>> 1, sn >>> 1];
    } else r = nodeOf(r, sibling);
    [fn, sn] = [fn >>> 1, sn >>> 1];
  }
  return sn === 0 && r.toLowerCase() === root.toLowerCase();
}

const DOMAIN = (chainId: number) => ({ name: "Tameion Desk", version: "1", chainId });
const STATEMENT_TYPES = {
  DeskStatement: [
    { name: "deal", type: "string" },
    { name: "root", type: "bytes32" },
    { name: "statementHash", type: "bytes32" },
    { name: "at", type: "uint64" },
  ],
} as const;
const EVENT_TYPES = {
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
} as const;

const same = (a?: string | null, b?: string | null) => !!a && !!b && a.toLowerCase() === b.toLowerCase();

/** Checks a, b, c and f: everything that needs no network. */
/** What a lender pins from outside the kit (docs/89 H4): the trader holds the statement key, so a kit
 *  alone proves only that it is consistent. */
export type Pins = {
  signer?: string;
  /** The desk's account, from the trader, confirmed by each counterparty (docs/96 V3). */
  desk?: string;
  venue?: string;
  parties?: Record<string, string>;
  /** The price fall and the basis the lender sizes the maximum loss at, in bps (defaults 3000, 200; docs/92 T3). */
  stressBps?: number;
  basisBps?: number;
};

/** "Name=0x…" per line → a map; blank lines ignored. */
export function parseParties(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const i = line.indexOf("=");
    if (i > 0) out[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return out;
}

const MOVES: Record<string, [string[], string]> = {
  shipped: [["bought"], "in_transit"],
  arrived: [["in_transit", "bought"], "in_stock"],
  to_fabrication: [["in_stock"], "in_fabrication"],
  consigned: [["in_stock"], "consigned"],
  returned: [["in_fabrication", "consigned"], "in_stock"],
};
const SALE_ORDER = ["in_stock", "consigned", "in_transit", "bought", "in_fabrication"];

/** Replay one deal's disclosed steps into milligrams by stage, as the desk does (check h). */
export function stagesFrom(canonicals: string[]): Record<string, bigint> {
  const st: Record<string, bigint> = Object.fromEntries(SALE_ORDER.map((s) => [s, 0n]));
  const seen = new Set<string>();
  for (const [n, c] of canonicals.entries()) {
    const f = Object.fromEntries(
      c
        .split("\n")
        .slice(1)
        .filter((l) => l.includes("="))
        .map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]),
    );
    const q = BigInt(f.quantity.split(" ")[0]);
    if ((f.kind === "bought") !== (n === 0)) throw new Error("A deal is one purchase, booked first; the desk refuses a second.");
    // The desk's own rules (docs/100 X4, docs/101 Y1, Y6): one step per kind and document number, printable ASCII.
    const doc = f.document ?? "";
    if (!/^[\x20-\x7e]+$/.test(doc)) throw new Error("A document number the desk would refuse (printable ASCII only).");
    if (seen.has(`${f.kind}\n${doc}`)) throw new Error(`${f.kind} under ${doc} twice: the desk books one step per document number.`);
    seen.add(`${f.kind}\n${doc}`);
    if (f.kind === "bought") {
      st.bought += q;
      continue;
    }
    const [src, dst] = f.kind === "sold" ? [SALE_ORDER, null] : (MOVES[f.kind] ?? (() => { throw new Error(`unknown step ${f.kind}`); })());
    let left = q;
    for (const s of src) {
      const take = left < st[s] ? left : st[s];
      st[s] -= take;
      left -= take;
    }
    if (left > 0n) throw new Error("a step moves more than the deal holds");
    if (dst) st[dst] += q;
  }
  return st;
}

const OZ = 31.1034768; // grams per troy ounce: one PAXG contract
const CLASSES: Record<string, string[]> = { eligible: ["in_stock", "in_transit"], consigned: ["consigned"], ineligible: ["bought", "in_fabrication"] };

/** Check m (docs/91 R1): every money figure a lender lends on, recomputed from the replayed grams, the
 *  statement's mark and its offset, to within a cent (the statement rounds the offset to 6 places). */
export function moneyMisses(st: DeskStatement, by: Record<string, bigint>): string[] {
  const mark = Number(st.markUsdc);
  const off = Number(st.offsetContracts);
  const oz = Object.fromEntries(Object.entries(CLASSES).map(([c, ss]) => [c, ss.reduce((a, s) => a + Number(by[s]), 0) / 1000 / OZ]));
  const held = oz.eligible + oz.consigned + oz.ineligible;
  const value = held * mark;
  const maxLoss = ((held - off) * mark * st.stressBps) / 10000 + (value * st.basisBps) / 10000;
  const eligibleValue = Math.round(oz.eligible * mark * 1e6) / 1e6;
  const want: [string, number, number, number][] = [
    ["heldContracts", Number(st.heldContracts), held, 1e-6],
    ["valueUsdc", Number(st.valueUsdc), value, 0.01],
    ["maxLossUsdc", Number(st.maxLossUsdc), maxLoss, 0.01],
    ["eligibleUsdc", Number(st.eligibleUsdc), Math.max(0, eligibleValue - maxLoss), 0.01],
    // Below ~0.005 oz the 6-place offset moves the share by more than a bps (docs/94 U6).
    ["offsetShareBps", Number(st.offsetShareBps), held ? (off / held) * 10000 : 0, held ? 1 + (10000 * 5e-7) / held : 1],
    ...Object.keys(CLASSES).map((c): [string, number, number, number] => [`${c} value`, Number((st[c] as ClassLine | undefined)?.valueUsdc ?? 0), oz[c] * mark, 0.01]),
  ];
  const miss = off > held + 1e-6 ? [`The offset ${st.offsetContracts} oz is more than the ${held.toFixed(6)} oz held.`] : [];
  // The deal's share of one short (docs/92 T4): short × held ÷ the desk's whole holding, never more than held.
  if (typeof st.shortContracts !== "string" || typeof st.deskHeldContracts !== "string") {
    miss.push("The statement does not say the short it read or the desk's whole holding; ask for a new one.");
  } else {
    const [short, deskHeld] = [Number(st.shortContracts), Number(st.deskHeldContracts)];
    const wantOff = deskHeld > 0 ? Math.min((short * held) / deskHeld, held) : 0;
    if (deskHeld + 1e-6 < held) miss.push(`The desk holds ${st.deskHeldContracts} oz in all, less than this deal's ${held.toFixed(6)}.`);
    if (!(Math.abs(off - wantOff) <= 2e-6)) miss.push(`offsetContracts ${st.offsetContracts}; this deal's share of the ${st.shortContracts} short is ${wantOff.toFixed(6)}.`);
    // The venue's short bounds the desk's stated total from below (one lot of slack), so a total lowered to hide stock,
    // or a short beyond the stock, is refused (docs/100 X1).
    if (minorUnits(st.shortContracts, 6) > minorUnits(st.deskHeldContracts, 6) + 1000n)
      miss.push(`The account is short ${st.shortContracts} oz, more than the ${st.deskHeldContracts} oz the desk says it holds: stock is left out of the desk's total, or the account is short beyond the stock.`);
  }
  for (const [k, got, v, tol] of want) if (!(Math.abs(got - v) <= tol)) miss.push(`${k} ${got}; the steps give ${v.toFixed(6)}.`);
  return miss;
}

/** The numbers a check compares must be numbers (docs/94 U3): a position written as text sorts as text. */
export function shapeError(kit: LenderKit): string | null {
  const st = kit.signed?.statement;
  const ints: [string, unknown][] = [
    ["leaves", kit.disclosure?.leaves],
    ["at", st?.at],
    ["stressBps", st?.stressBps],
    ["basisBps", st?.basisBps],
    ...(kit.disclosure?.items ?? []).map((i, n): [string, unknown] => [`items[${n}].index`, i.index]),
    ...(kit.milestones ?? []).map((m, n): [string, unknown] => [`milestones[${n}].at`, m.milestone?.at]),
  ];
  const bad = ints.find(([, v]) => !Number.isSafeInteger(v));
  return bad ? `${bad[0]} is not a whole number` : null;
}

export async function checkOffline(kit: LenderKit, pinsOrSigner?: Pins | string): Promise<Check[]> {
  const shape = shapeError(kit);
  if (shape) return [{ id: "c", title: "The file is a well-formed statement", ok: false, lines: [`Refused before any check: ${shape}.`] }];
  const pins: Pins = typeof pinsOrSigner === "string" ? { signer: pinsOrSigner } : (pinsOrSigner ?? {});
  const expectedSigner = pins.signer;
  const st = kit.signed.statement;
  const chainId = kit.anchor.chainId;
  const out: Check[] = [];

  const hash = keccak256(toHex(canonicalJson(st)));
  out.push({
    id: "a",
    title: "The numbers are the ones that were signed",
    ok: same(hash, kit.signed.statementHash),
    lines: [`Recomputed ${hash.slice(0, 18)}…; the kit says ${kit.signed.statementHash.slice(0, 18)}…`],
  });

  let recovered = "";
  try {
    recovered = await recoverTypedDataAddress({
      domain: DOMAIN(chainId),
      types: STATEMENT_TYPES,
      primaryType: "DeskStatement",
      message: { deal: st.deal, root: st.root, statementHash: kit.signed.statementHash, at: BigInt(st.at) },
      signature: kit.signed.signature,
    });
  } catch {
    /* recovered stays empty */
  }
  const want = expectedSigner || kit.signed.signer;
  out.push({
    id: "b",
    title: "Signed by the desk's statement key",
    ok: same(recovered, want),
    lines: [
      recovered ? `The signature recovers to ${recovered}.` : "The signature does not recover.",
      expectedSigner ? `You pinned ${expectedSigner}.` : `The kit names ${kit.signed.signer}; pin the key the desk publishes to be sure.`,
    ],
  });

  const items = kit.disclosure.items;
  const reach = items.map((it) => reachesRoot(it.canonical, it.index, kit.disclosure.leaves, it.path, st.root));
  // Each position once, in the book's order (docs/91 R2): a step shown twice or out of order is refused.
  const ordered = items.every((it, i) => i === 0 || it.index > items[i - 1].index);
  out.push({
    id: "c",
    title: "Every step shown belongs to the book that was anchored",
    ok: items.length > 0 && ordered && same(kit.disclosure.root, st.root) && reach.every(Boolean) && items.every((it) => it.canonical.includes(`\ndeal=${st.deal}\n`)),
    lines: [
      `${items.length} of ${kit.disclosure.leaves} steps of the desk are shown, at positions ${items.map((i) => i.index).join(", ")}; ${reach.filter(Boolean).length} reach the root from their own position.`,
      ...(ordered ? [] : ["A position is shown twice or out of order."]),
    ],
  });

  // The lender's own list when given; otherwise the kit's (then f is consistent, not pinned).
  // "Vault=0x…:arrived,returned" pins the role too (docs/91 R7).
  const parties = new Map(
    pins.parties && Object.keys(pins.parties).length
      ? Object.entries(pins.parties).map(([n, a]) => [n, a.split(":")[0].toLowerCase()])
      : (kit.parties ?? []).map((p) => [p.name, p.address.toLowerCase()]),
  );
  const roles = new Map(
    Object.entries(pins.parties ?? {})
      .filter(([, a]) => a.includes(":"))
      .map(([n, a]) => [n, a.split(":")[1].split(",").map((k) => k.trim())]),
  );
  const signed = kit.milestones ?? [];
  const fLines: string[] = [];
  // The desk the steps are signed for is the one the lender set up, not the one the kit names (docs/96 V3).
  const desk = (pins.desk || kit.desk || "").toLowerCase();
  let fOk = signed.length === items.length && signed.length > 0 && !!kit.desk && same(desk, kit.desk);
  if (kit.desk && !same(desk, kit.desk)) fLines.push(`This file is for desk ${kit.desk}, not the one you set up (${desk}).`);
  for (let i = 0; i < signed.length; i++) {
    const { milestone: m, signature } = signed[i];
    let reencodes = false;
    try {
      reencodes = milestoneCanonical(m) === items[i]?.canonical;
    } catch {
      reencodes = false;
    }
    if (!signature) {
      // A step that names a signer must carry the signature (docs/89 L3).
      fOk &&= reencodes && !m.signer;
      fLines.push(
        reencodes
          ? `${m.kind} ${m.quantity} g · ${m.document}: entered by the trader, unsigned — no counterparty vouches for it.`
          : `${m.kind} ${m.quantity} g · ${m.document}: does NOT match the step shown above.`,
      );
      continue;
    }
    let who = "";
    try {
      if (!desk) throw new Error("no desk");
      who = await recoverTypedDataAddress({
        domain: { name: "Tameion Desk", version: "2", chainId, verifyingContract: desk as Hex },
        types: EVENT_TYPES,
        primaryType: "ShipmentEvent",
        message: { deal: m.deal, kind: m.kind, goods: m.goods, quantity: m.quantity, usdc: m.usdc ?? "", counterparty: m.counterparty, document: m.document, at: BigInt(m.at) },
        signature: signature as Hex,
      });
    } catch {
      /* who stays empty */
    }
    const approved = parties.get(m.counterparty);
    const role = roles.get(m.counterparty);
    const inRole = !role || role.includes(m.kind);
    const ok = reencodes && same(who, approved) && same(who, m.signer) && inRole;
    fOk &&= ok;
    fLines.push(
      `${m.kind} ${m.quantity} g · ${m.document}: ${
        !reencodes
          ? "does NOT match the step shown above"
          : !inRole
            ? `${m.counterparty} may sign ${role!.join(", ")}, not ${m.kind}`
            : ok
              ? `${m.counterparty} signed it (${who})`
              : `NOT signed by ${m.counterparty}'s approved key`
      }.`,
    );
  }
  // docs/91 R10: goods that came into stock or transit on the trader's own word are said in the check, not only per step.
  const unsignedIn = signed
    .filter((s) => !s.signature && ["shipped", "arrived", "returned"].includes(s.milestone.kind))
    .reduce((a, s) => a + Number(s.milestone.quantity), 0);
  if (unsignedIn > 0) fLines.push(`${unsignedIn.toFixed(3)} g came into stock or transit on steps no counterparty signed: count them only on the trader's word.`);
  out.push({ id: "f", title: "Each counterparty signed its own step", ok: fOk, lines: fLines.length ? fLines : ["The kit carries no signed steps."] });

  // h. the statement's grams are what the disclosed steps add up to (docs/89 H5).
  try {
    const by = stagesFrom(items.map((i) => i.canonical));
    const claimed: Record<string, bigint> = Object.fromEntries((st.byStage ?? []).map((l) => [l.stage, minorUnits(l.grams.split(" ")[0], 3)]));
    const total = Object.values(by).reduce((a, b) => a + b, 0n);
    // The statement's own list of steps is the disclosed one (docs/89 H5 (1)): same steps, same order.
    const key = (m: Record<string, unknown>) => ["kind", "quantity", "document", "counterparty", "signer", "at"].map((k) => String(m[k] ?? "")).join("\n");
    const listed = ((st.milestones as Record<string, unknown>[] | undefined) ?? []).map(key);
    const sameList = listed.length === signed.length && signed.every((s, i) => key(s.milestone as unknown as Record<string, unknown>) === listed[i]);
    // The words a lender reads as facts follow the steps too: the stage and each class's grams (docs/94 U4).
    const WORDS: Record<string, string> = { bought: "bought, not delivered", in_transit: "in transit", in_stock: "in stock", in_fabrication: "in fabrication", consigned: "consigned" };
    let stage = "sold";
    let most = 0n;
    for (const k of ["bought", "in_transit", "in_stock", "in_fabrication", "consigned"]) if (by[k] > 0n && by[k] >= most) [stage, most] = [WORDS[k], by[k]];
    const classOk = Object.entries(CLASSES).every(([c, ss]) => minorUnits(((st[c] as ClassLine | undefined)?.grams ?? "0").split(" ")[0], 3) === ss.reduce((a, x) => a + by[x], 0n));
    const ok = Object.keys(by).every((s) => by[s] === (claimed[s] ?? 0n)) && total === minorUnits(st.held.split(" ")[0], 3) && sameList && st.stage === stage && classOk;
    const shown = Object.entries(by).filter(([, v]) => v > 0n).map(([k, v]) => `${k.replace("_", " ")} ${(Number(v) / 1000).toFixed(3)} g`);
    out.push({
      id: "h",
      title: "The grams are what the steps add up to",
      ok,
      lines: [
        `From the steps: ${shown.join(", ") || "nothing held"}.`,
        ...(sameList ? [] : ["The statement lists other steps than the ones shown."]),
        ...(st.stage === stage ? [] : [`The statement says "${st.stage}"; the steps say "${stage}".`]),
        ...(classOk ? [] : ["The grams by class (eligible, consigned, not eligible) are not what the steps add up to."]),
      ],
    });
    const miss = moneyMisses(st, by);
    // The market is pinned to the goods; stress and basis are the lender's floor; the cost is the signed purchase (docs/92 T2, T3).
    if (!marketOf(st) || st.market !== marketOf(st)) miss.push(`The statement prices ${st.goods} on ${st.market}; ${st.goods} is priced on ${marketOf(st) || "no market this page knows"}.`);
    const field = (c: string, k: string) => c.split("\n").find((l) => l.startsWith(`${k}=`))?.slice(k.length + 1);
    if (items.some((i) => field(i.canonical, "goods") !== st.goods)) miss.push("A disclosed step is for other goods than the statement's.");
    const [minStress, minBasis] = [pins.stressBps ?? 3000, pins.basisBps ?? 200];
    if (st.stressBps < minStress || st.basisBps < minBasis)
      miss.push(`The maximum loss is sized at a ${st.stressBps / 100}% fall and ${st.basisBps / 100}% basis; you asked for at least ${minStress / 100}% and ${minBasis / 100}%.`);
    const bought = items.find((i) => field(i.canonical, "kind") === "bought");
    const usdc = bought && field(bought.canonical, "usdc");
    if (usdc && usdc !== "-") {
      const micro = BigInt(usdc);
      const want = `${micro / 1_000_000n}.${(micro % 1_000_000n).toString().padStart(6, "0")} USDC`;
      if (st.costUsdc !== want) miss.push(`costUsdc ${st.costUsdc}; the signed purchase says ${want}.`);
    }
    out.push({
      id: "m",
      title: "The money follows from the grams",
      ok: miss.length === 0,
      lines: miss.length ? miss : [`Value, each class, the offset share, the maximum loss and the eligible value recomputed at ${st.markUsdc} per ounce.`],
    });
  } catch (e) {
    out.push({ id: "h", title: "The grams are what the steps add up to", ok: false, lines: [(e as Error).message] });
    out.push({ id: "m", title: "The money follows from the grams", ok: false, lines: ["No grams to compute from."] });
  }
  return out;
}

/** A whole-stock file is one moment of one desk (docs/96 V2): every kit at one root, `at`, key, desk, exchange account
 *  and totals, no deal or disclosed step twice. Null when it is; otherwise why not. */
export function bookError(kits: LenderKit[]): string | null {
  if (!kits.length) return "this is not a statement of the whole stock: it lists no deals";
  const moment = (k: LenderKit) => {
    const s = k.signed.statement;
    return [s.root, s.at, k.signed.signer, k.desk ?? "", s.venueAccount, s.deskHeldContracts, s.shortContracts].map((x) => String(x).toLowerCase()).join("|");
  };
  const idx = kits.flatMap((k) => k.disclosure.items.map((i) => i.index));
  if (new Set(kits.map(moment)).size !== 1 || new Set(kits.map((k) => k.signed.statement.deal)).size !== kits.length || new Set(idx).size !== idx.length)
    return "this is not one statement of the whole stock: its deals differ in moment, root, key, desk, exchange account or totals, or a deal or step appears twice";
  // Whole: its deals add up to the holding the desk states (docs/99 W1); each figure is printed to 6 places.
  const deskHeld = kits[0].signed.statement.deskHeldContracts;
  const heldSum = kits.reduce((a, k) => a + Number(k.signed.statement.heldContracts), 0);
  if (typeof deskHeld === "string" && Math.abs(heldSum - Number(deskHeld)) > 5e-7 * (kits.length + 1) + 1e-12)
    return `this file leaves out stock: its deals hold ${heldSum.toFixed(6)} oz, the desk says it holds ${deskHeld} oz; ask for the whole file`;
  return null;
}

/** Check w (docs/94 U1): statements of the same desk at one root, checked together: one desk total, the deals'
 *  holdings within it, their offsets within the one short. A single file cannot prove the total; several can bound it.
 *  With `shortNow` (check e's read): the account must still hold what they count (docs/96 V1), or a deal was sold. */
export function checkTogether(kit: LenderKit, others: LenderKit[], shortNow?: number): Check {
  const root = kit.signed.statement.root.toLowerCase();
  const same = [kit, ...others].filter((k) => k.signed.statement.root.toLowerCase() === root);
  const st = kit.signed.statement;
  const totals = new Set(same.map((k) => k.signed.statement.deskHeldContracts));
  const sumHeld = same.reduce((a, k) => a + Number(k.signed.statement.heldContracts), 0);
  const sumOff = same.reduce((a, k) => a + Number(k.signed.statement.offsetContracts), 0);
  const deskTotal = Number(st.deskHeldContracts ?? 0);
  const fell = shortNow !== undefined && shortNow + 0.0005 < sumOff;
  // 6-place rounding per deal (docs/99 W8).
  const tol = Math.max(2e-6, 5e-7 * (same.length + 1));
  const core = totals.size === 1 && sumHeld <= deskTotal + tol && sumOff <= Number(st.shortContracts ?? 0) + tol;
  const ok = core && !fell;
  return {
    id: "w",
    title: "The desk's statements agree with each other",
    ok,
    outdated: core && fell,
    lines: [
      `${same.length} statements at this root (of ${others.length + 1} given): ${totals.size} desk total(s); ${sumHeld.toFixed(6)} oz held against ${deskTotal.toFixed(6)}; ${sumOff.toFixed(6)} oz offset against the ${st.shortContracts ?? "?"} short${shortNow === undefined ? "" : `, ${shortNow.toFixed(6)} now`}.`,
      ...(fell ? [`Since these statements the short fell to ${shortNow!.toFixed(6)}, below the ${sumOff.toFixed(6)} oz they count: a deal was sold or the offset reduced. Ask for today's file.`] : []),
    ],
  };
}

/** Check t (docs/92 T8): how old the statement is, and that it was issued after its own last step. */
export function checkAge(kit: LenderKit, maxAgeHours = 24, nowSecs = Date.now() / 1000): Check {
  const st = kit.signed.statement;
  const ageH = (nowSecs - st.at) / 3600;
  const last = Math.max(0, ...kit.disclosure.items.map((i) => Number(i.canonical.split("\n").find((l) => l.startsWith("at="))?.slice(3) ?? 0)));
  return {
    id: "t",
    title: "The statement is recent",
    ok: ageH <= maxAgeHours && st.at >= last,
    lines: [`Issued ${new Date(st.at * 1000).toISOString().replace("T", " ").slice(0, 19)} UTC, ${ageH.toFixed(1)} h ago (at most ${maxAgeHours} h)${st.at >= last ? "" : ", before its own last step"}.`],
  };
}

/** Check p: the statement's mark is the venue's price at its time (1-minute candles, two minutes either side, ±0.5 %). */
export async function checkPrice(kit: LenderKit): Promise<Check> {
  const st = kit.signed.statement;
  const t = st.at * 1000;
  try {
    // The venue's 1-minute feed can lag a few seconds behind the minute (docs/60 P-125).
    let cs: { l: string; h: string }[] = [];
    for (const wait of [0, 5000, 15000]) {
      await new Promise((ok) => setTimeout(ok, wait));
      const r = await fetch(HL, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ type: "candleSnapshot", req: { coin: marketOf(st), interval: "1m", startTime: t - 120_000, endTime: t + 120_000 } }),
      });
      cs = (await r.json()) as { l: string; h: string }[];
      if (cs.length) break;
    }
    if (!cs.length)
      return { id: "p", title: "The price is the market's", ok: false, lines: [`Hyperliquid has no ${marketOf(st)} prices around the statement's time yet: check again in a minute.`] };
    const lo = Math.min(...cs.map((c) => Number(c.l)));
    const hi = Math.max(...cs.map((c) => Number(c.h)));
    const mark = Number(st.markUsdc);
    return {
      id: "p",
      title: "The price is the market's",
      ok: mark >= lo * 0.995 && mark <= hi * 1.005,
      lines: [`${marketOf(st)} traded ${lo}–${hi} in the two minutes around the statement; the statement uses ${st.markUsdc}.`],
    };
  } catch (e) {
    return { id: "p", title: "The price is the market's", ok: false, lines: [`Hyperliquid did not answer: ${(e as Error).message}`] };
  }
}

/** Check d: the root is in the anchoring transaction, on two public nodes. */
const OPERATOR_ABI = [{ type: "function", name: "operator", stateMutability: "view", inputs: [], outputs: [{ name: "", type: "address" }] }] as const;

export async function checkAnchor(kit: LenderKit): Promise<Check> {
  const chainKey = kit.anchor.chainId === 421614 ? "arbitrum-sepolia" : kit.anchor.chainId === 5042002 ? "arc-testnet" : "";
  const nodes = NODES[chainKey] ?? [];
  const pinned = ANCHORS[chainKey];
  const root = kit.signed.statement.root.toLowerCase().slice(2);
  const lines: string[] = [];
  let ok = nodes.length > 0 && same(pinned, kit.anchor.contract);
  if (!same(pinned, kit.anchor.contract)) lines.push(`The kit names record contract ${kit.anchor.contract}, not the one pinned here (${pinned ?? "none"}).`);
  for (const url of nodes) {
    const host = new URL(url).host;
    try {
      const c = createPublicClient({ transport: http(url) });
      // Who may anchor for this desk is the desk's own operator, read on chain, never the kit's word (docs/103 S2-1):
      // a stranger who builds a kit for a desk cannot anchor as that desk. A desk not on chain names no operator.
      const [tx, rc, operator] = await Promise.all([
        c.getTransaction({ hash: kit.anchor.tx }),
        c.getTransactionReceipt({ hash: kit.anchor.tx }),
        kit.desk ? c.readContract({ address: kit.desk, abi: OPERATOR_ABI, functionName: "operator" }).catch(() => null) : Promise.resolve(null),
      ]);
      const good = !!operator && same(tx.from, operator) && same(tx.to, kit.anchor.contract) && rc.status === "success" && tx.input.toLowerCase().endsWith(root);
      ok &&= good;
      if (!operator) lines.push(`${host}: the desk ${kit.desk ?? "(not named)"} is not on chain or names no operator, so who may anchor for it cannot be read.`);
      else if (!same(tx.from, operator)) lines.push(`${host}: the anchor was sent by ${tx.from}, not by the desk's operator ${operator}.`);
      lines.push(`${host}: block ${rc.blockNumber}, ${good ? "sent by the desk's operator, the transaction carries this root" : "does NOT match"}.`);
    } catch (e) {
      ok = false;
      lines.push(`${host}: ${(e as Error).message.split("\n")[0]}`);
    }
  }
  return { id: "d", title: "The book's root is on chain", ok, lines };
}

/** This deal's share of the account's funding, in micro-USDC, as the service computes it (docs/94 U5): its grams
 *  (exact) over the desk's holding, which is printed to 6 places, so the tolerance grows with the funding (docs/96 V5). */
export function fundingOk(st: DeskStatement, acct: number, claimed: number): boolean {
  const deskHeld = Number(st.deskHeldContracts ?? 0);
  if (!(deskHeld > 0)) return claimed === 0 || (Math.sign(claimed) === Math.sign(acct) && Math.abs(claimed) <= Math.abs(acct) + 1);
  const share = Number(st.held.split(" ")[0]) / OZ / deskHeld;
  return Math.abs(claimed - Math.round(acct * Math.min(share, 1))) <= 2 + (Math.abs(acct) * 5e-7) / deskHeld;
}

/** Check g: the funding claimed is no more than the venue's own record for the account, with the same sign. */
export async function checkFunding(kit: LenderKit, venuePin?: string): Promise<Check | null> {
  const st = kit.signed.statement;
  const claimedText = st.fundingUsdc as string | undefined;
  if (claimedText === undefined) return null;
  const first = Math.min(...(kit.milestones ?? []).map((m) => m.milestone.at), st.at);
  try {
    const r = await fetch(HL, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ type: "userFunding", user: venuePin || st.venueAccount, startTime: first * 1000, endTime: st.at * 1000 }),
    });
    const rec = (await r.json()) as { delta?: { coin?: string; usdc?: string } }[];
    // Micro-USDC as integers, signed: funding can be paid or received.
    const micro = (s: string) => (s.startsWith("-") ? -Number(minorUnits(s.slice(1), 6)) : Number(minorUnits(s, 6)));
    const acct = rec.filter((f) => f.delta?.coin === marketOf(st)).reduce((a, f) => a + micro(f.delta?.usdc ?? "0"), 0);
    const ok = fundingOk(st, acct, micro(claimedText));
    return {
      id: "g",
      title: "The cost of the offset is the exchange's own figure",
      ok,
      lines: [`Funding on the account's ${marketOf(st)} short since this deal's first step, from Hyperliquid: ${(acct / 1e6).toFixed(6)} USDC; this deal's share claimed: ${claimedText}.`],
    };
  } catch (e) {
    return { id: "g", title: "The cost of the offset is the exchange's own figure", ok: false, lines: [`Hyperliquid did not answer: ${(e as Error).message}`] };
  }
}

/** Every fill of `user` in [startMs, endMs], paged 2,000 at a time (the venue's page size; docs/89 L4). */
async function fillsBetween(user: string, startMs: number, endMs: number): Promise<{ coin: string; side: string; sz: string; time: number; tid?: number }[]> {
  const out: { coin: string; side: string; sz: string; time: number; tid?: number }[] = [];
  const seen = new Set<string>();
  let start = startMs;
  for (;;) {
    const r = await fetch(HL, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ type: "userFillsByTime", user, startTime: start, endTime: endMs }),
    });
    const page = (await r.json()) as { coin: string; side: string; sz: string; time: number; tid?: number }[];
    const fresh = page.filter((f) => !seen.has(`${f.tid}-${f.time}`));
    for (const f of fresh) seen.add(`${f.tid}-${f.time}`);
    out.push(...fresh);
    if (page.length < 2000 || fresh.length === 0) return out;
    start = Math.max(...page.map((f) => f.time));
  }
}

/** Check e: the offset claimed is no more than the short the venue held at the statement's time: its position
 *  now, less every fill since (the venue keeps only its 10,000 most recent fills, docs/91 R12).
 *  ponytail: a fill landing between the two reads shifts the result by that fill; check again. */
export async function checkVenue(kit: LenderKit, venuePin?: string): Promise<Check> {
  const st = kit.signed.statement;
  const atMs = st.at * 1000;
  const user = venuePin || st.venueAccount;
  try {
    const r = await fetch(HL, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ type: "clearinghouseState", user }) });
    const now = (await r.json()) as { time?: number; assetPositions?: { position: { coin: string; szi: string } }[] };
    // Sizes as integers of 1e-6 to stay exact.
    const micro = (s: string) => (s.startsWith("-") ? -Number(minorUnits(s.slice(1), 6)) : Number(minorUnits(s, 6)));
    const posNow = (now.assetPositions ?? []).filter((p) => p.position.coin === marketOf(st)).reduce((a, p) => a + micro(p.position.szi), 0);
    const since = await fillsBetween(user, atMs + 1, now.time || Date.now());
    const pos = posNow - since.filter((f) => f.coin === marketOf(st)).reduce((a, f) => a + (f.side === "A" ? -micro(f.sz) : micro(f.sz)), 0);
    // The short the statement read must be no more than the venue's (docs/92 T4); older statements carry only the offset.
    const claimed = micro(typeof st.shortContracts === "string" ? st.shortContracts : st.offsetContracts);
    // Only a short offsets (docs/89 M1); the account must be the one the lender pinned.
    const sameAccount = !venuePin || venuePin.toLowerCase() === st.venueAccount.toLowerCase();
    // The offset must still be there now (docs/94 U2): a statement is a point in time, a lender lends on the present.
    const gone = Math.max(0, -posNow) + 500 < micro(st.offsetContracts);
    // A short bought back since the statement means a sale or an offset taken off, whatever the level now: a short cut for
    // the statement and restored after it would otherwise absorb a later sale (docs/100 X2).
    const boughtBack = since.filter((f) => f.coin === marketOf(st) && f.side === "B").reduce((a, f) => a + micro(f.sz), 0);
    const cut = boughtBack > 500;
    // The short the statement read is the account's short then, neither more nor less (docs/99 W2); an older statement
    // names only its own offset, which may be less.
    const heldThen = Math.max(0, -pos);
    const under = typeof st.shortContracts === "string" && claimed + 500 < heldThen;
    const core = sameAccount && since.length < 10_000 && claimed <= heldThen + 500 && !under;
    const ok = core && !gone && !cut;
    return {
      id: "e",
      title: "The offset is real",
      ok,
      shortNow: Math.max(0, -posNow) / 1e6,
      outdated: core && (gone || cut),
      lines: [
        `${marketOf(st)} position of the exchange account at the statement's time, from Hyperliquid: ${(posNow / 1e6).toFixed(6)} now, less ${since.length} fills since = ${(pos / 1e6).toFixed(6)}; the statement read a short of ${typeof st.shortContracts === "string" ? st.shortContracts : st.offsetContracts}.`,
        ...(cut ? [`The account bought back ${(boughtBack / 1e6).toFixed(6)} oz of its ${marketOf(st)} short since this statement: a deal was sold or the offset taken off. Ask for a statement issued after it.`] : []),
        ...(under ? [`The statement read a short of ${st.shortContracts}; the account held ${(heldThen / 1e6).toFixed(6)} then.`] : []),
        ...(gone ? [`Since the statement the account's short fell to ${(Math.max(0, -posNow) / 1e6).toFixed(6)}; the ${st.offsetContracts} oz this statement counts is not there now: ask for a new one.`] : []),
        // The desk's whole holding is the trader's word: one file cannot prove it (docs/94 U1).
        ...(typeof st.deskHeldContracts === "string"
          ? [`The desk says it holds ${st.deskHeldContracts} oz in all; this file cannot prove that total. The short is the account's and backs every deal of this desk: check its statements together.`]
          : []),
        ...(since.length < 10_000 ? [] : ["Past the exchange's 10,000-fill horizon: the position then cannot be read."]),
      ],
    };
  } catch (e) {
    return { id: "e", title: "The offset is real", ok: false, lines: [`Hyperliquid did not answer: ${(e as Error).message}`] };
  }
}
