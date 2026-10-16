//! Tameion as a tool for other agents (docs/54 §3, PLAN 5.11): a Model
//! Context Protocol server over stdio that a company's own agent — Claude
//! Code, OpenClaw, Hermes, anything that speaks MCP — plugs in to ask what
//! the treasury owes, run a pass, or prepare a payment.
//!
//! It is a thin, read-mostly client of the hosted service (`tameion-app`):
//! every tool maps to one HTTP route, and this binary holds no key. "Prepare"
//! tools return the exact calldata and its description in words for the
//! owner's passkey; the authority stays where the contract put it. An
//! agent that calls this cannot pay anyone the owner did not approve, raise
//! a cap, or withdraw — it can see, run and prepare, and with a `buy` key ask
//! the service to pay an approved seller inside the caps (buy_service).
//!
//! Protocol: JSON-RPC 2.0 over newline-delimited stdio, MCP 2025-06-18
//! subset — `initialize`, `notifications/initialized`, `ping`,
//! `tools/list`, `tools/call`. No crate: the surface is five methods.
//!
//! Environment:
//!   TAMEION_URL   the service, default http://127.0.0.1:7880
//!   TAMEION_TOKEN the agent key the owner minted on the account page (PLAN 5.31), sent as Bearer
//!   TAMEION_MCP_TIMEOUT_SECS how long a call waits for the service, default 60
//!
//! Claude Code: `claude mcp add tameion -- cargo run -q -p operator --bin tameion-mcp`
//! (or the built binary), then ask it what the treasury owes.

use serde_json::{Value, json};
use std::io::{BufRead, Write};

const PROTOCOL: &str = "2025-06-18";
/// How long one tool call waits for the service: `TAMEION_MCP_TIMEOUT_SECS`,
/// default 60.
fn timeout_secs() -> u64 {
    std::env::var("TAMEION_MCP_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|s| *s > 0)
        .unwrap_or(60)
}

fn base() -> String {
    std::env::var("TAMEION_URL")
        .ok()
        .map(|u| u.trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:7880".into())
}

/// The tools, each one HTTP route of the service. Descriptions are written
/// for the calling model: what it gets, and what it cannot do.
fn tools() -> Vec<Value> {
    let addr = json!({ "type": "string", "description": "0x… address" });
    vec![
        json!({
            "name": "treasury_inbox",
            "annotations": { "title": "Treasury inbox", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true },
            "description": "The account in one read. What waits for the owner is `waiting` (with `waitingCount`): every bill to approve, seller to approve and invoice, the same items the account page counts — read that field, not one list alone. `caps` has perAction, daily and floor, plus `spent24h` (spent in the trailing 24 hours) and `leftNow` (what the contract still allows right now), read from the contract. `payees`: who the agent may pay, with `payableFrom` (unix) when each one's cooling-off ends. `pending`: a change the owner signed that waits its delay, in `words` (only what is pending). `purchases`: every paid-API buy, refused ones with why; `refused` is only the last pass's bill checks. Your own requests (from prepare_limits / prepare_mode / prepare_payees with an agent key) appear as `agentAsks`, `suggestedLimit`, `recommendAutopilot` and seller rows: one tap each for the owner. Also: mode, balance, what was paid, the runway, the last pass, invoices, owners. Read-only.",
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
            "description": "EIP-712 data for the BILL WALLET to sign to move its USDC held in Circle Gateway on another chain (sourceDomain: 3 Arbitrum Sepolia, 6 Base Sepolia, 0 Ethereum Sepolia, 26 Arc Testnet) into the policy. Fee and expiry come from Circle's estimate; the data names the policy as the only recipient. This tool cannot sign; the owner signs on the Tameion page (or with a key file via scripts/gateway_fund.sh) and the service relays the mint.",
            "inputSchema": { "type": "object", "required": ["policy", "amount", "sourceDomain"], "properties": { "policy": addr, "amount": { "type": "string" }, "sourceDomain": { "type": "integer" }, "from": addr } }
        }),
        json!({
            "name": "prepare_schedule",
            "annotations": { "title": "Prepare standing order (owner signs)", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false },
            "description": "Calldata for the OWNER'S PASSKEY to set a standing order on the policy: payee (must already be approved), USDC amount, everyDays, optional firstDue (unix). Once set, anyone may call payDue(id) on its due date inside the caps — the agent does it on its passes, and the bill pays even if Tameion is down. Cannot sign.",
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
            "description": "Bill a Tameion account as a vendor: post an invoice the PAYEE has already signed (EIP-712 Invoice(address payee,address policy,uint256 amount,uint64 dueAt,uint32 everyDays,uint16 discountBps,uint16 earlyDays,bytes32 ref,uint64 issuedAt), domain name \"Tameion Invoice\", version \"1\", chainId of the account's chain, verifyingContract = the account). This tool holds no key and signs nothing: pass the body you signed elsewhere. Nothing is paid on receipt — the account's agent checks it and the owner accepts or declines. A ref already sent is refused; twenty invoices an hour per account. amount is USDC minor units as a string (\"150000\" = 0.15 USDC).",
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

async fn call(http: &reqwest::Client, name: &str, args: &Value) -> Result<Value, String> {
    let b = base();
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    let (method, url, body): (&str, String, Option<Value>) = match name {
        "treasury_inbox" => (
            "GET",
            format!("{b}/api/level1/inbox?policy={}", s("policy")),
            None,
        ),
        "run_pass" => (
            "POST",
            format!("{b}/api/level1/run"),
            Some(json!({ "policy": s("policy") })),
        ),
        "pass_status" => ("GET", format!("{b}/api/watch/{}", s("id")), None),
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
        "invoices" => (
            "GET",
            format!("{b}/api/level1/inbox?policy={}", s("policy")),
            None,
        ),
        "purchases" => (
            "GET",
            format!("{b}/api/level1/purchases.jsonl?policy={}", s("policy")),
            None,
        ),
        "send_invoice" => (
            "POST",
            format!("{b}/api/level1/invoice"),
            Some(args["invoice"].clone()),
        ),
        "desk" => ("GET", format!("{b}/api/desk?policy={}", s("policy")), None),
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
            format!("{b}/api/level1/records?policy={}", s("policy")),
            None,
        ),
        "record_events" => (
            "GET",
            format!(
                "{b}/api/level1/record?policy={}&root={}",
                s("policy"),
                s("root")
            ),
            None,
        ),
        "watch_address" => (
            "POST",
            format!("{b}/api/watch"),
            Some(
                json!({ "address": s("address"), "chain": args["chain"].as_str().unwrap_or("arc-testnet"), "days": args["days"].as_u64().unwrap_or(90) }),
            ),
        ),
        other => return Err(format!("unknown tool {other}")),
    };
    let mut req = match method {
        "GET" => http.get(&url),
        _ => http.post(&url).json(&body.unwrap_or(json!({}))),
    };
    // PLAN 5.31: the capability the owner's passkey minted for this agent.
    if let Some(t) = std::env::var("TAMEION_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
    {
        req = req.bearer_auth(t.trim());
    }
    let resp = req.send().await.map_err(|e| transport_words(&url, &e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("{status}: {text}"));
    }
    // Purchases come as NDJSON (one receipt per line): an array for the model.
    if name == "purchases" {
        let lines: Vec<Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        return Ok(json!({ "purchases": lines }));
    }
    let v = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "text": text }));
    // The inbox, narrowed to what the tool is about.
    Ok(if name == "invoices" {
        v["invoices"].clone()
    } else {
        v
    })
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
            "the Tameion service at {url} is not reachable ({e}); check TAMEION_URL and that the service runs"
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

#[tokio::main]
async fn main() {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs()))
        .build()
        .expect("http client");
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(
                    out,
                    "{}",
                    error(&Value::Null, -32700, format!("parse error: {e}"))
                );
                continue;
            }
        };
        let id = msg["id"].clone();
        let method = msg["method"].as_str().unwrap_or("");
        let params = msg["params"].clone();
        // Notifications carry no id and get no reply.
        if id.is_null() {
            continue;
        }
        let answer = match method {
            "initialize" => reply(
                &id,
                json!({
                    "protocolVersion": PROTOCOL,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "tameion", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "Tameion is the account an agent operator gives its agents: they pay in USDC (paid APIs over x402, bills and invoices) only to payees the owner approved and only up to the mandate one contract enforces; a raise waits a delay anyone can see, a cut is instant, and every payment lands in a book anchored on chain. These tools read the account, run a pass, and prepare changes the owner signs with a passkey; none of them can add a payee, raise a cap or withdraw. One spends: buy_service has the service pay a seller the owner already approved, inside the caps (it needs an agent key with the buy scope). Start with treasury_inbox; what waits for the owner is its `waiting` list."
                }),
            ),
            "ping" => reply(&id, json!({})),
            "tools/list" => reply(&id, json!({ "tools": tools() })),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match call(&http, name, &args).await {
                    Ok(v) => reply(
                        &id,
                        json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }], "isError": false }),
                    ),
                    Err(e) => reply(
                        &id,
                        json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
                    ),
                }
            }
            other => error(&id, -32601, format!("method not found: {other}")),
        };
        let _ = writeln!(out, "{answer}");
        let _ = out.flush();
    }
}

#[cfg(test)]
mod annotation_tests {
    use super::tools;

    /// PLAN 5.42 (docs/62): every tool says whether it only reads, so an MCP
    /// host can ask before a call that changes anything. buy_service spends.
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
                name == "buy_service",
                "{name}: only buying is destructive"
            );
            if name.starts_with("prepare_")
                || matches!(
                    name,
                    "treasury_inbox"
                        | "pass_status"
                        | "records"
                        | "record_events"
                        | "invoices"
                        | "purchases"
                        | "desk"
                )
            {
                assert_eq!(a["readOnlyHint"], true, "{name} only reads or prepares");
            }
        }
    }
}
