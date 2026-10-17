//! Score every account in an app directory with `pistis` (docs/117 §13,
//! step a) and print one row per account and a histogram of the scores.
//!
//! ```text
//! cargo run -p operator --example score_all -- [app dir, default runtime/app]
//! ```
//!
//! Read-only: files and chain reads; nothing written, nothing signed, no key
//! read. The records and their slots are `ledger::symbolon::project`'s, filled
//! the way `symbolon_count` fills them, plus a settlement's own receipt:
//! - witness: a settlement's payment read back (receipt status 1 and exactly
//!   one matching `Paid`, `ArcClient::confirm_paid`), else the anchoring of
//!   its record's root read back (`ArcClient::anchored_in`), which also
//!   witnesses each decision;
//! - counterparty: the payee-signed invoice (`invoice::verify`) of the bill
//!   cycle a settlement paid; every such invoice is also a claim record;
//! - agent: none; no existing record carries the agent's signature over it.
//!
//! Escalations are refusals, counted unsigned (`project` leaves them out).
//! Money in (`<policy>.history.json`) carries no slot: it only shows whether
//! a counterparty's money came back. RPCs are the chain presets'
//! (`TAMEION_RPC_ARC`, `TAMEION_RPC_ARBITRUM_SEPOLIA` override them).

use alloy::primitives::{Address, B256};
use ledger::symbolon::{Extras, SlotSig, Symbolon, SymbolonKind, project, record_hash};
use ledger::{Currency, Decision, Event, EventBody, Money};
use operator::{audit, invoice, statement, watch};
use pistis::{Flow, Params, Record};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

/// A payee-signed invoice: payee, first due time, interval in seconds
/// (0 = one-off), and the slot it fills.
type Signed = (String, u64, u64, SlotSig);

/// The invoice whose bill cycle `cycle_key` (`<payee>@<due>`) is; the rule
/// `symbolon_count` uses.
fn invoice_paid<'a>(invoices: &'a [Signed], cycle_key: &str) -> Option<&'a SlotSig> {
    let (payee, due) = cycle_key.split_once('@')?;
    let due: u64 = due.parse().ok()?;
    invoices
        .iter()
        // `is_multiple_of(0)` holds only for 0: a one-off has one cycle.
        .find(|(p, first, every, _)| {
            p == payee && due >= *first && (due - first).is_multiple_of(*every)
        })
        .map(|(_, _, _, s)| s)
}

fn unsigned(record_hash: [u8; 32], kind: SymbolonKind) -> Symbolon {
    Symbolon {
        record_hash,
        kind,
        agent: None,
        counterparty: None,
        witness: None,
    }
}

fn usdc(m: Money) -> Option<u128> {
    (m.currency() == Currency::Usdc)
        .then(|| u128::try_from(m.to_minor()).ok())
        .flatten()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// One account's records with their slots filled, and how many chain
/// read-backs were tried and how many held.
async fn records_of(tenants: &Path, policy: &str) -> Result<(Vec<Record>, usize, usize), String> {
    let account = read_json(&tenants.join(format!("{policy}.json"))).ok_or("no account file")?;
    let chain = account["chain"].as_str().unwrap_or("arc-testnet");
    let preset = watch::chain(chain).ok_or_else(|| format!("unknown chain {chain}"))?;
    let client = watch::client_for(preset).map_err(|e| e.to_string())?;
    let policy_addr = Address::from_str(policy).map_err(|e| e.to_string())?;
    let read_back = |tx: B256, at: i64| SlotSig {
        alg: "chain-receipt".into(),
        key_id: format!("chain:{}", preset.chain_id),
        sig: tx.to_vec(),
        at,
    };
    let (mut tried, mut held) = (0, 0);
    let mut records = Vec::new();

    // Invoices the payee signed, checked as of their issue date: claims.
    let mut invoices: Vec<Signed> = Vec::new();
    let text = std::fs::read_to_string(tenants.join(format!("{policy}.invoices.jsonl")))
        .unwrap_or_default();
    for v in text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        let issued = v["issuedAt"].as_u64().unwrap_or_default();
        let (Ok(inv), Some(sig)) = (
            invoice::verify(preset.chain_id, policy_addr, &v, issued),
            v["signature"]
                .as_str()
                .and_then(|s| alloy::hex::decode(s).ok()),
        ) else {
            continue;
        };
        let payee = format!("{:#x}", inv.payee);
        let slot = SlotSig {
            alg: "secp256k1-eip712".into(),
            key_id: payee.clone(),
            sig,
            at: issued as i64,
        };
        let mut claim = unsigned(
            invoice::id(inv.payee, inv.reference).0,
            SymbolonKind::Invoice,
        );
        claim.counterparty = Some(slot.clone());
        records.push(Record {
            sym: claim,
            at: issued as i64,
            counterparty: payee.clone(),
            amount: inv.amount,
            flow: Flow::Claimed,
        });
        invoices.push((payee, inv.due_at, u64::from(inv.every_days) * 86_400, slot));
    }

    let log = tenants.join(policy).join("log");
    let anchors = audit::anchors(&log).unwrap_or_default();
    let mut files: Vec<_> = std::fs::read_dir(&log)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.sort();
    for path in files {
        let Some(root) = audit::root_from_filename(&path) else {
            continue;
        };
        let lines = audit::load(&path).map_err(|e| e.to_string())?;
        if audit::recompute(&lines).1 != root {
            return Err(format!("{}: the file is not its root", path.display()));
        }
        let events: Vec<Event> = lines
            .iter()
            .filter_map(|l| Event::decode(&l.canonical))
            .collect();
        let mut extras = Extras::default();
        if let Some(a) = anchors
            .iter()
            .find(|a| a["root"].as_str() == Some(&root.to_hex()) && !a["anchorTx"].is_null())
        {
            let field = |k: &str| a[k].as_str().unwrap_or_default();
            if let (Ok(tx), Ok(anchor), Ok(sender)) = (
                B256::from_str(field("anchorTx")),
                Address::from_str(field("anchor")),
                Address::from_str(field("agent")),
            ) {
                tried += 1;
                if let Ok(true) = client
                    .anchored_in(tx, anchor, sender, B256::from(*root.as_bytes()))
                    .await
                {
                    held += 1;
                    extras.anchored = Some(read_back(tx, a["at"].as_i64().unwrap_or_default()));
                }
            }
        }
        for e in &events {
            let EventBody::Settlement {
                counterparty,
                cycle_key,
                amount,
                tx_hash,
                settlement_key,
                ..
            } = &e.body
            else {
                continue;
            };
            let h = record_hash(e);
            if let Some(s) = invoice_paid(&invoices, cycle_key) {
                extras.counterparty.insert(h, s.clone());
            }
            if let (Ok(tx), Ok(to)) = (
                B256::from_str(tx_hash),
                Address::from_str(counterparty.as_str()),
            ) {
                tried += 1;
                let key = B256::from(*settlement_key.as_bytes());
                if client
                    .confirm_paid(tx, policy_addr, to, *amount, key)
                    .await
                    .is_ok()
                {
                    held += 1;
                    extras.witness.insert(h, read_back(tx, e.at.as_unix()));
                }
            }
        }
        let mut projected: BTreeMap<[u8; 32], Symbolon> = project(&events, &extras)
            .into_iter()
            .map(|s| (s.record_hash, s))
            .collect();
        for e in &events {
            let h = record_hash(e);
            let (sym, cp, amount, flow) = match &e.body {
                EventBody::Settlement {
                    counterparty,
                    amount,
                    ..
                } => (projected.remove(&h), counterparty, *amount, Flow::Paid),
                EventBody::Decision {
                    counterparty,
                    amount,
                    decision,
                    ..
                } => {
                    let flow = if *decision == Decision::Block {
                        Flow::Refused
                    } else {
                        Flow::Other
                    };
                    (projected.remove(&h), counterparty, *amount, flow)
                }
                EventBody::Escalation {
                    counterparty,
                    amount,
                    ..
                } => (
                    Some(unsigned(h, SymbolonKind::Decision)),
                    counterparty,
                    *amount,
                    Flow::Refused,
                ),
                _ => continue,
            };
            let (Some(sym), Some(amount)) = (sym, usdc(amount)) else {
                continue;
            };
            records.push(Record {
                sym,
                at: e.at.as_unix(),
                counterparty: cp.as_str().to_string(),
                amount,
                flow,
            });
        }
    }

    // Money in, from the chain history the service read: no slot, only
    // whether a counterparty's money came back.
    if let Some(h) = read_json(&tenants.join(format!("{policy}.history.json"))) {
        for t in h["transfers"].as_array().into_iter().flatten() {
            if t["out"].as_bool() != Some(false) {
                continue;
            }
            let (Some(cp), Some(amount), Some(at)) = (
                t["cp"].as_str(),
                t["minor"].as_str().and_then(|m| m.parse().ok()),
                t["at"].as_i64(),
            ) else {
                continue;
            };
            let hash = t["tx"]
                .as_str()
                .and_then(|x| B256::from_str(x).ok())
                .unwrap_or_default();
            records.push(Record {
                sym: unsigned(hash.0, SymbolonKind::Settlement),
                at,
                counterparty: cp.to_ascii_lowercase(),
                amount,
                flow: Flow::Received,
            });
        }
    }
    Ok((records, tried, held))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "runtime/app".into());
    let tenants = Path::new(&app).join("tenants");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let p = Params::default();
    let mut accounts: Vec<String> = std::fs::read_dir(&tenants)?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
        .filter(|n| n.len() == 42 && n.starts_with("0x") && Address::from_str(n).is_ok())
        .collect();
    accounts.sort();

    println!(
        "pistis over {} at {} ({now}): score = survival x independence x recency, loop window {} d, half-life {} d",
        tenants.display(),
        statement::iso8601(now),
        p.loop_window_days,
        p.half_life_days
    );
    println!(
        "{:<13} {:<16} {:>5} {:>8} {:>6} {:>6} {:>6} {:>8} {:>8} {:>8} {:>5}  read-backs held/tried",
        "account",
        "chain",
        "recs",
        "unsigned",
        "single",
        "double",
        "triple",
        "survival",
        "indep",
        "recency",
        "bps"
    );
    let mut buckets = [0usize; 10];
    let mut total = [0usize; 4];
    let (mut tried, mut held, mut scored) = (0, 0, 0);
    for policy in &accounts {
        let chain = read_json(&tenants.join(format!("{policy}.json")))
            .and_then(|v| v["chain"].as_str().map(str::to_string))
            .unwrap_or_default();
        let short = format!("{}…{}", &policy[..6], &policy[38..]);
        match records_of(&tenants, policy).await {
            Ok((records, t, h)) => {
                let s = pistis::score(&records, now, &p);
                let counts = [s.n_unsigned, s.n_single, s.n_double, s.n_triple];
                println!(
                    "{short:<13} {chain:<16} {:>5} {:>8} {:>6} {:>6} {:>6} {:>8.3} {:>8.3} {:>8.3} {:>5}  {h}/{t}",
                    counts.iter().sum::<usize>(),
                    s.n_unsigned,
                    s.n_single,
                    s.n_double,
                    s.n_triple,
                    s.survival,
                    s.independence,
                    s.recency,
                    s.bps
                );
                buckets[usize::from(s.bps / 1000).min(9)] += 1;
                for (t, c) in total.iter_mut().zip(counts) {
                    *t += c;
                }
                (tried, held, scored) = (tried + t, held + h, scored + 1);
            }
            Err(e) => println!("{short:<13} {chain:<16} not scored: {e}"),
        }
    }
    println!(
        "\ntotal: {scored} of {} accounts scored, {} records (each once): {} unsigned, {} single, {} double, {} triple-signed; chain read-backs {held}/{tried} held",
        accounts.len(),
        total.iter().sum::<usize>(),
        total[0],
        total[1],
        total[2],
        total[3]
    );
    println!("\nbps histogram ({scored} accounts):");
    for (i, n) in buckets.iter().enumerate() {
        let hi = if i == 9 {
            "10000]".to_string()
        } else {
            format!("{:>5})", (i + 1) * 1000)
        };
        println!("[{:>5}, {hi} {n:>4} {}", i * 1000, "#".repeat(*n));
    }
    Ok(())
}
