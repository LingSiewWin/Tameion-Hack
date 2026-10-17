//! `POST /api/level1/pay` (docs/115 §7, docs/60 P-270): the agent pays a payee
//! the owner approved, now, inside the mandate — one transaction, with every
//! check inside it.
//!
//! The route needs an agent key with the `buy` scope (the service's gate,
//! `level1::scope_for`), a reason (`why`, written to the book beside the
//! payment) and an `Idempotency-Key`, the agent's operation id. The payment's
//! action hash is keccak256(policy ‖ operationId), so the policy's `executed`
//! guard refuses a second payment under the same id even after a restart; the
//! same id with the same body answers with the first payment, a different body
//! is `idempotency_conflict`. Autopilot, the payee list and its cooling-off,
//! the per-payment and 24-hour caps and the floor are the contract's own
//! checks: run as the agent without sending first, so each refusal comes back
//! by its code with nothing sent, and enforced again by the transaction.

use crate::level1;
use crate::refusal::{self, Executed};
use alloy::primitives::{Address, B256, keccak256};
use ledger::{Currency, Money};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The HTTP status the service answers with, and the reply.
pub type Reply = (&'static str, Value);

/// keccak256(policy ‖ operationId): the payment's action hash, the chain's
/// replay guard for this operation id.
pub fn action_hash(policy: Address, operation_id: &str) -> B256 {
    let mut m = policy.to_vec();
    m.extend_from_slice(operation_id.as_bytes());
    keccak256(m)
}

/// 8 to 128 of `[A-Za-z0-9_.:-]`, the SDK's rule (`newOperationId` makes `op_`
/// and a UUID).
fn operation_id_ok(op: &str) -> bool {
    (8..=128).contains(&op.len())
        && op
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

/// The account's direct payments, one JSON line each: the book's feed for
/// them (the next pass puts the paid ones under its anchored root).
fn payments_path(app_dir: &Path, policy: &str) -> PathBuf {
    level1::tenants_dir(app_dir).join(format!("{}.payments.jsonl", policy.to_lowercase()))
}

/// How many lines of the payments file are already under an anchored root.
fn anchored_path(app_dir: &Path, policy: &str) -> PathBuf {
    level1::tenants_dir(app_dir).join(format!("{}.payments.anchored", policy.to_lowercase()))
}

fn record(app_dir: &Path, policy: &str, line: &Value) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(payments_path(app_dir, policy))
    {
        let _ = writeln!(f, "{line}");
    }
}

fn lines(app_dir: &Path, policy: &str) -> Vec<Value> {
    std::fs::read_to_string(payments_path(app_dir, policy))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// What an earlier line for this operation id answers: the same payment
/// again (`replayed`), or a conflict when the id named a different payment.
/// `None` when nothing was paid under it yet.
fn earlier(line: &Value, payee: Address, minor: i128, why: &str) -> Option<Reply> {
    let same = line["payee"].as_str() == Some(format!("{payee:#x}").as_str())
        && line["minor"].as_str() == Some(minor.to_string().as_str())
        && line["why"].as_str() == Some(why);
    if !same {
        return Some((
            "422 Unprocessable Entity",
            refusal::refuse(
                "idempotency_conflict",
                Executed::No,
                "",
                "",
                "this operation id was used for a different payment (payee, amount or why)",
                None,
            ),
        ));
    }
    (line["status"] == "paid").then(|| {
        (
            "200 OK",
            json!({ "paid": true, "replayed": true, "payment": line }),
        )
    })
}

/// A refusal read from a node's revert (the policy's error by selector) or
/// its transport error.
fn chain_refusal(err: &str, executed: Executed) -> Value {
    let code = crate::x402::revert_code(err).unwrap_or(if err.contains("revert") {
        "chain_refused"
    } else {
        "chain_unavailable"
    });
    let words = if code == "chain_unavailable" {
        format!("the chain did not answer as expected: {err}")
    } else {
        level1::refusal_words(err)
    };
    refusal::refuse(code, executed, "", "", words, None)
}

/// One payment at a time per account from this process: the agent's key sends
/// from one nonce sequence. ponytail: per process; a pass paying with the
/// same key at the same instant can still race a nonce, and the loser is
/// refused with nothing paid (the chain's guard holds either way).
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

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Pay `payee` `amount` USDC from the account `policy`, for `why`, under the
/// operation id `operation_id` (the request's `Idempotency-Key`).
pub async fn pay(app_dir: &Path, body: &str, operation_id: Option<&str>) -> Reply {
    let bad = |w: &str| {
        (
            "400 Bad Request",
            refusal::refuse("invalid_body", Executed::No, "", "", w, None),
        )
    };
    let Some(op) = operation_id.map(str::trim).filter(|o| !o.is_empty()) else {
        return bad(
            "Idempotency-Key is required: one operation id per payment, the same on a retry, so a payment is never made twice",
        );
    };
    if !operation_id_ok(op) {
        return bad(
            "Idempotency-Key must be 8 to 128 of A-Z a-z 0-9 _ . : - (e.g. op_ and a UUID)",
        );
    }
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return bad(&e.to_string()),
    };
    let Some(payee) = v["payee"]
        .as_str()
        .and_then(|p| Address::from_str(p.trim()).ok())
    else {
        return bad("payee must be the 0x… address of a payee the owner approved");
    };
    let minor = v["amount"]
        .as_str()
        .and_then(|a| Money::parse(a.trim(), Currency::Usdc).ok())
        .map(|m| m.to_minor())
        .filter(|m| *m > 0);
    let Some(minor) = minor else {
        return bad("amount is required: USDC as a string, e.g. \"0.25\"");
    };
    let why = v["why"].as_str().unwrap_or("").trim();
    if why.is_empty() || why.chars().count() > 500 {
        return bad(
            "why is required, at most 500 characters: every payment is written to the book with what it was for",
        );
    }
    let Some(t) = v["policy"].as_str().and_then(|p| level1::load(app_dir, p)) else {
        return (
            "404 Not Found",
            refusal::refuse(
                "not_found",
                Executed::No,
                "",
                "",
                "no such account on this service",
                None,
            ),
        );
    };
    let conflict = |code: &str, w: &str| {
        (
            "409 Conflict",
            refusal::refuse(code, Executed::No, "", "", w, None),
        )
    };
    if t.keyless {
        return conflict(
            "account_keyless",
            "this account has no agent key, so it cannot pay anyone by itself",
        );
    }
    if !t.deployed {
        return conflict(
            "account_not_deployed",
            "this account is not on chain yet; the first check puts it there",
        );
    }
    let (Some(c), Ok(policy), Ok(agent)) = (
        crate::watch::chain(&t.chain),
        Address::from_str(&t.policy),
        Address::from_str(&t.agent),
    ) else {
        return conflict(
            "service_error",
            "this account's record names no known chain",
        );
    };
    let hash = action_hash(policy, op);
    let lock = account_lock(&t.policy);
    let _turn = lock.lock().await;

    let before = lines(app_dir, &t.policy)
        .into_iter()
        .rev()
        .find(|l| l["operationId"].as_str() == Some(op));
    if let Some(r) = before.as_ref().and_then(|l| earlier(l, payee, minor, why)) {
        return r;
    }
    let client = match level1::agent_client(app_dir, &t, c) {
        Ok(client) => client,
        Err(e) => return conflict("service_error", &e),
    };
    let amount = Money::minor(minor, Currency::Usdc);
    let mut line = json!({
        "at": now_secs(), "operationId": op, "actionHash": format!("{hash:#x}"),
        "policy": t.policy, "payee": format!("{payee:#x}"), "amount": amount.to_string(),
        "minor": minor.to_string(), "why": why,
    });
    // The payment as the chain shows it once `executed(hash)` is true: its
    // transaction, written down, answered as paid.
    let (client, t) = (&client, &t);
    let settled = move |mut line: Value, tx: Option<B256>, replayed: bool| async move {
        let tx = match tx {
            Some(tx) => Some(tx),
            None => client.paid_tx_for(policy, hash, 20_000).await,
        };
        line["status"] = json!("paid");
        if let Some(tx) = tx {
            line["tx"] = json!(format!("{tx:#x}"));
            line["explorer"] = json!(format!("{}/tx/{tx:#x}", c.explorer));
        }
        record(app_dir, &t.policy, &line);
        (
            "200 OK",
            json!({ "paid": true, "replayed": replayed, "payment": line }),
        )
    };
    let earlier_tx = before
        .as_ref()
        .and_then(|l| l["tx"].as_str())
        .and_then(|h| B256::from_str(h).ok());
    // Settled under this id already (a reply lost before it was written down,
    // or another copy of the service): it stands, and is never paid twice.
    match client.executed(policy, hash).await {
        Ok(true) => return settled(line, earlier_tx, true).await,
        Ok(false) => {}
        Err(e) => {
            return (
                "503 Service Unavailable",
                refusal::refuse(
                    "chain_unavailable",
                    if before.is_some() {
                        Executed::Unknown
                    } else {
                        Executed::No
                    },
                    "",
                    "",
                    format!("the chain did not answer ({e}); nothing was sent"),
                    None,
                ),
            );
        }
    }
    // The contract's checks, as the agent, without sending: Autopilot, an
    // approved payee past its cooling-off, the caps, the floor, the balance.
    if let Err(e) = client.simulate_payment(policy, payee, amount, hash).await {
        return ("409 Conflict", chain_refusal(&e.to_string(), Executed::No));
    }
    // The pass keeps the agent's gas topped up; a payment between passes too.
    if let Err(e) = level1::top_up_agent_gas(c, agent).await {
        eprintln!("pay: agent gas top-up for {}: {e}", t.policy);
    }
    let tx = match client.pay_via_policy(policy, payee, amount, hash).await {
        Ok(tx) => tx,
        Err(e) => {
            let e = e.to_string();
            let sent = level1::sent_unread(&e).and_then(|h| B256::from_str(&h).ok());
            match client.executed(policy, hash).await {
                Ok(true) => return settled(line, sent, false).await,
                Ok(false) if sent.is_none() => {
                    return ("409 Conflict", chain_refusal(&e, Executed::No));
                }
                _ => {}
            }
            // Sent, or not known: written down so the same id answers once
            // the chain shows it; never a second payment under a new id.
            line["status"] = json!("sent");
            if let Some(tx) = sent {
                line["tx"] = json!(format!("{tx:#x}"));
            }
            record(app_dir, &t.policy, &line);
            return (
                "502 Bad Gateway",
                refusal::refuse(
                    "chain_unavailable",
                    Executed::Unknown,
                    "",
                    "Do not pay again under a new operation id: send the same one again in a minute; it answers with the payment once the chain shows it.",
                    format!("the payment may have been sent and was not confirmed: {e}"),
                    None,
                ),
            );
        }
    };
    // Read the payment back (PLAN 5.21): exactly one `Paid` naming this
    // payee, this amount and this action hash.
    let confirmed = client.confirm_paid(tx, policy, payee, amount, hash).await;
    line["status"] = json!(if confirmed.is_ok() {
        "paid"
    } else {
        "unconfirmed"
    });
    line["tx"] = json!(format!("{tx:#x}"));
    line["explorer"] = json!(format!("{}/tx/{tx:#x}", c.explorer));
    if let Err(e) = &confirmed {
        line["note"] = json!(format!("the receipt does not match the payment: {e}"));
    }
    record(app_dir, &t.policy, &line);
    (
        "200 OK",
        json!({ "paid": confirmed.is_ok(), "payment": line }),
    )
}

/// The paid direct payments not yet under an anchored root, as the pass's
/// events (purchases join the same way, `x402::unanchored_events`), and how
/// many lines that covers.
pub fn unanchored_events(app_dir: &Path, policy: &str) -> (Vec<(u64, ledger::EventBody)>, usize) {
    let all = lines(app_dir, policy);
    let done: usize = std::fs::read_to_string(anchored_path(app_dir, policy))
        .ok()
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0);
    let events = all
        .iter()
        .skip(done)
        .filter(|p| p["status"] == "paid")
        .filter_map(|p| {
            let hash =
                ledger::ActionHash::from_bytes(B256::from_str(p["actionHash"].as_str()?).ok()?.0);
            Some((
                p["at"].as_u64()?,
                ledger::EventBody::Settlement {
                    counterparty: ledger::Counterparty::new(p["payee"].as_str()?),
                    cycle_key: format!("pay:{}", p["operationId"].as_str()?),
                    amount: Money::minor(p["minor"].as_str()?.parse().ok()?, Currency::Usdc),
                    tx_hash: p["tx"].as_str().unwrap_or("").to_string(),
                    action_hash: hash,
                    settlement_key: hash,
                    note: format!("paid: {}", p["why"].as_str().unwrap_or("")),
                },
            ))
        })
        .collect();
    (events, all.len())
}

/// Record that the first `count` payment lines are in an anchored record.
pub fn mark_anchored(app_dir: &Path, policy: &str, count: usize) {
    let _ = std::fs::write(anchored_path(app_dir, policy), format!("{count}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: &str = "0x00000000000000000000000000000000000000c3";
    const PAYEE: &str = "0xee80b9f69d80a04d4b2e803c426715aa8c1bb1f3";

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tameion-pay-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(level1::tenants_dir(&d)).unwrap();
        d
    }

    fn body(amount: &str, why: &str) -> String {
        json!({ "policy": POLICY, "payee": PAYEE, "amount": amount, "why": why }).to_string()
    }

    #[test]
    fn the_action_hash_is_the_policy_then_the_operation_id() {
        let p = Address::from_str(POLICY).unwrap();
        let mut m = p.to_vec();
        m.extend_from_slice(b"op_1234567");
        assert_eq!(action_hash(p, "op_1234567"), keccak256(m));
        assert_ne!(action_hash(p, "op_1234567"), action_hash(p, "op_1234568"));
        assert!(operation_id_ok("op_0b6f0c1e-1d2a-4f39-9f1e-6c1d2b3a4e5f"));
        assert!(!operation_id_ok("short"));
        assert!(!operation_id_ok("op with spaces"));
    }

    /// docs/115 §7: each refusal of the mandate comes back by its code, from
    /// the contract's own error, with nothing sent.
    #[test]
    fn the_contracts_refusals_come_back_by_their_codes() {
        let revert = |sig: &str, word: &str| {
            let sel = alloy::hex::encode(&keccak256(sig.as_bytes())[..4]);
            format!(
                "server returned an error response: error code 3: execution reverted, data: \"0x{sel}{word}\""
            )
        };
        let w = |n: u8| format!("{:064x}", n);
        for (sig, word, code) in [
            ("NotAutopilot(uint8)", w(1), "mode_propose"),
            ("NotAutopilot(uint8)", w(0), "mode_watch"),
            ("PayeeNotAttested(address)", w(9), "payee_not_approved"),
            (
                "PayeeCoolingOff(address,uint256)",
                w(9),
                "payee_cooling_off",
            ),
            (
                "OverPerActionCap(uint256,uint256)",
                w(9),
                "over_per_payment",
            ),
            ("OverDailyCap(uint256,uint256)", w(9), "over_24h"),
            ("BelowReserveFloor(uint256,uint256)", w(9), "below_floor"),
        ] {
            let r = chain_refusal(&revert(sig, &word), Executed::No);
            assert_eq!(r["error"]["code"], code, "{sig}");
            assert_eq!(r["error"]["executed"], "no");
        }
        assert_eq!(
            chain_refusal("error sending request: connection refused", Executed::No)["error"]["code"],
            "chain_unavailable"
        );
    }

    #[tokio::test]
    async fn a_payment_without_an_operation_id_a_reason_or_an_account_is_refused() {
        let d = dir("input");
        let code = |r: Reply| (r.0, r.1["error"]["code"].as_str().unwrap_or("").to_string());
        let (s, c) = code(pay(&d, &body("0.001", "rent"), None).await);
        assert_eq!((s, c.as_str()), ("400 Bad Request", "invalid_body"));
        let (s, c) = code(pay(&d, &body("0.001", "rent"), Some("short")).await);
        assert_eq!((s, c.as_str()), ("400 Bad Request", "invalid_body"));
        let (s, c) = code(pay(&d, &body("0.001", " "), Some("op_12345678")).await);
        assert_eq!((s, c.as_str()), ("400 Bad Request", "invalid_body"));
        let (s, c) = code(pay(&d, &body("0", "rent"), Some("op_12345678")).await);
        assert_eq!((s, c.as_str()), ("400 Bad Request", "invalid_body"));
        let (s, c) = code(pay(&d, &body("0.001", "rent"), Some("op_12345678")).await);
        assert_eq!((s, c.as_str()), ("404 Not Found", "not_found"));
    }

    /// A replay of the same operation id answers with the stored payment and
    /// sends nothing (no chain is reachable here); a different payment under
    /// that id is `idempotency_conflict`.
    #[tokio::test]
    async fn a_replay_returns_the_stored_payment_and_a_different_body_conflicts() {
        let d = dir("replay");
        let t: level1::Tenant = serde_json::from_value(json!({
            "chain": "arbitrum-sepolia", "owner": "0x0000000000000000000000000000000000000001",
            "policy": POLICY, "agent": "0x0000000000000000000000000000000000000003",
            "operator": "0x0000000000000000000000000000000000000004", "watched": "0x0000000000000000000000000000000000000005",
            "recovery": "0x0000000000000000000000000000000000000000", "per_action": "1", "daily": "1", "floor": "0",
            "delay_secs": 60, "mode": 2, "created_block": 0, "history_from_block": 0, "created": 0, "deployed": true
        }))
        .unwrap();
        level1::save(&d, &t).unwrap();
        let op = "op_replay-0001";
        let stored = json!({
            "at": 1, "operationId": op, "actionHash": format!("{:#x}", action_hash(Address::from_str(POLICY).unwrap(), op)),
            "policy": POLICY, "payee": PAYEE, "amount": "0.001000 USDC", "minor": "1000", "why": "rent",
            "status": "paid", "tx": format!("0x{}", "ab".repeat(32)),
        });
        record(&d, POLICY, &stored);
        let (s, v) = pay(&d, &body("0.001", "rent"), Some(op)).await;
        assert_eq!(s, "200 OK", "{v}");
        assert_eq!(v["payment"], stored, "the stored payment, as it was");
        assert_eq!(v["replayed"], true);
        let (s, v) = pay(&d, &body("0.002", "rent"), Some(op)).await;
        assert_eq!(s, "422 Unprocessable Entity");
        assert_eq!(v["error"]["code"], "idempotency_conflict");
        assert_eq!(v["error"]["executed"], "no");
        // The book: the paid line becomes one settlement under the next root.
        let (events, upto) = unanchored_events(&d, POLICY);
        assert_eq!((events.len(), upto), (1, 1));
        mark_anchored(&d, POLICY, upto);
        assert!(unanchored_events(&d, POLICY).0.is_empty());
    }
}
