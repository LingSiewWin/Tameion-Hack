//! How many of an app dir's records stand behind one, two or three
//! signatures (`ledger::symbolon`). Every account's events files are decoded
//! and projected with only the slots that can be checked again:
//!
//! - witness: the root's anchoring transaction, read back on the account's
//!   chain (it succeeded, the agent sent it to the anchor, the root is in its
//!   calldata: `ArcClient::anchored_in`);
//! - counterparty: a payee-signed invoice whose EIP-712 signature recovers
//!   the payee (`invoice::verify`), for a settlement of that invoice's bill
//!   cycle (`<payee>@<dueAt + k * everyDays>`);
//! - agent: none; no record carries the agent's own signature yet.
//!
//! A record persisted again in a later file counts once, with its best slots.
//! A failed chain read is counted apart, never as "not witnessed".
//!
//!   cargo run -q -p operator --example symbolon_count -- [app dir]   (default runtime/app)
//!
//! Reads no key. RPCs are the chain presets' (`TAMEION_RPC_ARC`,
//! `TAMEION_RPC_ARBITRUM_SEPOLIA` override the public endpoints).

use alloy::primitives::{Address, B256};
use arc::ArcClient;
use ledger::symbolon::{Extras, Signedness, SlotSig, project, record_hash};
use ledger::{Event, EventBody};
use operator::sig::{self, Alg, KeyRef};
use operator::{audit, invoice, level1, watch};
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A received invoice whose signature recovers its payee: payee, first due
/// time, interval in seconds (0 = one-off), and the slot it fills.
type Signed = (String, u64, u64, SlotSig);

fn signed_invoices(app: &Path, policy: Address, chain_id: u64) -> Vec<Signed> {
    let path = level1::tenants_dir(app).join(format!("{policy:#x}.invoices.jsonl"));
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| {
            let issued = v["issuedAt"].as_u64()?;
            // Checked as of its issue date: the signature and the terms.
            let inv = invoice::verify(chain_id, policy, &v, issued).ok()?;
            let hex = |k: &str| alloy::hex::decode(v[k].as_str()?.trim_start_matches("0x")).ok();
            let sig = hex("signature")?;
            // The slot says how it was signed (`sig::slot_sig`): by the payee's
            // address, or by the ML-DSA-65 key the payee bound on this account.
            let pk = match inv.alg {
                Some(Alg::MlDsa65) => {
                    let bound = invoice::binding_for(app, &format!("{policy:#x}"), inv.payee);
                    invoice::floor(&inv, bound.as_ref()).ok()?;
                    Some(hex("publicKey")?)
                }
                _ => None,
            };
            let key = match &pk {
                Some(pk) => KeyRef::MlDsa65(pk),
                None => KeyRef::Address(inv.payee),
            };
            let alg = inv.alg.unwrap_or(Alg::Secp256k1Eip712);
            let d = invoice::digest(chain_id, &inv);
            let slot = sig::slot_sig(alg, &key, &d.0, &sig, issued as i64).ok()?;
            Some((
                format!("{:#x}", inv.payee),
                inv.due_at,
                u64::from(inv.every_days) * 86_400,
                slot,
            ))
        })
        .collect()
}

/// The invoice whose bill cycle `cycle_key` (`<payee>@<due>`) is.
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "runtime/app".into()),
    );
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(level1::tenants_dir(&app))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("log").is_dir())
        .collect();
    dirs.sort();
    let mut clients: BTreeMap<&'static str, ArcClient> = BTreeMap::new();
    let mut total = [0usize; 4];
    let (mut files_seen, mut not_witnessed, mut read_failed, mut undecodable) = (0, 0, 0, 0);
    let (mut invoices_signed, mut invoices_paid) = (0, 0);
    println!(
        "{:<42} {:>7} {:>8} {:>6} {:>6} {:>6}  chain",
        "account", "records", "unsigned", "single", "double", "triple"
    );
    for dir in dirs {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let Ok(policy) = Address::from_str(&name) else {
            continue;
        };
        let Some(chain) = level1::load(&app, &name).and_then(|t| watch::chain(&t.chain)) else {
            println!("{name}: no tenant file or an unknown chain; skipped");
            continue;
        };
        let client = match clients.entry(chain.key) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(ArcClient::read_only(&watch::rpc_for(chain))?),
        };
        let invoices = signed_invoices(&app, policy, chain.chain_id);
        invoices_signed += invoices.len();
        let log = dir.join("log");
        let anchors = audit::anchors(&log)?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(&log)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| audit::root_from_filename(p).is_some())
            .collect();
        files.sort();
        let mut best: BTreeMap<[u8; 32], Signedness> = BTreeMap::new();
        for f in files {
            files_seen += 1;
            let lines = audit::load(&f)?;
            let (_, root) = audit::recompute(&lines);
            let mut extras = Extras::default();
            let mut failed = false;
            // Only a file whose bytes recompute to its name can be witnessed.
            if audit::root_from_filename(&f) == Some(root) {
                let want = root.to_hex();
                for a in anchors
                    .iter()
                    .filter(|a| a["root"].as_str() == Some(want.as_str()))
                {
                    let field = |k: &str| a[k].as_str().unwrap_or_default().trim().to_string();
                    let (Ok(tx), Ok(anchor), Ok(agent)) = (
                        B256::from_str(&field("anchorTx")),
                        Address::from_str(&field("anchor")),
                        Address::from_str(&field("agent")),
                    ) else {
                        continue;
                    };
                    let root_b = B256::from(*root.as_bytes());
                    // One retry: the public endpoints rate-limit (docs/60 P-12, P-14).
                    let mut read = client.anchored_in(tx, anchor, agent, root_b).await;
                    if read.is_err() {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        read = client.anchored_in(tx, anchor, agent, root_b).await;
                    }
                    match read {
                        Ok(true) => {
                            extras.anchored = Some(SlotSig {
                                alg: "chain-receipt".into(),
                                key_id: format!("chain:{}", chain.chain_id),
                                sig: tx.to_vec(),
                                at: a["at"].as_i64().unwrap_or(0),
                            });
                            break;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            failed = true;
                            eprintln!("{name}: {want}: chain read failed: {e}");
                        }
                    }
                }
            }
            if extras.anchored.is_none() {
                if failed {
                    read_failed += 1;
                } else {
                    not_witnessed += 1;
                }
            }
            let events: Vec<Event> = lines
                .iter()
                .filter_map(|l| Event::decode(&l.canonical))
                .collect();
            undecodable += lines.len() - events.len();
            for e in &events {
                if let EventBody::Settlement { cycle_key, .. } = &e.body
                    && let Some(s) = invoice_paid(&invoices, cycle_key)
                {
                    invoices_paid += 1;
                    extras.counterparty.insert(record_hash(e), s.clone());
                }
            }
            for s in project(&events, &extras) {
                let b = best.entry(s.record_hash).or_insert(Signedness::Unsigned);
                *b = (*b).max(s.signedness());
            }
        }
        let mut counts = [0usize; 4];
        for s in best.values() {
            counts[*s as usize] += 1;
        }
        for (t, c) in total.iter_mut().zip(counts) {
            *t += c;
        }
        println!(
            "{name:<42} {:>7} {:>8} {:>6} {:>6} {:>6}  {}",
            best.len(),
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            chain.key
        );
    }
    println!(
        "{:<42} {:>7} {:>8} {:>6} {:>6} {:>6}",
        "total",
        total.iter().sum::<usize>(),
        total[0],
        total[1],
        total[2],
        total[3]
    );
    println!(
        "events files {files_seen}: not witnessed {not_witnessed}, chain read failed {read_failed}; lines not decodable {undecodable}"
    );
    println!(
        "payee-signed invoices that verify {invoices_signed}; settlement lines of their bill cycles {invoices_paid}"
    );
    Ok(())
}
