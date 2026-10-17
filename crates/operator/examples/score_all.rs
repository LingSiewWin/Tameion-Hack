//! Score every account in an app directory with `pistis` (docs/117 §13,
//! step a): read each account's records, fill a slot only from what verifies,
//! and print one row per account and a histogram of the scores.
//!
//! ```text
//! cargo run -p operator --example score_all -- [app dir, default runtime/app]
//! ```
//!
//! Read-only: files and chain reads, nothing written, nothing signed. The
//! slots, until `ledger::symbolon::project` lands:
//! - witness: a settlement's payment read back from its chain (receipt status
//!   1 and exactly one matching `Paid`, `ArcClient::confirm_paid`); a decision
//!   or an escalation by the transaction that anchored its record's root
//!   (`ArcClient::anchored_in`);
//! - counterparty: an invoice the payee signed (`invoice::verify`), on the
//!   invoice and on the settlement that paid it (the invoice id as settlement
//!   key, or the same payee and amount after it was issued);
//! - agent: none. No existing record carries the agent's signature over the
//!   record itself.
//!
//! Money that came in (`<policy>.history.json`) carries no slot: it only shows
//! whether a counterparty's money came back.

use alloy::primitives::{Address, B256};
use ledger::{Currency, Money};
use operator::{audit, invoice, statement, watch};
use pistis::{Flow, Params, Record, SlotSig, Symbolon, SymbolonKind};
use serde_json::Value;
use std::path::Path;
use std::str::FromStr;

// ponytail: a third reader of the event layout (statement.rs and the
// service's recall have their own); one decoder in `ledger::log` when the
// scaffold settles.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        (self.0.len() >= n).then(|| {
            let (head, tail) = self.0.split_at(n);
            self.0 = tail;
            head
        })
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u64()?;
        self.take(usize::try_from(n).ok()?)
    }
    fn str(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }
    /// USDC minor units; another currency or a negative amount is `None`.
    fn usdc(&mut self) -> Option<u128> {
        let minor = i128::from_be_bytes(self.take(16)?.try_into().ok()?);
        (self.u8()? == 1).then_some(())?;
        u128::try_from(minor).ok()
    }
}

enum Decoded {
    Decision {
        cp: String,
        minor: u128,
        block: bool,
    },
    Settlement {
        cp: String,
        minor: u128,
        tx: String,
        key: [u8; 32],
    },
    Escalation {
        cp: String,
        minor: u128,
    },
}

/// A decision, settlement or escalation from its canonical bytes
/// (`ledger::log::Event::canonical`, tags 2, 3, 4); `None` for anything else.
fn decode(canonical: &[u8]) -> Option<(i64, Decoded)> {
    let mut r = Reader(canonical);
    if r.str()? != "air-log/v0.1" {
        return None;
    }
    r.u64()?;
    let at = i64::try_from(r.u64()?).ok()?;
    let d = match r.u8()? {
        2 => {
            let cp = r.str()?;
            let minor = r.usdc()?;
            Decoded::Decision {
                cp,
                minor,
                block: r.u8()? == 3,
            }
        }
        3 => {
            let cp = r.str()?;
            r.str()?;
            let minor = r.usdc()?;
            let tx = r.str()?;
            r.bytes()?;
            let key = r.bytes()?.try_into().ok()?;
            Decoded::Settlement { cp, minor, tx, key }
        }
        4 => {
            let cp = r.str()?;
            Decoded::Escalation {
                cp,
                minor: r.usdc()?,
            }
        }
        _ => return None,
    };
    Some((at, d))
}

fn record(
    hash: [u8; 32],
    kind: SymbolonKind,
    at: i64,
    cp: &str,
    amount: u128,
    flow: Flow,
) -> Record {
    Record {
        sym: Symbolon {
            record_hash: hash,
            kind,
            agent: None,
            counterparty: None,
            witness: None,
        },
        at,
        counterparty: cp.to_ascii_lowercase(),
        amount,
        flow,
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// One account's records with their slots filled, and how many read-backs
/// were tried and how many held.
async fn records_of(tenants: &Path, policy: &str) -> Result<(Vec<Record>, usize, usize), String> {
    let account = read_json(&tenants.join(format!("{policy}.json"))).ok_or("no account file")?;
    let chain = account["chain"].as_str().unwrap_or("arc-testnet");
    let preset = watch::chain(chain).ok_or_else(|| format!("unknown chain {chain}"))?;
    let client = watch::client_for(preset).map_err(|e| e.to_string())?;
    let policy_addr = Address::from_str(policy).map_err(|e| e.to_string())?;
    let witness = |tx: B256, at: i64| SlotSig {
        alg: "chain-receipt".into(),
        key_id: format!("chain:{}", preset.chain_id),
        sig: tx.to_vec(),
        at,
    };
    let (mut tried, mut held) = (0, 0);
    let mut records = Vec::new();
    let mut paid_keys = Vec::new();

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
        let (leaves, recomputed) = audit::recompute(&lines);
        if recomputed != root {
            return Err(format!("{}: the file is not its root", path.display()));
        }
        // The record's root, read back from the transaction that anchored it.
        let mut anchored = None;
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
                    anchored = Some(witness(tx, a["at"].as_i64().unwrap_or_default()));
                }
            }
        }
        for (line, leaf) in lines.iter().zip(&leaves) {
            let Some((at, decoded)) = decode(&line.canonical) else {
                continue;
            };
            let hash = *leaf.as_bytes();
            let mut r = match &decoded {
                Decoded::Decision { cp, minor, block } => {
                    let flow = if *block { Flow::Refused } else { Flow::Other };
                    record(hash, SymbolonKind::Decision, at, cp, *minor, flow)
                }
                Decoded::Escalation { cp, minor } => {
                    record(hash, SymbolonKind::Decision, at, cp, *minor, Flow::Refused)
                }
                Decoded::Settlement { cp, minor, .. } => {
                    record(hash, SymbolonKind::Settlement, at, cp, *minor, Flow::Paid)
                }
            };
            if let Decoded::Settlement { cp, minor, tx, key } = decoded {
                // The payment itself, not the record of it: the witness of a
                // settlement is its receipt.
                if let (Ok(txh), Ok(to)) = (B256::from_str(&tx), Address::from_str(&cp)) {
                    tried += 1;
                    let amount = Money::minor(minor as i128, Currency::Usdc);
                    if client
                        .confirm_paid(txh, policy_addr, to, amount, B256::from(key))
                        .await
                        .is_ok()
                    {
                        held += 1;
                        r.sym.witness = Some(witness(txh, at));
                    }
                }
                paid_keys.push((records.len(), key));
            } else {
                r.sym.witness = anchored.clone();
            }
            records.push(r);
        }
    }

    // Invoices the payee signed: the claim, and the settlement that paid it.
    let invoices = std::fs::read_to_string(tenants.join(format!("{policy}.invoices.jsonl")))
        .unwrap_or_default();
    for v in invoices
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        let issued = v["issuedAt"].as_u64().unwrap_or_default();
        let Ok(inv) = invoice::verify(preset.chain_id, policy_addr, &v, issued) else {
            continue;
        };
        let Some(sig) = v["signature"]
            .as_str()
            .and_then(|s| alloy::hex::decode(s).ok())
        else {
            continue;
        };
        let id = invoice::id(inv.payee, inv.reference);
        let payee = format!("{:#x}", inv.payee);
        let signed = SlotSig {
            alg: "secp256k1-eip712".into(),
            key_id: payee.clone(),
            sig,
            at: issued as i64,
        };
        for (i, key) in &paid_keys {
            let s = &mut records[*i];
            if *key == id.0
                || (s.counterparty == payee && s.amount == inv.amount && s.at >= issued as i64)
            {
                s.sym.counterparty = Some(signed.clone());
            }
        }
        let mut claim = record(
            id.0,
            SymbolonKind::Invoice,
            issued as i64,
            &payee,
            inv.amount,
            Flow::Claimed,
        );
        claim.sym.counterparty = Some(signed);
        records.push(claim);
    }

    // Money in, from the chain history the service read: no slot, only
    // whether a counterparty's money came back.
    if let Some(h) = read_json(&tenants.join(format!("{policy}.history.json"))) {
        for t in h["transfers"].as_array().into_iter().flatten() {
            if t["out"].as_bool() != Some(false) {
                continue;
            }
            let (Some(cp), Some(minor), Some(at)) = (
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
            records.push(record(
                hash.0,
                SymbolonKind::Settlement,
                at,
                cp,
                minor,
                Flow::Received,
            ));
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
        "pistis over {} at {} ({}): score = survival x independence x recency, loop window {} d, half-life {} d",
        tenants.display(),
        statement::iso8601(now),
        now,
        p.loop_window_days,
        p.half_life_days
    );
    println!(
        "{:<13} {:<16} {:>5} {:>6} {:>6} {:>6} {:>8} {:>8} {:>8} {:>5}  read-backs held/tried",
        "account",
        "chain",
        "recs",
        "single",
        "double",
        "triple",
        "survival",
        "indep",
        "recency",
        "bps"
    );
    let mut buckets = [0usize; 10];
    let (mut all, mut n1, mut n2, mut n3, mut tried, mut held) = (0, 0, 0, 0, 0, 0);
    for policy in &accounts {
        let chain = read_json(&tenants.join(format!("{policy}.json")))
            .and_then(|v| v["chain"].as_str().map(str::to_string))
            .unwrap_or_default();
        let short = format!("{}…{}", &policy[..6], &policy[38..]);
        match records_of(&tenants, policy).await {
            Ok((records, t, h)) => {
                let s = pistis::score(&records, now, &p);
                println!(
                    "{short:<13} {chain:<16} {:>5} {:>6} {:>6} {:>6} {:>8.3} {:>8.3} {:>8.3} {:>5}  {h}/{t}",
                    records.len(),
                    s.n_single,
                    s.n_double,
                    s.n_triple,
                    s.survival,
                    s.independence,
                    s.recency,
                    s.bps
                );
                buckets[usize::from(s.bps / 1000).min(9)] += 1;
                all += records.len();
                (n1, n2, n3) = (n1 + s.n_single, n2 + s.n_double, n3 + s.n_triple);
                (tried, held) = (tried + t, held + h);
            }
            Err(e) => println!("{short:<13} {chain:<16} not scored: {e}"),
        }
    }
    println!(
        "\ntotal: {} accounts, {all} records: {n1} single, {n2} double, {n3} triple-signed, {} with no verified slot; read-backs {held}/{tried} held",
        accounts.len(),
        all - n1 - n2 - n3
    );
    println!("\nbps histogram ({} accounts):", accounts.len());
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
