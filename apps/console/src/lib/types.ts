// The shapes the service answers with (apps/classic/openapi.json). Only the fields
// the console reads; amounts are the ledger's strings ("0.200000 USDC").

export type Chain = { key: string; name: string; chainId: number; explorer: string; anchor: string | null };
export type Cfg = {
  agentId?: string;
  /** A public copy of this console (TAMEION_PUBLIC_URL), for links other people open (docs/95 P3). */
  publicUrl?: string | null;
  chains: Chain[];
  delaySecs: number;
  inviteNeeded?: boolean;
  operator?: string | null;
  gatewaySources?: { chainId: number; domain: number; name: string; usdc: string; wallet: string }[];
  /** the desk's statement key (docs/83 W7): a lender checks a statement against it */
  statementSigner?: string;
};

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

export type Pub = { qx: string; qy: string; credentialId: string };

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

export type WatchReport = {
  address: string;
  chain: { key: string; name: string; explorer: string };
  days: number;
  transfers: number;
  outgoing?: number;
  counterparties?: number;
  balance: string;
  liquidity30d: string;
  upcoming30d?: number;
  dueNow?: number;
  elapsedMs?: number;
  obligations: {
    counterparty: string;
    amount: string;
    every?: string;
    intervalSecs?: number;
    nextDue?: number;
    lastPaid?: number;
    confidence?: string;
    samples?: number;
    verdict?: string;
    why?: string;
    kind?: string;
  }[];
  maxLoss?: { agentKey24h?: string; dailyCap?: string; perActionCap?: string } | null;
};

export type WatchJob = {
  id: string;
  status: "queued" | "running" | "done" | "failed";
  progress?: { scannedBlocks?: number; totalBlocks?: number; phase?: string } | null;
  report?: WatchReport | null;
  error?: string | null;
};

// ---- the desk (docs/83 W5/W8): GET /api/desk and the lender's kit ----------------
export type DeskMilestone = {
  kind: "bought" | "shipped" | "arrived" | "to_fabrication" | "consigned" | "returned" | "sold";
  quantity: string;
  usdc?: string | null;
  counterparty: string;
  document: string;
  signer: string;
  signed: boolean;
  at: number;
  words: string;
  /** A priced step far from the venue's mark seen at booking, in words (GET /api/desk only). */
  priceNote?: string | null;
};
export type DeskClass = "eligible" | "consigned" | "ineligible";
export type DeskDeal = {
  id: string;
  goods: string;
  supplier: string;
  bought: string;
  cost: string;
  held: string;
  stage: string;
  byStage: { stage: string; words: string; class: DeskClass; grams: string }[];
  milestones: DeskMilestone[];
};
export type DeskBook = {
  deals: DeskDeal[];
  exposure: { goods: string; held: string; market: string; offsetTarget: string; unhedgedBase: string; autoLimit?: string; autoMoved?: string }[];
  parties: { name: string; address: string; kinds?: string[] | null; stopped?: boolean }[];
  root: string;
  venueAccount?: string | null;
  chainId: number;
  /** the venue's own read-back, read only (never trades): position negative = short */
  venue?: { account: string; market: string; position?: string | null; mark?: string | null; error?: string | null; readAt?: number | null } | null;
};
/** One saved lender kit, newest first (`GET /api/desk/statements`). */
export type DeskStatementRow = {
  file: string;
  at: number;
  deal: string;
  held: string;
  stage: string;
  offsetShareBps: string;
  maxLossUsdc: string;
  eligibleUsdc: string;
  fundingUsdc?: string | null;
  stressBps?: number;
  basisBps?: number;
  anchorTx: string;
  /** The deal's share of the short the statement counted, oz, and how many steps it covered (docs/96 V4, V7). */
  offsetContracts?: string;
  steps?: number;
};
export type DeskStatement = {
  deal: string;
  venueAccount: string;
  at: number;
  held: string;
  stage: string;
  market: string;
  markUsdc: string;
  valueUsdc: string;
  costUsdc: string;
  eligible: { grams: string; valueUsdc: string };
  ineligible: { grams: string; valueUsdc: string };
  consigned: { grams: string; valueUsdc: string };
  eligibleUsdc: string;
  maxLossUsdc: string;
  offsetShareBps: string;
  offsetContracts: string;
  heldContracts: string;
  stressBps: number;
  basisBps: number;
  hedgeControl: string;
  assumptions: string;
  unhedgedBase: string;
  /** this deal's share of funding on the short since it was bought; positive = the market paid the trader */
  fundingUsdc?: string;
  /** the deal's steps the statement covers */
  milestones?: unknown[];
};
export type DeskKit = {
  signed: { statement: DeskStatement; signature: string; signer: string; statementHash: string };
  anchor: { tx: string; chainId: number; contract: string; from: string };
  parties: { name: string; address: string }[];
};
export type DeskOffsetReply = {
  live: boolean;
  target?: string | null;
  coins: { coin: string; mark: string; before: string; action: string }[];
  readBack?: string | null;
  accountValue: string;
};
