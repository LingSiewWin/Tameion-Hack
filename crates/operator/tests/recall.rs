//! `GET /api/level1/recall` through the real binary (docs/117 §8): a true
//! event is verified against the root a node returns for `rootOf`, a forged
//! one is unverified, an unknown id is `recall_unknown`, no node means
//! `unknown` by rpc and never verified, and on a locked account an outsider
//! gets the proof without the event.

use ledger::log::{DecisionLog, EventBody, Hash32};
use ledger::{ActionHash, Counterparty, Currency, Money, Timestamp};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const POLICY: &str = "0x00000000000000000000000000000000000000e1";
const AGENT: &str = "0x00000000000000000000000000000000000000a9";
const ANCHOR: &str = "0x00000000000000000000000000000000000000a1";
const PAYEE: &str = "0x5555555555555555555555555555555555555555";

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(offset: u16, dir: &std::path::Path, rpc: &str) -> Server {
    let port = 27_000 + (std::process::id() % 9_000) as u16 + offset;
    let child = Command::new(env!("CARGO_BIN_EXE_tameion-app"))
        .env("PORT", port.to_string())
        .env("TAMEION_APP_DIR", dir)
        .env("TAMEION_APP_ADMIN_TOKEN", "t0k3n")
        .env("TAMEION_RPC_ARC", rpc)
        .env_remove("TAMEION_LEVEL1_INVITE")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tameion-app");
    let srv = Server { child, port };
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return srv;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("tameion-app did not listen on {port}");
}

fn get(port: u16, path: &str, extra: &str) -> (u16, Value) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\n{extra}Connection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out
        .split_whitespace()
        .nth(1)
        .and_then(|x| x.parse().ok())
        .unwrap_or(0);
    let body = out.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

/// A node that answers every `eth_call` (the anchor's `rootOf`) with `root`.
fn node(root: Hash32) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            let mut buf = vec![0u8; 16_384];
            let mut n = 0;
            let body = loop {
                let k = s.read(&mut buf[n..]).unwrap_or(0);
                if k == 0 {
                    break None;
                }
                n += k;
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len: usize = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse().ok())?
                        })
                        .unwrap_or(0);
                    if body.len() >= len {
                        break Some(body[..len].to_string());
                    }
                }
            };
            let Some(body) = body else { continue };
            let req: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let reply = if req["method"] == "eth_call" {
                json!({ "jsonrpc": "2.0", "id": req["id"], "result": root.to_hex() })
            } else {
                json!({ "jsonrpc": "2.0", "id": req["id"], "error": { "code": -32601, "message": "unsupported" } })
            };
            let out = reply.to_string();
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                out.len()
            );
        }
    });
    url
}

/// An account (locked: one read key) with one anchored record of three
/// events: an observation, a payment to PAYEE, a question to the owner.
fn account(tag: &str) -> (std::path::PathBuf, Hash32) {
    let dir =
        std::env::temp_dir().join(format!("tameion-recall-http-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let log_dir = dir.join("tenants").join(POLICY).join("log");
    let read = operator::level1::token_hash("k-read");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(
        dir.join("tenants").join(format!("{POLICY}.json")),
        format!(
            r#"{{"chain":"arc-testnet","owner":"0x0000000000000000000000000000000000000001","policy":"{POLICY}","agent":"{AGENT}","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":true,"agent_tokens":[{{"id":"r1","label":"reader","scopes":["read"],"expires":4102444800,"issued":0,"hash":"{read}"}}]}}"#
        ),
    )
    .unwrap();
    let usdc = |s: &str| Money::parse(s, Currency::Usdc).unwrap();
    let at = Timestamp::from_unix(1_800_000_000);
    let mut log = DecisionLog::new();
    log.append(
        at,
        EventBody::Observation {
            balance: usdc("1"),
            transfers_seen: 3,
            obligations_found: 1,
            total_due: usdc("0.05"),
            note: "Holding 1 USDC.".into(),
        },
    );
    log.append(
        at,
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
        at,
        EventBody::Escalation {
            counterparty: Counterparty::new("0x6666666666666666666666666666666666666666"),
            amount: usdc("9"),
            reason: "new payee".into(),
            note: "Asked the owner.".into(),
        },
    );
    operator::audit::persist(&log_dir, &log).unwrap();
    operator::audit::append_anchor(
        &log_dir,
        log.root(),
        Some(format!("0x{}", "cd".repeat(32))),
        Some(AGENT.into()),
        ANCHOR.into(),
        1_800_000_000,
        log.len(),
    )
    .unwrap();
    (dir, log.root())
}

#[test]
fn a_true_event_verifies_against_the_chains_root_and_the_rest_do_not() {
    let (dir, root) = account("true");
    let srv = start(0, &dir, &node(root));
    let reader = "Authorization: Bearer k-read\r\n";
    let q = format!("/api/level1/recall?policy={POLICY}&ids=evt%3A1,evt:9,evt:x");
    let (st, v) = get(srv.port, &q, reader);
    assert_eq!(st, 200, "{v}");
    assert_eq!(
        (v["ok"].as_bool(), v["contractVersion"].is_string()),
        (Some(true), true)
    );
    assert_eq!(v["record"]["root"], root.to_hex());
    assert_eq!(v["record"]["latest"], true);
    assert_eq!(v["chainRead"]["how"], "rootOf");
    assert_eq!(v["chainRead"]["rootOf"], root.to_hex());
    assert_eq!(
        (v["verified"].as_u64(), v["unknown"].as_u64()),
        (Some(1), Some(2))
    );
    let r = &v["results"];
    assert_eq!(r[0]["status"], "verified", "{v}");
    assert_eq!(r[0]["root"], root.to_hex());
    assert_eq!(r[0]["event"]["kind"], "SETTLEMENT");
    assert_eq!(r[0]["event"]["payee"], PAYEE);
    assert_eq!(r[0]["event"]["note"], "Paid the hosting bill.");
    assert_eq!(
        (
            r[1]["status"].as_str(),
            r[1]["code"].as_str(),
            r[1]["by"].as_str()
        ),
        (Some("unknown"), Some("recall_unknown"), Some("record"))
    );
    assert_eq!(
        (r[2]["status"].as_str(), r[2]["by"].as_str()),
        (Some("unknown"), Some("input"))
    );
    // An outsider on this locked account: the proof, not the event.
    let (st, v) = get(srv.port, &q, "");
    assert_eq!(st, 200);
    assert_eq!(v["results"][0]["status"], "verified");
    assert!(v["results"][0]["event"].is_null(), "{v}");
    assert!(v["results"][0]["leaf"].is_string() && v["results"][0]["path"].is_array());

    // Forged on disk: the payment's amount edited in its canonical bytes.
    let file = dir
        .join("tenants")
        .join(POLICY)
        .join("log")
        .join(format!("{}.events.jsonl", root.to_hex()));
    let text = std::fs::read_to_string(&file).unwrap();
    let forged = text.replacen(
        &format!("{:032x}", 50_000u128),
        &format!("{:032x}", 5_000_000u128),
        1,
    );
    assert_ne!(forged, text);
    std::fs::write(&file, forged).unwrap();
    let (st, v) = get(
        srv.port,
        &format!("/api/level1/recall?policy={POLICY}&ids=evt:1"),
        reader,
    );
    assert_eq!(st, 200);
    assert_eq!(
        (
            v["results"][0]["status"].as_str(),
            v["results"][0]["by"].as_str()
        ),
        (Some("unverified"), Some("leaf")),
        "{v}"
    );
    assert!(
        v["results"][0]["event"].is_null(),
        "a forged event's fields are never served"
    );

    // The refusals: no ids, too many, a bad root, no such account, no such record.
    for (path, want) in [
        (format!("/api/level1/recall?policy={POLICY}"), 400),
        (
            format!(
                "/api/level1/recall?policy={POLICY}&ids={}",
                vec!["evt:0"; 33].join(",")
            ),
            400,
        ),
        (
            format!("/api/level1/recall?policy={POLICY}&ids=evt:0&root=0x12"),
            400,
        ),
        (
            "/api/level1/recall?policy=0x00000000000000000000000000000000000000e2&ids=evt:0"
                .to_string(),
            404,
        ),
        (
            format!(
                "/api/level1/recall?policy={POLICY}&ids=evt:0&root=0x{}",
                "11".repeat(32)
            ),
            404,
        ),
    ] {
        let (st, v) = get(srv.port, &path, reader);
        assert_eq!(st, want, "{path}: {v}");
        assert!(v["error"]["code"].is_string(), "{path}: the envelope: {v}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_a_node_nothing_is_verified() {
    let (dir, _) = account("rpc");
    let srv = start(1, &dir, "http://127.0.0.1:9");
    let (st, v) = get(
        srv.port,
        &format!("/api/level1/recall?policy={POLICY}&ids=evt:0,evt:1"),
        "Authorization: Bearer k-read\r\n",
    );
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["verified"], 0);
    for r in v["results"].as_array().unwrap() {
        assert_eq!(
            (r["status"].as_str(), r["by"].as_str(), r["code"].as_str()),
            (Some("unknown"), Some("rpc"), Some("chain_unavailable")),
            "{r}"
        );
        assert!(r["event"].is_null());
    }
    let _ = std::fs::remove_dir_all(&dir);
}
