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
//!   cargo run -q -p operator --example symbolon_count -- [app dir] [--show <seq|leaf>]
//!
//! The app dir defaults to runtime/app. `--show` also prints the three slots
//! of each record with that event seq (one per account) or that leaf (hex):
//! slot, alg, key id, and whether its check passed in this run. A slot is
//! filled only after its check passed, so filled means verified.
//!
//! Reads no key. RPCs are the chain presets' (`TAMEION_RPC_ARC`,
//! `TAMEION_RPC_ARBITRUM_SEPOLIA` override the public endpoints).

use alloy::primitives::Address;
use arc::ArcClient;
use ledger::symbolon::{SlotSig, Symbolon};
use operator::symbolon::{account, anchorings, signed_invoices};
use operator::{level1, watch};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::path::PathBuf;
use std::str::FromStr;

/// One slot of `--show`: hashes are fine on this surface (an operator's terminal).
fn print_slot(name: &str, s: Option<&SlotSig>) {
    let Some(s) = s else {
        println!("  {name:<12} none                verified no");
        return;
    };
    // A 32-byte artefact is the anchoring tx: printed whole. A signature: its head.
    let sig = if s.sig.len() == 32 {
        format!("tx 0x{}", alloy::hex::encode(&s.sig))
    } else {
        let head = alloy::hex::encode(&s.sig[..s.sig.len().min(8)]);
        format!("sig 0x{head}… ({} bytes)", s.sig.len())
    };
    println!(
        "  {name:<12} {:<19} verified yes  key {}  {sig}  at {}",
        s.alg, s.key_id, s.at
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut app, mut show) = (PathBuf::from("runtime/app"), None::<String>);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--show" {
            show = args
                .next()
                .map(|w| w.trim_start_matches("0x").to_lowercase());
        } else {
            app = PathBuf::from(a);
        }
    }
    // `--show`: per account and record, the occurrence with the most slots.
    let mut shown: BTreeMap<(String, [u8; 32]), (u64, Symbolon)> = BTreeMap::new();
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
        let mut read = Vec::new();
        for a in anchorings(&log, None)? {
            // One retry: the public endpoints rate-limit (docs/60 P-12, P-14).
            let mut r = client.anchored_in(a.tx, a.anchor, a.agent, a.root).await;
            if r.is_err() {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                r = client.anchored_in(a.tx, a.anchor, a.agent, a.root).await;
            }
            read.push((a, r.map_err(|e| e.to_string())));
        }
        let acc = account(&log, &invoices, chain.chain_id, None, &read)?;
        for (root, e) in &acc.read_failed {
            eprintln!("{name}: {}: chain read failed: {e}", root.to_hex());
        }
        files_seen += acc.files;
        not_witnessed += acc.not_witnessed;
        read_failed += acc.read_failed.len();
        undecodable += acc.undecodable;
        invoices_paid += acc.invoices_paid;
        if let Some(w) = show.as_deref() {
            for (h, f) in &acc.records {
                if w == f.seq.to_string() || w == alloy::hex::encode(h) {
                    shown.insert((name.clone(), *h), (f.seq, f.symbolon.clone()));
                }
            }
        }
        let counts = acc.counts();
        for (t, c) in total.iter_mut().zip(counts) {
            *t += c;
        }
        println!(
            "{name:<42} {:>7} {:>8} {:>6} {:>6} {:>6}  {}",
            acc.records.len(),
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
    if let Some(w) = &show {
        if shown.is_empty() {
            println!("--show {w}: no projected record has that seq or leaf");
        }
        for ((account, h), (seq, s)) in &shown {
            println!(
                "record 0x{} · {:?} · seq {seq} · account {account} · {:?}",
                alloy::hex::encode(h),
                s.kind,
                s.signedness()
            );
            print_slot("agent", s.agent.as_ref());
            print_slot("counterparty", s.counterparty.as_ref());
            print_slot("witness", s.witness.as_ref());
        }
    }
    Ok(())
}
