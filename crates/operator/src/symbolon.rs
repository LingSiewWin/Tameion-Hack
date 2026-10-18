//! One account's records as symbolons (`ledger::symbolon`), each with only
//! the slots that can be checked again and held in this run. The example
//! `symbolon_count` and `GET /api/level1/symbolon` both read through here:
//!
//! - witness: the root's anchoring transaction, read back on the account's
//!   chain by the caller's `read` (it succeeded, the agent sent it to the
//!   anchor, the root is in its calldata: `ArcClient::anchored_in`);
//! - counterparty: a payee-signed invoice whose signature recovers the payee
//!   (`invoice::verify`), for a settlement of that invoice's bill cycle
//!   (`<payee>@<dueAt + k * everyDays>`);
//! - agent: none; no record carries the agent's own signature yet.
//!
//! A record persisted again in a later file counts once, with its best slots.
//! A failed chain read is kept apart, never counted as "not witnessed".

use crate::sig::{self, Alg, KeyRef};
use crate::{audit, invoice, level1};
use alloy::primitives::{Address, B256};
use ledger::log::Hash32;
use ledger::symbolon::{Extras, SlotSig, Symbolon, project, record_hash};
use ledger::{Event, EventBody};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A received invoice whose signature recovers its payee: payee, first due
/// time, interval in seconds (0 = one-off), and the slot it fills.
pub type Signed = (String, u64, u64, SlotSig);

/// The account's received invoices whose signature and terms verify, each as
/// of its issue date.
pub fn signed_invoices(app: &Path, policy: Address, chain_id: u64) -> Vec<Signed> {
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
pub fn invoice_paid<'a>(invoices: &'a [Signed], cycle_key: &str) -> Option<&'a SlotSig> {
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

/// One record at its best: the events file (its root) and the line it was
/// read from, the event's time and canonical bytes, and its slots.
pub struct Found {
    pub root: Hash32,
    pub seq: u64,
    pub at: i64,
    pub canonical: Vec<u8>,
    pub symbolon: Symbolon,
}

/// One account's records, by record hash, and what the run saw on the way.
#[derive(Default)]
pub struct Account {
    pub records: BTreeMap<[u8; 32], Found>,
    /// Events files read.
    pub files: usize,
    /// Files the chain answered for and did not witness, or never anchored.
    pub not_witnessed: usize,
    /// Files whose anchoring could not be read, with the last error.
    pub read_failed: Vec<(Hash32, String)>,
    /// Lines whose bytes are not exactly an event.
    pub undecodable: usize,
    /// Settlement lines of a signed invoice's bill cycle.
    pub invoices_paid: usize,
}

impl Account {
    /// How many records stand behind none, one, two and three slots.
    pub fn counts(&self) -> [usize; 4] {
        let mut c = [0; 4];
        for f in self.records.values() {
            c[f.symbolon.signedness() as usize] += 1;
        }
        c
    }
}

/// One anchor line to read back on chain: the transaction, the anchor
/// contract, the agent that sent it, the root it anchors, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchoring {
    pub tx: B256,
    pub anchor: Address,
    pub agent: Address,
    pub root: B256,
    pub at: i64,
}

/// What reading one anchoring back said: yes, no, or the error.
pub type Read = Result<bool, String>;

/// The anchor lines in `log` that name a transaction, an anchor and an agent,
/// oldest first (of the root `only`, when given): what the caller reads back.
pub fn anchorings(log: &Path, only: Option<Hash32>) -> io::Result<Vec<Anchoring>> {
    Ok(audit::anchors(log)?
        .iter()
        .filter_map(|a| {
            let field = |k: &str| a[k].as_str().unwrap_or_default().trim().to_string();
            let root = Hash32::from_hex(&field("root"))?;
            if only.is_some_and(|o| o != root) {
                return None;
            }
            Some(Anchoring {
                tx: B256::from_str(&field("anchorTx")).ok()?,
                anchor: Address::from_str(&field("anchor")).ok()?,
                agent: Address::from_str(&field("agent")).ok()?,
                root: B256::from(*root.as_bytes()),
                at: a["at"].as_i64().unwrap_or(0),
            })
        })
        .collect())
}

/// Every events file in `log` (or only `<only>.events.jsonl`) projected with
/// the slots that held: `invoices` for the counterparty, and for the witness
/// the first anchoring of a file whose bytes recompute to its name that
/// `read` says yes to (an anchoring missing from `read` was not read).
/// `chain_id` names the witness (`chain:<id>`). No log yet is an empty
/// account; a missing `only` file is `NotFound`.
pub fn account(
    log: &Path,
    invoices: &[Signed],
    chain_id: u64,
    only: Option<Hash32>,
    read: &[(Anchoring, Read)],
) -> io::Result<Account> {
    let mut acc = Account::default();
    let mut files: Vec<PathBuf> = match only {
        Some(r) => vec![log.join(format!("{}.events.jsonl", r.to_hex()))],
        None => match std::fs::read_dir(log) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| audit::root_from_filename(p).is_some())
                .collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(acc),
            Err(e) => return Err(e),
        },
    };
    files.sort();
    for f in files {
        acc.files += 1;
        let lines = audit::load(&f)?;
        let (_, root) = audit::recompute(&lines);
        let mut extras = Extras::default();
        let mut failed = None;
        // Only a file whose bytes recompute to its name can be witnessed.
        if audit::root_from_filename(&f) == Some(root) {
            let want = B256::from(*root.as_bytes());
            for (a, r) in read.iter().filter(|(a, _)| a.root == want) {
                match r {
                    Ok(true) => {
                        extras.anchored = Some(SlotSig {
                            alg: "chain-receipt".into(),
                            key_id: format!("chain:{chain_id}"),
                            sig: a.tx.to_vec(),
                            at: a.at,
                        });
                        break;
                    }
                    Ok(false) => {}
                    Err(e) => failed = Some(e.clone()),
                }
            }
        }
        if extras.anchored.is_none() {
            match failed {
                Some(e) => acc.read_failed.push((root, e)),
                None => acc.not_witnessed += 1,
            }
        }
        let mut events = Vec::new();
        let mut canonical: BTreeMap<[u8; 32], Vec<u8>> = BTreeMap::new();
        for l in &lines {
            if let Some(e) = Event::decode(&l.canonical) {
                canonical.insert(record_hash(&e), l.canonical.clone());
                events.push(e);
            }
        }
        acc.undecodable += lines.len() - events.len();
        for e in &events {
            if let EventBody::Settlement { cycle_key, .. } = &e.body
                && let Some(s) = invoice_paid(invoices, cycle_key)
            {
                acc.invoices_paid += 1;
                extras.counterparty.insert(record_hash(e), s.clone());
            }
        }
        let when: BTreeMap<[u8; 32], (u64, i64)> = events
            .iter()
            .map(|e| (record_hash(e), (e.seq, e.at.as_unix())))
            .collect();
        for s in project(&events, &extras) {
            // The first occurrence with the most slots stands for the record.
            if acc
                .records
                .get(&s.record_hash)
                .is_some_and(|f| f.symbolon.signedness() >= s.signedness())
            {
                continue;
            }
            let (seq, at) = when.get(&s.record_hash).copied().unwrap_or_default();
            let found = Found {
                root,
                seq,
                at,
                canonical: canonical.remove(&s.record_hash).unwrap_or_default(),
                symbolon: s,
            };
            acc.records.insert(found.symbolon.record_hash, found);
        }
    }
    Ok(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::SignerSync;
    use alloy::signers::local::PrivateKeySigner;
    use ledger::canon::ActionHash;
    use ledger::log::DecisionLog;
    use ledger::money::{Currency, Money};
    use ledger::obligations::Counterparty;
    use ledger::symbolon::Signedness;
    use ledger::time::Timestamp;

    const CHAIN: u64 = 421_614;
    const DUE: u64 = 1_792_000_000;

    /// A payee-signed invoice for `policy` (30-day cycle from `DUE`), as the
    /// invoice store keeps it, and the settlement of its second cycle in a
    /// pass that also observed and decided. One anchored file, one not.
    #[test]
    fn an_account_is_counted_once_per_record_with_its_best_slots() {
        let app = std::env::temp_dir().join(format!("symbolon-acc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app);
        let policy = Address::from_str("0x26e6901e08b9928bbbc7d3523c9102da5e361e53").unwrap();
        let log = level1::tenants_dir(&app)
            .join(format!("{policy:#x}"))
            .join("log");
        std::fs::create_dir_all(&log).unwrap();

        let vendor = PrivateKeySigner::random();
        let inv = invoice::Invoice {
            payee: vendor.address(),
            policy,
            amount: 2_000,
            due_at: DUE,
            every_days: 30,
            discount_bps: 0,
            early_days: 0,
            reference: alloy::primitives::keccak256("INV-1"),
            issued_at: DUE - 86_400,
            issuer: None,
            alg: None,
            key_hash: None,
        };
        let s = vendor
            .sign_hash_sync(&invoice::digest(CHAIN, &inv))
            .unwrap();
        std::fs::write(
            level1::tenants_dir(&app).join(format!("{policy:#x}.invoices.jsonl")),
            format!("{}\n", invoice::to_json(&inv, &s.as_bytes())),
        )
        .unwrap();
        let invoices = signed_invoices(&app, policy, CHAIN);
        assert_eq!(invoices.len(), 1, "the payee's signature verifies");

        let usdc = |s: &str| Money::parse(s, Currency::Usdc).unwrap();
        let payee = format!("{:#x}", vendor.address());
        let mut pass = DecisionLog::new();
        pass.append(
            Timestamp::from_unix(1_792_100_000),
            EventBody::Observation {
                balance: usdc("1"),
                transfers_seen: 1,
                obligations_found: 1,
                total_due: usdc("0.002"),
                note: "saw".into(),
            },
        );
        pass.append(
            Timestamp::from_unix(1_792_100_001),
            EventBody::Settlement {
                counterparty: Counterparty::new(&payee),
                cycle_key: format!("{payee}@{}", DUE + 30 * 86_400),
                amount: usdc("0.002"),
                tx_hash: "0xfeed".into(),
                action_hash: ActionHash::from_bytes([1; 32]),
                settlement_key: ActionHash::from_bytes([2; 32]),
                note: "paid".into(),
            },
        );
        let first = audit::persist(&log, &pass).unwrap();
        let root = audit::root_from_filename(&first).unwrap();
        audit::append_anchor(
            &log,
            root,
            Some(format!("0x{}", "ab".repeat(32))),
            Some("0x0000000000000000000000000000000000000003".into()),
            "0x0000000000000000000000000000000000000004".into(),
            1_792_100_002,
            2,
        )
        .unwrap();

        let anchored = anchorings(&log, None).unwrap();
        assert_eq!(anchored.len(), 1);
        assert_eq!(anchored[0].root, B256::from(*root.as_bytes()));
        let say = |r: Read| vec![(anchored[0].clone(), r)];

        // The chain says yes: witness and counterparty, double.
        let acc = account(&log, &invoices, CHAIN, None, &say(Ok(true))).unwrap();
        assert_eq!(acc.files, 1);
        assert_eq!(acc.records.len(), 1, "an observation is not a record");
        let f = acc.records.values().next().unwrap();
        assert_eq!((f.root, f.seq, f.at), (root, 1, 1_792_100_001));
        assert_eq!(f.symbolon.signedness(), Signedness::Double);
        assert_eq!(f.symbolon.witness.as_ref().unwrap().key_id, "chain:421614");
        assert_eq!(
            f.symbolon.counterparty.as_ref().unwrap().key_id,
            payee,
            "the payee's address names the counterparty slot"
        );
        assert!(Event::decode(&f.canonical).is_some());
        assert_eq!(acc.counts(), [0, 0, 1, 0]);
        assert_eq!(acc.invoices_paid, 1);

        // The chain does not answer: no witness, and kept apart from "not witnessed".
        let down = account(
            &log,
            &invoices,
            CHAIN,
            None,
            &say(Err("connection refused".into())),
        )
        .unwrap();
        assert_eq!(down.counts(), [0, 1, 0, 0]);
        assert_eq!(
            down.read_failed,
            vec![(root, "connection refused".to_string())]
        );
        assert_eq!(down.not_witnessed, 0);

        // The same pass persisted again, never anchored: still one record, at its best.
        let mut again = DecisionLog::new();
        for e in pass.events() {
            again.append(e.at, e.body.clone());
        }
        again.append(
            Timestamp::from_unix(1_792_100_003),
            EventBody::Observation {
                balance: usdc("0.998"),
                transfers_seen: 2,
                obligations_found: 1,
                total_due: usdc("0"),
                note: "after".into(),
            },
        );
        let second = audit::persist(&log, &again).unwrap();
        assert_ne!(second, first);
        let both = account(&log, &invoices, CHAIN, None, &say(Ok(true))).unwrap();
        assert_eq!((both.files, both.not_witnessed), (2, 1));
        assert_eq!(both.counts(), [0, 0, 1, 0], "one record, its best slots");
        assert_eq!(both.records.values().next().unwrap().root, root);

        // One root asked by name; a root with no file is NotFound.
        assert_eq!(anchorings(&log, Some(Hash32::ZERO)).unwrap(), []);
        let one = account(&log, &[], CHAIN, Some(root), &say(Ok(false))).unwrap();
        assert_eq!(
            (one.files, one.not_witnessed, one.counts()),
            (1, 1, [1, 0, 0, 0])
        );
        let none = account(&log, &[], CHAIN, Some(Hash32::ZERO), &[]);
        assert_eq!(none.err().unwrap().kind(), io::ErrorKind::NotFound);
        // No log yet: an empty account, not an error.
        let empty = account(&app.join("nowhere"), &[], CHAIN, None, &[]).unwrap();
        assert_eq!((empty.files, empty.records.len()), (0, 0));
        let _ = std::fs::remove_dir_all(&app);
    }
}
