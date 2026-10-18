//! The MCP tool table and its JSON-RPC handling, shared by the two transports
//! (docs/115 S16): `tameion-mcp` over stdio, and the service's `POST /mcp`
//! (Streamable HTTP, MCP 2025-06-18) for a remote agent with nothing to install.
//!
//! Every tool maps to one HTTP route of the service and holds no key. "Prepare"
//! tools return the exact calldata and its description in words for the
//! owner's passkey; the authority stays where the contract put it. An agent
//! that calls these cannot pay anyone the owner did not approve, raise a cap,
//! or withdraw — it can see, run and prepare, and with a `buy` key ask the
//! service to pay an approved seller (buy_service) or an approved payee (pay)
//! inside the caps.
//!
//! Protocol: JSON-RPC 2.0, MCP 2025-06-18 subset — `initialize`,
//! `notifications/initialized`, `ping`, `tools/list`, `tools/call`. No crate:
//! the surface is five methods.

use crate::refusal::{self, Executed};
use serde_json::{Value, json};

pub const PROTOCOL: &str = "2025-06-18";
/// How long one tool call waits for the service: `TAMEION_MCP_TIMEOUT_SECS`,
/// default 60.
pub fn timeout_secs() -> u64 {
    std::env::var("TAMEION_MCP_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|s| *s > 0)
        .unwrap_or(60)
}

/// The service a tool call goes to, and the agent key it carries.
pub struct Upstream {
    pub http: reqwest::Client,
    /// `http(s)://host:port`, no trailing slash.
    pub base: String,
    /// The capability the owner's passkey minted for this agent (PLAN 5.31),
    /// sent as `Authorization: Bearer`.
    pub token: Option<String>,
}

impl Upstream {
    /// `tameion-mcp`'s: `TAMEION_URL` (default http://127.0.0.1:7880) and
    /// `TAMEION_TOKEN`.
    pub fn from_env() -> Self {
        Upstream {
            http: client(),
            base: std::env::var("TAMEION_URL")
                .ok()
                .map(|u| u.trim_end_matches('/').to_string())
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:7880".into()),
            token: std::env::var("TAMEION_TOKEN")
                .ok()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty()),
        }
    }
}

/// One HTTP client for the process (a pool, not a client per call).
fn client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs()))
                .build()
                .expect("http client")
        })
        .clone()
}

/// A tool argument inside a URL: percent-encoded, so a value cannot add a
/// query parameter or a path segment to the route it names (the HTTP
/// transport takes arguments from anyone who can reach it).
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// The tools, each one HTTP route of the service. Descriptions are written
/// for the calling model: what it gets, and what it cannot do.
pub fn tools() -> Vec<Value> {
    let addr = json!({ "type": "string", "description": "0x… address" });
    vec![
        json!({
            "name": "treasury_inbox",
            "annotations": { "title": "Treasury inbox", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "The account in one read. What waits for the owner is `waiting` (with `waitingCount`): every bill to approve, seller to approve and invoice, the same items the account page counts — read that field, not one list alone. `caps` has perAction, daily and floor, plus `spent24h` (spent in the trailing 24 hours) and `leftNow` (what the contract still allows right now), read from the contract. `payees`: who the agent may pay, with `payableFrom` (unix) when each one's cooling-off ends. `pending`: a change the owner signed that waits its delay, in `words` (only what is pending). `purchases`: every paid-API buy, refused ones with why; `refused` is only the last pass's bill checks. Your own requests (from prepare_limits / prepare_mode / prepare_payees with an agent key) appear as `agentAsks`, `suggestedLimit`, `recommendAutopilot` and seller rows: one tap each for the owner. Also: mode, balance, what was paid, the runway, the last pass, invoices, owners. Read-only.",
            "inputSchema": { "type": "object", "required": ["policy"], "properties": { "policy": addr } }
        }),
        json!({
            "name": "top",
            "annotations": { "title": "Burn rate and runway", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "How fast the account spends and how long its money lasts: `rate` (USDC a day, from the payments settled in the last `windowDays` of the account's anchored record, never a forecast), `runway.byRateDays` (days until the balance reaches the floor the owner keeps, at that rate; null while nothing was spent) and `runway.firstShortfallDays` (days until the first bill, at its schedule, that the money above the floor cannot cover; null when it covers every bill in the next 90 days), and `forces`: the last ten events that moved the rate (bill_added, bill_removed, mode_changed, cap_changed, refused) with `deltaPerDay`. Quote these numbers; do not compute your own. Read-only.",
            "inputSchema": { "type": "object", "required": ["policy"], "properties": { "policy": addr } }
        }),
        json!({
            "name": "run_pass",
            "annotations": { "title": "Run a pass", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "Ask the agent to check the treasury's bills now (one pass per policy per ten minutes; a recent pass answers in between). Returns the job id; poll pass_status. Moves no money by itself in Propose mode; in Autopilot the agent pays approved payees inside the contract's caps, as always.",
            "inputSchema": { "type": "object", "required": ["policy"], "properties": { "policy": addr } }
        }),
        json!({
            "name": "pass_status",
            "annotations": { "title": "Pass status", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "description": "Status, progress and report of a pass (job id from run_pass or treasury_inbox.lastPass.id).",
            "inputSchema": { "type": "object", "required": ["id"], "properties": { "id": { "type": "string" } } }
        }),
        json!({
            "name": "prepare_payees",
            "annotations": { "title": "Prepare payee approval (owner signs)", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "description": "Ask the owner to approve (allow=true) or revoke (allow=false) payees. You cannot sign: the answer says who signs (`signer`: only the owner's passkey), when it applies (`applies`: an approval starts the contract's cooling-off; a revocation is instant) and what the owner sees (`ownerSees`). Called with an agent key, the request is kept and shows on the account page as one row with your `reason`, one tap for the owner; tell your user to open the account page.",
            "inputSchema": { "type": "object", "required": ["policy", "payees"], "properties": { "policy": addr, "payees": { "type": "array", "items": addr }, "allow": { "type": "boolean", "default": true }, "reason": { "type": "string", "description": "why, in one sentence the owner reads beside the tap" } } }
        }),
        json!({
            "name": "prepare_limits",
            "annotations": { "title": "Prepare limits (owner signs)", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "description": "Ask the owner to set the agent's limits (per payment, per day, reserve floor, in USDC). You cannot sign: the answer says `signer` (only the owner's passkey), `applies` (a cut: at once; a raise: after the account's delay, and the owner can cancel until then) and `ownerSees`. Called with an agent key, the request shows on the account page as the agent's suggestion with your `reason`, one tap for the owner.",
            "inputSchema": { "type": "object", "required": ["policy", "perAction", "daily", "floor"], "properties": { "policy": addr, "perAction": { "type": "string" }, "daily": { "type": "string" }, "floor": { "type": "string" }, "reason": { "type": "string", "description": "why, in one sentence the owner reads beside the tap" } } }
        }),
        json!({
            "name": "prepare_mode",
            "annotations": { "title": "Prepare autonomy level (owner signs)", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "description": "Ask the owner to set the autonomy level: 0 Watch (pause), 1 Propose, 2 Autopilot. You cannot sign: the answer says `signer` (only the owner's passkey), `applies` (lowering: at once; raising: after the account's delay, cancellable) and `ownerSees`. Called with an agent key, a request for Autopilot shows on the account page as one tap for the owner, with your `reason`.",
            "inputSchema": { "type": "object", "required": ["policy", "mode"], "properties": { "policy": addr, "mode": { "type": "integer", "enum": [0, 1, 2] }, "reason": { "type": "string", "description": "why, in one sentence the owner reads beside the tap" } } }
        }),
        json!({
            "name": "prepare_fund",
            "annotations": { "title": "Prepare Gateway funding (wallet signs)", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "EIP-712 data for the BILL WALLET to sign to move its USDC held in Circle Gateway on another chain (sourceDomain: 3 Arbitrum Sepolia, 6 Base Sepolia, 0 Ethereum Sepolia, 26 Arc Testnet) into the policy. Fee and expiry come from Circle's estimate; the data names the policy as the only recipient. This tool cannot sign; the owner signs on the Tami page (or with a key file via scripts/gateway_fund.sh) and the service relays the mint.",
            "inputSchema": { "type": "object", "required": ["policy", "amount", "sourceDomain"], "properties": { "policy": addr, "amount": { "type": "string" }, "sourceDomain": { "type": "integer" }, "from": addr } }
        }),
        json!({
            "name": "prepare_schedule",
            "annotations": { "title": "Prepare standing order (owner signs)", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "description": "Calldata for the OWNER'S PASSKEY to set a standing order on the policy: payee (must already be approved), USDC amount, everyDays, optional firstDue (unix). Once set, anyone may call payDue(id) on its due date inside the caps — the agent does it on its passes, and the bill pays even if Tami is down. Cannot sign.",
            "inputSchema": { "type": "object", "required": ["policy", "payee", "amount", "everyDays"], "properties": { "policy": addr, "payee": addr, "amount": { "type": "string" }, "everyDays": { "type": "number" }, "firstDue": { "type": "integer" } } }
        }),
        json!({
            "name": "prepare_renew",
            "annotations": { "title": "Prepare agent renewal (owner signs)", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "description": "Calldata for the OWNER'S PASSKEY to renew (or shorten) the agent's authority to pay: renewAgent(until). days from now (default 90, at most 365) or until (unix). The inbox's agentAuthority says when it lapses; a lapsed agent proposes and pays nothing by itself. Cannot sign.",
            "inputSchema": { "type": "object", "required": ["policy"], "properties": { "policy": addr, "days": { "type": "integer" }, "until": { "type": "integer" } } }
        }),
        json!({
            "name": "import_bills",
            "annotations": { "title": "Import bills", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "description": "Tell the agent about bills it has not seen in history (payee, USDC amount, every N days, and any early-payment discount the invoice offers: discountPercent off when paid earlyDays before due — the agent takes it only when every other bill in 30 days stays covered). Only payees the policy already attests are kept — approving is the owner's act — so this cannot add a payee. Returns kept and refused bills with reasons.",
            "inputSchema": { "type": "object", "required": ["policy", "bills"], "properties": { "policy": addr, "bills": { "type": "array", "items": { "type": "object", "required": ["payee", "amount"], "properties": { "payee": addr, "amount": { "type": "string" }, "everyDays": { "type": "integer", "default": 30 }, "nextDue": { "type": "integer" }, "discountPercent": { "type": "number", "description": "early-payment discount on the invoice, at most 10" }, "earlyDays": { "type": "integer", "description": "paid at least this many days before due to earn it" } } } } } }
        }),
        json!({
            "name": "buy_service",
            "annotations": { "title": "Buy a paid API call (spends USDC)", "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true },
            "description": "Call a paid API that answers HTTP 402 (x402) and pay it from the account, inside its limits: only a seller the owner approved, never above maxUsdc, the per-payment cap or what is left of the 24-hour limit, only on Autopilot. Needs TAMEION_TOKEN with the 'buy' scope (the owner mints it with the passkey). A seller nobody approved is not paid: it comes back as needsYou and waits on the account page. Returns the service's response body and the payment (amount, seller, transaction). The reason is written down with the purchase.",
            "inputSchema": { "type": "object", "required": ["policy", "url", "maxUsdc", "reason"], "properties": { "policy": addr, "url": { "type": "string" }, "method": { "type": "string", "default": "GET" }, "body": {}, "maxUsdc": { "type": "string", "description": "the most this one call may cost, e.g. \"0.01\"" }, "reason": { "type": "string", "description": "why the agent needs it; kept with the purchase" } } }
        }),
        json!({
            "name": "pay",
            "annotations": { "title": "Pay an approved payee (spends USDC)", "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": true },
            "description": "Pay a payee the owner approved, from the account, now: only on Autopilot, only a payee past its cooling-off, never above the per-payment cap or what is left of the 24-hour limit; the account's contract checks each of these in the same transaction as the payment. Needs TAMEION_TOKEN with the 'buy' scope. `why` is written to the account's book beside the payment. `operationId` names this one payment: make a new one per payment (e.g. op_ and a UUID; 8 to 128 of A-Z a-z 0-9 _ . : -) and reuse it on a retry: the same id never pays twice (the chain refuses it) and answers with the first payment. Returns the payment (amount, payee, why, transaction) or a refusal whose code says what to do: mode_propose (ask the owner for Autopilot), payee_not_approved, payee_cooling_off, over_per_payment, over_24h, below_floor, idempotency_conflict (the id was used for a different payment).",
            "inputSchema": { "type": "object", "required": ["policy", "payee", "amount", "why", "operationId"], "properties": { "policy": addr, "payee": addr, "amount": { "type": "string", "description": "USDC, e.g. \"0.25\"" }, "why": { "type": "string", "description": "what this payment is for; written to the book" }, "operationId": { "type": "string", "description": "one id per payment, the same on a retry" } } }
        }),
        json!({
            "name": "purchases",
            "annotations": { "title": "x402 purchases (receipts)", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "Every paid-API purchase this account made over x402 (buy_service), oldest first, as receipts: url, seller (payTo), amount, network, tx, status (paid, unconfirmed, refused, needs-you) with the reason given and, for refusals, why; id — the x402 payment-identifier sent with the payment when the seller takes one, else the settlement transaction (idFrom says which); receipt — the seller's signed offer-receipt when it sent one; sha256 of the bytes the seller returned. Sellers bought from on a cadence also appear in treasury_inbox's bills with provenance \"x402 receipt\". Read-only.",
            "inputSchema": { "type": "object", "required": ["policy"], "properties": { "policy": addr } }
        }),
        json!({
            "name": "invoices",
            "annotations": { "title": "Invoices waiting", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "Invoices vendors (or their agents) sent to this account and the owner has not answered yet: payee, amountText, dueAt, everyDays (0 = one payment), early-payment terms, approvedPayee, frozen, and the agent's notes in words (does the amount match what this payee usually bills, is the payee approved). Accepting or paying one is the owner's tap on the account page. Read-only.",
            "inputSchema": { "type": "object", "required": ["policy"], "properties": { "policy": addr } }
        }),
        json!({
            "name": "send_invoice",
            "annotations": { "title": "Send a signed invoice", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "Bill a Tami account as a vendor: post an invoice the PAYEE has already signed (EIP-712 Invoice(address payee,address policy,uint256 amount,uint64 dueAt,uint32 everyDays,uint16 discountBps,uint16 earlyDays,bytes32 ref,uint64 issuedAt), domain name \"Tameion Invoice\", version \"1\", chainId of the account's chain, verifyingContract = the account). This tool holds no key and signs nothing: pass the body you signed elsewhere. Nothing is paid on receipt — the account's agent checks it and the owner accepts or declines. A ref already sent is refused; twenty invoices an hour per account. amount is USDC minor units as a string (\"150000\" = 0.15 USDC).",
            "inputSchema": { "type": "object", "required": ["invoice"], "properties": { "invoice": { "type": "object", "required": ["policy", "payee", "amount", "dueAt", "everyDays", "discountBps", "earlyDays", "ref", "issuedAt", "signature"], "properties": { "policy": addr, "payee": addr, "amount": { "type": "string" }, "dueAt": { "type": "integer" }, "everyDays": { "type": "integer" }, "discountBps": { "type": "integer" }, "earlyDays": { "type": "integer" }, "ref": { "type": "string", "description": "0x… 32 bytes" }, "issuedAt": { "type": "integer" }, "signature": { "type": "string", "description": "0x… 65 bytes, by the payee" } } } } }
        }),
        json!({
            "name": "records",
            "annotations": { "title": "Anchored records", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "Every anchored root of the account (root, anchor transaction, time, event count, agent, anchor contract). To check one yourself: fetch its events with record_events and recompute the root as the service's llms.txt describes (keccak256 Merkle tree, RFC 6962 split, 0x00 leaf / 0x01 node prefixes over each event's `canonical` bytes), then compare with `rootOf(agent)` on the anchor contract. A match covers the anchored bytes; `note`/`kind` are human text beside them.",
            "inputSchema": { "type": "object", "required": ["policy"], "properties": { "policy": addr } }
        }),
        json!({
            "name": "record_events",
            "annotations": { "title": "Record events", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "description": "The events behind one anchored root, as written before anchoring: each line's `canonical` (hex) is the committed bytes, in order; `seq`, `at`, `kind` and `note` are carried for people. Recompute the root from `canonical` alone (see records).",
            "inputSchema": { "type": "object", "required": ["policy", "root"], "properties": { "policy": addr, "root": { "type": "string" } } }
        }),
        json!({
            "name": "recall",
            "annotations": { "title": "Check a recollection", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "Check events you remember about the account against its anchored record and the chain before you rely on them. Ids are evt:N, N being the event's seq in record_events, read in the account's latest anchored record unless you pass that record's root. Each comes back verified (its bytes hash into the root the chain holds: rootOf, or the transaction that anchored that record), unverified (the record changed after it was anchored, or its root is not on chain: do not rely on it) or unknown (code recall_unknown: no such event; by rpc: no chain to ask, so nothing is verified). A verified event carries its kind, time, payee, amount and note as recorded when your key may read the account. Read-only.",
            "inputSchema": { "type": "object", "required": ["policy", "ids"], "properties": { "policy": addr, "ids": { "type": "array", "maxItems": 32, "items": { "type": "string", "pattern": "^evt:(0|[1-9][0-9]*)$" }, "description": "event ids, e.g. [\"evt:0\", \"evt:3\"]" }, "root": { "type": "string", "description": "0x… root of an earlier record (records lists them); default: the latest" } } }
        }),
        json!({
            "name": "watch_address",
            "annotations": { "title": "Watch an address", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "Read-only report for any address (no key, nothing sent): recurring bills found in its USDC history with evidence, 30-day liquidity, its USDC in Circle Gateway on other chains, what the agent would pay vs ask under given caps, and the max loss. Returns a job id; poll pass_status. Chains: arc-testnet, arbitrum, base, ethereum.",
            "inputSchema": { "type": "object", "required": ["address"], "properties": { "address": addr, "chain": { "type": "string", "default": "arc-testnet" }, "days": { "type": "integer", "default": 90 } } }
        }),
    ]
}

/// The gold desk's tools (dropped from the product on 2026-10-16, docs/109
/// §10): kept, and still routed in [`call`], but never listed, so an agent
/// is not offered a product that is gone. Removed with the desk's code.
#[allow(dead_code)]
fn desk_tools() -> Vec<Value> {
    let addr = json!({ "type": "string", "description": "0x… address" });
    vec![
        json!({
            "name": "desk",
            "annotations": { "title": "The trader's desk", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "A trader's physical book (docs/83): every deal with where its goods are (bought, in transit, in stock, in fabrication, consigned, sold), each step in words with who vouched for it (a counterparty's signature, or the owner's own unsigned entry), the lender's classes (eligible / consigned / not eligible), the exposure in grams, the offset target on Hyperliquid, the approved counterparties and the root. Needs an agent key with `read`. Read-only.",
            "inputSchema": { "type": "object", "required": ["policy"], "properties": { "policy": addr } }
        }),
        json!({
            "name": "send_milestone",
            "annotations": { "title": "Send a signed step", "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true },
            "description": "As a trader's counterparty (supplier, carrier, vault, buyer), post a step you ALREADY signed: EIP-712 ShipmentEvent(string deal,string kind,string goods,string quantity,string usdc,string counterparty,string document,uint64 at), domain name \"Tameion Desk\", version \"2\", chainId of the desk's chain, verifyingContract = the desk's account (the policy); usdc is \"\" for steps that carry no price. kind: bought | shipped | arrived | to_fabrication | consigned | returned | sold. This tool holds no key and signs nothing. The desk books it only if the signature recovers to the address the trader approved under `counterparty`; a signed purchase or sale then moves the trader's offset on Hyperliquid inside the hedge engine's rules.",
            "inputSchema": { "type": "object", "required": ["policy", "milestone", "signature"], "properties": { "policy": addr, "signature": { "type": "string", "description": "0x… 65 bytes" }, "milestone": { "type": "object", "required": ["deal", "kind", "goods", "quantity", "counterparty", "document", "at"], "properties": { "deal": { "type": "string" }, "kind": { "type": "string" }, "goods": { "type": "string", "enum": ["gold"] }, "quantity": { "type": "string", "description": "grams, e.g. \"10.000\"" }, "usdc": { "type": "string", "description": "price or proceeds; only for bought / sold" }, "counterparty": { "type": "string" }, "document": { "type": "string" }, "at": { "type": "integer" } } } } }
        }),
        json!({
            "name": "desk_statements_all",
            "annotations": { "title": "Statement of the whole stock", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
            "description": "Issue every open deal's lender statement at one moment, over one root and one on-chain anchor, as one file {\"kits\": [...], \"at\"}: what a lender financing the trader's whole stock checks together (scripts/verify_desk.py <file> or /lender), so one short is never claimed by two deals. Needs an agent key with `run`.",
            "inputSchema": { "type": "object", "required": ["policy"], "properties": { "policy": addr } }
        }),
        json!({
            "name": "stop_counterparty",
            "annotations": { "title": "Stop a counterparty", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "Stop one of the trader's approved counterparties at once: from now its signature books nothing on the desk, so it can no longer move the offset; steps it signed before stay booked. Use it when its key may be lost or its steps look wrong (a price far from the market, a step outside its role, steps out of order), and tell the owner why. It only shrinks trust, so an agent key with `run` may do it; approving the counterparty again needs the owner's passkey.",
            "inputSchema": { "type": "object", "required": ["policy", "name"], "properties": { "policy": addr, "name": { "type": "string", "description": "the counterparty's name on the desk, e.g. \"Supplier\"" } } }
        }),
        json!({
            "name": "desk_statement",
            "annotations": { "title": "Statement for a lender", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true },
            "description": "Issue the lender's kit for one deal: a borrowing-base statement (held, eligible / consigned / not eligible, share offset, printed maximum loss, eligible value after it, who controls the offset) signed by the desk's statement key, the deal's steps disclosed against the desk's root with each counterparty's signature, and the root anchored on chain now (costs the service a little gas). A lender checks it at /lender or with scripts/verify_desk.py. Needs an agent key with `run`.",
            "inputSchema": { "type": "object", "required": ["policy", "deal"], "properties": { "policy": addr, "deal": { "type": "string" } } }
        }),
    ]
}

/// One tool call: the service's reply in the agent envelope (docs/115 §2), or
/// the refusal (`Err`), the whole reply with its `error` object.
pub async fn call(up: &Upstream, name: &str, args: &Value) -> Result<Value, Value> {
    let b = &up.base;
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    // Arguments that go into a URL.
    let u = |k: &str| enc(args[k].as_str().unwrap_or(""));
    let (method, url, body): (&str, String, Option<Value>) = match name {
        "treasury_inbox" => (
            "GET",
            format!("{b}/api/level1/inbox?policy={}", u("policy")),
            None,
        ),
        "top" => (
            "GET",
            format!("{b}/api/level1/status?policy={}", u("policy")),
            None,
        ),
        "run_pass" => (
            "POST",
            format!("{b}/api/level1/run"),
            Some(json!({ "policy": s("policy") })),
        ),
        "pass_status" => ("GET", format!("{b}/api/watch/{}", u("id")), None),
        "prepare_payees" => (
            "POST",
            format!("{b}/api/level1/prepare-payees"),
            Some(
                json!({ "policy": s("policy"), "payees": args["payees"], "allow": args["allow"].as_bool().unwrap_or(true), "reason": args["reason"] }),
            ),
        ),
        "prepare_limits" => (
            "POST",
            format!("{b}/api/level1/prepare-limits"),
            Some(
                json!({ "policy": s("policy"), "perAction": s("perAction"), "daily": s("daily"), "floor": s("floor"), "reason": args["reason"] }),
            ),
        ),
        "prepare_mode" => (
            "POST",
            format!("{b}/api/level1/prepare-mode"),
            Some(json!({ "policy": s("policy"), "mode": args["mode"], "reason": args["reason"] })),
        ),
        "prepare_fund" => (
            "POST",
            format!("{b}/api/level1/prepare-fund"),
            Some(
                json!({ "policy": s("policy"), "amount": s("amount"), "sourceDomain": args["sourceDomain"], "from": args["from"] }),
            ),
        ),
        "prepare_schedule" => (
            "POST",
            format!("{b}/api/level1/prepare-schedule"),
            Some(
                json!({ "policy": s("policy"), "payee": s("payee"), "amount": s("amount"), "everyDays": args["everyDays"], "firstDue": args["firstDue"] }),
            ),
        ),
        "prepare_renew" => (
            "POST",
            format!("{b}/api/level1/prepare-renew"),
            Some(json!({ "policy": s("policy"), "days": args["days"], "until": args["until"] })),
        ),
        "import_bills" => (
            "POST",
            format!("{b}/api/level1/bills"),
            Some(json!({ "policy": s("policy"), "bills": args["bills"] })),
        ),
        "buy_service" => (
            "POST",
            format!("{b}/api/level1/buy"),
            Some(
                json!({ "policy": s("policy"), "url": s("url"), "method": args["method"].as_str().unwrap_or("GET"), "body": args["body"], "maxUsdc": s("maxUsdc"), "reason": s("reason") }),
            ),
        ),
        "pay" => (
            "POST",
            format!("{b}/api/level1/pay"),
            Some(
                json!({ "policy": s("policy"), "payee": s("payee"), "amount": s("amount"), "why": s("why") }),
            ),
        ),
        "invoices" => (
            "GET",
            format!("{b}/api/level1/inbox?policy={}", u("policy")),
            None,
        ),
        "purchases" => (
            "GET",
            format!("{b}/api/level1/purchases.jsonl?policy={}", u("policy")),
            None,
        ),
        "send_invoice" => (
            "POST",
            format!("{b}/api/level1/invoice"),
            Some(args["invoice"].clone()),
        ),
        "desk" => ("GET", format!("{b}/api/desk?policy={}", u("policy")), None),
        "send_milestone" => (
            "POST",
            format!("{b}/api/desk/milestone"),
            Some(
                json!({ "policy": s("policy"), "milestone": args["milestone"], "signature": s("signature") }),
            ),
        ),
        "desk_statement" => (
            "POST",
            format!("{b}/api/desk/statement"),
            Some(json!({ "policy": s("policy"), "deal": s("deal") })),
        ),
        "desk_statements_all" => (
            "POST",
            format!("{b}/api/desk/statements/all"),
            Some(json!({ "policy": s("policy") })),
        ),
        "stop_counterparty" => (
            "POST",
            format!("{b}/api/desk/party/revoke"),
            Some(json!({ "policy": s("policy"), "name": s("name") })),
        ),
        "records" => (
            "GET",
            format!("{b}/api/level1/records?policy={}", u("policy")),
            None,
        ),
        "record_events" => (
            "GET",
            format!(
                "{b}/api/level1/record?policy={}&root={}",
                u("policy"),
                u("root")
            ),
            None,
        ),
        "recall" => {
            // An array of ids, or one comma-separated string; each escaped, so
            // an id cannot add a parameter to the route.
            let ids: Vec<String> = match &args["ids"] {
                Value::Array(a) => a
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
                Value::String(x) => x.split(',').map(str::to_string).collect(),
                _ => Vec::new(),
            };
            let ids: Vec<String> = ids.iter().map(|i| enc(i.trim())).collect();
            let mut url = format!(
                "{b}/api/level1/recall?policy={}&ids={}",
                u("policy"),
                ids.join(",")
            );
            if let Some(r) = args["root"].as_str().filter(|r| !r.trim().is_empty()) {
                url.push_str(&format!("&root={}", enc(r.trim())));
            }
            ("GET", url, None)
        }
        "watch_address" => (
            "POST",
            format!("{b}/api/watch"),
            Some(
                json!({ "address": s("address"), "chain": args["chain"].as_str().unwrap_or("arc-testnet"), "days": args["days"].as_u64().unwrap_or(90) }),
            ),
        ),
        other => {
            return Err(refusal::refuse(
                "invalid_body",
                Executed::No,
                "",
                "Call one of the tools tools/list names.",
                format!("unknown tool {other}"),
                None,
            ));
        }
    };
    let mut req = match method {
        "GET" => up.http.get(&url),
        _ => up.http.post(&url).json(&body.unwrap_or(json!({}))),
    };
    // PLAN 5.31: the capability the owner's passkey minted for this agent.
    if let Some(t) = &up.token {
        req = req.bearer_auth(t);
    }
    // A payment is named by the agent's operation id: the same id never pays twice.
    if name == "pay" && !s("operationId").trim().is_empty() {
        req = req.header("Idempotency-Key", s("operationId").trim());
    }
    let resp = req.send().await.map_err(|e| transport_refusal(&url, &e))?;
    let status = resp.status();
    // A stored answer to a repeated request: the header does not reach an MCP
    // client, so the result says it (docs/60 P-283).
    let replayed = resp
        .headers()
        .get("idempotent-replayed")
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"true"));
    let text = resp.text().await.map_err(|e| transport_refusal(&url, &e))?;
    if !status.is_success() {
        let code = status.as_u16();
        let v = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({ "error": text }));
        // A service older than the envelope answers the sentence alone.
        let money = matches!(name, "buy_service" | "pay");
        return Err(refusal::envelope(code, &v, money).unwrap_or(v));
    }
    // Purchases come as NDJSON (one receipt per line): an array for the model.
    if name == "purchases" {
        let lines: Vec<Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        return Ok(refusal::ok(json!({ "purchases": lines })));
    }
    let mut v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "text": text }));
    if replayed && v.is_object() {
        v["replayed"] = json!(true);
    }
    // The inbox, narrowed to what the tool is about.
    Ok(refusal::ok(match name {
        "invoices" => json!({ "invoices": v["invoices"] }),
        "top" => json!({
            "policy": v["policy"], "rate": v["rate"], "windowDays": v["windowDays"],
            "runway": v["runway"], "forces": v["forces"],
        }),
        _ => v,
    }))
}

/// The service did not answer: a refusal in the envelope, `words` as before.
/// No answer in time may still complete (`executed: unknown`); a refused
/// connection did nothing.
fn transport_refusal(url: &str, e: &reqwest::Error) -> Value {
    let (code, executed) = if e.is_timeout() {
        ("upstream_unavailable", Executed::Unknown)
    } else {
        ("service_unavailable", Executed::No)
    };
    refusal::refuse(code, executed, "", "", transport_words(url, e), None)
}

/// What an agent reads when the service did not answer (docs/60 P-184): "error
/// sending request" read as "the service is down" after a slow read.
fn transport_words(url: &str, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        format!(
            "no answer from {url} in {} s; the call may still complete. Read treasury_inbox before retrying, so nothing is asked twice.",
            timeout_secs()
        )
    } else if e.is_connect() {
        format!(
            "the Tami service at {url} is not reachable ({e}); check TAMEION_URL and that the service runs"
        )
    } else {
        format!("{url}: {e}")
    }
}

fn reply(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// What the server tells the calling model at `initialize`.
const INSTRUCTIONS: &str = "Tami is the account an agent operator gives its agents: they pay in USDC (paid APIs over x402, bills and invoices) only to payees the owner approved and only up to the mandate one contract enforces; a raise waits a delay anyone can see, a cut is instant, and every payment lands in a book anchored on chain. These tools read the account, run a pass, and prepare changes the owner signs with a passkey; none of them can add a payee, raise a cap or withdraw. Two spend, both with an agent key that has the buy scope: buy_service has the service pay a seller the owner already approved, inside the caps, and pay pays an approved payee directly, inside the caps, with the reason written to the book. Start with treasury_inbox; what waits for the owner is its `waiting` list. Before you rely on something you remember about the account, check it with recall: the record and the chain answer, not your memory.";

/// One JSON-RPC message, either transport. `None` for a notification or a
/// response (no `id`, or no `method`): those get no reply.
pub async fn handle(up: &Upstream, msg: &Value) -> Option<Value> {
    let id = msg["id"].clone();
    let method = msg["method"].as_str()?;
    if id.is_null() {
        return None;
    }
    let params = &msg["params"];
    Some(match method {
        "initialize" => reply(
            &id,
            json!({
                "protocolVersion": PROTOCOL,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tameion", "version": refusal::CONTRACT_VERSION },
                "instructions": INSTRUCTIONS
            }),
        ),
        "ping" => reply(&id, json!({})),
        "tools/list" => reply(&id, json!({ "tools": tools() })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            // docs/115 §2 (MCP 2025-06-18): the whole reply as structuredContent;
            // a refusal's text is its `error` object.
            match call(up, name, &args).await {
                Ok(v) => reply(
                    &id,
                    json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }], "structuredContent": v, "isError": false }),
                ),
                Err(v) => reply(
                    &id,
                    json!({ "content": [{ "type": "text", "text": v["error"].to_string() }], "structuredContent": v, "isError": true }),
                ),
            }
        }
        other => error(&id, -32601, format!("method not found: {other}")),
    })
}

/// A request line of stdio: the reply to write, if any (a line that is not
/// JSON is a parse error with a null id).
pub async fn stdio_line(up: &Upstream, line: &str) -> Option<Value> {
    match serde_json::from_str::<Value>(line) {
        Ok(msg) => handle(up, &msg).await,
        Err(e) => Some(error(&Value::Null, -32700, format!("parse error: {e}"))),
    }
}

/// Protocol versions this endpoint takes in `MCP-Protocol-Version`: ours, and
/// the one the spec says to assume when the header is absent.
const HTTP_VERSIONS: &[&str] = &[PROTOCOL, "2025-03-26"];

/// `/mcp` on the service: the Streamable HTTP transport (MCP 2025-06-18,
/// basic/transports), the subset a tools-only server needs. A POST carries one
/// JSON-RPC message: a request is answered with one `application/json` object,
/// a notification or a response with 202 and no body. No SSE stream and no
/// session: the server never speaks first, so GET (and DELETE) answer 405, as
/// the spec allows, and no `Mcp-Session-Id` is issued. Tool calls go to this
/// same service over loopback with the caller's `Authorization: Bearer`, so
/// every route keeps its own key check and envelope. A browser `Origin` other
/// than this machine or `TAMEION_PUBLIC_URL` is refused (the spec's
/// DNS-rebinding rule). Returns the whole HTTP response.
pub async fn http(method: &str, raw: &str, body: &str) -> Vec<u8> {
    if !origin_allowed(header(raw, "origin")) {
        return rpc_reply(
            "403 Forbidden",
            &error(
                &Value::Null,
                -32600,
                "this Origin may not call the MCP endpoint".into(),
            ),
        );
    }
    if method != "POST" {
        let v = error(
            &Value::Null,
            -32600,
            "POST one JSON-RPC message to this endpoint; it opens no server stream".into(),
        );
        let b = v.to_string();
        return format!(
            "HTTP/1.1 405 Method Not Allowed\r\nAllow: POST\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{b}",
            b.len()
        )
        .into_bytes();
    }
    if let Some(v) = header(raw, "mcp-protocol-version")
        && !HTTP_VERSIONS.contains(&v)
    {
        return rpc_reply(
            "400 Bad Request",
            &error(
                &Value::Null,
                -32600,
                format!("unsupported MCP-Protocol-Version {v}; this server speaks {PROTOCOL}"),
            ),
        );
    }
    let msg = match serde_json::from_str::<Value>(body) {
        Ok(v) if v.is_object() => v,
        Ok(_) => {
            return rpc_reply(
                "400 Bad Request",
                &error(
                    &Value::Null,
                    -32600,
                    "one JSON-RPC message per POST (no batches)".into(),
                ),
            );
        }
        Err(e) => {
            return rpc_reply(
                "400 Bad Request",
                &error(&Value::Null, -32700, format!("parse error: {e}")),
            );
        }
    };
    let up = Upstream {
        http: client(),
        base: self_base(),
        token: bearer(raw),
    };
    match handle(&up, &msg).await {
        Some(answer) => rpc_reply("200 OK", &answer),
        None => b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n".to_vec(),
    }
}

fn rpc_reply(status: &str, v: &Value) -> Vec<u8> {
    let b = v.to_string();
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{b}",
        b.len()
    )
    .into_bytes()
}

fn header<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    raw.lines().take_while(|l| !l.is_empty()).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

fn bearer(raw: &str) -> Option<String> {
    header(raw, "authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// This service as its own loopback client sees it: `PORT` and `HOST` with
/// `tameion-app`'s defaults (7880, 127.0.0.1); a wildcard bind is reached on
/// loopback.
fn self_base() -> String {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(7880);
    let host = match std::env::var("HOST").unwrap_or_default().as_str() {
        "" | "0.0.0.0" | "localhost" => "127.0.0.1".to_string(),
        "::" | "::1" => "[::1]".to_string(),
        h if h.contains(':') => format!("[{h}]"),
        h => h.to_string(),
    };
    format!("http://{host}:{port}")
}

/// No `Origin` (an agent, not a browser), this machine, or the public copy's
/// own origin (`TAMEION_PUBLIC_URL`). Also the origin a passkey assertion must
/// come from (`level1::verify_passkey_assertion`).
pub(crate) fn origin_allowed(origin: Option<&str>) -> bool {
    let Some(o) = origin.map(|o| o.trim().trim_end_matches('/')) else {
        return true;
    };
    let authority = o.split_once("://").map_or("", |(_, a)| a);
    let host = authority
        .rsplit_once(':')
        .filter(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(authority, |(h, _)| h);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
        || std::env::var("TAMEION_PUBLIC_URL")
            .is_ok_and(|u| u.trim().trim_end_matches('/').eq_ignore_ascii_case(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PLAN 5.42 (docs/62): every tool says whether it only reads, so an MCP
    /// host can ask before a call that changes anything. buy_service and pay
    /// spend.
    #[test]
    fn every_tool_declares_read_only_or_not() {
        for t in tools() {
            let a = &t["annotations"];
            assert!(
                a["readOnlyHint"].is_boolean(),
                "{} lacks readOnlyHint",
                t["name"]
            );
            let name = t["name"].as_str().unwrap();
            // Spending is the one call that cannot be undone (PLAN 5.54).
            assert_eq!(
                a["destructiveHint"],
                matches!(name, "buy_service" | "pay"),
                "{name}: only spending is destructive"
            );
            if name.starts_with("prepare_")
                || matches!(
                    name,
                    "treasury_inbox"
                        | "top"
                        | "pass_status"
                        | "records"
                        | "record_events"
                        | "recall"
                        | "invoices"
                        | "purchases"
                        | "desk"
                )
            {
                assert_eq!(a["readOnlyHint"], true, "{name} only reads or prepares");
            }
        }
        let pay = tools().into_iter().find(|t| t["name"] == "pay").unwrap();
        let required = pay["inputSchema"]["required"].as_array().unwrap();
        for k in ["policy", "payee", "amount", "why", "operationId"] {
            assert!(required.contains(&json!(k)), "pay needs {k}");
        }
    }

    #[test]
    fn a_tool_argument_cannot_reach_another_route() {
        assert_eq!(
            enc("0xdaf055fff0ad86f112a5f7f9055eee4e5a9ad4c1"),
            "0xdaf055fff0ad86f112a5f7f9055eee4e5a9ad4c1"
        );
        assert_eq!(enc("6acfc3a5-0001"), "6acfc3a5-0001");
        assert_eq!(
            enc("../desk/live?device=x&policy=y"),
            "..%2Fdesk%2Flive%3Fdevice%3Dx%26policy%3Dy"
        );
    }

    #[test]
    fn only_agents_this_machine_and_the_public_copy_may_post() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("http://localhost:3000")));
        assert!(origin_allowed(Some("http://127.0.0.1:7880")));
        assert!(origin_allowed(Some("http://[::1]:7880")));
        assert!(!origin_allowed(Some("https://evil.example")));
        assert!(!origin_allowed(Some("http://localhost.evil.example")));
        assert!(!origin_allowed(Some("null")));
    }

    /// docs/60 P-283: a stored answer to a repeated pay says so in the result,
    /// since the `Idempotent-Replayed` header never reaches an MCP client.
    #[tokio::test]
    async fn a_replayed_answer_says_so_in_the_result() {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap();
            let body = r#"{"paid":true,"payment":{"tx":"0x01"}}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nIdempotent-Replayed: true\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            String::from_utf8_lossy(&buf[..n]).to_string()
        });
        let up = Upstream {
            http: client(),
            base: format!("http://127.0.0.1:{port}"),
            token: None,
        };
        let args = json!({ "policy": "0x01", "payee": "0x02", "amount": "0.001", "why": "rent", "operationId": "op_12345678" });
        let v = call(&up, "pay", &args).await.unwrap();
        assert_eq!(v["replayed"], true);
        assert_eq!(v["payment"]["tx"], "0x01");
        let req = seen.join().unwrap().to_ascii_lowercase();
        assert!(req.starts_with("post /api/level1/pay "), "{req}");
        assert!(req.contains("idempotency-key: op_12345678"), "{req}");
    }

    /// docs/117 §8: recall's ids and root are escaped into one GET, so an id
    /// cannot add a parameter; a string of ids is split like an array.
    #[tokio::test]
    async fn recall_escapes_its_ids_into_one_get() {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = std::thread::spawn(move || {
            let mut lines = Vec::new();
            for _ in 0..2 {
                let (mut s, _) = l.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap();
                let body = r#"{"results":[]}"#;
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                lines.push(
                    String::from_utf8_lossy(&buf[..n])
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string(),
                );
            }
            lines
        });
        let up = Upstream {
            http: client(),
            base: format!("http://127.0.0.1:{port}"),
            token: None,
        };
        let a = json!({ "policy": "0x01", "ids": ["evt:1", "evt:2&root=0x00"], "root": "0xab" });
        call(&up, "recall", &a).await.unwrap();
        let b = json!({ "policy": "0x01", "ids": "evt:0, evt:4" });
        call(&up, "recall", &b).await.unwrap();
        let lines = seen.join().unwrap();
        assert_eq!(
            lines[0],
            "GET /api/level1/recall?policy=0x01&ids=evt%3A1,evt%3A2%26root%3D0x00&root=0xab HTTP/1.1"
        );
        assert_eq!(
            lines[1],
            "GET /api/level1/recall?policy=0x01&ids=evt%3A0,evt%3A4 HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn notifications_and_responses_get_no_reply_and_requests_do() {
        let up = Upstream {
            http: client(),
            base: "http://127.0.0.1:9".into(),
            token: None,
        };
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(handle(&up, &note).await, None);
        let response = json!({ "jsonrpc": "2.0", "id": 7, "result": {} });
        assert_eq!(handle(&up, &response).await, None);
        let list = handle(
            &up,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await
        .unwrap();
        assert_eq!(list["result"]["tools"], json!(tools()));
        let bad = stdio_line(&up, "{not json").await.unwrap();
        assert_eq!(bad["error"]["code"], -32700);
    }
}
