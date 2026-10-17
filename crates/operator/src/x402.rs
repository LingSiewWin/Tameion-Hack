//! Paid APIs inside the caps (PLAN 5.54).
//!
//! A service that wants paying answers HTTP 402 with `PAYMENT-REQUIRED`
//! (x402 v2, HTTP transport). The agent picks the offer it can pay from the
//! account — scheme `exact`, this chain, USDC — signs one purchase (seller,
//! amount, nonce, deadline) with its key, and retries with
//! `PAYMENT-SIGNATURE` carrying the spec's ERC-7710 payload: the policy is
//! both the delegation manager and the delegator. The seller's facilitator
//! redeems it on the policy, which applies everything `pay` applies — the
//! approved payee, the caps, the 24-hour window, the floor, Autopilot, an
//! unlapsed agent — and pays the gas. The agent holds no money and no gas.
//!
//! What the owner controls: buying is closed until the passkey mints an agent
//! key with the `buy` scope; every seller is a payee the owner approved; the
//! caller names the most it will pay per call. A seller nobody approved is
//! written down as "needs you", not paid.
//!
//! # Receipt v1 (docs/76 §6, 2026-10-15)
//!
//! Every purchase line is a receipt the owner can audit, not only a log:
//! besides `url`, `seller` (the offer's `payTo`), `amount`/`minor`, `network`
//! and `tx` (the settlement's `transaction`) it carries
//!
//! - `id` — the x402 `payment-identifier` extension's `id` when the seller
//!   advertises the extension (`PaymentRequired.extensions["payment-identifier"]`,
//!   spec: `specs/extensions/payment_identifier.md`), which this client sets
//!   to `pay_<purchase digest>` — the key the contract itself de-duplicates on
//!   — and echoes in `PaymentPayload.extensions["payment-identifier"].info.id`;
//!   otherwise our own deterministic id, the settlement transaction hash
//!   (`idFrom` says which);
//! - `receipt` — the seller's signed receipt when it sends one, read from
//!   `SettlementResponse.extensions["offer-receipt"].info.receipt`
//!   (`{format, payload{version, network, resourceUrl, payer, issuedAt,
//!   transaction?}, signature}`, spec: `specs/extensions/extension-offer-and-receipt.md`
//!   §5), kept verbatim and not verified here;
//! - `sha256` — of the resource bytes the seller returned (every byte read,
//!   even past the 64 KiB kept), so the answer the agent acted on is pinned.
//!
//! Settled purchases are also bill evidence ([`receipt_bills`]): a seller
//! bought from on a cadence is a bill in the account's report, with
//! provenance "x402 receipt".

use crate::level1::{self, Tenant};
use alloy::primitives::{Address, B256};
use alloy::signers::SignerSync;
use ledger::obligations::{Counterparty, DetectionConfig, Direction, Obligation, Transfer, detect};
use ledger::{Currency, Money, Timestamp};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// x402 v2 HTTP transport headers.
pub const PAYMENT_REQUIRED: &str = "payment-required";
pub const PAYMENT_SIGNATURE: &str = "PAYMENT-SIGNATURE";
pub const PAYMENT_RESPONSE: &str = "payment-response";

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

pub fn encode_header(v: &Value) -> String {
    B64.encode(v.to_string())
}

pub fn decode_header(s: &str) -> Result<Value, String> {
    let raw = B64
        .decode(s.trim())
        .map_err(|e| format!("not base64: {e}"))?;
    serde_json::from_slice(&raw).map_err(|e| format!("not JSON: {e}"))
}

/// The offer in `required.accepts` this account can pay: `exact`, on
/// `eip155:<chain_id>`, in `usdc`.
pub fn pick_offer(required: &Value, chain_id: u64, usdc: Address) -> Option<Value> {
    let network = format!("eip155:{chain_id}");
    required["accepts"].as_array()?.iter().find_map(|o| {
        let asset = Address::from_str(o["asset"].as_str()?).ok()?;
        (o["scheme"] == "exact" && o["network"] == network.as_str() && asset == usdc)
            .then(|| o.clone())
    })
}

/// Seller, amount (USDC minor units) and timeout of an offer.
pub fn offer_terms(o: &Value) -> Result<(Address, u128, u64), String> {
    let pay_to = o["payTo"]
        .as_str()
        .and_then(|a| Address::from_str(a).ok())
        .ok_or("offer has no payTo address")?;
    let amount = o["amount"]
        .as_str()
        .and_then(|a| a.parse::<u128>().ok())
        .filter(|a| *a > 0)
        .ok_or("offer has no positive integer amount")?;
    let timeout = o["maxTimeoutSeconds"].as_u64().unwrap_or(60);
    Ok((pay_to, amount, timeout))
}

/// The `PaymentPayload` for the ERC-7710 method: the offer echoed back with
/// `assetTransferMethod: erc7710`, the policy as manager and delegator.
pub fn payment_payload(required: &Value, offer: &Value, policy: Address, context: &[u8]) -> Value {
    let mut accepted = offer.clone();
    if !accepted["extra"].is_object() {
        accepted["extra"] = json!({});
    }
    accepted["extra"]["assetTransferMethod"] = json!("erc7710");
    json!({
        "x402Version": 2,
        "resource": required["resource"],
        "accepted": accepted,
        "payload": {
            "delegationManager": format!("{policy:#x}"),
            "permissionContext": alloy::hex::encode_prefixed(context),
            "delegator": format!("{policy:#x}"),
        }
    })
}

/// The `payment-identifier` extension key, as the spec names it.
pub const PAYMENT_IDENTIFIER: &str = "payment-identifier";
/// The `offer-receipt` extension key, as the spec names it.
pub const OFFER_RECEIPT: &str = "offer-receipt";

/// Whether the seller's `PaymentRequired` advertises `payment-identifier`
/// (`extensions["payment-identifier"]` present; its `info.required` says
/// whether an id is mandatory, and this client always sends one when the
/// extension is there).
pub fn wants_payment_identifier(required: &Value) -> bool {
    required["extensions"][PAYMENT_IDENTIFIER].is_object()
}

/// This client's idempotency key for a purchase: `pay_<digest>` — the spec
/// recommends a prefixed unique id of 16–128 chars from `[A-Za-z0-9_-]`;
/// the purchase digest is unique per signature and is exactly what the
/// contract refuses twice (`AlreadyExecuted`), so a seller or facilitator
/// de-duplicating on the id and the chain agree on what "the same payment" is.
pub fn payment_identifier(digest: B256) -> String {
    format!("pay_{}", alloy::hex::encode(digest))
}

/// Echo the seller's extension and append the `id`, as the spec's
/// `PaymentPayload` example does (`extensions["payment-identifier"].info.id`).
pub fn with_payment_identifier(payload: &mut Value, required: &Value, id: &str) {
    let mut ext = required["extensions"][PAYMENT_IDENTIFIER].clone();
    if !ext["info"].is_object() {
        ext["info"] = json!({ "required": false });
    }
    ext["info"]["id"] = json!(id);
    payload["extensions"][PAYMENT_IDENTIFIER] = ext;
}

/// The seller's signed receipt in a settlement response, if any:
/// `extensions["offer-receipt"].info.receipt` (spec §5.1), returned only on
/// success. Kept as sent; verifying the signature is a later step.
pub fn receipt_of(settlement: &Value) -> Option<Value> {
    let r = &settlement["extensions"][OFFER_RECEIPT]["info"]["receipt"];
    r.is_object().then(|| r.clone())
}

/// The policy's refusals, in the owner's words, found by selector in a
/// node's revert message.
pub fn refusal_words(err: &str) -> Option<&'static str> {
    const WORDS: &[(&str, &str)] = &[
        (
            "NotAutopilot(uint8)",
            // docs/60 P-174: said where the owner reads refusals, so the words name the fix.
            "the agent buys only on Autopilot: this account asks before paying, so the purchase was refused; turn Autopilot on to let it buy inside your limits",
        ),
        (
            "AgentLapsed(uint64)",
            "the agent's authority has lapsed; renew it on the account page",
        ),
        (
            "PayeeNotAttested(address)",
            "the seller is not one of your approved payees",
        ),
        (
            "PayeeCoolingOff(address,uint256)",
            "the seller was approved too recently; it can be paid once the waiting period ends",
        ),
        (
            "OverPerActionCap(uint256,uint256)",
            "the price is above the per-payment limit",
        ),
        (
            "OverDailyCap(uint256,uint256)",
            "the price is above what is left of the 24-hour limit",
        ),
        (
            "BelowReserveFloor(uint256,uint256)",
            "paying would take the balance under the floor you set",
        ),
        (
            "InsufficientBalance(uint256,uint256)",
            "the account does not hold enough",
        ),
        ("AlreadyExecuted(bytes32)", "this purchase was already paid"),
        (
            "BadAgentSignature()",
            "the purchase was not signed by this account's agent",
        ),
        (
            "PurchaseExpired(uint64)",
            "the purchase expired before the seller redeemed it",
        ),
        ("UnsupportedExecution()", "it is not a plain USDC payment"),
        ("WindowFull()", "too many payments in the last 24 hours"),
    ];
    let lower = err.to_ascii_lowercase();
    WORDS.iter().find_map(|(sig, words)| {
        let sel = alloy::hex::encode(&alloy::primitives::keccak256(sig.as_bytes())[..4]);
        lower.contains(&sel).then_some(*words)
    })
}

/// Whether an address points inside the host or its network (SSRF):
/// loopback, private, link-local, unspecified, broadcast, `0.0.0.0/8`,
/// carrier-grade NAT `100.64/10`, IPv6 unique-local and link-local, and an
/// IPv4 address mapped into IPv6 (judged as the IPv4 it carries).
fn inside(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => {
            let o = v.octets();
            v.is_loopback()
                || v.is_private()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_broadcast()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        std::net::IpAddr::V6(v) => match v.to_ipv4_mapped() {
            Some(m) => inside(std::net::IpAddr::V4(m)),
            None => {
                let s0 = v.segments()[0];
                v.is_loopback()
                    || v.is_unspecified()
                    || (s0 & 0xfe00) == 0xfc00
                    || (s0 & 0xffc0) == 0xfe80
            }
        },
    }
}

/// A URL outside the host and a client that can only reach the address that
/// was checked: every address the name resolves to must be public, the client
/// is pinned to one of them (a DNS answer that changes between the check and
/// the request — rebinding — cannot redirect it), and redirects are refused.
/// `TAMEION_X402_ALLOW_LOCAL=1` lifts the check for a local proof.
/// (Audit 2026-10-15: mapped IPv6, link-local IPv6 and 100.64/10 passed, and the
/// request resolved the name a second time.)
pub async fn public_client(
    url: &str,
    timeout: std::time::Duration,
    user_agent: &str,
) -> Result<(reqwest::Url, reqwest::Client), String> {
    let u = reqwest::Url::parse(url).map_err(|e| format!("not a URL: {e}"))?;
    if u.scheme() != "https" && u.scheme() != "http" {
        return Err("only http(s) services can be reached".into());
    }
    let builder = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(user_agent);
    if std::env::var("TAMEION_X402_ALLOW_LOCAL").as_deref() == Ok("1") {
        return Ok((u, builder.build().map_err(|e| e.to_string())?));
    }
    let host = u.host_str().ok_or("URL has no host")?.to_string();
    let port = u.port_or_known_default().unwrap_or(443);
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .collect();
    let Some(first) = addrs.first().copied() else {
        return Err(format!("{host} resolves to nothing"));
    };
    if let Some(a) = addrs.iter().find(|a| inside(a.ip())) {
        return Err(format!(
            "{host} resolves to an internal address ({})",
            a.ip()
        ));
    }
    let client = builder
        .resolve(&host, first)
        .build()
        .map_err(|e| e.to_string())?;
    Ok((u, client))
}

fn random_nonce() -> Result<B256, String> {
    use std::io::Read;
    let mut b = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| e.to_string())?;
    Ok(B256::from(b))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Where an account's purchases are written: one JSON line each.
pub fn purchases_path(app_dir: &Path, policy: &str) -> PathBuf {
    level1::tenants_dir(app_dir).join(format!("{}.purchases.jsonl", policy.to_lowercase()))
}

/// Purchases signed and handed to a seller whose outcome is not known yet:
/// seller (lowercase) -> {digest, deadline}. Until the deadline the seller can
/// still redeem the signature, so another purchase from it waits (audit
/// 2026-10-15, docs/60 P-75: a stalled seller made the caller retry, the retry
/// signed a new nonce, and the seller redeemed both).
fn pending_path(app_dir: &Path, policy: &str) -> PathBuf {
    level1::tenants_dir(app_dir).join(format!("{}.purchases.pending.json", policy.to_lowercase()))
}

fn pending(app_dir: &Path, policy: &str) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(pending_path(app_dir, policy))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn set_pending(app_dir: &Path, policy: &str, seller: &str, entry: Option<Value>) {
    let mut m = pending(app_dir, policy);
    match entry {
        Some(e) => {
            m.insert(seller.to_lowercase(), e);
        }
        None => {
            m.remove(&seller.to_lowercase());
        }
    }
    let _ = std::fs::write(pending_path(app_dir, policy), Value::Object(m).to_string());
}

/// The P-75 guard before a new purchase from `seller`: an earlier signature the
/// chain does not show spent (`spent`, the caller asks `executed` for its
/// digest) and whose deadline has not passed refuses it, in words; otherwise
/// the earlier entry is cleared and the purchase goes on.
fn guard(app_dir: &Path, policy: &str, seller: &str, spent: bool, now: u64) -> Result<(), String> {
    if let Some(p) = pending(app_dir, policy).get(&seller.to_lowercase()) {
        let deadline = p["deadline"].as_u64().unwrap_or(0);
        if !spent && now <= deadline {
            return Err(format!(
                "an earlier purchase from this seller can still be redeemed until {deadline} (unix); wait for it before buying again, so it is never paid twice"
            ));
        }
        set_pending(app_dir, policy, seller, None);
    }
    Ok(())
}

/// This seller's usual price for the account: the median of its last five paid
/// purchases, USDC minor units; `None` before the first (docs/60 P-217).
pub fn usual_price(app_dir: &Path, policy: &str, seller: &str) -> Option<u128> {
    let mut last: Vec<u128> = purchases(app_dir, policy)
        .iter()
        .filter(|p| p["status"] == "paid")
        .filter(|p| {
            p["seller"]
                .as_str()
                .is_some_and(|x| x.eq_ignore_ascii_case(seller))
        })
        .filter_map(|p| p["minor"].as_str().and_then(|m| m.parse().ok()))
        .take(5)
        .collect();
    if last.is_empty() {
        return None;
    }
    last.sort_unstable();
    Some(last[last.len() / 2])
}

/// What code allows for one purchase, in the decider's terms (docs/60 P-217): a
/// price more than a quarter above the seller's usual goes to the owner — ask or
/// refuse, never pay — as a bill's amount off its usual does; a first purchase
/// has no usual and no band. Computed by the decider's own rule for a verdict.
pub fn purchase_allowed(price: u128, usual: Option<u128>) -> Vec<crate::decide::Action> {
    let off_band = usual.is_some_and(|u| price.saturating_mul(4) > u.saturating_mul(5));
    let verdict = if off_band {
        ledger::Decision::Review
    } else {
        ledger::Decision::Allow
    };
    crate::decide::allowed_for_bill(verdict, true, false, 0)
}

fn price_approvals_path(app_dir: &Path, policy: &str) -> PathBuf {
    level1::tenants_dir(app_dir).join(format!("{}.price-approvals.json", policy.to_lowercase()))
}

/// The owner's tap on a "Let the agent buy from … at …" row: the next purchase
/// from `seller` at up to `max_minor` passes the price band once, until `until`.
pub fn approve_price(app_dir: &Path, policy: &str, seller: &str, max_minor: u128, until: u64) {
    let path = price_approvals_path(app_dir, policy);
    let mut m = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    m.insert(
        seller.to_lowercase(),
        json!({ "max": max_minor.to_string(), "until": until }),
    );
    let _ = std::fs::write(path, Value::Object(m).to_string());
}

/// Whether the owner approved this price for this seller (and use it up).
fn take_price_approval(app_dir: &Path, policy: &str, seller: &str, price: u128, now: u64) -> bool {
    let path = price_approvals_path(app_dir, policy);
    let Some(mut m) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
    else {
        return false;
    };
    let key = seller.to_lowercase();
    let ok = m.get(&key).is_some_and(|a| {
        a["until"].as_u64().unwrap_or(0) >= now
            && a["max"]
                .as_str()
                .and_then(|x| x.parse::<u128>().ok())
                .unwrap_or(0)
                >= price
    });
    if ok {
        m.remove(&key);
        let _ = std::fs::write(path, Value::Object(m).to_string());
    }
    ok
}

/// The account's purchase lock (docs/60 P-214). ponytail: one lock per account
/// for the life of the process; a lock per seller if one account must buy from
/// many sellers at the same instant.
fn account_lock(policy: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    type Locks = std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>;
    static LOCKS: std::sync::OnceLock<std::sync::Mutex<Locks>> = std::sync::OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(policy.to_lowercase())
        .or_default()
        .clone()
}

/// The P-75 rule as one step under the account's lock (docs/60 P-214): read the
/// seller's pending signature, ask whether the chain spent it (`spent`), judge
/// (`guard`), sign and simulate (`sign`, which returns its result and the new
/// pending entry), and record that entry before the lock is let go. Without
/// the lock two concurrent buys from one seller both passed the check and both
/// were signed; now the second finds the first pending and is refused in words.
/// `Err` is the guard's refusal; `Ok(Err)` is `sign`'s.
async fn one_signature_per_seller<T, S, SF, G, GF>(
    app_dir: &Path,
    policy: &str,
    seller: &str,
    spent: S,
    sign: G,
) -> Result<Result<T, String>, String>
where
    S: FnOnce(B256) -> SF,
    SF: std::future::Future<Output = bool>,
    G: FnOnce() -> GF,
    GF: std::future::Future<Output = Result<(T, Value), String>>,
{
    let lock = account_lock(policy);
    let _held = lock.lock().await;
    let held = pending(app_dir, policy)
        .get(&seller.to_lowercase())
        .and_then(|p| p["digest"].as_str().and_then(|d| B256::from_str(d).ok()));
    let spent = match held {
        Some(d) => spent(d).await,
        None => false,
    };
    guard(app_dir, policy, seller, spent, now_secs())?;
    Ok(sign().await.map(|(out, entry)| {
        set_pending(app_dir, policy, seller, Some(entry));
        out
    }))
}

/// After the seller answered that it settled (P-75, docs/60 P-162): its pending
/// entry is cleared only when the outcome is known — the receipt read back
/// (`confirmed`), or the chain shows this purchase's digest spent (`spent`). A
/// failed read-back with the digest unspent is not known: the seller may still
/// redeem the signature until its deadline, so the entry stays and the guard
/// refuses a second signature to the same seller until then.
fn settled(app_dir: &Path, policy: &str, seller: &str, confirmed: bool, spent: bool) {
    if confirmed || spent {
        set_pending(app_dir, policy, seller, None);
    }
}

fn record(app_dir: &Path, policy: &str, line: &Value) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(purchases_path(app_dir, policy))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// The account's purchases, newest first.
pub fn purchases(app_dir: &Path, policy: &str) -> Vec<Value> {
    let mut v: Vec<Value> = std::fs::read_to_string(purchases_path(app_dir, policy))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    v.reverse();
    v
}

/// The account's purchase lines as written, oldest first, one JSON object
/// per line (`GET /api/level1/purchases.jsonl`): the receipt v1 fields on
/// every settled line, the refusal and its words on the others.
pub fn jsonl(app_dir: &Path, policy: &str) -> String {
    let mut out = String::new();
    for l in std::fs::read_to_string(purchases_path(app_dir, policy))
        .unwrap_or_default()
        .lines()
    {
        if serde_json::from_str::<Value>(l).is_ok() {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// The settled purchases as the detector's input: the policy paid `seller`
/// `minor` at `at`, under the settlement transaction (or, for a purchase the
/// seller never reported, the purchase digest — the `Paid` event names it).
pub fn settled_transfers(app_dir: &Path, policy: &str) -> Vec<Transfer> {
    purchases(app_dir, policy)
        .iter()
        .filter(|p| p["status"] == "paid")
        .filter_map(|p| {
            let id = p["tx"]
                .as_str()
                .filter(|t| !t.is_empty())
                .or(p["actionHash"].as_str())?;
            Some(Transfer {
                tx_hash: id.to_lowercase(),
                counterparty: Counterparty::new(p["seller"].as_str()?),
                amount: Money::minor(p["minor"].as_str()?.parse().ok()?, Currency::Usdc),
                at: Timestamp::from_unix(p["at"].as_i64()?),
                direction: Direction::Out,
            })
        })
        .collect()
}

/// History and receipts as one evidence set, and which transactions are
/// receipts.
///
/// An ERC-7710 settlement is itself a USDC transfer out of the policy, so the
/// chain read may already hold it. The choice made here: **the chain row
/// wins** — its time is the block's and its amount is what moved, while the
/// receipt's `at` is this host's clock when the seller answered — and the
/// receipt only marks that transaction as x402 evidence. A receipt whose
/// transaction the chain read does not hold (a history window that starts
/// after it, a seller that never reported it) is added as its own row. Nothing
/// is counted twice: one transaction, one row.
pub fn merge_evidence(
    history: &[Transfer],
    receipts: &[Transfer],
) -> (Vec<Transfer>, BTreeSet<String>) {
    let mut merged: Vec<Transfer> = history.to_vec();
    let known: BTreeSet<String> = history.iter().map(|t| t.tx_hash.to_lowercase()).collect();
    let mut x402: BTreeSet<String> = BTreeSet::new();
    for r in receipts {
        let tx = r.tx_hash.to_lowercase();
        if x402.insert(tx.clone()) && !known.contains(&tx) {
            merged.push(r.clone());
        }
    }
    (merged, x402)
}

/// Bills the receipts prove: `detect` over history and receipts together,
/// kept where at least one piece of evidence is a receipt. The caller tags
/// them "x402 receipt".
///
/// No dust line here: the dust rule exists because a stranger can plant a
/// transfer *into* a history (docs/21 §7); a receipt is a payment this
/// account signed, with its reason, and a feed bought for 0.001 USDC a day
/// is a bill at 0.001 USDC a day.
pub fn receipt_bills(history: &[Transfer], receipts: &[Transfer]) -> Vec<Obligation> {
    if receipts.is_empty() {
        return Vec::new();
    }
    let (merged, x402) = merge_evidence(history, receipts);
    let cfg = DetectionConfig {
        dust_line: Some(Money::zero(Currency::Usdc)),
        ..DetectionConfig::default()
    };
    detect(&merged, cfg)
        .into_iter()
        .filter(|o| o.evidence.iter().any(|e| x402.contains(&e.to_lowercase())))
        .collect()
}

/// How many purchase lines of `policy` are already in an anchored record.
fn anchored_path(app_dir: &Path, policy: &str) -> PathBuf {
    level1::tenants_dir(app_dir).join(format!("{}.purchases.anchored", policy.to_lowercase()))
}

/// The purchases not yet in an anchored record, as decision-log events, and
/// the line count to mark once the pass that carries them is anchored. A paid
/// purchase is a `Settlement` (seller, amount, transaction, the purchase
/// digest as the action hash, the reason as the note); a refused one or one
/// that waits for the owner is an `Escalation` with its reason in words.
pub fn unanchored_events(app_dir: &Path, policy: &str) -> (Vec<(u64, ledger::EventBody)>, usize) {
    let lines: Vec<Value> = std::fs::read_to_string(purchases_path(app_dir, policy))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let done: usize = std::fs::read_to_string(anchored_path(app_dir, policy))
        .ok()
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0);
    let events = lines
        .iter()
        .skip(done)
        .filter_map(|p| {
            let at = p["at"].as_u64()?;
            let who = ledger::Counterparty::new(p["seller"].as_str()?);
            let amount = Money::minor(p["minor"].as_str()?.parse::<i128>().ok()?, Currency::Usdc);
            let what = format!(
                "x402 {}: {}",
                p["url"].as_str().unwrap_or(""),
                p["reason"].as_str().unwrap_or("")
            );
            let body = if p["status"] == "paid" {
                let digest = B256::from_str(p["actionHash"].as_str()?).ok()?;
                let hash = ledger::ActionHash::from_bytes(digest.0);
                ledger::EventBody::Settlement {
                    counterparty: who,
                    cycle_key: format!("x402:{}", p["url"].as_str().unwrap_or("")),
                    amount,
                    tx_hash: p["tx"].as_str().unwrap_or("").to_string(),
                    action_hash: hash,
                    settlement_key: hash,
                    note: format!("bought {what}"),
                }
            } else {
                ledger::EventBody::Escalation {
                    counterparty: who,
                    amount,
                    reason: p["why"].as_str().unwrap_or("").to_string(),
                    note: format!("{} {what}", p["status"].as_str().unwrap_or("")),
                }
            };
            Some((at, body))
        })
        .collect();
    (events, lines.len())
}

/// Record that the first `count` purchase lines are in an anchored record.
pub fn mark_anchored(app_dir: &Path, policy: &str, count: usize) {
    let _ = std::fs::write(anchored_path(app_dir, policy), format!("{count}\n"));
}

/// At most `BODY_LIMIT` bytes of a response: a seller cannot make the
/// service hold an unbounded body in memory.
const BODY_LIMIT: usize = 64 * 1024;

async fn bounded_text(r: reqwest::Response) -> String {
    bounded_body(r).await.0
}

/// The body, kept to `BODY_LIMIT` bytes, with the sha256 of **every** byte
/// the seller sent (the receipt pins the whole answer, not the part kept)
/// and whether anything was cut.
async fn bounded_body(mut r: reqwest::Response) -> (String, String, bool) {
    let mut out: Vec<u8> = Vec::new();
    let mut hash = Sha256::new();
    let mut truncated = false;
    while let Ok(Some(chunk)) = r.chunk().await {
        hash.update(&chunk);
        let room = BODY_LIMIT.saturating_sub(out.len());
        if chunk.len() > room {
            truncated = true;
        }
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
    }
    (
        String::from_utf8_lossy(&out).into_owned(),
        alloy::hex::encode(hash.finalize()),
        truncated,
    )
}

/// A refusal: the HTTP status the service answers with and the reply.
pub type Refusal = (&'static str, Value);

/// What the caller asks for.
pub struct BuyRequest<'a> {
    pub url: &'a str,
    pub method: &'a str,
    pub body: Option<String>,
    /// The most the caller will pay for this call, USDC minor units.
    pub max_minor: u128,
    pub reason: &'a str,
}

fn usdc_text(minor: u128) -> String {
    Money::minor(minor as i128, Currency::Usdc).to_string()
}

/// Buy one call of a paid API for the account `t`, inside its limits.
pub async fn buy(app_dir: &Path, t: &Tenant, r: BuyRequest<'_>) -> Result<Value, Refusal> {
    let conflict = |e: String| ("409 Conflict", json!({ "error": e }));
    if t.keyless {
        return Err(conflict(
            "this account has no agent key, so it cannot buy anything by itself".into(),
        ));
    }
    if !t.deployed {
        return Err(conflict(
            "this account is not on chain yet; the first check puts it there".into(),
        ));
    }
    let c = crate::watch::chain(&t.chain)
        .ok_or(("400 Bad Request", json!({ "error": "unknown chain" })))?;
    let policy = Address::from_str(&t.policy).map_err(|e| conflict(e.to_string()))?;
    let owner = Address::from_str(&t.owner).map_err(|e| conflict(e.to_string()))?;
    let signer = level1::agent_key(app_dir, owner)
        .ok_or_else(|| conflict("the agent key for this account is not on this host".into()))?;
    let (url, http) = public_client(
        r.url,
        std::time::Duration::from_secs(30),
        "tameion-agent/1 (x402)",
    )
    .await
    .map_err(|e| ("400 Bad Request", json!({ "error": e })))?;
    let method = reqwest::Method::from_str(&r.method.to_ascii_uppercase())
        .map_err(|_| ("400 Bad Request", json!({ "error": "bad method" })))?;
    let send = |extra: Option<String>| {
        let mut q = http.request(method.clone(), url.clone());
        if let Some(b) = &r.body {
            q = q.header("content-type", "application/json").body(b.clone());
        }
        if let Some(h) = extra {
            q = q.header(PAYMENT_SIGNATURE, h);
        }
        q.send()
    };
    let bad_gateway = |e: String| ("502 Bad Gateway", json!({ "error": e }));

    let first = send(None).await.map_err(|e| bad_gateway(e.to_string()))?;
    if first.status().as_u16() != 402 {
        let status = first.status().as_u16();
        let body = bounded_text(first).await;
        return Ok(json!({ "paid": false, "status": status, "body": body }));
    }
    let required = match first.headers().get(PAYMENT_REQUIRED) {
        Some(h) => decode_header(h.to_str().unwrap_or("")),
        None => serde_json::from_str::<Value>(&bounded_text(first).await)
            .map_err(|_| "402 without a PAYMENT-REQUIRED header".to_string()),
    }
    .map_err(bad_gateway)?;
    let client = level1::read_client(c).map_err(conflict)?;
    let usdc = client.token();
    let offer = pick_offer(&required, c.chain_id, usdc).ok_or_else(|| {
        (
            "402 Payment Required",
            json!({
                "error": format!("the service does not take USDC on {} by the exact scheme, so the account cannot pay it", c.name),
                "accepts": required["accepts"],
            }),
        )
    })?;
    let (pay_to, amount, timeout) = offer_terms(&offer).map_err(bad_gateway)?;
    let seller_host = url.host_str().unwrap_or("").to_string();
    let base = json!({
        "at": now_secs(), "policy": t.policy, "url": r.url, "host": seller_host,
        "seller": format!("{pay_to:#x}"), "amount": usdc_text(amount), "minor": amount.to_string(),
        "network": format!("eip155:{}", c.chain_id),
        "reason": r.reason,
    });
    let mut line = base.clone();
    let refuse = |line: &mut Value, status: &'static str, words: String| -> Refusal {
        line["status"] = json!("refused");
        line["why"] = json!(words.clone());
        record(app_dir, &t.policy, line);
        (status, json!({ "error": words, "purchase": line }))
    };
    if amount > r.max_minor {
        let w = format!(
            "the price {} is above the most you allowed for one call ({})",
            usdc_text(amount),
            usdc_text(r.max_minor)
        );
        return Err(refuse(&mut line, "409 Conflict", w));
    }
    // A seller nobody approved is the owner's decision, not the agent's.
    if !client.is_attested(policy, pay_to).await.unwrap_or(false) {
        line["status"] = json!("needs-you");
        line["why"] = json!(format!(
            "{seller_host} asks {} per call, paid to {pay_to:#x}, which is not one of your approved payees. Approve it once on the account page and the agent buys from it inside your limits.",
            usdc_text(amount)
        ));
        record(app_dir, &t.policy, &line);
        return Err((
            "409 Conflict",
            json!({ "error": line["why"], "needsYou": line }),
        ));
    }
    // A price far above what this seller usually charges is the owner's call,
    // as a bill's amount off its usual is (docs/60 P-217).
    let usual = usual_price(app_dir, &t.policy, &format!("{pay_to:#x}"));
    if !purchase_allowed(amount, usual).contains(&crate::decide::Action::Pay)
        && !take_price_approval(
            app_dir,
            &t.policy,
            &format!("{pay_to:#x}"),
            amount,
            now_secs(),
        )
    {
        let u = usual.unwrap_or_default();
        line["status"] = json!("needs-you");
        line["band"] = json!(true);
        line["usual"] = json!(usdc_text(u));
        line["why"] = json!(format!(
            "Let the agent buy from {seller_host} at {} (usually {}): more than a quarter above what it usually pays this seller, so it asks you first. The agent's reason: {}",
            usdc_text(amount),
            usdc_text(u),
            r.reason
        ));
        record(app_dir, &t.policy, &line);
        return Err((
            "409 Conflict",
            json!({ "error": line["why"], "needsYou": line }),
        ));
    }
    if !level1::issuer_frozen(t, &[pay_to]).await.is_empty() {
        return Err(refuse(
            &mut line,
            "409 Conflict",
            level1::frozen_words(&[format!("{pay_to:#x}")]),
        ));
    }

    // One redeemable signature per seller at a time (P-75): an earlier one the
    // seller may still redeem, or that the chain shows redeemed, settles first.
    // Checked, signed and recorded under the account's lock (docs/60 P-214).
    let seller_key = format!("{pay_to:#x}");
    let signed = one_signature_per_seller(
        app_dir,
        &t.policy,
        &seller_key,
        |d| {
            let client = &client;
            async move { client.executed(policy, d).await.unwrap_or(false) }
        },
        || async {
            let nonce = random_nonce()?;
            // A short window: the seller holds a signature it can redeem until then.
            let deadline = now_secs() + timeout.clamp(30, 120);
            let digest = arc::purchase_digest(c.chain_id, policy, pay_to, amount, nonce, deadline);
            let sig = signer.sign_hash_sync(&digest).map_err(|e| e.to_string())?;
            let context = arc::purchase_context(nonce, deadline, &sig.as_bytes());
            // Ask the chain first: a refusal is said here, in words, before the
            // seller is involved.
            let calldata = arc::redeem_calldata(
                context.clone(),
                arc::purchase_execution(usdc, pay_to, amount),
            );
            if let Err(e) = client.call_raw(None, policy, calldata).await {
                let e = e.to_string();
                let w = refusal_words(&e).map(String::from).unwrap_or(e);
                return Err(format!("the account refuses: {w}"));
            }
            let entry = json!({ "digest": format!("{digest:#x}"), "deadline": deadline });
            Ok(((digest, context, deadline), entry))
        },
    )
    .await;
    let (digest, context, deadline) = match signed {
        Ok(Ok(v)) => v,
        Ok(Err(w)) | Err(w) => return Err(refuse(&mut line, "409 Conflict", w)),
    };
    line["actionHash"] = json!(format!("{digest:#x}"));
    let mut payload = payment_payload(&required, &offer, policy, &context);
    // Receipt v1: the payment identifier, when the seller takes one, is this
    // purchase's digest, sent with the payment and kept on the line.
    if wants_payment_identifier(&required) {
        let id = payment_identifier(digest);
        with_payment_identifier(&mut payload, &required, &id);
        line["id"] = json!(id);
        line["idFrom"] = json!(PAYMENT_IDENTIFIER);
    }
    // Without the extension the settlement transaction is the id; until one
    // is known the purchase digest stands in (the `Paid` event names it).
    let identify = |line: &mut Value| {
        if line["id"].is_null() {
            match line["tx"].as_str() {
                Some(tx) => {
                    line["id"] = json!(tx);
                    line["idFrom"] = json!("transaction");
                }
                None => {
                    line["id"] = line["actionHash"].clone();
                    line["idFrom"] = json!("actionHash");
                }
            }
        }
    };
    let header = encode_header(&payload);
    // No answer from the seller: it holds a signature it may still redeem.
    // Say so, keep it pending, and refuse a retry until the deadline (P-75).
    let paid = send(Some(header)).await.map_err(|e| {
        bad_gateway(format!(
            "the seller did not answer ({e}); it may still collect this purchase until {deadline} (unix) — do not retry before then"
        ))
    })?;
    let status = paid.status().as_u16();
    let settlement = paid
        .headers()
        .get(PAYMENT_RESPONSE)
        .and_then(|h| decode_header(h.to_str().ok()?).ok());
    let content_type = paid
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    let (body, body_sha256, body_truncated) = bounded_body(paid).await;
    // Receipt v1: what the seller returned, pinned by hash, and its signed
    // receipt when it sends one.
    line["sha256"] = json!(body_sha256);
    if body_truncated {
        line["bodyTruncated"] = json!(true);
    }
    if let Some(receipt) = settlement.as_ref().and_then(receipt_of) {
        line["receipt"] = receipt;
    }
    let tx = settlement
        .as_ref()
        .filter(|s| s["success"] == true)
        .and_then(|s| s["transaction"].as_str())
        .and_then(|h| B256::from_str(h).ok());
    let Some(tx) = tx.filter(|_| (200..300).contains(&status)) else {
        let why = settlement
            .as_ref()
            .and_then(|s| s["errorReason"].as_str())
            .unwrap_or("no settlement")
            .to_string();
        // A seller can redeem and still answer "failed": the chain, not the
        // seller, says whether the money moved.
        if client.executed(policy, digest).await.unwrap_or(false) {
            line["status"] = json!("paid");
            match client.paid_tx_for(policy, digest, 2_000).await {
                Some(h) => {
                    line["tx"] = json!(format!("{h:#x}"));
                    line["explorer"] = json!(format!("{}/tx/{h:#x}", c.explorer));
                }
                None => {
                    line["explorer"] = json!(format!("{}/address/{policy:#x}", c.explorer));
                }
            }
            line["why"] = json!(format!(
                "the seller answered {status} ({why}) but the account shows this purchase paid; ask the seller for the response"
            ));
            identify(&mut line);
            set_pending(app_dir, &t.policy, &seller_key, None);
            record(app_dir, &t.policy, &line);
            return Err((
                "502 Bad Gateway",
                json!({ "error": line["why"], "purchase": line }),
            ));
        }
        return Err(refuse(
            &mut line,
            "502 Bad Gateway",
            format!("the seller did not settle ({status}: {why})"),
        ));
    };
    // Read the payment back from the chain (PLAN 5.21): exactly one `Paid`
    // naming this seller, this amount and this purchase.
    let confirmed = client
        .confirm_paid(
            tx,
            policy,
            pay_to,
            Money::minor(amount as i128, Currency::Usdc),
            digest,
        )
        .await;
    line["status"] = json!(if confirmed.is_ok() {
        "paid"
    } else {
        "unconfirmed"
    });
    line["tx"] = json!(format!("{tx:#x}"));
    line["explorer"] = json!(format!("{}/tx/{tx:#x}", c.explorer));
    if let Err(e) = &confirmed {
        line["why"] = json!(format!("the receipt does not match the purchase: {e}"));
    }
    identify(&mut line);
    // The chain is asked only when the receipt did not read back.
    let spent = confirmed.is_ok() || client.executed(policy, digest).await.unwrap_or(false);
    settled(app_dir, &t.policy, &seller_key, confirmed.is_ok(), spent);
    record(app_dir, &t.policy, &line);
    let mut body = body;
    body.truncate(64 * 1024);
    Ok(json!({
        "paid": confirmed.is_ok(),
        "purchase": line,
        "status": status,
        "contentType": content_type,
        "body": body,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs/60 P-217: a price more than a quarter above the seller's usual (the
    /// median of its last five purchases) is not paid by the agent alone; a first
    /// purchase has no usual and no band; the owner's tap lets one through.
    #[test]
    fn a_purchase_far_above_its_usual_price_asks_the_owner() {
        use crate::decide::Action;
        let dir = std::env::temp_dir().join(format!("tameion-x402-band-{}", std::process::id()));
        std::fs::create_dir_all(level1::tenants_dir(&dir)).unwrap();
        let policy = "0x00000000000000000000000000000000000000c2";
        let seller = "0x00000000000000000000000000000000000000a1";
        assert_eq!(
            usual_price(&dir, policy, seller),
            None,
            "first purchase: no history"
        );
        assert!(
            purchase_allowed(1_000_000, None).contains(&Action::Pay),
            "no band without history"
        );
        for (i, m) in [1000u128, 1000, 1200, 900, 1000, 50].iter().enumerate() {
            record(
                &dir,
                policy,
                &json!({ "at": i, "seller": seller, "minor": m.to_string(), "status": if *m == 50 { "refused" } else { "paid" } }),
            );
        }
        assert_eq!(
            usual_price(&dir, policy, seller),
            Some(1000),
            "median of the paid ones"
        );
        assert!(
            purchase_allowed(1250, Some(1000)).contains(&Action::Pay),
            "a quarter above: still paid"
        );
        let twice = purchase_allowed(2000, Some(1000));
        assert!(
            !twice.contains(&Action::Pay) && twice.contains(&Action::Ask),
            "{twice:?}"
        );
        assert!(!take_price_approval(&dir, policy, seller, 2000, 10));
        approve_price(&dir, policy, seller, 2000, 100);
        assert!(
            !take_price_approval(&dir, policy, seller, 2001, 10),
            "above what the owner approved"
        );
        assert!(take_price_approval(&dir, policy, seller, 2000, 10));
        assert!(
            !take_price_approval(&dir, policy, seller, 2000, 10),
            "used once"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// docs/60 P-214: two concurrent buys from one seller. The chain round trip
    /// between the check and the pending write is where both used to pass; under
    /// the account's lock exactly one is signed and the other is refused in
    /// words. A buy from another seller of the same account is not refused.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_concurrent_buys_from_one_seller_sign_once() {
        let dir = std::env::temp_dir().join(format!("tameion-x402-lock-{}", std::process::id()));
        std::fs::create_dir_all(level1::tenants_dir(&dir)).unwrap();
        let policy = "0x00000000000000000000000000000000000000c1";
        let signed = std::sync::atomic::AtomicUsize::new(0);
        let buy = |seller: &'static str, byte: u8| {
            let (dir, signed) = (&dir, &signed);
            async move {
                one_signature_per_seller(dir, policy, seller, |_| async { false }, || async move {
                    // The signature and the chain's simulation: a round trip.
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    signed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let entry = json!({ "digest": format!("{:#x}", B256::repeat_byte(byte)), "deadline": now_secs() + 60 });
                    Ok((byte, entry))
                })
                .await
            }
        };
        let s1 = "0x00000000000000000000000000000000000000a1";
        let (a, b, other) = tokio::join!(
            buy(s1, 1),
            buy(s1, 2),
            buy("0x00000000000000000000000000000000000000a2", 3)
        );
        let _ = std::fs::remove_dir_all(&dir);
        let ok = [&a, &b].iter().filter(|r| matches!(r, Ok(Ok(_)))).count();
        assert_eq!(ok, 1, "{a:?} {b:?}");
        let refused = [a, b]
            .into_iter()
            .find_map(|r| r.err())
            .expect("one refused");
        assert!(refused.contains("can still be redeemed"), "{refused}");
        assert!(matches!(other, Ok(Ok(3))), "{other:?}");
        assert_eq!(
            signed.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "one per seller"
        );
    }

    #[test]
    fn a_seller_whose_receipt_does_not_read_back_keeps_its_signature_pending_until_the_deadline() {
        // docs/60 P-162: the seller answers "settled" with a transaction whose receipt does not hold this purchase (a lie,
        // or a node that has not seen it yet) and the chain does not show the digest spent. The signature it holds is
        // still redeemable until its deadline, so a second purchase from it must be refused until then (P-75).
        let dir = std::env::temp_dir().join(format!("x402-p162-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(level1::tenants_dir(&dir)).unwrap();
        let policy = "0x00000000000000000000000000000000000000aa";
        let seller = "0x00000000000000000000000000000000000000bb";
        let (now, deadline) = (1_792_150_000_u64, 1_792_150_120_u64);
        let held =
            json!({ "digest": format!("{:#x}", B256::repeat_byte(7)), "deadline": deadline });
        set_pending(&dir, policy, seller, Some(held.clone()));
        settled(&dir, policy, seller, false, false);
        let refused = guard(&dir, policy, seller, false, now + 5)
            .expect_err("a second signature to a seller that may still redeem the first");
        assert!(
            refused.contains("can still be redeemed until 1792150120"),
            "{refused}"
        );
        assert!(guard(&dir, policy, seller, false, deadline).is_err());
        // Past the deadline, or once the chain shows the first one spent, the next purchase goes on.
        assert!(guard(&dir, policy, seller, false, deadline + 1).is_ok());
        set_pending(&dir, policy, seller, Some(held.clone()));
        assert!(guard(&dir, policy, seller, true, now + 5).is_ok());
        // A known outcome clears it at once: the receipt read back, or the chain shows the digest spent.
        for (confirmed, spent) in [(true, false), (false, true)] {
            set_pending(&dir, policy, seller, Some(held.clone()));
            settled(&dir, policy, seller, confirmed, spent);
            assert!(
                guard(&dir, policy, seller, false, now + 5).is_ok(),
                "confirmed={confirmed} spent={spent}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    const USDC: &str = "0x3600000000000000000000000000000000000000";

    fn required() -> Value {
        json!({
            "x402Version": 2,
            "resource": { "url": "https://api.example.com/x", "description": "d", "mimeType": "application/json" },
            "accepts": [
                { "scheme": "exact", "network": "eip155:84532", "amount": "10", "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e", "payTo": "0x209693Bc6afc0C5328bA36FaF03C514EF312287C", "maxTimeoutSeconds": 60, "extra": { "name": "USDC", "version": "2" } },
                { "scheme": "exact", "network": "eip155:5042002", "amount": "1000", "asset": USDC, "payTo": "0x209693Bc6afc0C5328bA36FaF03C514EF312287C", "maxTimeoutSeconds": 60, "extra": { "name": "USDC", "version": "2" } }
            ]
        })
    }

    #[test]
    fn picks_the_offer_on_this_chain_in_usdc_and_echoes_it_as_erc7710() {
        let usdc = Address::from_str(USDC).unwrap();
        let o = pick_offer(&required(), 5_042_002, usdc).expect("arc offer");
        let (to, amount, timeout) = offer_terms(&o).unwrap();
        assert_eq!(amount, 1000);
        assert_eq!(timeout, 60);
        assert_eq!(
            to,
            Address::from_str("0x209693Bc6afc0C5328bA36FaF03C514EF312287C").unwrap()
        );
        assert!(pick_offer(&required(), 1, usdc).is_none());
        let policy = Address::repeat_byte(0x11);
        let p = payment_payload(&required(), &o, policy, &[1, 2]);
        assert_eq!(p["x402Version"], 2);
        assert_eq!(p["accepted"]["extra"]["assetTransferMethod"], "erc7710");
        assert_eq!(p["accepted"]["extra"]["name"], "USDC");
        assert_eq!(p["payload"]["permissionContext"], "0x0102");
        assert_eq!(p["payload"]["delegator"], p["payload"]["delegationManager"]);
        // the header round-trips
        assert_eq!(decode_header(&encode_header(&p)).unwrap(), p);
    }

    #[test]
    fn a_zero_or_missing_amount_is_not_an_offer() {
        assert!(offer_terms(&json!({ "payTo": USDC, "amount": "0" })).is_err());
        assert!(offer_terms(&json!({ "payTo": USDC })).is_err());
        assert!(offer_terms(&json!({ "amount": "5" })).is_err());
    }

    #[test]
    fn refusals_are_named_from_the_revert_data() {
        // OverPerActionCap(uint256,uint256) = 0x97b81df2 (docs/58: measured on Arc)
        assert_eq!(
            refusal_words("execution reverted: custom error 0x97b81df2000000"),
            Some("the price is above the per-payment limit")
        );
        assert_eq!(refusal_words("something else"), None);
        // NotAutopilot(uint8) = 0xe264d26a: the words say the fix (docs/60 P-174).
        assert!(
            refusal_words("execution reverted: custom error 0xe264d26a0000")
                .is_some_and(|w| w.starts_with("the agent buys only on Autopilot"))
        );
    }

    #[test]
    fn the_digest_matches_the_contracts_formula() {
        // Vector computed with `cast keccak`/`cast abi-encode` over the EIP-712
        // encoding; the forge test pins the contract to the same value.
        let d = arc::purchase_digest(
            5_042_002,
            Address::repeat_byte(0x11),
            Address::repeat_byte(0x22),
            1000,
            B256::repeat_byte(0x33),
            1_800_000_000,
        );
        assert_eq!(format!("{d:#x}"), DIGEST_VECTOR);
    }

    #[test]
    fn purchases_join_the_next_anchored_record_once() {
        let dir = std::env::temp_dir().join(format!("tameion-x402-log-{}", std::process::id()));
        std::fs::create_dir_all(level1::tenants_dir(&dir)).unwrap();
        let policy = "0x00000000000000000000000000000000000000aa";
        record(
            &dir,
            policy,
            &json!({ "at": 10, "seller": "0x00000000000000000000000000000000000000bb", "minor": "1000", "url": "https://x/y", "reason": "price", "status": "paid", "tx": "0x01", "actionHash": format!("{:#x}", B256::repeat_byte(7)) }),
        );
        record(
            &dir,
            policy,
            &json!({ "at": 11, "seller": "0x00000000000000000000000000000000000000cc", "minor": "3000", "url": "https://x/z", "reason": "btc", "status": "refused", "why": "the price is above the per-payment limit" }),
        );
        let (events, upto) = unanchored_events(&dir, policy);
        assert_eq!(upto, 2);
        assert_eq!(events.len(), 2);
        match &events[0].1 {
            ledger::EventBody::Settlement {
                action_hash, note, ..
            } => {
                assert_eq!(action_hash.as_bytes(), &[7u8; 32]);
                assert!(note.contains("price"));
            }
            other => panic!("paid is a settlement, got {other:?}"),
        }
        assert!(
            matches!(&events[1].1, ledger::EventBody::Escalation { reason, .. } if reason.contains("per-payment"))
        );
        mark_anchored(&dir, policy, upto);
        assert!(
            unanchored_events(&dir, policy).0.is_empty(),
            "anchored once"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    const DIGEST_VECTOR: &str =
        "0xeaefdc747acaa58aa77dfb453bd5b1a11a7fd2068c7d629234fae5dcb86f65b0";

    /// Receipt v1: the identifier is sent only when the seller advertises the
    /// extension, in the spec's shape and within its id rules; the seller's
    /// receipt is read from where the spec puts it.
    #[test]
    fn the_payment_identifier_is_echoed_in_the_specs_shape_and_the_receipt_read_back() {
        let mut req = required();
        assert!(!wants_payment_identifier(&req));
        req["extensions"] = json!({ "payment-identifier": { "info": { "required": true }, "schema": { "type": "object" } } });
        assert!(wants_payment_identifier(&req));
        let id = payment_identifier(B256::repeat_byte(0xab));
        assert!(
            id.starts_with("pay_") && (16..=128).contains(&id.len()),
            "{id}"
        );
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        let mut payload = json!({ "x402Version": 2 });
        with_payment_identifier(&mut payload, &req, &id);
        let ext = &payload["extensions"]["payment-identifier"];
        assert_eq!(ext["info"]["id"], id);
        assert_eq!(ext["info"]["required"], true, "the seller's info is echoed");
        assert_eq!(ext["schema"]["type"], "object");

        // §6.5 of the offer-receipt spec: the receipt sits under info.receipt.
        let settlement = json!({
            "success": true, "transaction": "0x12", "network": "eip155:8453", "payer": "0xab",
            "extensions": { "offer-receipt": { "info": { "receipt": {
                "format": "eip712",
                "payload": { "version": 1, "network": "eip155:8453", "resourceUrl": "https://api.example.com/premium-data", "payer": "0xab", "issuedAt": 1703123456, "transaction": "" },
                "signature": "0x1234"
            } } } }
        });
        let r = receipt_of(&settlement).expect("receipt");
        assert_eq!(
            r["payload"]["resourceUrl"],
            "https://api.example.com/premium-data"
        );
        assert!(receipt_of(&json!({ "success": true })).is_none());
    }

    /// Settled purchases are the detector's evidence, once per transaction:
    /// a settlement the chain read already holds keeps the chain's row.
    #[test]
    fn receipts_become_a_bill_without_counting_a_settlement_twice() {
        let dir = std::env::temp_dir().join(format!("tameion-x402-bills-{}", std::process::id()));
        std::fs::create_dir_all(level1::tenants_dir(&dir)).unwrap();
        let policy = "0x00000000000000000000000000000000000000ab";
        let seller = "0x00000000000000000000000000000000005e11e2";
        for (i, tx) in ["0xA1", "0xa2", "0xa3", "0xa4"].iter().enumerate() {
            record(
                &dir,
                policy,
                &json!({ "at": 1_000 + 86_400 * i as i64, "seller": seller, "minor": "1000", "url": "https://x/feed",
                         "reason": "feed", "status": "paid", "tx": tx, "actionHash": format!("{:#x}", B256::repeat_byte(i as u8)) }),
            );
        }
        // A refused line and one the seller never reported (digest as its id).
        record(
            &dir,
            policy,
            &json!({ "at": 1_000, "seller": seller, "minor": "1000", "url": "https://x/feed", "reason": "feed", "status": "refused", "why": "over cap" }),
        );
        let receipts = settled_transfers(&dir, policy);
        assert_eq!(receipts.len(), 4);
        assert!(receipts.iter().all(|t| t.direction == Direction::Out));
        assert_eq!(
            receipts.iter().filter(|t| t.tx_hash == "0xa1").count(),
            1,
            "lowercased"
        );

        // The chain read already holds the first settlement (block time, not
        // the host's clock) and an unrelated owner payment.
        let chain = |tx: &str, cp: &str, at: i64| Transfer {
            tx_hash: tx.into(),
            counterparty: Counterparty::new(cp),
            amount: Money::minor(1000, Currency::Usdc),
            at: Timestamp::from_unix(at),
            direction: Direction::Out,
        };
        let history = vec![
            chain("0xa1", seller, 990),
            chain("0xbb", "0x0000000000000000000000000000000000000bbb", 500),
        ];
        let (merged, x402) = merge_evidence(&history, &receipts);
        assert_eq!(merged.len(), 5, "4 receipts + 1 owner payment, 0xa1 once");
        assert_eq!(x402.len(), 4);
        assert_eq!(
            merged.iter().find(|t| t.tx_hash == "0xa1").unwrap().at,
            Timestamp::from_unix(990),
            "the chain row wins"
        );

        let bills = receipt_bills(&history, &receipts);
        assert_eq!(bills.len(), 1, "{bills:?}");
        assert_eq!(bills[0].counterparty.as_str(), seller);
        assert_eq!(bills[0].sample_count, 4);
        assert_eq!(bills[0].interval.as_i64(), 86_400);
        assert!(receipt_bills(&history, &[]).is_empty());
        // 0.001 USDC a day is below the Arc dust line and still a bill here.
        assert!(bills[0].typical_amount.to_minor() == 1000);

        let lines = jsonl(&dir, policy);
        assert_eq!(lines.lines().count(), 5);
        assert!(lines.ends_with('\n'));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod ssrf_tests {
    use super::inside;
    use std::net::IpAddr;

    #[test]
    fn addresses_inside_the_host_or_its_network_are_refused() {
        for a in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "0.1.2.3",
            "100.64.0.1",
            "100.127.255.255",
            "255.255.255.255",
            "::1",
            "::",
            "fc00::1",
            "fdaa::3",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(inside(a.parse::<IpAddr>().unwrap()), "{a} should be inside");
        }
        for a in [
            "1.1.1.1",
            "100.128.0.1",
            "8.8.8.8",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(
                !inside(a.parse::<IpAddr>().unwrap()),
                "{a} should be public"
            );
        }
    }
}
