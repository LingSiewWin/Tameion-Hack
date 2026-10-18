// The shapes the service answers with. Copied from apps/console/src/lib/types.ts (Tenant, Demo, NameRecord, Call, NeedRow, Invoice,
// Paid, Seller, Purchase, Runway, Momentum, Preview, Party, Inbox) without change; Status, Records, RecordEvent and
// Recall are the service's own schemas in apps/classic/openapi.json (components.schemas.*), only the fields named there.
// Amounts are the ledger's strings ("0.200000 USDC"); the page shows them, it never computes money (DESIGN Rule 3).

export type Tenant = {
  policy: string;
  owner: string;
  chain: string;
  kind?: string;
  watched?: string;
  /** docs/79: the account's handle (`acme`), and the wallets read for its bills */
  name?: string;
  wallets?: string[];
  /** the spare key the owner contract holds: a wallet that can name a new passkey after the delay */
  recovery?: string;
  /** a spare key proposed (the first wallet connected), not on chain until the passkey sets it */
  spare_key?: string | null;
  qx?: string;
  qy?: string;
  [k: string]: unknown;
};

/** `GET /api/level1/demo` (docs/115 §3): the judge's five minutes, when the host runs demo mode. */
export type Demo = { enabled: boolean; vendor: string; bill: { usdc: string; everySecs: number }; delaySecs: number };

/** `GET /api/level1/name/<handle>` (docs/79 §6a Service 6). */
export type NameRecord = { name: string; account: string; owner: string; chains: { key: string; name: string; chainId: number; deployed: boolean; account?: string }[] };

export type Call = { to: string; data: string; summary?: string };

export type NeedRow = {
  counterparty: string;
  amount: string;
  why?: string;
  to: string;
  data: string;
  actionHash?: string;
  attest?: Call | null;
  /** What the agent said it is for, when it asked (docs/60 P-177). */
  reason?: string;
};

export type Invoice = {
  id: string;
  payee: string;
  amountText: string;
  dueAt?: number;
  everyDays?: number;
  discountBps?: number;
  earlyDays?: number;
  approvedPayee?: boolean;
  notes?: string[];
  pay?: Call | null;
};

export type Paid = {
  counterparty: string;
  amount: string;
  tx?: string;
  at?: number;
  explorer?: string;
  by?: string;
  bought?: string;
  reason?: string;
  early?: boolean;
  saved?: string;
  dueAt?: number;
};

/** A seller to approve, or (`askedBy`) a payee an outside agent asked for: then `amount` is the per-payment limit, not a price. */
export type Seller = Call & {
  host?: string;
  amount: string;
  seller: string;
  reason?: string;
  askedBy?: string;
  /** a bill the agent found in the history, offered before it falls due (docs/60 P-228); `amount` is then the bill's */
  bill?: { everySecs: number; nextDue: number; samples?: number };
};

/** One x402 purchase the agent tried, as the service recorded it (paid, refused, or waiting on the owner). */
export type Purchase = { at: number; host?: string; seller?: string; amount: string; status: string; why?: string; reason?: string; tx?: string; url?: string };

export type Runway = {
  basis?: string;
  days: number;
  horizonDays: number;
  billsInHorizon?: string;
  horizonTotal?: string;
  shortfall?: string;
  idleCover?: string;
  returnable?: string;
  returnTo?: string;
  firstAtRisk?: { amount: string; counterparty: string; onUnix: number; inHours?: number } | null;
};

/** The account's momentum (docs/117 §1): burn rate over the window from settled payments, both runways, what moved the rate. */
export type Momentum = {
  rate: string;
  windowDays: number;
  runway: { byRateDays: number | null; firstShortfallDays: number | null };
  forces: { at: number; kind: string; counterparty: string; deltaPerDay: string }[];
};

export type Preview = {
  status: "reading" | "done" | "failed";
  days?: number;
  transfers?: number;
  bills?: { counterparty: string; amount: string; every?: string; intervalSecs?: number; nextDue?: number }[];
  next30?: string;
  fundMinor?: string;
  suggestedLimit?: string | null;
  usdc?: string;
};

/** docs/81: a party as a person reads it — never an address. `kind` says where
 *  the name came from: the owner, this account's own, a proof (tameion, domain,
 *  ens), the API it was bought from (x402), its own invoice (claimed), a clash
 *  with a name another address holds, or nothing yet (described). */
export type Party = {
  label: string;
  kind: "owner" | "you" | "tameion" | "domain" | "ens" | "x402" | "invoice" | "clash" | "new";
  verified: boolean;
  note: string;
  tail: string;
  domain?: string;
};

export type Inbox = {
  /** docs/73 T2/T3, served once the route exists (docs/66 "Needs a route change" 4) */
  protection?: {
    bills30d?: string;
    protectedUsd?: string;
    coveragePct?: number;
    funding30d?: { usdc: string; entries?: number; from?: number; to?: number } | null;
  } | null;
  /** when this browser read it (ms); set by the page, not the service */
  readAt?: number;
  policy: string;
  mode: "Watch" | "Propose" | "Autopilot" | string;
  /** `spent24h` and `leftNow` are the contract's own trailing-24-hour figures (deployed accounts only). */
  caps: { perAction: string; daily: string; floor: string; spent24h?: string; leftNow?: string };
  balance: string;
  /** One trust-raising change waiting for the delay. `kinds` bit 0 = limits, bit 1 = mode. */
  pending?: { mode?: string; eta?: number; kinds?: number; perActionCap?: string; dailyCap?: string; words?: string } | null;
  /** Everything that waits for the owner, in one list (the same items the page counts), and how many. */
  waiting?: { kind: string; what: string; counterparty?: string; amount?: string; why?: string }[];
  waitingCount?: number;
  /** Payees the contract attests, and from when the agent may pay each (the waiting period). */
  payees?: { address: string; approvedAt: number; payableFrom: number; payableNow: boolean }[];
  purchases?: Purchase[];
  /** Purchases held because they cost more than 25 % above that seller's usual price: one owner tap lets one through. */
  pricesToApprove?: { seller: string; host?: string; amount: string; minor?: string; usual?: string; reason?: string; url?: string; at?: number; why?: string }[];
  protectedPerDay: string;
  chosenDaily?: string;
  deployed?: boolean;
  kind?: string;
  keyless?: boolean;
  halted?: unknown;
  lastPass?: {
    id: string;
    at: number;
    obligations: number;
    transfers?: number;
    modelUnavailable?: boolean;
    historyNote?: string | null;
    anchorTx?: string | null;
  } | null;
  running?: {
    id: string;
    progress?: { scannedBlocks?: number; totalBlocks?: number; etaSecs?: number | null; phase?: string } | null;
  } | null;
  nextPassSecs?: number | null;
  paid: Paid[];
  paidTotal: string;
  paidByAgent?: number;
  paidByYou?: number;
  needsYou: NeedRow[];
  invoices?: Invoice[];
  sellersToApprove?: Seller[];
  run?: (Call & { count: number; total: string }) | null;
  refused?: { counterparty: string; amount: string; why?: string }[];
  runway?: Runway | null;
  momentum?: Momentum | null;
  preview?: Preview | null;
  pull?: {
    allowance: string;
    wallet: string;
    walletBalance: string;
    suggestedAllowance: string;
    /** the service's own words for how it sized the allowance */
    suggestedBasis?: string;
    usdc: string;
    approveData: string;
    revokeData: string;
    error?: string | null;
  }
    /** the bill wallet's chain read failed: the service sends only why (tameion-app.rs `pull`, docs/60 P-243) */
    | {
        error: string;
        allowance?: undefined;
        wallet?: undefined;
        walletBalance?: undefined;
        suggestedAllowance?: undefined;
        suggestedBasis?: undefined;
        usdc?: undefined;
        approveData?: undefined;
        revokeData?: undefined;
      }
    | null;
  suggestedLimit?: { daily: string; perAction: string; basis: string; floor?: string; byAgent?: boolean } | null;
  recommendAutopilot?: boolean;
  /** docs/81: the bills the owner typed (before the first pass turns them into candidates) */
  typedBills?: { payee: string; amount: string; everySecs: number; nextDue: number }[];
  /** docs/81: every party in this reply, by lowercase address */
  parties?: Record<string, Party>;
  approvedByYou?: number;
  agentAuthority?: { lapsesAt: number; lapsed: boolean; daysLeft: number; renewSuggested: boolean } | null;
  standingOrders?: { id: number; active: boolean; amount: string; payee: string; everySecs: number; nextDue: number; due?: boolean }[];
  standingOrderCandidates?: { payee: string; amount: string; everySecs: number; nextDue: number; basis?: string; confidence?: number | string }[];
  owners?: { keys: { qx: string; qy: string; label?: string }[]; threshold: number } | null;
  approvals?: { target: string; data: string; have: number; need: number; keys?: number[] }[];
  joinRequests?: { qx: string; qy: string; label: string }[];
  guardians?: { list: string[]; threshold: number; pendingEta?: number } | null;
  agentTokens?: { id: string; label?: string; scopes: string[]; expires: number }[];
  gated?: boolean;
  terms?: Record<string, unknown> | null;
  exposure?: {
    holdings?: { size: string; coin: string; chain: string }[];
    notionalUsd?: string;
    ethTotal?: string;
    ethMark?: string;
    fall30Usd?: string;
    fall30Days?: number | null;
    hedge?: { on?: boolean } | null;
    source?: string;
  } | null;
  holdingsElsewhere?: {
    total?: string;
    chains?: { chain: string; balance: string; domain: number }[];
    arriving?: { chain: string; amount: string }[];
  } | null;
  delegation?: { implementation?: string; wallet?: string; initializeData?: string; active?: boolean } | null;
  error?: string;
};

// ---- from apps/classic/openapi.json components.schemas ------------------------------------------------------------

/** `Refusal.error` (docs/115 §2): every refusal on an agent-facing route. `executed` says whether money may have moved. */
export type RefusalError = {
  code: string;
  by: "auth" | "input" | "mandate" | "budget" | "payee" | "idempotency" | "chain" | "upstream" | "service";
  executed: "yes" | "no" | "unknown";
  audience: "user" | "agent";
  meaning: string;
  nextStep: string;
  retryAfterSecs: number;
  words: string;
};
/** `contractVersion` is absent only when this app's own proxy could not reach the service. */
export type Refusal = { ok: false; contractVersion?: string; error: RefusalError };

/** `Force`: an event that moved the burn rate, and by how much a day. */
export type Force = { at: number; kind: "bill_added" | "bill_removed" | "mode_changed" | "cap_changed" | "refused"; counterparty: string; deltaPerDay: string };

/** `Ok` + `Status` (`GET /api/level1/status?policy=`). */
export type Status = {
  ok: true;
  contractVersion: string;
  recentUpdates: { date: string; note: string }[];
  codes?: string;
  policy?: string;
  mode?: unknown;
  /** the contract is on Watch (null when the chain read failed) */
  paused?: boolean | null;
  /** a read-back mismatch only a human clears */
  halt?: boolean;
  stoppedTokens?: unknown[];
  rails?: { id: string; currency: string; finality?: string | null; finalitySecs: number | null; includedSecs?: number | null; measuredAt?: number | null }[];
  /** USDC a day over windowDays (null when the chain read failed) */
  rate?: string | null;
  windowDays?: number;
  runway?: { byRateDays: number | null; firstShortfallDays: number | null } | null;
  forces?: Force[] | null;
};

/** `Ok` + `Records` (`GET /api/level1/records?policy=`): every root this account's agent anchored, newest last. */
export type Records = {
  ok: true;
  contractVersion: string;
  policy: string;
  chain?: unknown;
  records: { root: string; anchorTx?: unknown; at?: unknown; events?: unknown; agent?: unknown; anchor?: unknown }[];
};

/** One line of `GET /api/level1/record?policy=&root=` (NDJSON, in order). `canonical` is hashed; `kind` and `note` are for people. */
export type RecordEvent = { seq: number; at: number; kind: string; note: string; canonical: string };

/** `Ok` + `Recall` (`GET /api/level1/recall?policy=&ids=`). */
export type Recall = {
  ok: true;
  contractVersion: string;
  policy: string;
  chain: string;
  record: { root: string; latest: boolean; events: number; anchorTx?: unknown; at?: unknown };
  chainRead?: unknown;
  verified: number;
  unverified: number;
  unknown: number;
  results: {
    id: string;
    status: "verified" | "unverified" | "unknown";
    by?: string;
    code?: string;
    words: string;
    leaf?: string;
    path?: { sibling: string; left: boolean }[];
    root?: string;
    event?: unknown;
  }[];
};

/** One filled slot of a record. A slot is present only after its check passed, so `verified` is true when it is there. */
export type Slot = { alg: string; keyId: string; verified: boolean };

/** One record of `GET /api/level1/symbolon`. A locked account gives an outsider `root`, `seq` and `leaf` only. */
export type SymbolonRecord = {
  root: string;
  seq: number;
  leaf: string;
  kind?: string;
  at?: number;
  counterparty?: string;
  amount?: string;
  minor?: string;
  /** a decision's verdict: ALLOW, REVIEW, or BLOCK (a refusal) */
  decision?: string;
  tx?: string;
  note?: string;
  anchorTx?: string | null;
  agent?: Slot | null;
  counterpartySlot?: Slot | null;
  witness?: Slot | null;
};

/** `Ok` + `Symbolon` (`GET /api/level1/symbolon?policy=`): the records oldest first, how many stand behind 0-3 slots, and the
 *  roots whose anchoring could not be read this time (their records show no witness; that is not "not witnessed"). */
export type Symbolon = {
  ok: true;
  contractVersion: string;
  policy: string;
  chain: string;
  explorer?: string | null;
  locked: boolean;
  counts: { unsigned: number; single: number; double: number; triple: number } | null;
  records: SymbolonRecord[];
  chainReadFailed: string[];
};
