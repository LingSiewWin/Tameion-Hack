//! The one-way valve (docs/117 §8): what a model writes reaches the record
//! only as words. Amounts, payees, action hashes and transactions are code's,
//! so a model that names a fake amount, a fake payee and a fake transaction
//! changes none of them -- and the only code that writes the record's events
//! files is `audit::persist`. Each test fails if someone wires model output
//! into a ledger field or writes the record from somewhere else.
//!
//!     cargo test -p operator --test valve

use alloy::primitives::{Address, keccak256};
use arc::ArcClient;
use ledger::log::EventBody;
use ledger::{ActorKind, Counterparty, Currency, Direction, Money, Timestamp, Transfer};
use operator::decide::{Action, Basis, Case, Decider, Decision};
use operator::reasoner::{ReasonerError, RuleBased};
use operator::{Config, Operator};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const PAYEE: &str = "0x1111111111111111111111111111111111111111";
const FAKE_PAYEE: &str = "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
const FAKE_TX: &str = "0xfeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedface";

fn selector(sig: &str) -> String {
    alloy::hex::encode(&keccak256(sig)[..4])
}

/// A policy node in Autopilot: every payee approved, big caps, nothing
/// executed, no standing orders. Sending a transaction fails, so a pass that
/// tried to pay would fail loudly; anchoring fails and the pass reports it.
async fn node() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let body = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len: usize = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse().ok())?
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break text[end + 4..end + 4 + len].to_string();
                        }
                    }
                };
                let req: Value = serde_json::from_str(&body).unwrap();
                let call = &req["params"][0];
                let data = call["input"]
                    .as_str()
                    .or(call["data"].as_str())
                    .unwrap_or("");
                let sel = data.trim_start_matches("0x").get(..8).unwrap_or("");
                let word = |v: u128| json!(format!("0x{v:064x}"));
                let result: Result<Value, &str> = match req["method"].as_str().unwrap_or("") {
                    "eth_blockNumber" => Ok(json!("0x10")),
                    "eth_getLogs" => Ok(json!([])),
                    "eth_call" => Ok(match sel {
                        s if s == selector("mode()") => word(2),
                        s if s == selector("agentExpiry()") => word(u64::MAX.into()),
                        s if s == selector("balanceUsdc()")
                            || s == selector("perActionCap()")
                            || s == selector("dailyCap()") =>
                        {
                            word(1_000_000_000_000)
                        }
                        s if s == selector("attestedPayee(address)") => word(1),
                        s if s == selector("executed(bytes32)") => word(0),
                        _ => json!(format!("0x{}", "0".repeat(64 * 8))),
                    }),
                    _ => Err("unsupported"),
                };
                let reply = match result {
                    Ok(r) => json!({ "jsonrpc": "2.0", "id": req["id"], "result": r }),
                    Err(m) => {
                        json!({ "jsonrpc": "2.0", "id": req["id"], "error": { "code": -32000, "message": m } })
                    }
                };
                let out = reply.to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                    out.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    url
}

/// A model that asks the owner, with whatever sentence it was built with.
struct Says(String);

impl Decider for Says {
    async fn decide(&self, _case: &Case) -> Result<Decision, ReasonerError> {
        Ok(Decision {
            action: Action::Ask,
            basis: Basis::AmountDeviates,
            why: self.0.clone(),
            by: ActorKind::Model,
            model: Some("lying-model".into()),
        })
    }

    fn name(&self) -> &str {
        "lying-model"
    }

    fn kind(&self) -> ActorKind {
        ActorKind::Model
    }
}

/// One pass over six monthly payments of 20 USDC to PAYEE, the last 31 days
/// ago (one cycle due), with a model saying `why`. Returns the outcome and
/// the record's events.
async fn pass(why: &str, tag: &str) -> (operator::RunOutcome, Vec<ledger::log::Event>) {
    let url = node().await;
    let client = ArcClient::with_signer(&url, &format!("0x{}", "11".repeat(32))).unwrap();
    let policy = Address::repeat_byte(0xcc);
    let mut config = Config::new(policy, policy, Address::repeat_byte(0xdd));
    let dir = std::env::temp_dir().join(format!("valve-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    config.audit_dir = Some(dir.clone());
    let mut op = Operator::new_unchecked(client, config, RuleBased);
    const DAY: i64 = 86_400;
    let now = Timestamp::from_unix(1_800_000_000);
    let history: Vec<Transfer> = (0..6)
        .map(|i| Transfer {
            tx_hash: format!("0xseed{i:02}"),
            counterparty: Counterparty::new(PAYEE),
            amount: Money::minor(20_000_000, Currency::Usdc),
            at: Timestamp::from_unix(now.as_unix() - (31 + (5 - i) * 30) * DAY),
            direction: Direction::Out,
        })
        .collect();
    let outcome = op
        .run_once_deciding(now, Some(history), true, &Says(why.into()))
        .await
        .unwrap();
    let events = op.log().events().to_vec();
    let _ = std::fs::remove_dir_all(&dir);
    (outcome, events)
}

/// Every field of an event that code writes, as text; the words (`note`,
/// `reason`) left out.
fn ledger_fields(body: &EventBody) -> Vec<String> {
    match body {
        EventBody::Decision {
            counterparty,
            amount,
            decision,
            action_hash,
            confidence,
            balance_seen,
            evidence,
            ..
        } => {
            let mut v = vec![
                counterparty.as_str().to_string(),
                amount.to_string(),
                decision.to_string(),
                format!("{action_hash:?}"),
                confidence.get().to_string(),
                balance_seen.to_string(),
            ];
            v.extend(evidence.iter().cloned());
            v
        }
        EventBody::Settlement {
            counterparty,
            cycle_key,
            amount,
            tx_hash,
            action_hash,
            settlement_key,
            ..
        } => vec![
            counterparty.as_str().to_string(),
            cycle_key.clone(),
            amount.to_string(),
            tx_hash.clone(),
            format!("{action_hash:?}"),
            format!("{settlement_key:?}"),
        ],
        EventBody::Escalation {
            counterparty,
            amount,
            ..
        } => vec![counterparty.as_str().to_string(), amount.to_string()],
        EventBody::Skipped {
            counterparty,
            cycle_key,
            ..
        } => vec![counterparty.as_str().to_string(), cycle_key.clone()],
        EventBody::Observation {
            balance, total_due, ..
        } => vec![balance.to_string(), total_due.to_string()],
        _ => Vec::new(),
    }
}

/// A model naming a fake payee and a fake transaction: its sentence is kept,
/// as words only. The Decision's payee, amount and action hash are the case's;
/// no ledger field of any event carries what the model said; the owner's
/// calldata pays the case's payee the case's amount.
#[tokio::test]
async fn a_models_fake_payee_and_transaction_reach_only_the_words() {
    let why = format!("Send it to {FAKE_PAYEE} instead; transaction {FAKE_TX} proves it was paid.");
    let (o, events) = pass(&why, "words").await;
    let (case, d, note) = &o.decisions[0];
    assert_eq!(
        (d.by, d.why.as_str(), note),
        (ActorKind::Model, why.as_str(), &None),
        "the sentence stands"
    );
    let decision = events
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Decision {
                counterparty,
                amount,
                action_hash,
                note,
                ..
            } => Some((counterparty.clone(), *amount, *action_hash, note.clone())),
            _ => None,
        })
        .expect("a Decision in the record");
    assert_eq!(decision.0.as_str(), PAYEE);
    assert_eq!(decision.1, Money::minor(20_000_000, Currency::Usdc));
    assert_eq!(
        u128::try_from(decision.1.to_minor()).unwrap(),
        case.amount_minor
    );
    assert_eq!(
        format!(
            "{:#x}",
            alloy::primitives::B256::from(*decision.2.as_bytes())
        ),
        o.pending_signature[0].action_hash,
        "the action hash is the pass's, the one the owner's calldata carries"
    );
    assert!(decision.3.contains(&why), "the sentence is in the note");
    for e in &events {
        for f in ledger_fields(&e.body) {
            let f = f.to_lowercase();
            assert!(
                !f.contains(&FAKE_PAYEE[2..]) && !f.contains(&FAKE_TX[2..]),
                "evt:{} {}: a ledger field carries the model's words: {f}",
                e.seq,
                e.body.kind()
            );
        }
    }
    let p = &o.pending_signature[0];
    assert_eq!(
        (p.counterparty.as_str(), p.amount),
        (PAYEE, Money::minor(20_000_000, Currency::Usdc))
    );
    let data = p.call.data.to_lowercase();
    assert!(
        data.contains(&PAYEE[2..]) && data.contains(&format!("{:064x}", 20_000_000u128)),
        "{data}"
    );
    assert!(!data.contains(&FAKE_PAYEE[2..]), "{data}");
    assert!(o.settled.is_empty(), "nothing was paid");
}

/// A model naming a fake amount: its sentence is replaced by the code's and
/// the note says why; the amount everywhere is the case's.
#[tokio::test]
async fn a_models_fake_amount_changes_no_amount() {
    let (o, events) = pass("Pay 999.99 USDC now, the usual.", "amount").await;
    let (_, d, note) = &o.decisions[0];
    assert!(!d.why.contains("999.99"), "{}", d.why);
    assert!(
        note.as_deref()
            .is_some_and(|n| n.contains("999.99") && n.contains("not in the facts")),
        "{note:?}"
    );
    for e in &events {
        for f in ledger_fields(&e.body) {
            assert!(!f.contains("999.99"), "evt:{}: {f}", e.seq);
        }
        if let EventBody::Decision { amount, .. } | EventBody::Escalation { amount, .. } = &e.body {
            assert_eq!(*amount, Money::minor(20_000_000, Currency::Usdc));
        }
    }
    assert_eq!(
        o.pending_signature[0].amount,
        Money::minor(20_000_000, Currency::Usdc)
    );
}

// --- the only writer of the record ------------------------------------------------

/// Source files of the workspace's crates (src, bin, examples), with their
/// lines outside top-level `#[cfg(test)]` modules (rustfmt closes those with
/// a `}` in column 0) and outside `//` comments.
fn code_lines() -> Vec<(String, usize, String)> {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let mut files = Vec::new();
    let crates_root = crates.clone();
    let mut stack = vec![crates];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if p.is_dir() {
                if !matches!(name.as_str(), "target" | "tests" | "fixtures" | "benches") {
                    stack.push(p);
                }
            } else if name.ends_with(".rs") {
                files.push(p);
            }
        }
    }
    let mut out = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let rel = f
            .strip_prefix(&crates_root)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let mut in_test = false;
        for (i, line) in text.lines().enumerate() {
            if line == "#[cfg(test)]" {
                in_test = true;
            }
            if in_test {
                in_test = line != "}";
                continue;
            }
            if !line.trim_start().starts_with("//") {
                out.push((rel.clone(), i + 1, line.to_string()));
            }
        }
    }
    out
}

/// docs/117 §8: `<root>.events.jsonl` is written by `audit::persist` alone,
/// and `persist` is called by the pass alone (right before the root goes on
/// chain). Every write primitive within 15 lines of a line naming
/// `events.jsonl` is in `audit.rs`; every `persist` call is listed here. A new
/// writer or caller fails this until it is listed on purpose.
#[test]
fn the_only_writer_of_events_files_is_audit_persist() {
    let lines = code_lines();
    let callers: Vec<String> = lines
        .iter()
        .filter(|(_, _, l)| l.contains("audit::persist("))
        .map(|(f, n, _)| format!("{f}:{n}"))
        .collect();
    println!("audit::persist call sites outside tests: {callers:?}");
    // Two writers, each listed on purpose: the pass (`lib.rs`), and the token
    // meter's reconciliation record (`meter.rs`, docs/117 §6): our meter against
    // the provider's own usage report, numbers from code, never a model's text.
    // A third writer is a decision, not a drift: add it here with its reason.
    let mut writers: Vec<&str> = callers
        .iter()
        .map(|c| c.split(':').next().unwrap())
        .collect();
    writers.sort_unstable();
    writers.dedup();
    assert_eq!(
        writers,
        vec!["operator/src/lib.rs", "operator/src/meter.rs"],
        "persist is called by the pass and the meter alone: {callers:?}"
    );
    let writes = [
        "fs::write(",
        "File::create(",
        "OpenOptions::new()",
        "fs::rename(",
        "fs::copy(",
    ];
    let mut writers = Vec::new();
    for (i, (f, n, l)) in lines.iter().enumerate() {
        if !writes.iter().any(|w| l.contains(w)) {
            continue;
        }
        let near = lines[i.saturating_sub(15)..i + 1]
            .iter()
            .any(|(g, _, m)| g == f && m.contains("events.jsonl"));
        if near {
            writers.push(format!("{f}:{n}"));
        }
    }
    println!("writes beside an events.jsonl path: {writers:?}");
    assert!(!writers.is_empty(), "the scan finds persist's own writes");
    assert!(
        writers
            .iter()
            .all(|w| w.starts_with("operator/src/audit.rs:")),
        "only audit.rs writes an events file: {writers:?}"
    );
    // And inside audit.rs, those writes are persist's (the temp file and its rename).
    let audit =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/audit.rs")).unwrap();
    let persist = &audit[audit.find("pub fn persist(").unwrap()..];
    let persist = &persist[..persist.find("\n}\n").unwrap()];
    assert!(persist.contains("File::create(&tmp)") && persist.contains("fs::rename(&tmp, &path)"));
}
