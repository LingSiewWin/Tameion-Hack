//! Recall, checked (docs/117 §8). The record runs one way into an agent's
//! memory, and the ledger audits what the agent says it remembers: a cited
//! event is re-hashed and proved against the root on chain, not looked up.
//!
//! An event is named `evt:<seq>` inside one anchored record. Every pass writes
//! its own record from sequence 0 (`level1`'s `drive` starts a fresh log), so
//! one id names a different event in every record, and a citation is read in
//! one record: by default the account's latest anchored one, whose root
//! `rootOf(agent)` holds on chain; an earlier one by its root, checked by the
//! transaction that anchored it.
//!
//! A check recomputes the event's leaf from its canonical bytes (never the
//! file's word for it) and its path among the record's leaves, and folds the
//! path to the root the chain attests. `verified` needs that chain read:
//! without one the answer is `unknown`, never `verified`.

use crate::audit;
use crate::decide::Evidence;
use ledger::log::{Event, EventBody, Hash32, path_from_leaves, verify_inclusion};
use ledger::money::Money;
use serde_json::{Value, json};
use std::io;
use std::path::Path;

/// The most ids one recall checks.
pub const MAX_IDS: usize = 32;

/// The most events a decision is offered to cite.
pub const MAX_EVIDENCE: usize = 8;

/// `evt:<digits>` in its one written form (no sign, space or leading zero),
/// and nothing else.
pub fn parse_id(s: &str) -> Option<u64> {
    let d = s.strip_prefix("evt:")?;
    if d.is_empty()
        || d.len() > 19
        || (d.len() > 1 && d.starts_with('0'))
        || !d.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    d.parse().ok()
}

/// Every `[evt:N]` in `text`, in order. `Err` with the fragment at the first
/// `[evt:` that is not exactly `[evt:<digits>]`: a citation the parser does
/// not accept is refused, never dropped.
pub fn citations(text: &str) -> Result<Vec<u64>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("[evt:") {
        let after = &rest[i + 1..];
        let Some(end) = after.find(']') else {
            return Err(after.chars().take(24).collect());
        };
        let id = &after[..end];
        out.push(parse_id(id).ok_or_else(|| id.chars().take(24).collect::<String>())?);
        rest = &after[end + 1..];
    }
    Ok(out)
}

/// `text` with every well-formed `[evt:N]` taken out: an id is not a number
/// a sentence claims.
pub fn without_citations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("[evt:") {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        match after.find(']') {
            Some(end) if parse_id(&after[1..end]).is_some() => rest = &after[end + 1..],
            _ => {
                out.push_str("[evt:");
                rest = &after[5..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// What an event's canonical bytes say, read with [`Event::decode`] (the strict
/// inverse of the bytes its leaf is hashed from): `id`, `kind`, `seq`, `at`, the
/// `note`, and for an event about a payee its `payee` with the `amount` (and
/// `minor`, its minor units), the verdict (`decision`), the settling
/// transaction (`tx`) or the `reason` where the event has them; an
/// observation's `balance`. `None` when the bytes are not exactly an event.
pub fn fields(canonical: &[u8]) -> Option<Value> {
    let e = Event::decode(canonical)?;
    let mut v = json!({
        "id": e.id(), "kind": e.body.kind(), "seq": e.seq, "at": e.at.as_unix(), "note": e.body.note(),
    });
    let money = |v: &mut Value, m: &Money| {
        v["amount"] = json!(m.to_string());
        v["minor"] = json!(m.to_minor().to_string());
    };
    match &e.body {
        EventBody::Observation { balance, .. } => v["balance"] = json!(balance.to_string()),
        EventBody::Decision {
            counterparty,
            amount,
            decision,
            ..
        } => {
            v["payee"] = json!(counterparty.as_str());
            money(&mut v, amount);
            v["decision"] = json!(decision.to_string());
        }
        EventBody::Settlement {
            counterparty,
            amount,
            tx_hash,
            ..
        } => {
            v["payee"] = json!(counterparty.as_str());
            money(&mut v, amount);
            v["tx"] = json!(tx_hash);
        }
        EventBody::Escalation {
            counterparty,
            amount,
            reason,
            ..
        } => {
            v["payee"] = json!(counterparty.as_str());
            money(&mut v, amount);
            v["reason"] = json!(reason);
        }
        EventBody::Skipped { counterparty, .. } => v["payee"] = json!(counterparty.as_str()),
        _ => {}
    }
    Some(v)
}

/// One anchored record, read back.
pub struct Record {
    pub root: Hash32,
    /// Its line in `anchors.jsonl` (anchorTx, agent, anchor, at, events).
    pub anchor: Value,
    /// Whether it is the latest anchored record, the one `rootOf` should hold.
    pub latest: bool,
    pub lines: Vec<audit::Line>,
}

/// The anchored record named `root`, else the latest anchored one (the last
/// line of `anchors.jsonl` with a transaction), with its events. `Ok(None)`
/// when nothing is anchored, or not that root.
pub fn record(log_dir: &Path, root: Option<Hash32>) -> io::Result<Option<Record>> {
    let anchored: Vec<Value> = audit::anchors(log_dir)?
        .into_iter()
        .filter(|a| a["anchorTx"].is_string())
        .collect();
    let root_of = |a: &Value| a["root"].as_str().and_then(Hash32::from_hex);
    let last = anchored.last().and_then(root_of);
    let Some(want) = root.or(last) else {
        return Ok(None);
    };
    let Some(anchor) = anchored.iter().rev().find(|a| root_of(a) == Some(want)) else {
        return Ok(None);
    };
    let lines = audit::load(&log_dir.join(format!("{}.events.jsonl", want.to_hex())))?;
    Ok(Some(Record {
        root: want,
        anchor: anchor.clone(),
        latest: Some(want) == last,
        lines,
    }))
}

/// What the chain says of a record's root: `Ok(Some(root))` it holds it
/// (`rootOf`, or the transaction that anchored it), `Ok(None)` it answered
/// and holds neither, `Err` it did not answer.
pub type Attested = Result<Option<Hash32>, String>;

/// One cited id, checked against `rec` and the root the chain attests:
/// `verified` (the leaf recomputed from the event's bytes reaches that root),
/// `unverified` (it does not, or the root is not on chain), or `unknown` (no
/// such event, an id the parser refuses, or no chain to ask). A verified
/// event carries its fields, read from the hashed bytes, when `open` (the
/// caller may read the account; otherwise the leaf and path only, as the
/// record route serves an outsider).
pub fn check(id: &str, rec: &Record, chain: &Attested, open: bool) -> Value {
    // The catalogue's meaning and next step beside each code (docs/115 §2).
    let unknown = |by: &str, code: &str, words: String| {
        let entry = crate::refusal::code(code);
        json!({ "id": id, "status": "unknown", "by": by, "code": code, "words": words,
            "meaning": entry.map(|c| c.meaning), "nextStep": entry.map(|c| c.next_step) })
    };
    let Some(seq) = parse_id(id) else {
        return unknown(
            "input",
            "recall_unknown",
            format!(
                "{:?} is not an event id; ids are evt:<digits>",
                id.chars().take(40).collect::<String>()
            ),
        );
    };
    let Some(line) = usize::try_from(seq).ok().and_then(|i| rec.lines.get(i)) else {
        return unknown(
            "record",
            "recall_unknown",
            format!(
                "the record {} holds {} events; evt:{seq} is not one of them",
                rec.root,
                rec.lines.len()
            ),
        );
    };
    let i = seq as usize;
    let (leaves, _) = audit::recompute(&rec.lines);
    let leaf = leaves[i];
    let path = path_from_leaves(&leaves, i).unwrap_or_default();
    let mut v = json!({
        "id": id,
        "leaf": leaf.to_hex(),
        "path": path.iter().map(|s| json!({ "sibling": s.sibling.to_hex(), "left": s.sibling_is_left })).collect::<Vec<_>>(),
    });
    let (status, by, words) = match chain {
        Err(e) => (
            "unknown",
            Some("rpc"),
            format!("the chain did not answer ({e}); without its root nothing is verified"),
        ),
        Ok(None) => (
            "unverified",
            Some("root"),
            "the chain holds neither this record's root nor a transaction anchoring it".to_string(),
        ),
        Ok(Some(root)) if verify_inclusion(leaf, &path, *root) => (
            "verified",
            None,
            format!(
                "the event's bytes hash into the root {root} the chain holds"
            ),
        ),
        Ok(Some(_)) => (
            "unverified",
            Some("leaf"),
            "the event's bytes, hashed with the rest of its record, do not reach the root the chain holds: the record changed after it was anchored".to_string(),
        ),
    };
    v["status"] = json!(status);
    v["words"] = json!(words);
    if let Some(by) = by {
        v["by"] = json!(by);
    }
    if by == Some("rpc") {
        let entry = crate::refusal::code("chain_unavailable");
        v["code"] = json!("chain_unavailable");
        v["meaning"] = json!(entry.map(|c| c.meaning));
        v["nextStep"] = json!(entry.map(|c| c.next_step));
    }
    if status == "verified" {
        if let Ok(Some(root)) = chain {
            v["root"] = json!(root.to_hex());
        }
        if open {
            v["event"] = fields(&line.canonical).unwrap_or(Value::Null);
        }
    }
    v
}

/// What a decision may cite about `payee`: the events of the latest anchored
/// record in `log_dir` that name it, newest first, at most [`MAX_EVIDENCE`],
/// and that record's root. Nothing anchored, nothing about the payee, or a
/// record that cannot be read: no evidence, and the case is decided as before.
// ponytail: reads the record once per case; once per pass if a pass ever
// decides dozens of bills.
pub fn evidence(log_dir: &Path, payee: &str, now: i64) -> (Option<String>, Vec<Evidence>) {
    let Ok(Some(rec)) = record(log_dir, None) else {
        return (None, Vec::new());
    };
    let list: Vec<Evidence> = rec
        .lines
        .iter()
        .rev()
        .filter_map(|l| {
            let f = fields(&l.canonical)?;
            f["payee"]
                .as_str()
                .is_some_and(|p| p.eq_ignore_ascii_case(payee))
                .then(|| Evidence {
                    id: f["id"].as_str().unwrap_or("").to_string(),
                    kind: f["kind"].as_str().unwrap_or("").to_string(),
                    amount_minor: f["minor"].as_str().and_then(|m| m.parse().ok()),
                    days_ago: (now - f["at"].as_i64().unwrap_or(now)).max(0) / 86_400,
                })
        })
        .take(MAX_EVIDENCE)
        .collect();
    (Some(rec.root.to_hex()), list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ledger::log::{DecisionLog, EventBody};
    use ledger::money::{Currency, Money};
    use ledger::time::Timestamp;
    use ledger::{ActionHash, Counterparty};

    const PAYEE: &str = "0x5555555555555555555555555555555555555555";

    fn usdc(s: &str) -> Money {
        Money::parse(s, Currency::Usdc).unwrap()
    }

    /// An anchored record of three events: an observation, a payment to
    /// PAYEE and a question to the owner about another payee.
    fn anchored(dir: &Path) -> Hash32 {
        let mut log = DecisionLog::new();
        log.append(
            Timestamp::from_unix(1_800_000_000),
            EventBody::Observation {
                balance: usdc("1"),
                transfers_seen: 3,
                obligations_found: 1,
                total_due: usdc("0.05"),
                note: "Holding 1 USDC.".into(),
            },
        );
        log.append(
            Timestamp::from_unix(1_800_000_000),
            EventBody::Settlement {
                counterparty: Counterparty::new(PAYEE),
                cycle_key: format!("{PAYEE}@1800000000"),
                amount: usdc("0.05"),
                tx_hash: "0xfeed".into(),
                action_hash: ActionHash::from_bytes([1; 32]),
                settlement_key: ActionHash::from_bytes([2; 32]),
                note: "Paid the hosting bill.".into(),
            },
        );
        log.append(
            Timestamp::from_unix(1_800_000_000),
            EventBody::Escalation {
                counterparty: Counterparty::new("0x6666666666666666666666666666666666666666"),
                amount: usdc("9"),
                reason: "new payee".into(),
                note: "Asked the owner.".into(),
            },
        );
        audit::persist(dir, &log).unwrap();
        audit::append_anchor(
            dir,
            log.root(),
            Some("0xabc".into()),
            Some("0x00000000000000000000000000000000000000a9".into()),
            "0x00000000000000000000000000000000000000a1".into(),
            1_800_000_000,
            log.len(),
        )
        .unwrap();
        log.root()
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tameion-recall-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn ids_and_citations_parse_only_in_their_one_form() {
        assert_eq!(parse_id("evt:0"), Some(0));
        assert_eq!(parse_id("evt:42"), Some(42));
        for bad in [
            "evt:",
            "evt:-1",
            "evt: 1",
            "evt:01",
            "evt:1a",
            "EVT:1",
            "evt:0x1",
            "evt:1:2",
            "1",
            "evt:99999999999999999999",
        ] {
            assert_eq!(parse_id(bad), None, "{bad}");
        }
        assert_eq!(
            citations("Paid as last time [evt:1], asked [evt:12]."),
            Ok(vec![1, 12])
        );
        assert_eq!(citations("no citation, 4 payments"), Ok(vec![]));
        for (text, bad) in [
            ("[evt:1, evt:2]", "evt:1, evt:2"),
            ("[evt:0xdead]", "evt:0xdead"),
            ("[evt:3", "evt:3"),
        ] {
            assert_eq!(citations(text), Err(bad.to_string()), "{text}");
        }
        assert_eq!(
            without_citations("Paid 0.05 [evt:1] as usual [evt:x]."),
            "Paid 0.05  as usual [evt:x]."
        );
    }

    #[test]
    fn a_true_event_verifies_a_forged_one_does_not_and_no_chain_is_unknown() {
        let d = dir("check");
        let root = anchored(&d);
        let rec = record(&d, None).unwrap().unwrap();
        assert!(rec.latest);
        assert_eq!(rec.root, root);
        let v = check("evt:1", &rec, &Ok(Some(root)), true);
        assert_eq!(v["status"], "verified", "{v}");
        assert_eq!(v["root"], root.to_hex());
        assert_eq!(v["event"]["kind"], "SETTLEMENT");
        // The cited id is the event's own (`Event::id`, read back by `Event::decode`).
        assert_eq!(v["event"]["id"], v["id"]);
        assert_eq!(v["event"]["payee"], PAYEE);
        assert_eq!(v["event"]["amount"], usdc("0.05").to_string());
        assert_eq!(v["event"]["tx"], "0xfeed");
        assert_eq!(v["event"]["note"], "Paid the hosting bill.");
        // An outsider on a locked account: the proof, not the event.
        let closed = check("evt:1", &rec, &Ok(Some(root)), false);
        assert_eq!(closed["status"], "verified");
        assert!(closed["event"].is_null());
        // No chain: never verified.
        let v = check("evt:1", &rec, &Err("connection refused".into()), true);
        assert_eq!(
            (v["status"].as_str(), v["by"].as_str()),
            (Some("unknown"), Some("rpc"))
        );
        assert!(v["event"].is_null());
        // The chain holds another root and no transaction anchors this one.
        let v = check("evt:1", &rec, &Ok(None), true);
        assert_eq!(
            (v["status"].as_str(), v["by"].as_str()),
            (Some("unverified"), Some("root"))
        );
        // No such event; an id the parser refuses.
        let v = check("evt:3", &rec, &Ok(Some(root)), true);
        assert_eq!(
            (v["status"].as_str(), v["code"].as_str()),
            (Some("unknown"), Some("recall_unknown"))
        );
        assert_eq!(
            v["nextStep"],
            crate::refusal::code("recall_unknown").unwrap().next_step
        );
        let v = check("evt:1;rm", &rec, &Ok(Some(root)), true);
        assert_eq!(
            (v["status"].as_str(), v["by"].as_str()),
            (Some("unknown"), Some("input"))
        );

        // Forge the payment: one byte of its canonical amount changed on disk.
        let file = d.join(format!("{}.events.jsonl", root.to_hex()));
        let text = std::fs::read_to_string(&file).unwrap();
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        let mut forged: Value = serde_json::from_str(&lines[1]).unwrap();
        let c = forged["canonical"].as_str().unwrap().to_string();
        let amount = format!("{:032x}", 50_000u128);
        assert!(c.contains(&amount));
        forged["canonical"] = json!(c.replacen(&amount, &format!("{:032x}", 5_000_000u128), 1));
        lines[1] = forged.to_string();
        std::fs::write(&file, lines.join("\n")).unwrap();
        let rec = record(&d, None).unwrap().unwrap();
        for id in ["evt:0", "evt:1", "evt:2"] {
            let v = check(id, &rec, &Ok(Some(root)), true);
            assert_eq!(
                (v["status"].as_str(), v["by"].as_str()),
                (Some("unverified"), Some("leaf")),
                "{id}: {v}"
            );
            assert!(
                v["event"].is_null(),
                "a forged event's fields are never served"
            );
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn evidence_is_the_latest_records_events_about_the_payee() {
        let d = dir("evidence");
        assert_eq!(evidence(&d, PAYEE, 0), (None, vec![]), "nothing anchored");
        let root = anchored(&d);
        let (r, list) = evidence(
            &d,
            &PAYEE.to_uppercase().replace("0X", "0x"),
            1_800_000_000 + 2 * 86_400,
        );
        assert_eq!(r, Some(root.to_hex()));
        assert_eq!(
            list,
            vec![Evidence {
                id: "evt:1".into(),
                kind: "SETTLEMENT".into(),
                amount_minor: Some(50_000),
                days_ago: 2,
            }]
        );
        assert!(
            record(&d, Some(Hash32::ZERO)).unwrap().is_none(),
            "not that root"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
