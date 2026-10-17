//! The organiser's three sentences, as three tests. The hackathon brief
//! (docs/04): "A ledger checks that debits equal credits. It does not check
//! that the vendor was the right one, that the invoice was real, or that a
//! retry didn't pay it twice."
//!
//! Each test runs the real `tameion-app` against anvil carrying Arc's chain
//! id (so the service's Arc testnet preset points at it) and the policy bytes
//! the service deploys for every account (`contracts/bytecode/`). Each reads
//! the chain and the account's files before the service's reply, so a test
//! whose mechanism is removed fails on what happened to the money. Without
//! `anvil` on PATH each test says why and does nothing.

use alloy::primitives::{Address, B256, U256, keccak256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
use arc::{ArcClient, PrincipalSettlement};
use operator::invoice::{self, Invoice};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

/// The policy's creation code, the bytes `tameion-app` deploys (`level1.rs`).
const POLICY_CODE: &str = include_str!("../../../contracts/bytecode/BudgetPolicy.hex");
/// Arc testnet's chain id: the service's `arc-testnet` preset, on anvil.
const ARC_TESTNET: u64 = 5_042_002;
/// The ERC-8004 registry the policy and the anchor are built with.
const REGISTRY: &str = "0x8004A818BFB912233c491871b3d84c89A494BD9e";
/// Native wei per USDC minor unit on Arc (`BudgetPolicyNative`).
const WEI_PER_UNIT: u128 = 1_000_000_000_000;
const MEMORY_ANCHOR: &str = "lib/arc-agent-kit/contracts/src/MemoryAnchor.sol";

struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("a free port")
}

fn wait_listening(port: u16, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "{what} did not listen on {port}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn key_hex(k: &PrivateKeySigner) -> String {
    alloy::hex::encode_prefixed(k.to_bytes())
}

/// One account on its own anvil: the owner, the agent key the service holds,
/// the relayer (the policy's operator), one payee the owner approved and that
/// has cooled off, 10 USDC in the policy, Autopilot, and an agent key (the
/// bearer) with read, run and buy.
struct World {
    test: &'static str,
    rt: tokio::runtime::Runtime,
    url: String,
    owner: PrivateKeySigner,
    agent: PrivateKeySigner,
    relayer: PrivateKeySigner,
    payee: PrivateKeySigner,
    policy: Address,
    anchor: Option<Address>,
    bearer: String,
    dirs: Vec<PathBuf>,
    // Declared before the chain, so the services stop first.
    apps: Vec<Proc>,
    _anvil: Proc,
}

impl Drop for World {
    fn drop(&mut self) {
        self.apps.clear();
        // Kept after a failure: the service's log and files are the evidence.
        if !std::thread::panicking() {
            for d in &self.dirs {
                let _ = std::fs::remove_dir_all(d);
            }
        }
    }
}

impl World {
    /// `None`, with the reason printed, when anvil (or, for the anchor,
    /// forge and the anchor's source) is not there.
    fn new(test: &'static str, with_anchor: bool) -> Option<World> {
        let port = free_port();
        let anvil = match Command::new("anvil")
            .args([
                "--port",
                &port.to_string(),
                "--chain-id",
                &ARC_TESTNET.to_string(),
            ])
            .arg("--silent")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => Proc(c),
            Err(e) => {
                eprintln!("organiser::{test} skipped: anvil is not on PATH ({e}); install foundry");
                return None;
            }
        };
        wait_listening(port, "anvil");
        let url = format!("http://127.0.0.1:{port}");
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let (owner, agent, relayer, payee) = (
            PrivateKeySigner::random(),
            PrivateKeySigner::random(),
            PrivateKeySigner::random(),
            PrivateKeySigner::random(),
        );
        let raw = ProviderBuilder::new().connect_http(url.parse().expect("url"));
        let rpc = |method: &'static str, params: Value| -> Value {
            rt.block_on(raw.raw_request(method.into(), params))
                .unwrap_or_else(|e| panic!("{method}: {e}"))
        };
        for who in [owner.address(), agent.address(), relayer.address()] {
            rpc(
                "anvil_setBalance",
                json!([who, format!("{:#x}", U256::from(10u128.pow(21)))]),
            );
        }
        let anchor = if with_anchor {
            let from = rpc("eth_accounts", json!([]))[0]
                .as_str()
                .unwrap_or_default()
                .to_string();
            Some(deploy_anchor(test, &url, &from)?)
        } else {
            None
        };
        let chain = ArcClient::with_signer(&url, &key_hex(&owner)).expect("owner client");
        let args = (
            owner.address(),
            agent.address(),
            relayer.address(),
            Address::from_str(REGISTRY).expect("registry"),
            U256::from(42u64),
            U256::from(1_000_000u64), // 1 USDC a payment
            U256::from(5_000_000u64), // 5 USDC a day
            U256::ZERO,               // no floor
            U256::from(60u64),        // the delay, and a new payee's cooling-off
            U256::from(2u64),         // Autopilot (a uint8, one ABI word like the rest)
        )
            .abi_encode_params();
        let code = alloy::hex::decode(POLICY_CODE.trim()).expect("policy code");
        let policy = rt.block_on(chain.deploy(&code, &args)).expect("deploy");
        rt.block_on(chain.send_native(policy, U256::from(10_000_000 * WEI_PER_UNIT)))
            .expect("fund 10 USDC");
        let approve = [
            &keccak256("setPayee(address,bool)")[..4],
            &(payee.address(), true).abi_encode_params(),
        ]
        .concat();
        rt.block_on(chain.send_call(policy, approve))
            .expect("approve the payee");
        // The payee's cooling-off passes on the chain's clock.
        rpc("evm_increaseTime", json!([61]));
        rpc("evm_mine", json!([]));
        let bearer = format!("organiser-{}", alloy::hex::encode(B256::random()));
        drop(raw);
        Some(World {
            test,
            rt,
            url,
            owner,
            agent,
            relayer,
            payee,
            policy,
            anchor,
            bearer,
            dirs: Vec::new(),
            apps: Vec::new(),
            _anvil: anvil,
        })
    }

    /// Start a copy of the service on its own disk with this account's record
    /// and agent key, as the service writes them; returns its port.
    fn app(&mut self) -> u16 {
        let port = free_port();
        let dir = std::env::temp_dir().join(format!(
            "tameion-organiser-{}-{}-{port}",
            self.test,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let owner = format!("{:#x}", self.owner.address());
        let policy = format!("{:#x}", self.policy);
        let tenants = dir.join("tenants");
        std::fs::create_dir_all(tenants.join(&owner)).expect("tenant dir");
        std::fs::write(tenants.join(&owner).join("agent.key"), key_hex(&self.agent))
            .expect("agent key");
        let record = json!({
            "chain": "arc-testnet", "owner": owner, "policy": policy,
            "agent": format!("{:#x}", self.agent.address()),
            "operator": format!("{:#x}", self.relayer.address()), "watched": policy,
            "recovery": "0x0000000000000000000000000000000000000000",
            "per_action": "1", "daily": "5", "floor": "0", "delay_secs": 60, "mode": 2,
            "created_block": 0, "history_from_block": 0, "created": 0, "deployed": true,
            "agent_tokens": [{
                "id": "t1", "label": "organiser", "scopes": ["read", "run", "buy"],
                "expires": 4_102_444_800u64, "issued": 0,
                "hash": operator::level1::token_hash(&self.bearer),
            }],
        });
        std::fs::write(tenants.join(format!("{policy}.json")), record.to_string())
            .expect("tenant record");
        let log = std::fs::File::create(dir.join("app.log")).expect("log");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tameion-app"));
        cmd.current_dir(&dir)
            .env("PORT", port.to_string())
            .env("TAMEION_APP_DIR", &dir)
            .env("TAMEION_RPC_ARC", &self.url)
            .env("TAMEION_RELAYER_KEY", key_hex(&self.relayer))
            .env("TAMEION_REASONER", "rule")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("TAMEION_LEVEL1_INVITE")
            .env_remove("TAMEION_LEVEL1_EVERY_SECS")
            .env_remove("TAMEION_LEVEL1_WATCH_SECS")
            .env_remove("TAMEION_ANCHOR")
            .stdout(log.try_clone().expect("log"))
            .stderr(log);
        if let Some(a) = self.anchor {
            cmd.env("TAMEION_ANCHOR", format!("{a:#x}"));
        }
        self.apps
            .push(Proc(cmd.spawn().expect("spawn tameion-app")));
        self.dirs.push(dir);
        wait_listening(port, "tameion-app");
        port
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.bearer)
    }

    fn pay_body(&self, payee: Address, amount: &str, why: &str) -> String {
        json!({ "policy": format!("{:#x}", self.policy), "payee": format!("{payee:#x}"), "amount": amount, "why": why })
            .to_string()
    }

    /// `POST /api/level1/pay` as the agent, under operation id `op`.
    fn pay(&self, port: u16, payee: Address, amount: &str, op: &str) -> (u16, String, Value) {
        let auth = self.auth();
        call(
            port,
            "POST",
            "/api/level1/pay",
            &[("Authorization", &auth), ("Idempotency-Key", op)],
            &self.pay_body(payee, amount, "hosting, October"),
        )
    }

    /// A pass, as the agent asks for one; its report once it is done.
    fn pass(&self, port: u16) -> Value {
        let auth = self.auth();
        let body = json!({ "policy": format!("{:#x}", self.policy) }).to_string();
        let (st, _, v) = call(
            port,
            "POST",
            "/api/level1/run",
            &[("Authorization", &auth)],
            &body,
        );
        assert!(st == 200 || st == 202, "run: {st} {v}");
        let id = v["id"].as_str().expect("job id").to_string();
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let (_, _, j) = call(
                port,
                "GET",
                &format!("/api/watch/{id}"),
                &[("Authorization", &auth)],
                "",
            );
            match j["status"].as_str() {
                Some("done") => return j["report"].clone(),
                Some("failed") => panic!("the pass failed: {j}"),
                _ => assert!(Instant::now() < deadline, "the pass did not finish: {j}"),
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn reader(&self) -> ArcClient {
        ArcClient::read_only(&self.url).expect("reader")
    }

    /// Every `Paid` the policy emitted.
    fn paid(&self) -> Vec<PrincipalSettlement> {
        let c = self.reader();
        let head = self.rt.block_on(c.block_number()).expect("head");
        self.rt
            .block_on(c.paid_events(self.policy, 0, head))
            .expect("Paid events")
    }

    fn wei(&self, who: Address) -> U256 {
        self.rt
            .block_on(self.reader().native_wei(who))
            .expect("balance")
    }

    fn executed(&self, hash: B256) -> bool {
        self.rt
            .block_on(self.reader().executed(self.policy, hash))
            .expect("executed")
    }
}

/// The anchor the pass writes its root to, compiled from its source by forge.
fn deploy_anchor(test: &str, url: &str, from: &str) -> Option<Address> {
    let contracts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts");
    if !contracts.join(MEMORY_ANCHOR).exists() {
        eprintln!(
            "organiser::{test} skipped: {MEMORY_ANCHOR} is not checked out (git submodule update --init)"
        );
        return None;
    }
    let build =
        std::env::temp_dir().join(format!("tameion-organiser-forge-{}", std::process::id()));
    let out = Command::new("forge")
        .current_dir(&contracts)
        .args([
            "create",
            &format!("{MEMORY_ANCHOR}:MemoryAnchor"),
            "--rpc-url",
            url,
        ])
        .args(["--unlocked", "--from", from, "--broadcast"])
        .arg("--out")
        .arg(build.join("out"))
        .arg("--cache-path")
        .arg(build.join("cache"))
        .args(["--constructor-args", REGISTRY])
        .output();
    let _ = std::fs::remove_dir_all(&build);
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            eprintln!("organiser::{test} skipped: forge is not on PATH ({e}); install foundry");
            return None;
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let at = text
        .lines()
        .find_map(|l| l.strip_prefix("Deployed to: "))
        .and_then(|a| Address::from_str(a.trim()).ok());
    Some(at.unwrap_or_else(|| {
        panic!(
            "forge create MemoryAnchor: {text}{}",
            String::from_utf8_lossy(&out.stderr)
        )
    }))
}

fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> String {
    let extra: String = headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}\r\n"))
        .collect();
    format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// One request: the status, the head, and the body as JSON (`Null` if it is not).
fn call(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, Value) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.write_all(request(method, path, headers, body).as_bytes())
        .expect("send");
    let mut out = String::new();
    s.read_to_string(&mut out).expect("reply");
    let status = out
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    (
        status,
        head.to_string(),
        serde_json::from_str(body).unwrap_or(Value::Null),
    )
}

/// The whole request sent, then the connection closed before any reply: a
/// client that timed out. The service has the request; the client has nothing.
fn abandon(port: u16, path: &str, headers: &[(&str, &str)], body: &str) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.write_all(request("POST", path, headers, body).as_bytes())
        .expect("send");
    s.shutdown(Shutdown::Both).expect("hang up");
}

fn signed(key: &PrivateKeySigner, inv: &Invoice) -> String {
    let sig = key
        .sign_hash_sync(&invoice::digest(ARC_TESTNET, inv))
        .expect("sign");
    invoice::to_json(inv, &sig.as_bytes()).to_string()
}

/// "…that the vendor was the right one." The owner approved one payee; the
/// agent asks to pay another. The contract refuses it before anything is
/// sent: no `Paid` to it, its balance untouched, the operation not executed,
/// and the reply is `payee_not_approved` with `executed: "no"`. The approved
/// payee is paid by the same agent key first, so the refusal is the payee's.
#[test]
fn wrong_vendor() {
    let Some(mut w) = World::new("wrong_vendor", false) else {
        return;
    };
    let port = w.app();
    let stranger = PrivateKeySigner::random().address();
    let (right_op, wrong_op) = ("op_organiser-right-vendor", "op_organiser-wrong-vendor");
    let right = w.pay(port, w.payee.address(), "0.10", right_op);
    let wrong = w.pay(port, stranger, "0.10", wrong_op);

    let paid = w.paid();
    assert!(
        paid.iter().all(|p| p.to != stranger),
        "the policy paid a vendor nobody approved: {paid:?}"
    );
    assert_eq!(
        w.wei(stranger),
        U256::ZERO,
        "the unapproved vendor holds money"
    );
    assert!(
        !w.executed(operator::pay::action_hash(w.policy, wrong_op)),
        "the refused operation is marked executed on chain"
    );
    assert_eq!(
        paid.len(),
        1,
        "one payment, to the approved payee: {paid:?}"
    );
    assert_eq!(paid[0].to, w.payee.address());

    assert_eq!(right.0, 200, "{}", right.2);
    assert_eq!(right.2["paid"], true, "{}", right.2);
    assert_eq!(wrong.0, 409, "{}", wrong.2);
    assert_eq!(
        wrong.2["error"]["code"], "payee_not_approved",
        "{}",
        wrong.2
    );
    assert_eq!(wrong.2["error"]["executed"], "no", "{}", wrong.2);
    println!(
        "wrong_vendor: approved payee paid 0.10 USDC, tx {:#x}; unapproved {stranger:#x} refused {} (executed: {}); Paid events on policy {:#x}: {}; unapproved balance {} wei",
        paid[0].tx,
        wrong.2["error"]["code"],
        wrong.2["error"]["executed"],
        w.policy,
        paid.len(),
        w.wei(stranger)
    );
}

/// "…that the invoice was real." An invoice is the payee signing it with the
/// key the money goes to. A body naming the approved payee but signed by
/// another key gets 422 with the reason, is not among the account's invoices
/// and is not in the record the next pass writes and anchors; the payee's own
/// invoice, sent beside it, is in both.
#[test]
fn phantom_invoice() {
    let Some(mut w) = World::new("phantom_invoice", true) else {
        return;
    };
    let port = w.app();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let real = Invoice {
        payee: w.payee.address(),
        policy: w.policy,
        amount: 250_000,
        due_at: now + 7 * 86_400,
        every_days: 0,
        discount_bps: 0,
        early_days: 0,
        reference: keccak256("INV-2026-10-001"),
        issued_at: now,
        issuer: None,
    };
    let phantom = Invoice {
        reference: keccak256("INV-2026-10-002"),
        ..real.clone()
    };
    let forger = PrivateKeySigner::random();
    let path = "/api/level1/invoice";
    let sent_real = call(port, "POST", path, &[], &signed(&w.payee, &real));
    let sent_phantom = call(port, "POST", path, &[], &signed(&forger, &phantom));
    let bare = |h: B256| alloy::hex::encode(h);
    let real_id = bare(invoice::id(real.payee, real.reference));
    let phantom_marks = [
        bare(invoice::id(phantom.payee, phantom.reference)),
        bare(phantom.reference),
    ];
    let tenants = w.dirs[0].join("tenants");
    let policy = format!("{:#x}", w.policy);

    let invoices = std::fs::read_to_string(tenants.join(format!("{policy}.invoices.jsonl")))
        .unwrap_or_default();
    assert!(
        phantom_marks.iter().all(|m| !invoices.contains(m.as_str())),
        "the phantom invoice was written down: {invoices}"
    );
    assert!(
        invoices.contains(&real_id),
        "the payee's own invoice is missing: {invoices}"
    );
    let report = w.pass(port);
    let root = report["root"].as_str().expect("root").to_string();
    let record = tenants
        .join(&policy)
        .join("log")
        .join(format!("{root}.events.jsonl"));
    let events = std::fs::read_to_string(&record)
        .unwrap_or_else(|e| panic!("{}: {e}; report {report}", record.display()));
    assert!(
        phantom_marks.iter().all(|m| !events.contains(m.as_str())),
        "the phantom invoice is in the record: {events}"
    );
    assert!(
        events.contains(&real_id),
        "the payee's invoice is not in the record: {events}"
    );
    let anchor = w.anchor.expect("anchor");
    let on_chain =
        w.rt.block_on(w.reader().anchored_root(anchor, w.agent.address()))
            .expect("anchored root");
    assert_eq!(
        format!("{on_chain:#x}"),
        root,
        "the chain holds another root: {report}"
    );

    assert_eq!(sent_real.0, 201, "{}", sent_real.2);
    assert_eq!(sent_phantom.0, 422, "{}", sent_phantom.2);
    assert_eq!(
        sent_phantom.2["error"]["executed"], "no",
        "{}",
        sent_phantom.2
    );
    let reason = sent_phantom.2["error"]["words"]
        .as_str()
        .unwrap_or_default();
    assert!(
        reason.contains("not signed by the payee"),
        "{}",
        sent_phantom.2
    );
    println!(
        "phantom_invoice: real invoice 201; phantom 422 \"{reason}\"; invoices.jsonl lines {}; pass root {root} anchored by tx {} at {anchor:#x}; record lines {}, phantom in it: no",
        invoices.lines().count(),
        report["anchorTx"].as_str().unwrap_or("none"),
        events.lines().count()
    );
}

/// "…or that a retry didn't pay it twice." One payment under one operation id
/// (the `Idempotency-Key`), sent four times: the first send's client hangs up
/// before the reply (a timeout); the retry and one more send get the first
/// payment back, marked `Idempotent-Replayed: true`; another copy of the
/// service, which never saw the id, answers from the chain's
/// `executed(actionHash)` with the same transaction. One `Paid` on chain.
#[test]
fn double_pay_on_retry() {
    let Some(mut w) = World::new("double_pay_on_retry", false) else {
        return;
    };
    let port = w.app();
    let op = "op_organiser-retry-0001";
    let body = w.pay_body(w.payee.address(), "0.25", "hosting, October");
    let auth = w.auth();
    let headers = [("Authorization", auth.as_str()), ("Idempotency-Key", op)];
    let path = "/api/level1/pay";
    abandon(port, path, &headers, &body);
    // The retry, while the first send may still be running (409 idempotency_pending).
    std::thread::sleep(Duration::from_millis(300));
    let (mut pending, deadline) = (0, Instant::now() + Duration::from_secs(60));
    let retry = loop {
        let r = call(port, "POST", path, &headers, &body);
        if r.2["error"]["code"] != "idempotency_pending" || Instant::now() > deadline {
            break r;
        }
        pending += 1;
        std::thread::sleep(Duration::from_millis(250));
    };
    let again = call(port, "POST", path, &headers, &body);
    // Another copy of the service: its own disk, no replay store, the same account.
    let other = w.app();
    let elsewhere = call(other, "POST", path, &headers, &body);

    let hash = operator::pay::action_hash(w.policy, op);
    let paid: Vec<_> = w
        .paid()
        .into_iter()
        .filter(|p| p.action_hash == hash)
        .collect();
    assert_eq!(
        paid.len(),
        1,
        "one operation id, {} payments on chain: {paid:?}",
        paid.len()
    );
    assert!(w.executed(hash), "executed(actionHash) is false");
    assert_eq!(
        w.wei(w.payee.address()),
        U256::from(250_000 * WEI_PER_UNIT),
        "the payee was not paid exactly once"
    );
    let tx = format!("{:#x}", paid[0].tx);

    for (name, (st, head, v)) in [("retry", &retry), ("again", &again)] {
        assert_eq!(*st, 200, "{name}: {v}");
        assert!(head.contains("Idempotent-Replayed: true"), "{name}: {head}");
        assert_eq!(v["payment"]["tx"], tx.as_str(), "{name}: {v}");
    }
    assert_eq!(again.2, retry.2, "the same answer");
    assert_eq!(elsewhere.0, 200, "{}", elsewhere.2);
    assert_eq!(elsewhere.2["replayed"], true, "{}", elsewhere.2);
    assert_eq!(elsewhere.2["payment"]["tx"], tx.as_str(), "{}", elsewhere.2);
    println!(
        "double_pay_on_retry: operation {op}, action hash {hash:#x}: first send abandoned; retry 200 replayed after {pending} x 409 idempotency_pending; again 200 replayed; second service 200 replayed from executed(); Paid events for it: {}; tx {tx}; executed: {}",
        paid.len(),
        w.executed(hash)
    );
}
