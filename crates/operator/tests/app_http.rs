//! The hosted service's routes, through the real binary on a random port:
//! what answers without a chain (health, chains, OpenAPI, Level 1 config),
//! what refuses (bad bodies, missing admin token, unknown runs), and that
//! the page and the wizard are served. No RPC is called.

use std::io::Read;
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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

fn start() -> Server {
    start_with(0, &[("TAMEION_LEVEL1_INVITE", "sesame")])
}

/// Boot the binary on its own port with extra environment; the invite word
/// is unset unless `extra` sets it.
fn start_with(offset: u16, extra: &[(&str, &str)]) -> Server {
    let port = 17_000 + (std::process::id() % 9_000) as u16 + offset;
    let dir =
        std::env::temp_dir().join(format!("tameion-app-test-{}-{offset}", std::process::id()));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tameion-app"));
    cmd.env("PORT", port.to_string())
        .env("TAMEION_APP_DIR", &dir)
        .env("TAMEION_APP_ADMIN_TOKEN", "t0k3n")
        .env_remove("TAMEION_LEVEL1_INVITE")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in extra {
        cmd.env(k, v);
    }
    let child = cmd.spawn().expect("spawn tameion-app");
    // Owned before the wait so a timeout panic still kills and reaps it.
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

fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    use std::io::Write;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status: u16 = out
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let body = out.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

fn http_with(
    port: u16,
    method: &str,
    path: &str,
    body: &str,
    extra: &str,
) -> (u16, String, String) {
    use std::io::Write;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status: u16 = out
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    (status, head.to_string(), body.to_string())
}

/// PLAN 5.42 (docs/62 F4): a retried POST with the same Idempotency-Key gets
/// the first answer back, marked as a replay; the same key with a different
/// body is refused; no key behaves as before.
#[test]
fn an_idempotency_key_replays_the_first_answer_and_refuses_a_different_body() {
    let srv = start_with(3, &[]);
    let p = srv.port;
    let body = r#"{"policy":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","amount":"1"}"#;
    let key = "Idempotency-Key: test-key-1\r\n";
    let (st1, h1, b1) = http_with(p, "POST", "/api/level1/prepare-withdraw", body, key);
    assert_eq!(st1, 200, "{b1}");
    assert!(!h1.contains("Idempotent-Replayed"));
    let (st2, h2, b2) = http_with(p, "POST", "/api/level1/prepare-withdraw", body, key);
    assert_eq!(st2, 200);
    assert_eq!(b2, b1, "the same answer");
    assert!(h2.contains("Idempotent-Replayed: true"), "{h2}");
    let other = body.replace("\"1\"", "\"2\"");
    let (st3, _, b3) = http_with(p, "POST", "/api/level1/prepare-withdraw", &other, key);
    assert_eq!(st3, 422, "{b3}");
    let (st4, h4, _) = http_with(p, "POST", "/api/level1/prepare-withdraw", &other, "");
    assert_eq!(st4, 200);
    assert!(!h4.contains("Idempotent-Replayed"));
    // The account page installs on a phone.
    let (st6, h6, b6) = http_with(p, "GET", "/manifest.webmanifest", "", "");
    assert_eq!(st6, 200);
    assert!(h6.contains("application/manifest+json") && b6.contains("\"start_url\": \"/\""));
    // The agent-readable map says how to retry safely and what no agent can do.
    let (st5, _, b5) = http_with(p, "GET", "/llms.txt", "", "");
    assert_eq!(st5, 200);
    assert!(b5.contains("Idempotency-Key") && b5.contains("What no agent can do here"));
}

/// PLAN 5.45: a partner's key lists the accounts it brought; nothing without it.
#[test]
fn a_partner_key_lists_its_accounts_and_nothing_works_without_it() {
    let srv = start_with(4, &[]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-4", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // sha256("partner-test-key")
    use sha2::{Digest, Sha256};
    let h: String = Sha256::digest(b"partner-test-key")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(
        dir.join("partners.json"),
        format!(r#"[{{"id":"ledger-co","name":"Ledger & Co","token_hash":"{h}","created":0}}]"#),
    )
    .unwrap();
    let (st, _, b) = http_with(p, "GET", "/api/partner/public?id=ledger-co", "", "");
    assert_eq!(st, 200, "{b}");
    assert!(b.contains("Ledger & Co"));
    let (st, _, _) = http_with(p, "GET", "/api/partner/accounts", "", "");
    assert_eq!(st, 401);
    let (st, _, _) = http_with(
        p,
        "GET",
        "/api/partner/accounts",
        "",
        "Authorization: Bearer wrong\r\n",
    );
    assert_eq!(st, 401);
    let (st, _, b) = http_with(
        p,
        "GET",
        "/api/partner/accounts",
        "",
        "Authorization: Bearer partner-test-key\r\n",
    );
    assert_eq!(st, 200, "{b}");
    assert!(
        b.contains("\"accounts\":[]") && b.contains("\"live\":0"),
        "{b}"
    );
}

#[test]
fn the_service_answers_what_needs_no_chain_and_refuses_what_it_should() {
    let srv = start();
    let p = srv.port;

    let (st, b) = http(p, "GET", "/healthz", "");
    assert_eq!((st, b.as_str()), (200, "ok"));

    let (st, b) = http(p, "GET", "/api/chains", "");
    assert_eq!(st, 200);
    let v: serde_json::Value = serde_json::from_str(&b).unwrap();
    let keys: Vec<&str> = v["chains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["key"].as_str().unwrap())
        .collect();
    assert!(
        keys.contains(&"arc-testnet")
            && keys.contains(&"arbitrum")
            && keys.contains(&"base")
            && keys.contains(&"ethereum")
    );
    for c in v["chains"].as_array().unwrap() {
        assert!(c["note"].as_str().unwrap().len() > 3);
    }

    let (st, b) = http(p, "GET", "/api/openapi.json", "");
    assert_eq!(st, 200);
    let o: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert!(o["paths"]["/api/watch"]["post"].is_object());

    let (st, b) = http(p, "GET", "/", "");
    assert_eq!(st, 200);
    assert!(
        b.contains("Needs you") && b.contains("Protected"),
        "the account page is the front door"
    );
    let visible = b.split("<script").next().unwrap_or("");
    assert!(
        !visible.contains("calldata") && !visible.contains("nonce") && !visible.contains("relayer"),
        "no mechanism words on the front door"
    );
    // PLAN 5.37 (docs/62): the trust line, in the market's words, on the
    // front door itself — where the key is NOT, before what the agent does.
    assert!(
        visible.contains("cannot add a payee, raise its own limit, or withdraw")
            && visible.contains("Tameion holds no key that can take your money"),
        "the trust line is on the front door"
    );
    // PLAN 5.41: the price model where the buyer decides; no placeholder left.
    assert!(
        visible.contains("What it costs.")
            && visible.contains("nothing per payment")
            && !b.contains("{{"),
        "the price is on the front door, and no placeholder is left"
    );
    // docs/61: the illustrations are inlined, so the page's tokens theme them.
    assert!(
        b.matches("<svg ").count() >= 3 && !b.contains("{{HERO_SVG}}"),
        "the three illustrations are inlined"
    );
    let (st, b) = http(p, "GET", "/watch", "");
    assert_eq!(st, 200);
    assert!(b.contains("Tameion Watch"));
    assert!(
        b.contains("try the demo treasury"),
        "a visitor with no address has somewhere to click"
    );
    assert!(
        b.contains("id=\"toLevel1\""),
        "a report hands off to Level 1 with its caps and payees"
    );
    let (st, b) = http(
        p,
        "GET",
        "/api/level1/inbox?policy=0x0000000000000000000000000000000000000001",
        "",
    );
    assert_eq!(st, 404, "{b}");
    let (st, b) = http(p, "GET", "/level1", "");
    assert_eq!(st, 200);
    assert!(b.contains("Create passkey"));
    assert!(
        b.contains("Ask for Autopilot (passkey)"),
        "Level 2 is on the page"
    );
    assert!(
        b.contains("src=\"/level1/call.js\""),
        "the page decodes calldata with its own routine"
    );
    let (st, js) = http(p, "GET", "/level1/call.js", "");
    assert_eq!(st, 200);
    assert!(
        js.contains("function challengeFor") && js.contains("setMode(uint8)"),
        "the clear-signing routine, with setMode known"
    );
    assert!(
        b.contains("/api/level1/prepare-mode"),
        "and it signs with the passkey, not the operator"
    );

    let (st, b) = http(p, "GET", "/api/level1/config", "");
    assert_eq!(st, 200);
    let c: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(c["inviteNeeded"], true);
    assert!(c["delaySecs"].as_u64().unwrap() >= 1);

    // Refusals, each by name.
    let (st, b) = http(p, "POST", "/api/watch", "{not json");
    assert_eq!(st, 400, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/watch",
        r#"{"chain":"nope","address":"0x1"}"#,
    );
    assert_eq!(st, 400);
    assert!(b.contains("chain must be"));
    let (st, b) = http(
        p,
        "POST",
        "/api/watch",
        r#"{"chain":"arc-testnet","address":"0x1"}"#,
    );
    assert_eq!(st, 400);
    assert!(b.contains("address must be"));
    let (st, _) = http(p, "GET", "/api/watch/does-not-exist", "");
    assert_eq!(st, 404);
    // docs/60 P-253: the admin token in the Authorization header, never the URL.
    let (st, _) = http(p, "GET", "/api/runs", "");
    assert_eq!(st, 401);
    let (st, _, _) = http_with(p, "GET", "/api/runs", "", "Authorization: Bearer wrong\r\n");
    assert_eq!(st, 403);
    let (st, _) = http(p, "GET", "/api/runs?token=t0k3n", "");
    assert_eq!(st, 400, "the URL form is refused");
    let (st, _, b) = http_with(p, "GET", "/api/runs", "", "Authorization: Bearer t0k3n\r\n");
    assert_eq!(st, 200);
    assert!(b.contains("\"runs\""));
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/deploy",
        r#"{"chain":"arc-testnet","qx":"0x01","qy":"0x02"}"#,
    );
    assert_eq!(st, 403, "no invite: {b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/deploy",
        r#"{"chain":"arc-testnet","invite":"sesame","qx":"0x01"}"#,
    );
    assert_eq!(st, 400, "{b}");
    assert!(b.contains("qx and qy"));
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/run",
        r#"{"policy":"0x0000000000000000000000000000000000000001"}"#,
    );
    assert_eq!(st, 404, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-payees",
        r#"{"policy":"0x0000000000000000000000000000000000000001","payees":["0x0000000000000000000000000000000000000002"]}"#,
    );
    assert_eq!(st, 200, "{b}");
    let pp: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert!(
        pp["data"].as_str().unwrap().starts_with("0x48a56503"),
        "setPayees selector"
    );
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/mode",
        r#"{"policy":"0x0000000000000000000000000000000000000001","action":"autopilot"}"#,
    );
    assert_eq!(st, 404, "{b}");
    let (st, b) = http(p, "POST", "/api/level1/mode", r#"{"action":"autopilot"}"#);
    assert_eq!(st, 400, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-cancel",
        r#"{"policy":"0x0000000000000000000000000000000000000001"}"#,
    );
    assert_eq!(st, 200, "{b}");
    let pc: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(
        pc["data"].as_str().unwrap(),
        "0xdedad97d",
        "cancelPending() selector, no args"
    );
    assert!(o["paths"]["/api/level1/mode"]["post"].is_object());
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-mode",
        r#"{"policy":"0x0000000000000000000000000000000000000001","mode":2}"#,
    );
    assert_eq!(st, 200, "{b}");
    let pm: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(
        pm["data"].as_str().unwrap(),
        "0x21175b4a0000000000000000000000000000000000000000000000000000000000000002",
        "setMode(uint8) with 2"
    );
    assert!(pm["summary"].as_str().unwrap().contains("Autopilot"));
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-mode",
        r#"{"policy":"0x0000000000000000000000000000000000000001","mode":3}"#,
    );
    assert_eq!(st, 400, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-withdraw",
        r#"{"policy":"0x0000000000000000000000000000000000000001","to":"0x0000000000000000000000000000000000000002","amount":"0.01"}"#,
    );
    assert_eq!(st, 200, "{b}");
    let pw: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert!(
        pw["data"].as_str().unwrap().starts_with("0xf3fef3a3"),
        "withdraw(address,uint256) selector: {b}"
    );
    assert!(
        pw["data"].as_str().unwrap().ends_with("2710"),
        "0.01 USDC = 10000 units"
    );
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-withdraw",
        r#"{"policy":"0x0000000000000000000000000000000000000001","to":"0x0000000000000000000000000000000000000002","amount":"0"}"#,
    );
    assert_eq!(st, 400, "{b}");
    let (st, b) = http(
        p,
        "GET",
        "/api/level1/record?policy=0x0000000000000000000000000000000000000001&root=0x1111111111111111111111111111111111111111111111111111111111111111",
        "",
    );
    assert_eq!(st, 404, "{b}");
    let (st, b) = http(
        p,
        "GET",
        "/api/level1/record?policy=../../etc&root=passwd",
        "",
    );
    assert_eq!(st, 400, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-limits",
        r#"{"policy":"0x0000000000000000000000000000000000000001","perAction":"0.01","daily":"1","floor":"0"}"#,
    );
    assert_eq!(st, 200, "{b}");
    let pl: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert!(
        pl["data"].as_str().unwrap().starts_with("0x189ae5f2"),
        "setLimits selector: {b}"
    );
    assert!(
        pl["summary"].as_str().unwrap().contains("per payment"),
        "{b}"
    );
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-limits",
        r#"{"policy":"0x0000000000000000000000000000000000000001","perAction":"2","daily":"1","floor":"0"}"#,
    );
    assert_eq!(st, 400, "per-action above daily: {b}");
    // Where the agent reports, by passkey (PLAN 5.3): both routes want a tenant.
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-notify",
        r#"{"policy":"0x0000000000000000000000000000000000000001","url":"https://example.invalid/hook"}"#,
    );
    assert_eq!(st, 404, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/notify-signed",
        r#"{"policy":"0x0000000000000000000000000000000000000001","url":"https://example.invalid/hook","issued":1,"auth":{}}"#,
    );
    assert_eq!(st, 404, "{b}");
    // Funding from Gateway (PLAN 5.15): the routes exist, want a tenant, and
    // never touch Circle or the chain for an unknown one.
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-fund",
        r#"{"policy":"0x0000000000000000000000000000000000000001","amount":"1","sourceDomain":3}"#,
    );
    assert_eq!(st, 404, "{b}");
    let (st, b) = http(p, "POST", "/api/level1/prepare-fund", r#"{"amount":"1"}"#);
    assert_eq!(st, 400, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/fund",
        r#"{"policy":"0x0000000000000000000000000000000000000001","burnIntent":{},"signature":"0x00"}"#,
    );
    assert_eq!(st, 404, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/prepare-recovery",
        r#"{"owner":"0x0000000000000000000000000000000000000009","recovery":"0x0000000000000000000000000000000000000002"}"#,
    );
    assert_eq!(st, 200, "{b}");
    let prc: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(
        prc["to"].as_str().unwrap(),
        "0x0000000000000000000000000000000000000009",
        "the target is the owner contract"
    );
    assert!(
        prc["data"].as_str().unwrap().starts_with("0xf0d85c89"),
        "setRecovery selector: {b}"
    );
    let (st, b) = http(
        p,
        "GET",
        "/api/level1/records?policy=0x0000000000000000000000000000000000000001",
        "",
    );
    assert_eq!(st, 404, "{b}");
    let (st, b) = http(p, "GET", "/api/level1/records?policy=nope", "");
    assert_eq!(st, 400, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/notify",
        r#"{"policy":"0x0000000000000000000000000000000000000001","url":"https://example.invalid/hook"}"#,
    );
    assert_eq!(st, 403, "no token: {b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/notify",
        r#"{"policy":"0x0000000000000000000000000000000000000001","url":"https://example.invalid/hook","token":"t0k3n"}"#,
    );
    assert_eq!(st, 404, "unknown tenant with the token: {b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/bills",
        r#"{"policy":"0x0000000000000000000000000000000000000001","bills":[{"payee":"0x0000000000000000000000000000000000000002","amount":"1","everyDays":30}]}"#,
    );
    assert_eq!(st, 404, "unknown tenant: {b}");
    let (st, b) = http(p, "POST", "/api/level1/bills", r#"{"bills":[]}"#);
    assert_eq!(st, 400, "{b}");
    let (st, _) = http(p, "GET", "/nothing-here", "");
    assert_eq!(st, 404);
}

/// A host bound to a public address without an invite word must not take
/// deploys from anyone: it mints a word (printed at start) and gates on it.
#[test]
fn a_public_host_without_an_invite_word_mints_one_and_gates_deploys() {
    let srv = start_with(100, &[("HOST", "0.0.0.0")]);
    let p = srv.port;
    let (st, b) = http(p, "GET", "/api/level1/config", "");
    assert_eq!(st, 200, "{b}");
    let c: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(c["inviteNeeded"], true, "public host, no word set: {b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/deploy",
        r#"{"chain":"arc-testnet","qx":"0x01","qy":"0x02"}"#,
    );
    assert_eq!(st, 403, "{b}");

    // Loopback without a word stays open, for local use.
    let local = start_with(101, &[]);
    let (st, b) = http(local.port, "GET", "/api/level1/config", "");
    assert_eq!(st, 200, "{b}");
    let c: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(c["inviteNeeded"], false, "loopback, no word: {b}");
}

/// PLAN 5.54: an account with no agent keys is open to read and run, never
/// to spend on services. Buying needs a key with the `buy` scope, always.
/// Receipt v1 (docs/76 §6): the purchase lines are served as NDJSON with the
/// receipt fields, 404 for an account the service does not hold, and behind
/// the `read` scope once the owner minted a key — the inbox's gate.
#[test]
fn purchases_are_served_as_receipt_lines_behind_the_read_gate() {
    let srv = start_with(6, &[]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-6", std::process::id()));
    let policy = "0x00000000000000000000000000000000000000c1";
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    let tenant = |tokens: &str| {
        std::fs::write(
            dir.join("tenants").join(format!("{policy}.json")),
            format!(
                r#"{{"chain":"arc-testnet","owner":"0x0000000000000000000000000000000000000001","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":2,"created_block":0,"history_from_block":0,"created":0,"deployed":true,"agent_tokens":[{tokens}]}}"#
            ),
        )
        .unwrap();
    };
    tenant("");
    std::fs::write(
        dir.join("tenants").join(format!("{policy}.purchases.jsonl")),
        concat!(
            r#"{"at":1,"seller":"0x00000000000000000000000000000000005e11e2","amount":"0.001000 USDC","minor":"1000","network":"eip155:5042002","url":"http://s/v1/mid","status":"paid","tx":"0xa1","id":"0xa1","idFrom":"transaction","sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","reason":"price"}"#,
            "\nnot json\n",
            r#"{"at":2,"seller":"0x00000000000000000000000000000000005e11e2","amount":"0.001000 USDC","minor":"1000","url":"http://s/v1/mid","status":"refused","why":"over cap","reason":"price"}"#,
            "\n"
        ),
    )
    .unwrap();
    let (st, head, body) = http_with(
        p,
        "GET",
        &format!("/api/level1/purchases.jsonl?policy={policy}"),
        "",
        "",
    );
    assert_eq!(st, 200, "{body}");
    assert!(
        head.to_lowercase().contains("application/x-ndjson"),
        "{head}"
    );
    let lines: Vec<serde_json::Value> = body
        .lines()
        .map(|l| serde_json::from_str(l).expect(l))
        .collect();
    assert_eq!(lines.len(), 2, "the unparsable line is dropped: {body}");
    assert_eq!(lines[0]["id"], "0xa1");
    assert_eq!(lines[0]["idFrom"], "transaction");
    assert_eq!(lines[0]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(lines[1]["status"], "refused");

    let (st, _, _) = http_with(
        p,
        "GET",
        "/api/level1/purchases.jsonl?policy=0x00000000000000000000000000000000000000c2",
        "",
        "",
    );
    assert_eq!(st, 404);

    // Once the owner minted an agent key, the route needs one with `read`.
    let hash = operator::level1::token_hash("k-read");
    tenant(&format!(
        r#"{{"id":"t1","label":"t","scopes":["read"],"expires":4102444800,"issued":0,"hash":"{hash}"}}"#
    ));
    let path = format!("/api/level1/purchases.jsonl?policy={policy}");
    let (st, _, body) = http_with(p, "GET", &path, "", "");
    assert_eq!(st, 401, "{body}");
    assert!(body.contains(r#""scope":"read""#), "{body}");
    let (st, _, body) = http_with(p, "GET", &path, "", "Authorization: Bearer k-read\r\n");
    assert_eq!(st, 200, "{body}");
    assert_eq!(body.lines().count(), 2);
}

#[test]
fn buying_a_service_is_closed_without_a_buy_key() {
    let srv = start_with(5, &[]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-5", std::process::id()));
    let policy = "0x00000000000000000000000000000000000000b1";
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    std::fs::write(
        dir.join("tenants").join(format!("{policy}.json")),
        format!(
            r#"{{"chain":"arc-testnet","owner":"0x0000000000000000000000000000000000000001","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":2,"created_block":0,"history_from_block":0,"created":0,"deployed":true}}"#
        ),
    )
    .unwrap();
    let body = format!(
        r#"{{"policy":"{policy}","url":"https://example.com/paid","maxUsdc":"0.01","reason":"test"}}"#
    );
    let (st, _, b) = http_with(p, "POST", "/api/level1/buy", &body, "");
    assert_eq!(st, 401, "{b}");
    assert!(b.contains(r#""scope":"buy""#), "{b}");
    let (st, _, b) = http_with(
        p,
        "POST",
        "/api/level1/buy",
        &body,
        "Authorization: Bearer not-a-key\r\n",
    );
    assert_eq!(st, 403, "{b}");
}

/// docs/79 §6a Service 7: an account named before any wallet. Names resolve
/// and belong to the first passkey; the wallets route is gated like `/bills`
/// (a stranger is refused on a reserved account); the first wallet is proposed
/// as the spare key and never written over `recovery`; caps nobody typed
/// follow the bills. No RPC: every path here answers before the chain.
#[test]
fn a_named_account_resolves_holds_its_name_and_connects_wallets() {
    let srv = start_with(7, &[]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-7", std::process::id()));
    let policy = "0x00000000000000000000000000000000000000d1";
    let (qx, qy) = (
        format!("0x{}", "11".repeat(32)),
        format!("0x{}", "22".repeat(32)),
    );
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    // The reserve token the browser that opened the account holds (docs/60
    // P-87); the record keeps only its sha256.
    let tok = "5eed".repeat(16);
    let tok_hash = operator::level1::token_hash(&tok);
    let record = |recovery_explicit: bool| {
        format!(
            r#"{{"chain":"arbitrum-sepolia","owner":"0x00000000000000000000000000000000000000a1","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000000","recovery":"0x0000000000000000000000000000000000000000","per_action":"50.000000 USDC","daily":"100.000000 USDC","floor":"0.000000 USDC","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":false,"qx":"{qx}","qy":"{qy}","name":"acme-co","caps_provisional":true,"recovery_explicit":{recovery_explicit},"reserve_token_hash":"{tok_hash}"}}"#
        )
    };
    let file = dir.join("tenants").join(format!("{policy}.json"));
    let write = |recovery_explicit: bool| std::fs::write(&file, record(recovery_explicit)).unwrap();
    write(false);

    // Names: unknown 404, malformed 400, held 200 with where it lives.
    assert_eq!(http(p, "GET", "/api/level1/name/nobody-here", "").0, 404);
    assert_eq!(http(p, "GET", "/api/level1/name/-bad", "").0, 400);
    let (st, b) = http(p, "GET", "/api/level1/name/ACME-co", "");
    assert_eq!(st, 200, "{b}");
    let n: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(n["account"], policy);
    assert_eq!(n["chains"][0]["key"], "arbitrum-sepolia");
    assert_eq!(n["chains"][0]["deployed"], false);

    // Reserve: the name is the first passkey's (409 for anyone else); the same
    // passkey on the same chain gets its account back; no name and no wallet,
    // or a malformed name, is refused before anything is computed.
    let reserve = |name: &str, x: &str| {
        format!(
            r#"{{"chain":"arbitrum-sepolia","qx":"{x}","qy":"{qy}","name":"{name}","lazy":true}}"#
        )
    };
    let other = format!("0x{}", "33".repeat(32));
    let (st, b) = http(p, "POST", "/api/level1/deploy", &reserve("acme-co", &other));
    assert_eq!(st, 409, "{b}");
    let (st, b) = http(p, "POST", "/api/level1/deploy", &reserve("acme-co", &qx));
    assert_eq!(st, 200, "{b}");
    assert!(b.contains(policy), "{b}");
    // A re-claim matches public values: the record back, never a token, and
    // the public view carries neither the token nor its hash (P-87).
    let r: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert!(r["reserveToken"].is_null(), "{b}");
    assert!(
        !b.contains(&tok_hash) && !b.contains("reserve_token"),
        "{b}"
    );
    let (_, b) = http(p, "GET", &format!("/api/level1/tenant/{policy}"), "");
    assert!(b.contains(&qx) && !b.contains(&tok_hash), "{b}");
    // The held name on another chain is refused: anyone with the public key
    // could otherwise open it there with their own spare key.
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/deploy",
        &format!(
            r#"{{"chain":"arc-testnet","qx":"{qx}","qy":"{qy}","name":"acme-co","lazy":true}}"#
        ),
    );
    assert_eq!(st, 409, "{b}");
    let (st, b) = http(p, "POST", "/api/level1/deploy", &reserve("Bad_Name", &qx));
    assert_eq!(st, 400, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/deploy",
        &format!(r#"{{"chain":"arbitrum-sepolia","qx":"{qx}","qy":"{qy}","lazy":true}}"#),
    );
    assert_eq!(st, 400, "{b}");
    assert!(b.contains("name the account"), "{b}");

    // Wallets on a reserved account: only the reserve token. A stranger who
    // read the record's own qx/qy from the public route is refused (P-87), so
    // is no token and a wrong one.
    let w1 = "0x74858f049e471962588a726e48264e448cadad02";
    let add =
        |w: &str, t: &str| format!(r#"{{"policy":"{policy}","add":"{w}","reserveToken":"{t}"}}"#);
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/wallets",
        &format!(r#"{{"policy":"{policy}","add":"{w1}","qx":"{qx}","qy":"{qy}"}}"#),
    );
    assert_eq!(st, 403, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/wallets",
        &format!(r#"{{"policy":"{policy}","add":"{w1}"}}"#),
    );
    assert_eq!(st, 403, "{b}");
    let (st, b) = http(p, "POST", "/api/level1/wallets", &add(w1, &"0".repeat(64)));
    assert_eq!(st, 403, "{b}");
    // The owner: listed, proposed as the spare key, `recovery` untouched.
    let (st, b) = http(p, "POST", "/api/level1/wallets", &add(w1, &tok));
    assert_eq!(st, 200, "{b}");
    let r: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(r["wallets"], serde_json::json!([w1]));
    assert_eq!(r["spareKey"], w1);
    assert_eq!(r["recovery"], "0x0000000000000000000000000000000000000000");
    assert!(
        r["spareKeyCall"]["data"]
            .as_str()
            .unwrap()
            .ends_with(&w1[2..]),
        "{b}"
    );
    assert_eq!(r["caps"]["perAction"], "50.000000 USDC");
    assert_eq!(r["caps"]["provisional"], true);
    // Again: no duplicate. A second wallet is listed, not proposed.
    let w2 = "0x00000000000000000000000000000000000000e2";
    http(p, "POST", "/api/level1/wallets", &add(w1, &tok));
    let (_, b) = http(p, "POST", "/api/level1/wallets", &add(w2, &tok));
    let r: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(r["wallets"], serde_json::json!([w1, w2]));
    assert_eq!(r["spareKey"], w1);
    // 0x0 is never a wallet; five at most.
    let zero = "0x0000000000000000000000000000000000000000";
    assert_eq!(
        http(p, "POST", "/api/level1/wallets", &add(zero, &tok)).0,
        400
    );
    for i in 3..=5 {
        let w = format!("0x00000000000000000000000000000000000000e{i}");
        assert_eq!(
            http(p, "POST", "/api/level1/wallets", &add(&w, &tok)).0,
            200
        );
    }
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/wallets",
        &add("0x00000000000000000000000000000000000000e6", &tok),
    );
    assert_eq!(st, 400, "{b}");
    // Removing the proposed wallet withdraws the proposal.
    let (_, b) = http(
        p,
        "POST",
        "/api/level1/wallets",
        &format!(r#"{{"policy":"{policy}","remove":"{w1}","reserveToken":"{tok}"}}"#),
    );
    let r: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(r["wallets"].as_array().unwrap().len(), 4);
    assert!(r["spareKey"].is_null(), "{b}");

    // A typed first bill moves caps nobody typed: twice the largest. The
    // record's public qx/qy are refused here too; the token is not.
    let bill =
        r#"[{"payee":"0x1111111111111111111111111111111111111111","amount":"30","everyDays":30}]"#;
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/bills",
        &format!(r#"{{"policy":"{policy}","qx":"{qx}","qy":"{qy}","bills":{bill}}}"#),
    );
    assert_eq!(st, 403, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/bills",
        &format!(r#"{{"policy":"{policy}","reserveToken":"{tok}","bills":{bill}}}"#),
    );
    assert_eq!(st, 200, "{b}");
    let t: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("tenants").join(format!("{policy}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(t["per_action"], "60.000000 USDC");
    assert_eq!(t["daily"], "60.000000 USDC");

    // A recovery the owner chose at reserve (here: none, said explicitly) is
    // never replaced by a connected wallet.
    write(true);
    let (_, b) = http(p, "POST", "/api/level1/wallets", &add(w1, &tok));
    let r: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert!(r["spareKey"].is_null(), "{b}");

    // A new token takes the passkey (P-87): no assertion 401, a stale or
    // forged one refused, a real one over the page-computed challenge mints a
    // token that works while the old one stops.
    use p256::ecdsa::signature::hazmat::PrehashSigner;
    use sha2::{Digest, Sha256};
    let sk = p256::ecdsa::SigningKey::from_slice(&[7u8; 32]).unwrap();
    let pt = sk.verifying_key().to_encoded_point(false);
    let (kx, ky) = (
        format!("0x{}", alloy::hex::encode(pt.x().unwrap())),
        format!("0x{}", alloy::hex::encode(pt.y().unwrap())),
    );
    std::fs::write(
        &file,
        record(false)
            .replace(&format!("0x{}", "11".repeat(32)), &kx)
            .replace(&format!("0x{}", "22".repeat(32)), &ky),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let assert_for = |issued: u64, key: &p256::ecdsa::SigningKey| {
        let ch =
            operator::level1::reserve_token_challenge(421_614, policy.parse().unwrap(), issued);
        let cd = format!(
            r#"{{"type":"webauthn.get","challenge":"{}","origin":"http://localhost"}}"#,
            operator::level1::b64url(ch.as_slice())
        );
        // rpIdHash = sha256("localhost"), flags UP|UV (WebAuthn §7.2; the service checks both).
        let ad = [&Sha256::digest(b"localhost")[..], &[0x05, 0, 0, 0, 0]].concat();
        let mut h = Sha256::new();
        h.update(&ad);
        h.update(Sha256::digest(cd.as_bytes()));
        let sig: p256::ecdsa::Signature = key.sign_prehash(&h.finalize()).unwrap();
        let (r, s) = sig.split_bytes();
        serde_json::json!({
            "policy": policy, "issued": issued,
            "auth": { "r": format!("0x{}", alloy::hex::encode(r)), "s": format!("0x{}", alloy::hex::encode(s)),
                "challengeIndex": cd.find("\"challenge\"").unwrap(), "typeIndex": cd.find("\"type\"").unwrap(),
                "authenticatorData": format!("0x{}", alloy::hex::encode(&ad)), "clientDataJSON": cd },
        })
        .to_string()
    };
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/reserve-token",
        &format!(r#"{{"policy":"{policy}"}}"#),
    );
    assert_eq!(st, 401, "{b}");
    let stranger = p256::ecdsa::SigningKey::from_slice(&[9u8; 32]).unwrap();
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/reserve-token",
        &assert_for(now, &stranger),
    );
    assert_eq!(st, 403, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/reserve-token",
        &assert_for(now - 3600, &sk),
    );
    assert_eq!(st, 400, "{b}");
    let signed = assert_for(now, &sk);
    let (st, b) = http(p, "POST", "/api/level1/reserve-token", &signed);
    assert_eq!(st, 200, "{b}");
    let fresh = serde_json::from_str::<serde_json::Value>(&b).unwrap()["reserveToken"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(fresh.len(), 64);
    assert_eq!(
        http(p, "POST", "/api/level1/reserve-token", &signed).0,
        400,
        "used once"
    );
    assert_eq!(
        http(p, "POST", "/api/level1/wallets", &add(w1, &tok)).0,
        403,
        "old token"
    );
    assert_eq!(
        http(p, "POST", "/api/level1/wallets", &add(w1, &fresh)).0,
        200
    );

    // Deployed (docs/60 P-93): /wallets is never open. With no agent keys a
    // stranger (no token, or the public qx/qy) is refused and the device token
    // still works.
    let deployed = record(false).replace(r#""deployed":false"#, r#""deployed":true"#);
    std::fs::write(&file, &deployed).unwrap();
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/wallets",
        &format!(r#"{{"policy":"{policy}","add":"{w1}"}}"#),
    );
    assert_eq!(st, 403, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/wallets",
        &format!(r#"{{"policy":"{policy}","add":"{w1}","qx":"{qx}","qy":"{qy}"}}"#),
    );
    assert_eq!(st, 403, "{b}");
    let (st, b) = http(p, "POST", "/api/level1/wallets", &add(w1, &tok));
    assert_eq!(st, 200, "{b}");
    // With an `import` agent key: its bearer works, a `read` key does not, the
    // device token still does; /bills keeps the router's gate (unchanged).
    let (import, read) = ("agent-import-key", "agent-read-key");
    std::fs::write(
        &file,
        deployed.replace(
            r#""deployed":true"#,
            &format!(
                r#""deployed":true,"agent_tokens":[{{"id":"k1","label":"x","scopes":["import"],"expires":99999999999,"issued":0,"hash":"{}"}},{{"id":"k2","label":"y","scopes":["read"],"expires":99999999999,"issued":0,"hash":"{}"}}]"#,
                operator::level1::token_hash(import),
                operator::level1::token_hash(read)
            ),
        ),
    )
    .unwrap();
    let bare = format!(r#"{{"policy":"{policy}","add":"{w2}"}}"#);
    let (st, _, b) = http_with(
        p,
        "POST",
        "/api/level1/wallets",
        &bare,
        &format!("Authorization: Bearer {import}\r\n"),
    );
    assert_eq!(st, 200, "{b}");
    let (st, _, b) = http_with(
        p,
        "POST",
        "/api/level1/wallets",
        &bare,
        &format!("Authorization: Bearer {read}\r\n"),
    );
    assert_eq!(st, 403, "{b}");
    assert_eq!(http(p, "POST", "/api/level1/wallets", &bare).0, 403);
    let (st, b) = http(p, "POST", "/api/level1/wallets", &add(w1, &tok));
    assert_eq!(st, 200, "{b}");
    // docs/60 P-225: the owner's device token, as the page sends it with a
    // bills import, opens the import on its own account; P-226: the import
    // itself then needs the owner's passkey over the bills.
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/bills",
        &format!(r#"{{"policy":"{policy}","reserveToken":"{tok}","bills":{bill}}}"#),
    );
    assert_eq!(st, 401, "{b}");
    assert!(b.contains("owner's passkey"), "{b}");
}

/// docs/81: the owner names a party from the device that opened the account
/// and nowhere else; the public view never carries the names.
#[test]
fn only_the_opening_device_names_payees_and_the_names_stay_private() {
    let srv = start_with(11, &[]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-11", std::process::id()));
    let policy = "0x00000000000000000000000000000000000000d2";
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    let tok = "c0de".repeat(16);
    let tok_hash = operator::level1::token_hash(&tok);
    std::fs::write(
        dir.join("tenants").join(format!("{policy}.json")),
        format!(
            r#"{{"chain":"arbitrum-sepolia","owner":"0x00000000000000000000000000000000000000a1","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000000","recovery":"0x0000000000000000000000000000000000000000","per_action":"50.000000 USDC","daily":"100.000000 USDC","floor":"0.000000 USDC","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":false,"name":"names-co","reserve_token_hash":"{tok_hash}"}}"#
        ),
    )
    .unwrap();
    let alice = "0x1111111111111111111111111111111111119a25";
    let name = |n: &str, t: &str| {
        format!(r#"{{"policy":"{policy}","address":"{alice}","name":"{n}","reserveToken":"{t}"}}"#)
    };
    // No token, a wrong token: refused. An agent key is not enough either.
    assert_eq!(
        http(p, "POST", "/api/level1/contacts", &name("Alice", "")).0,
        403
    );
    assert_eq!(
        http(p, "POST", "/api/level1/contacts", &name("Alice", "nope")).0,
        403
    );
    // A name that reads as an address is refused; a real one is kept.
    let (st, b) = http(p, "POST", "/api/level1/contacts", &name("0xfeed", &tok));
    assert_eq!(st, 400, "{b}");
    let (st, b) = http(
        p,
        "POST",
        "/api/level1/contacts",
        &name("Alice  (design)", &tok),
    );
    assert_eq!(st, 200, "{b}");
    assert!(b.contains(r#""name":"Alice (design)""#), "{b}");
    // The public view does not carry the owner's names.
    let (_, b) = http(p, "GET", &format!("/api/level1/tenant/{policy}"), "");
    assert!(!b.contains("Alice") && !b.contains("contacts"), "{b}");
    // An empty name forgets it.
    let (st, b) = http(p, "POST", "/api/level1/contacts", &name("", &tok));
    assert_eq!(st, 200, "{b}");
    assert!(b.contains(r#""contacts":[]"#), "{b}");

    // A typed name resolves (docs/81 phase B): another account's handle with no
    // network; nonsense is refused before any lookup; an unknown chain is 400.
    let (st, b) = http(
        p,
        "GET",
        "/api/level1/resolve?chain=arbitrum-sepolia&q=Names-Co",
        "",
    );
    assert_eq!(st, 200, "{b}");
    assert!(
        b.contains(policy) && b.contains(r#""kind":"tameion""#) && b.contains(r#""tail":"00D2""#),
        "{b}"
    );
    let (st, b) = http(
        p,
        "GET",
        "/api/level1/resolve?chain=arbitrum-sepolia&q=not%20a%20name!",
        "",
    );
    assert_eq!(st, 404, "{b}");
    let (st, _) = http(p, "GET", "/api/level1/resolve?chain=nowhere&q=names-co", "");
    assert_eq!(st, 400);
}

/// docs/103 S2-4: minting a counterparty invite and turning an answer down are the owner's device only; an agent key
/// with `run` is refused, and a `read` key may still list the answers.
#[test]
fn invites_and_dismissals_take_the_owners_device_not_an_agent_key() {
    let srv = start_with(12, &[]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-12", std::process::id()));
    let policy = "0x00000000000000000000000000000000000000d3";
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    let tok = "d00d".repeat(16);
    let (run, read) = ("agent-run-key", "agent-read-key");
    std::fs::write(
        dir.join("tenants").join(format!("{policy}.json")),
        format!(
            r#"{{"chain":"arbitrum-sepolia","owner":"0x00000000000000000000000000000000000000a1","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000000","recovery":"0x0000000000000000000000000000000000000000","per_action":"50.000000 USDC","daily":"100.000000 USDC","floor":"0.000000 USDC","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":false,"name":"invite-co","reserve_token_hash":"{}","agent_tokens":[{{"id":"k1","label":"run","scopes":["run","read"],"expires":99999999999,"issued":0,"hash":"{}"}},{{"id":"k2","label":"read","scopes":["read"],"expires":99999999999,"issued":0,"hash":"{}"}}]}}"#,
            operator::level1::token_hash(&tok),
            operator::level1::token_hash(run),
            operator::level1::token_hash(read)
        ),
    )
    .unwrap();
    let invite = format!(r#"{{"policy":"{policy}","role":"Supplier"}}"#);
    let bearer = |k: &str| format!("Authorization: Bearer {k}\r\n");
    let (st, _, b) = http_with(p, "POST", "/api/desk/party/invite", &invite, &bearer(run));
    assert_eq!(st, 403, "an agent key mints no invite: {b}");
    let (st, _, b) = http_with(
        p,
        "POST",
        "/api/desk/party/invite",
        &invite,
        &format!("X-Tameion-Device: {tok}\r\n"),
    );
    assert_eq!(st, 200, "{b}");
    let secret = b
        .split(r#""invite":""#)
        .nth(1)
        .and_then(|r| r.split('"').next())
        .unwrap()
        .to_string();
    let id = format!("{:#x}", alloy::primitives::keccak256(secret.as_bytes()));
    let (st, _, b) = http_with(
        p,
        "GET",
        &format!("/api/desk/party/proposals?policy={policy}"),
        "",
        &bearer(read),
    );
    assert_eq!(st, 200, "a read key may list the answers: {b}");
    let dismiss = format!(r#"{{"policy":"{policy}","id":"{id}"}}"#);
    let (st, _, b) = http_with(p, "POST", "/api/desk/party/dismiss", &dismiss, &bearer(run));
    assert_eq!(st, 403, "an agent key dismisses nothing: {b}");
    let (st, _, b) = http_with(
        p,
        "POST",
        "/api/desk/party/dismiss",
        &dismiss,
        &format!("X-Tameion-Device: {tok}\r\n"),
    );
    assert_eq!(st, 200, "{b}");
}

/// A Server-Sent Events route stays open, so read what it has said so far: the
/// status line, the headers and the first event, or the whole of an error reply.
fn stream_head(port: u16, path: &str, extra: &str) -> (u16, String) {
    use std::io::Write;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let req =
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{extra}Connection: keep-alive\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    while let Ok(n) = std::io::Read::read(&mut s, &mut buf) {
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if String::from_utf8_lossy(&out).contains("event: hello") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    (status, text)
}

/// docs/103 S4-1: an account whose owner minted an agent key serves its event
/// stream, and every other `read` route, to that key and to the owner's own
/// device (the token the desk's routes already open to). A stranger, a made-up
/// token and another account's device are refused; the device still opens no
/// `run`: that needs its key.
#[test]
fn the_owners_device_reads_a_keyed_account_without_its_agent_key() {
    let srv = start_with(61, &[]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-61", std::process::id()));
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    let (mine, other) = (
        "0x00000000000000000000000000000000000000e1",
        "0x00000000000000000000000000000000000000e2",
    );
    let (dev_mine, dev_other) = ("d3a1".repeat(16), "d3b2".repeat(16));
    let key = "agent-read-key";
    for (policy, dev) in [(mine, &dev_mine), (other, &dev_other)] {
        std::fs::write(
            dir.join("tenants").join(format!("{policy}.json")),
            format!(
                r#"{{"chain":"arbitrum-sepolia","owner":"0x00000000000000000000000000000000000000a1","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000000","recovery":"0x0000000000000000000000000000000000000000","per_action":"50.000000 USDC","daily":"100.000000 USDC","floor":"0.000000 USDC","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":false,"name":"keyed-co","reserve_token_hash":"{}","agent_tokens":[{{"id":"k1","label":"read","scopes":["read"],"expires":99999999999,"issued":0,"hash":"{}"}}]}}"#,
                operator::level1::token_hash(dev),
                operator::level1::token_hash(key)
            ),
        )
        .unwrap();
    }
    std::fs::write(
        dir.join("tenants")
            .join(format!("{mine}.purchases.jsonl")),
        concat!(
            r#"{"at":1,"seller":"0x00000000000000000000000000000000005e11e2","amount":"0.001000 USDC","minor":"1000","url":"http://s/v1/mid","status":"paid","tx":"0xa1","id":"0xa1","reason":"price"}"#,
            "\n"
        ),
    )
    .unwrap();
    let events = format!("/api/level1/events?policy={mine}");
    let device = |t: &str| format!("X-Tameion-Device: {t}\r\n");

    let (st, text) = stream_head(p, &events, "");
    assert_eq!(st, 401, "no token, no stream: {text}");
    assert!(text.contains(r#""scope":"read""#), "{text}");
    let (st, text) = stream_head(p, &events, &device(&dev_mine));
    assert_eq!(st, 200, "the account's own device reads its events: {text}");
    assert!(
        text.contains("text/event-stream") && text.contains("event: hello"),
        "{text}"
    );
    let (st, text) = stream_head(p, &events, &device(&dev_other));
    assert_eq!(
        st, 401,
        "another account's device is a stranger here: {text}"
    );
    let (st, text) = stream_head(p, &events, &device("not-a-token"));
    assert_eq!(st, 401, "{text}");
    let (st, text) = stream_head(p, &events, &format!("Authorization: Bearer {key}\r\n"));
    assert_eq!(st, 200, "the agent key still reads: {text}");

    let purchases = format!("/api/level1/purchases.jsonl?policy={mine}");
    let (st, _, b) = http_with(p, "GET", &purchases, "", &device(&dev_mine));
    assert_eq!(st, 200, "the same device opens the other read routes: {b}");
    let (st, _, b) = http_with(p, "GET", &purchases, "", &device(&dev_other));
    assert_eq!(st, 401, "{b}");
    let (st, _, b) = http_with(
        p,
        "POST",
        "/api/level1/run",
        &format!(r#"{{"policy":"{mine}"}}"#),
        &device(&dev_mine),
    );
    // docs/60 P-225: the owner's own "Check now" — the device opens `run`.
    assert_eq!(st, 202, "the owner's device runs a check: {b}");
}

/// UAT U3 (docs/60 P-182): a prepared change says who signs it and when it
/// applies; asked with an agent key, it is kept so the owner's page shows it
/// as one row with the agent's reason. No RPC answers here, so `applies` is
/// the general rule; the contract's numbers make it exact on a live account.
#[test]
fn an_agents_prepared_change_says_who_signs_and_is_kept_for_the_owner() {
    let srv = start_with(13, &[("TAMEION_RPC_ARC", "http://127.0.0.1:9")]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-13", std::process::id()));
    let policy = "0x00000000000000000000000000000000000000d1";
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    let hash = operator::level1::token_hash("k-prep");
    std::fs::write(
        dir.join("tenants").join(format!("{policy}.json")),
        format!(
            r#"{{"chain":"arc-testnet","owner":"0x0000000000000000000000000000000000000001","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":true,"agent_tokens":[{{"id":"t9","label":"ops agent","scopes":["read","prepare"],"expires":4102444800,"issued":0,"hash":"{hash}"}}]}}"#
        ),
    )
    .unwrap();
    let key = "Authorization: Bearer k-prep\r\n";
    let (st, _, b) = http_with(
        p,
        "POST",
        "/api/level1/prepare-limits",
        &format!(
            r#"{{"policy":"{policy}","perAction":"0.004","daily":"0.008","floor":"0","reason":"the weather API raised its price"}}"#
        ),
        key,
    );
    assert_eq!(st, 200, "{b}");
    let v: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert!(
        v["signer"].as_str().unwrap().contains("owner's passkey"),
        "{b}"
    );
    assert!(
        v["applies"].as_str().unwrap().contains("owner can cancel"),
        "{b}"
    );
    assert!(
        v["ownerSees"]
            .as_str()
            .unwrap()
            .contains("one row on the account page"),
        "{b}"
    );
    let (st, _, b) = http_with(
        p,
        "POST",
        "/api/level1/prepare-mode",
        &format!(
            r#"{{"policy":"{policy}","mode":2,"reason":"three weeks of approvals, none refused"}}"#
        ),
        key,
    );
    assert_eq!(st, 200, "{b}");
    let asks: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("tenants").join(format!("{policy}.asks.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(asks["limits"]["perAction"], "0.004000 USDC");
    assert_eq!(asks["limits"]["reason"], "the weather API raised its price");
    assert_eq!(asks["limits"]["by"], "ops agent");
    assert_eq!(asks["mode"]["mode"], 2);
    // Without a key on a keyed account nothing is prepared, and nothing kept.
    let (st, _, _) = http_with(
        p,
        "POST",
        "/api/level1/prepare-limits",
        &format!(r#"{{"policy":"{policy}","perAction":"0.001","daily":"0.008","floor":"0"}}"#),
        "",
    );
    assert_eq!(st, 401);
}

/// UAT wave 2 (docs/60 P-170, P-176, P-173): on an account with agent keys the
/// owner's own device still prepares changes (the passkey signs them), only for
/// its own account; the anchored records stay open to anyone; the owner's live
/// stream opens with the device header; a run or a statement still needs a key.
#[test]
fn the_owners_device_prepares_and_anyone_reads_the_records() {
    let srv = start_with(14, &[("TAMEION_RPC_ARC", "http://127.0.0.1:9")]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-14", std::process::id()));
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    let tenant = |policy: &str, device: &str| {
        let read = operator::level1::token_hash("k-read");
        let dev = operator::level1::token_hash(device);
        std::fs::write(
            dir.join("tenants").join(format!("{policy}.json")),
            format!(
                r#"{{"chain":"arc-testnet","owner":"0x0000000000000000000000000000000000000001","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":true,"reserve_token_hash":"{dev}","agent_tokens":[{{"id":"r1","label":"reader","scopes":["read"],"expires":4102444800,"issued":0,"hash":"{read}"}}]}}"#
            ),
        )
        .unwrap();
    };
    let (a, b) = (
        "0x00000000000000000000000000000000000000e1",
        "0x00000000000000000000000000000000000000e2",
    );
    tenant(a, "dev-a");
    tenant(b, "dev-b");
    let limits = |policy: &str| {
        format!(r#"{{"policy":"{policy}","perAction":"0.01","daily":"1","floor":"0"}}"#)
    };
    let dev_a = "x-tameion-device: dev-a\r\n";
    // P-170: the owner's device prepares on its own account, never on another.
    let (st, _, body) = http_with(p, "POST", "/api/level1/prepare-limits", &limits(a), dev_a);
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("owner's passkey"), "{body}");
    let (st, _, _) = http_with(p, "POST", "/api/level1/prepare-limits", &limits(b), dev_a);
    assert_eq!(st, 401, "another account's device opens nothing");
    let (st, _, _) = http_with(p, "POST", "/api/level1/prepare-limits", &limits(a), "");
    assert_eq!(st, 401, "no credential, no prepare");
    let (st, _, _) = http_with(
        p,
        "POST",
        "/api/level1/run",
        &format!(r#"{{"policy":"{a}"}}"#),
        dev_a,
    );
    assert_eq!(st, 202, "docs/60 P-225: the owner's device runs a check");
    // Agent key scopes unchanged: a read key cannot prepare.
    let (st, _, _) = http_with(
        p,
        "POST",
        "/api/level1/prepare-limits",
        &limits(a),
        "Authorization: Bearer k-read\r\n",
    );
    assert_eq!(st, 403);
    // P-176: the records the page says anyone can check answer without a key.
    let (st, _, body) = http_with(p, "GET", &format!("/api/level1/records?policy={a}"), "", "");
    assert_ne!(st, 401, "{body}");
    let (st, _, body) = http_with(
        p,
        "GET",
        &format!(
            "/api/level1/record?policy={a}&root=0x1111111111111111111111111111111111111111111111111111111111111111"
        ),
        "",
        "",
    );
    assert_ne!(st, 401, "{body}");
    let (st, _, _) = http_with(
        p,
        "GET",
        &format!("/api/level1/statement?policy={a}"),
        "",
        "",
    );
    assert_eq!(st, 401, "the statement (bills, payees) stays behind read");
    // P-173: the owner's stream opens with the device header, as the page sends it.
    let (st, _) = stream_head(p, &format!("/api/level1/events?policy={a}"), dev_a);
    assert_eq!(st, 200);
    let (st, _) = stream_head(p, &format!("/api/level1/events?policy={a}"), "");
    assert_eq!(st, 401);
}

/// docs/60 P-212: on a locked account an outsider's record is each event's
/// `seq` and leaf hash, not the event — and the root rebuilt from those leaves
/// is the anchored root, so "Check it yourself" still matches. The owner's
/// device and a read key get the events.
#[test]
fn a_locked_accounts_record_shows_outsiders_leaves_not_events() {
    use ledger::log::{leaf_from_canonical, root_from_leaves};
    let srv = start_with(15, &[("TAMEION_RPC_ARC", "http://127.0.0.1:9")]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-15", std::process::id()));
    let a = "0x00000000000000000000000000000000000000e1";
    let log = dir.join("tenants").join(a).join("log");
    std::fs::create_dir_all(&log).unwrap();
    let (read, dev) = (
        operator::level1::token_hash("k-read"),
        operator::level1::token_hash("dev-a"),
    );
    std::fs::write(
        dir.join("tenants").join(format!("{a}.json")),
        format!(
            r#"{{"chain":"arc-testnet","owner":"0x0000000000000000000000000000000000000001","policy":"{a}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":true,"reserve_token_hash":"{dev}","agent_tokens":[{{"id":"r1","label":"reader","scopes":["read"],"expires":4102444800,"issued":0,"hash":"{read}"}}]}}"#
        ),
    )
    .unwrap();
    let events: [&[u8]; 3] = [
        b"Holding 0.998000 USDC",
        b"bought x402:http://seller/v1?coin=ETH",
        b"ETH price for the weekly report",
    ];
    let leaves: Vec<_> = events.iter().map(|c| leaf_from_canonical(c)).collect();
    let root = root_from_leaves(&leaves).to_hex();
    let file: String = events
        .iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{}\n",
                serde_json::json!({ "seq": i, "at": 1, "kind": "observation", "note": "a secret note", "canonical": format!("0x{}", c.iter().map(|b| format!("{b:02x}")).collect::<String>()) })
            )
        })
        .collect();
    std::fs::write(log.join(format!("{root}.events.jsonl")), file).unwrap();
    let path = format!("/api/level1/record?policy={a}&root={root}");
    let (st, _, body) = http_with(p, "GET", &path, "", "");
    assert_eq!(st, 200, "{body}");
    assert!(
        !body.contains("canonical") && !body.contains("secret") && !body.contains("observation"),
        "{body}"
    );
    let got: Vec<ledger::log::Hash32> = body
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            ledger::log::Hash32::from_hex(v["leaf"].as_str().unwrap()).unwrap()
        })
        .collect();
    assert_eq!(
        root_from_leaves(&got).to_hex(),
        root,
        "the leaves rebuild the anchored root"
    );
    for who in [
        "x-tameion-device: dev-a\r\n",
        "Authorization: Bearer k-read\r\n",
    ] {
        let (st, _, body) = http_with(p, "GET", &path, "", who);
        assert_eq!(st, 200);
        assert!(
            body.contains("canonical") && body.contains("secret"),
            "{who}: {body}"
        );
    }
}

/// docs/60 P-221: a tenant file that cannot be read is treated as locked. The
/// record serves leaf hashes, not events, and a gated route refuses (503)
/// instead of opening.
#[test]
fn an_unreadable_tenant_shows_no_events_and_opens_no_gate() {
    let srv = start_with(16, &[("TAMEION_RPC_ARC", "http://127.0.0.1:9")]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-16", std::process::id()));
    let a = "0x00000000000000000000000000000000000000e1";
    let log = dir.join("tenants").join(a).join("log");
    std::fs::create_dir_all(&log).unwrap();
    // Half a tenant, as a read in the middle of a non-atomic write found it.
    std::fs::write(
        dir.join("tenants").join(format!("{a}.json")),
        r#"{"chain":"arc-testnet","owner":"0x0000"#,
    )
    .unwrap();
    let root = ledger::log::root_from_leaves(&[ledger::log::leaf_from_canonical(
        b"Holding 0.998000 USDC",
    )])
    .to_hex();
    std::fs::write(
        log.join(format!("{root}.events.jsonl")),
        format!("{}\n", serde_json::json!({ "seq": 0, "at": 1, "kind": "observation", "note": "a secret note", "canonical": "0x486f6c64696e6720302e39393830303020555344" })),
    )
    .unwrap();
    let (st, _, body) = http_with(
        p,
        "GET",
        &format!("/api/level1/record?policy={a}&root={root}"),
        "",
        "",
    );
    assert_eq!(st, 200, "{body}");
    assert!(
        body.contains("\"leaf\"") && !body.contains("canonical") && !body.contains("secret"),
        "{body}"
    );
    let (st, _, body) = http_with(p, "GET", &format!("/api/level1/inbox?policy={a}"), "", "");
    assert_eq!(st, 503, "{body}");
}

/// docs/60 P-225: with an agent key on the account, the owner's own device still
/// runs a check, saves bills and decides invoices for its own account — never
/// another account's, and never `buy`; agent-key scopes are unchanged. A bills
/// `check` answers before the page spends a passkey tap.
#[test]
fn the_owners_device_runs_checks_saves_bills_and_decides_invoices() {
    let srv = start_with(17, &[("TAMEION_RPC_ARC", "http://127.0.0.1:9")]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-17", std::process::id()));
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    let tenant = |policy: &str, device: &str| {
        let read = operator::level1::token_hash("k-read");
        let dev = operator::level1::token_hash(device);
        std::fs::write(
            dir.join("tenants").join(format!("{policy}.json")),
            format!(
                r#"{{"chain":"arc-testnet","owner":"0x0000000000000000000000000000000000000001","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":false,"reserve_token_hash":"{dev}","agent_tokens":[{{"id":"r1","label":"reader","scopes":["read"],"expires":4102444800,"issued":0,"hash":"{read}"}}]}}"#
            ),
        )
        .unwrap();
    };
    let (a, b) = (
        "0x00000000000000000000000000000000000000e1",
        "0x00000000000000000000000000000000000000e2",
    );
    tenant(a, "dev-a");
    tenant(b, "dev-b");
    let dev_a = "x-tameion-device: dev-a\r\n";
    // `check: true`: the gate's answer without the import (which reads the chain).
    let bills = |policy: &str| format!(r#"{{"policy":"{policy}","check":true}}"#);
    let routes = [
        (
            "/api/level1/run",
            format!(r#"{{"policy":"{a}"}}"#),
            format!(r#"{{"policy":"{b}"}}"#),
        ),
        ("/api/level1/bills", bills(a), bills(b)),
        (
            "/api/level1/invoice-decide",
            format!(
                r#"{{"policy":"{a}","id":"0x{}","accept":true}}"#,
                "11".repeat(32)
            ),
            format!(
                r#"{{"policy":"{b}","id":"0x{}","accept":true}}"#,
                "11".repeat(32)
            ),
        ),
    ];
    for (route, mine, theirs) in &routes {
        let (st, _, body) = http_with(p, "POST", route, mine, dev_a);
        // Past the gate: an invoice decision then asks for the owner's passkey
        // signature (the device opens the route; the passkey decides).
        if route.ends_with("invoice-decide") {
            assert!(
                st == 401 && body.contains("owner's passkey"),
                "{route}: {st} {body}"
            );
        } else {
            assert!(
                st != 401 && st != 403,
                "{route}: the owner's device on its own account: {st} {body}"
            );
        }
        let (st, _, _) = http_with(p, "POST", route, theirs, dev_a);
        assert_eq!(st, 401, "{route}: another account's device opens nothing");
        let (st, _, _) = http_with(p, "POST", route, mine, "");
        assert_eq!(st, 401, "{route}: no credential");
        let (st, _, _) = http_with(p, "POST", route, mine, "Authorization: Bearer k-read\r\n");
        assert_eq!(st, 403, "{route}: a read key still cannot");
    }
    // The bills import carries the device token in its body too, as the page sends it.
    let in_body = format!(r#"{{"policy":"{a}","check":true,"reserveToken":"dev-a"}}"#);
    let (st, _, body) = http_with(p, "POST", "/api/level1/bills", &in_body, "");
    assert_eq!(st, 200, "{body}");
    // Asked first, before any passkey tap: allowed with the device, refused without.
    let check = format!(r#"{{"policy":"{a}","check":true}}"#);
    let (st, _, body) = http_with(p, "POST", "/api/level1/bills", &check, dev_a);
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("nothing was saved"), "{body}");
    let (st, _, _) = http_with(p, "POST", "/api/level1/bills", &check, "");
    assert_eq!(st, 401);
    // Never buy: buying is the agent's.
    let buy = format!(
        r#"{{"policy":"{a}","url":"https://example.com/x","maxUsdc":"0.01","reason":"a test"}}"#
    );
    let (st, _, _) = http_with(p, "POST", "/api/level1/buy", &buy, dev_a);
    assert_eq!(st, 401, "the device does not buy");
}

/// docs/60 P-226: on an account with agent keys, a bill import that only the
/// owner's device opened needs the owner's passkey over exactly those bills:
/// without it 401 naming the passkey, with another key's signature 403, with
/// the owner's the bills are saved.
#[test]
fn a_device_bill_import_needs_the_owners_passkey_over_the_bills() {
    use p256::ecdsa::signature::hazmat::PrehashSigner;
    use sha2::{Digest, Sha256};
    // The chain is the service itself: every read fails at once (not a refused
    // connection, which the client waits out), so the test does not wait.
    let own = 17_000 + (std::process::id() % 9_000) as u16 + 18;
    let srv = start_with(
        18,
        &[("TAMEION_RPC_ARC", &format!("http://127.0.0.1:{own}"))],
    );
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-18", std::process::id()));
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    let a = "0x00000000000000000000000000000000000000e1";
    let sk = p256::ecdsa::SigningKey::from_slice(&[7u8; 32]).unwrap();
    let pt = sk.verifying_key().to_encoded_point(false);
    let (kx, ky) = (
        format!("0x{}", alloy::hex::encode(pt.x().unwrap())),
        format!("0x{}", alloy::hex::encode(pt.y().unwrap())),
    );
    let (read, dev) = (
        operator::level1::token_hash("k-read"),
        operator::level1::token_hash("dev-a"),
    );
    std::fs::write(
        dir.join("tenants").join(format!("{a}.json")),
        format!(
            r#"{{"chain":"arc-testnet","owner":"0x0000000000000000000000000000000000000001","policy":"{a}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":false,"qx":"{kx}","qy":"{ky}","reserve_token_hash":"{dev}","agent_tokens":[{{"id":"r1","label":"reader","scopes":["read"],"expires":4102444800,"issued":0,"hash":"{read}"}}]}}"#
        ),
    )
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let bill = operator::level1::SeededBill {
        payee: "0x179dca89f69faef5765d2e7445aa5452b8159520".into(),
        amount: "0.05".into(),
        every_days: 30,
        next_due: now,
        discount_bps: 0,
        early_days: 0,
        every_secs: 0,
    };
    let chain_id = operator::watch::chain("arc-testnet").unwrap().chain_id;
    let body = |key: Option<&p256::ecdsa::SigningKey>| {
        let mut v = serde_json::json!({ "policy": a, "bills": [{ "payee": bill.payee, "amount": bill.amount, "everyDays": 30, "nextDue": now }] });
        if let Some(key) = key {
            let ch = operator::level1::bills_challenge(
                chain_id,
                a.parse().unwrap(),
                std::slice::from_ref(&bill),
                now,
            )
            .unwrap();
            let cd = format!(
                r#"{{"type":"webauthn.get","challenge":"{}","origin":"http://localhost"}}"#,
                operator::level1::b64url(ch.as_slice())
            );
            // rpIdHash = sha256("localhost"), flags UP|UV (WebAuthn §7.2; the service checks both).
            let ad = [&Sha256::digest(b"localhost")[..], &[0x05, 0, 0, 0, 0]].concat();
            let mut h = Sha256::new();
            h.update(&ad);
            h.update(Sha256::digest(cd.as_bytes()));
            let sig: p256::ecdsa::Signature = key.sign_prehash(&h.finalize()).unwrap();
            let (r, s) = sig.split_bytes();
            v["issued"] = serde_json::json!(now);
            v["auth"] = serde_json::json!({ "r": format!("0x{}", alloy::hex::encode(r)), "s": format!("0x{}", alloy::hex::encode(s)),
                "challengeIndex": cd.find("\"challenge\"").unwrap(), "typeIndex": cd.find("\"type\"").unwrap(),
                "authenticatorData": format!("0x{}", alloy::hex::encode(&ad)), "clientDataJSON": cd });
        }
        v.to_string()
    };
    let dev_a = "x-tameion-device: dev-a\r\n";
    let (st, _, b) = http_with(p, "POST", "/api/level1/bills", &body(None), dev_a);
    assert_eq!(st, 401, "{b}");
    assert!(b.contains("owner's passkey"), "{b}");
    let stranger = p256::ecdsa::SigningKey::from_slice(&[9u8; 32]).unwrap();
    let (st, _, b) = http_with(
        p,
        "POST",
        "/api/level1/bills",
        &body(Some(&stranger)),
        dev_a,
    );
    assert_eq!(st, 403, "{b}");
    let (st, _, b) = http_with(p, "POST", "/api/level1/bills", &body(Some(&sk)), dev_a);
    assert_eq!(st, 200, "{b}");
    let saved = std::fs::read_to_string(dir.join("tenants").join(format!("{a}.json"))).unwrap();
    assert!(
        saved.contains("0x179dca89f69faef5765d2e7445aa5452b8159520"),
        "{saved}"
    );
    // The same signature again is refused (one use).
    let (st, _, b) = http_with(p, "POST", "/api/level1/bills", &body(Some(&sk)), dev_a);
    assert_eq!(st, 400, "{b}");
}

/// One HTTP request's body from a socket (headers, then `Content-Length` bytes).
fn request_body(s: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = s.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).to_string();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let len = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if body.len() >= len {
                return body.to_string();
            }
        }
    }
    String::new()
}

/// A node for the buy path: `attestedPayee` true, `isBlacklisted` false, the
/// purchase's simulation reverts `NotAutopilot(1)` (the account is on
/// Propose), every other read zero. Returns its port.
fn propose_node() -> u16 {
    use std::io::Write;
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let sel = |sig: &str| alloy::hex::encode(&alloy::primitives::keccak256(sig.as_bytes())[..4]);
    let (attested, blacklisted) = (sel("attestedPayee(address)"), sel("isBlacklisted(address)"));
    let not_autopilot = format!("0x{}{:064x}", sel("NotAutopilot(uint8)"), 1);
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let body = request_body(&mut s);
            let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let answer = |r: &serde_json::Value| {
                let id = r["id"].clone();
                let input = r["params"][0]["input"]
                    .as_str()
                    .or(r["params"][0]["data"].as_str())
                    .unwrap_or("")
                    .trim_start_matches("0x")
                    .to_string();
                let word = |v: u8| format!("0x{v:064x}");
                match r["method"].as_str().unwrap_or("") {
                    "eth_chainId" => {
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": "0x66eee" })
                    }
                    "eth_blockNumber" => {
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": "0x10" })
                    }
                    "eth_getLogs" => {
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": [] })
                    }
                    "eth_call" if input.starts_with(&attested) => {
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": word(1) })
                    }
                    "eth_call" if input.starts_with(&blacklisted) || input.len() < 300 => {
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": word(0) })
                    }
                    "eth_call" => serde_json::json!({ "jsonrpc": "2.0", "id": id,
                        "error": { "code": 3, "message": "execution reverted", "data": not_autopilot } }),
                    _ => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": "0x0" }),
                }
            };
            let out = match req.as_array() {
                Some(batch) => serde_json::Value::Array(batch.iter().map(answer).collect()),
                None => answer(&req),
            }
            .to_string();
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                out.len()
            );
        }
    });
    port
}

/// A paid API that asks 0.001 USDC on Arbitrum Sepolia from `pay_to`. Returns its port.
fn paid_api(pay_to: &'static str) -> u16 {
    use std::io::Write;
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let c = operator::watch::chain("arbitrum-sepolia").unwrap();
    let required = operator::x402::encode_header(&serde_json::json!({
        "x402Version": 2,
        "accepts": [{ "scheme": "exact", "network": format!("eip155:{}", c.chain_id), "amount": "1000",
            "asset": c.usdc.unwrap(), "payTo": pay_to, "maxTimeoutSeconds": 60, "extra": {} }],
    }));
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let _ = request_body(&mut s);
            let _ = write!(
                s,
                "HTTP/1.1 402 Payment Required\r\nPAYMENT-REQUIRED: {required}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            );
        }
    });
    port
}

/// docs/115 §2: every refusal an agent gets is one object with its code, who
/// refused, whether anything was done and who must act; every answer carries
/// `ok` and the contract's date. A page keeps the sentence it shows. The
/// Propose refusal comes from the purchase's own simulation on a local node.
#[test]
fn the_agent_envelope_names_every_refusal() {
    let node = propose_node();
    let seller = "0x00000000000000000000000000000000005e11e2";
    let api = paid_api(seller);
    let srv = start_with(
        41,
        &[
            (
                "TAMEION_RPC_ARBITRUM_SEPOLIA",
                &format!("http://127.0.0.1:{node}"),
            ),
            ("TAMEION_X402_ALLOW_LOCAL", "1"),
        ],
    );
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-41", std::process::id()));
    let (policy, owner) = (
        "0x00000000000000000000000000000000000000f1",
        "0x00000000000000000000000000000000000000a1",
    );
    std::fs::create_dir_all(dir.join("tenants").join(owner)).unwrap();
    // A throwaway agent key for the simulation; it signs nothing that is sent.
    std::fs::write(
        dir.join("tenants").join(owner).join("agent.key"),
        format!("0x{}", "11".repeat(32)),
    )
    .unwrap();
    let (buy, read) = (
        operator::level1::token_hash("k-buy"),
        operator::level1::token_hash("k-read"),
    );
    let tenant = |deployed: bool| {
        std::fs::write(
            dir.join("tenants").join(format!("{policy}.json")),
            format!(
                r#"{{"chain":"arbitrum-sepolia","owner":"{owner}","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":{deployed},"agent_tokens":[{{"id":"b1","label":"buyer","scopes":["buy"],"expires":4102444800,"issued":0,"hash":"{buy}"}},{{"id":"r1","label":"reader","scopes":["read"],"expires":4102444800,"issued":0,"hash":"{read}"}}]}}"#
            ),
        )
        .unwrap();
    };
    tenant(true);
    let json = |b: &str| serde_json::from_str::<serde_json::Value>(b).unwrap_or_default();
    let inbox = format!("/api/level1/inbox?policy={policy}");

    // A wrong agent key: the agent fixes it.
    let (st, _, b) = http_with(p, "GET", &inbox, "", "Authorization: Bearer not-a-key\r\n");
    assert_eq!(st, 403, "{b}");
    let v = json(&b);
    assert_eq!(v["ok"], false, "{b}");
    assert_eq!(v["contractVersion"], operator::refusal::CONTRACT_VERSION);
    assert_eq!(v["error"]["code"], "token_invalid", "{b}");
    assert_eq!(v["error"]["audience"], "agent");
    assert_eq!(v["error"]["by"], "auth");
    assert_eq!(v["error"]["executed"], "no");
    assert_eq!(v["error"]["words"], "unknown agent key");
    assert_eq!(v["scope"], "read", "the fields beside the refusal stay");
    // The same refusal to a page keeps the sentence the page shows.
    let (st, _, b) = http_with(
        p,
        "GET",
        &inbox,
        "",
        "Authorization: Bearer not-a-key\r\nSec-Fetch-Site: same-origin\r\nSec-Fetch-Mode: cors\r\n",
    );
    assert_eq!(st, 403);
    assert_eq!(json(&b)["error"], "unknown agent key", "{b}");
    assert_eq!(json(&b)["ok"], false, "{b}");
    // Node's fetch says `Sec-Fetch-Mode` and is no page (docs/60 P-274): the SDK gets the code.
    let (_, _, b) = http_with(
        p,
        "GET",
        &inbox,
        "",
        "Authorization: Bearer not-a-key\r\nSec-Fetch-Mode: cors\r\n",
    );
    assert_eq!(json(&b)["error"]["code"], "token_invalid", "{b}");
    let (st, _, b) = http_with(p, "GET", &inbox, "", "");
    assert_eq!(
        (st, json(&b)["error"]["code"].clone()),
        (401, "token_missing".into()),
        "{b}"
    );

    // On Propose the agent buys nothing: the owner must turn Autopilot on.
    let order = format!(
        r#"{{"policy":"{policy}","url":"http://127.0.0.1:{api}/v1/mid","maxUsdc":"0.01","reason":"envelope test"}}"#
    );
    let (st, _, b) = http_with(
        p,
        "POST",
        "/api/level1/buy",
        &order,
        "Authorization: Bearer k-buy\r\n",
    );
    assert_eq!(st, 409, "{b}");
    let v = json(&b);
    assert_eq!(v["error"]["code"], "mode_propose", "{b}");
    assert_eq!(v["error"]["audience"], "user");
    assert_eq!(v["error"]["by"], "mandate");
    assert_eq!(
        v["error"]["executed"], "no",
        "refused before the seller saw a signature"
    );
    assert!(
        v["error"]["words"]
            .as_str()
            .unwrap()
            .contains("only on Autopilot"),
        "{b}"
    );
    assert_eq!(
        v["purchase"]["status"], "refused",
        "the purchase line is kept beside it"
    );

    // The same Idempotency-Key with another body: this request did nothing.
    let key = "Idempotency-Key: env-1\r\n";
    let first = r#"{"policy":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","amount":"1"}"#;
    let (st, _, b) = http_with(p, "POST", "/api/level1/prepare-withdraw", first, key);
    assert_eq!(st, 200, "{b}");
    assert_eq!(json(&b)["ok"], true, "{b}");
    let other = first.replace("\"1\"", "\"2\"");
    let (st, _, b) = http_with(p, "POST", "/api/level1/prepare-withdraw", &other, key);
    assert_eq!(st, 422, "{b}");
    let v = json(&b);
    assert_eq!(v["error"]["code"], "idempotency_conflict", "{b}");
    assert_eq!(v["error"]["executed"], "no");
    assert_eq!(v["error"]["audience"], "agent");

    // An answer: ok, the contract's date, and the account's switches.
    tenant(false);
    let (st, _, b) = http_with(p, "GET", &inbox, "", "Authorization: Bearer k-read\r\n");
    assert_eq!(st, 200, "{b}");
    let v = json(&b);
    assert_eq!(v["ok"], true, "{b}");
    assert_eq!(v["contractVersion"], operator::refusal::CONTRACT_VERSION);
    assert_eq!(v["paused"], false, "Propose is not paused");
    assert_eq!(v["halt"], false);
    assert!(v["recentUpdates"].as_array().is_some_and(|u| !u.is_empty()));
    assert!(v["policy"].is_string(), "the route's fields, unchanged");
    // The inbox as the SDK (Node's fetch) and as a page read it: `ok` and the date too (docs/60 P-274).
    for extra in [
        "Authorization: Bearer k-read\r\nSec-Fetch-Mode: cors\r\n",
        "Authorization: Bearer k-read\r\nSec-Fetch-Site: same-origin\r\nSec-Fetch-Mode: cors\r\n",
    ] {
        let (st, _, b) = http_with(p, "GET", &inbox, "", extra);
        let v = json(&b);
        assert_eq!(st, 200, "{b}");
        assert_eq!(
            (v["ok"].clone(), v["contractVersion"].clone()),
            (true.into(), operator::refusal::CONTRACT_VERSION.into()),
            "{extra}"
        );
    }

    // The catalogue, open to anyone.
    let (st, _, b) = http_with(p, "GET", "/api/level1/codes", "", "");
    assert_eq!(st, 200, "{b}");
    let v = json(&b);
    assert_eq!(v["ok"], true);
    let codes = v["codes"].as_array().unwrap();
    assert_eq!(codes.len(), operator::refusal::CODES.len());
    assert!(
        codes
            .iter()
            .any(|c| c["code"] == "mode_propose" && c["audience"] == "user")
    );

    // The status an SDK reads first: the date, what changed, the switches.
    let (st, _, b) = http_with(p, "GET", "/api/level1/status", "", "");
    assert_eq!(st, 200, "{b}");
    let v = json(&b);
    assert_eq!(v["contractVersion"], operator::refusal::CONTRACT_VERSION);
    assert!(v["recentUpdates"][0]["note"].is_string(), "{b}");
    let (st, _, b) = http_with(
        p,
        "GET",
        &format!("/api/level1/status?policy={policy}"),
        "",
        "",
    );
    assert_eq!(st, 200, "{b}");
    let v = json(&b);
    assert_eq!(
        (v["mode"].clone(), v["paused"].clone(), v["halt"].clone()),
        ("Propose".into(), false.into(), false.into()),
        "{b}"
    );
    let (st, _, b) = http_with(
        p,
        "GET",
        "/api/level1/status?policy=0x00000000000000000000000000000000000000f2",
        "",
        "",
    );
    assert_eq!(
        (st, json(&b)["error"]["code"].clone()),
        (404, "not_found".into()),
        "{b}"
    );
}

/// docs/60 P-248…P-253, P-273 (seat S15): a body over the cap is refused from its head; an idempotent reply
/// goes back only to the caller that sent the first request, and still after a restart; the owner's device
/// stops one agent key at once, and the stop outlives a restart; the admin token counts only in a header.
#[test]
fn the_service_caps_bodies_binds_replays_to_their_caller_and_stops_one_key() {
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-151", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tenants")).unwrap();
    let policy = "0x00000000000000000000000000000000000015a1";
    let dev = "d15a".repeat(16);
    std::fs::write(
        dir.join("tenants").join(format!("{policy}.json")),
        format!(
            r#"{{"chain":"arc-testnet","owner":"0x0000000000000000000000000000000000000001","policy":"{policy}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":true,"reserve_token_hash":"{}","agent_tokens":[{{"id":"t-prep","label":"ops","scopes":["read","prepare"],"expires":4102444800,"issued":0,"hash":"{}"}},{{"id":"t-read","label":"reader","scopes":["read"],"expires":4102444800,"issued":0,"hash":"{}"}}]}}"#,
            operator::level1::token_hash(&dev),
            operator::level1::token_hash("k-prep"),
            operator::level1::token_hash("k-read"),
        ),
    )
    .unwrap();
    let env = [("TAMEION_RPC_ARC", "http://127.0.0.1:9")];
    let srv = start_with(151, &env);
    let p = srv.port;

    // P-248: 2 MiB declared, 413 from the head; the default cap is 1 MiB.
    let big = "x".repeat(2 << 20);
    let (st, _, b) = http_with(p, "POST", "/api/level1/run", &big, "");
    assert_eq!(st, 413, "{b}");
    assert!(b.contains("at most 1048576"), "{b}");

    // P-273: the stored reply is the caller's.
    let body =
        format!(r#"{{"policy":"{policy}","perAction":"0.004","daily":"0.008","floor":"0"}}"#);
    let idem = "Idempotency-Key: s15-op-1\r\n";
    let prep = format!("{idem}Authorization: Bearer k-prep\r\n");
    let (st, h1, first) = http_with(p, "POST", "/api/level1/prepare-limits", &body, &prep);
    assert_eq!(st, 200, "{first}");
    assert!(!h1.contains("Idempotent-Replayed"));
    let (st, h, b) = http_with(p, "POST", "/api/level1/prepare-limits", &body, idem);
    assert_eq!(
        st, 401,
        "no agent key: refused, never the stored reply: {b}"
    );
    assert!(!h.contains("Idempotent-Replayed"));
    let other = format!("{idem}Authorization: Bearer k-read\r\n");
    let (st, _, b) = http_with(p, "POST", "/api/level1/prepare-limits", &body, &other);
    assert_eq!(st, 403, "another key without the scope: {b}");
    let (st, h, b) = http_with(p, "POST", "/api/level1/prepare-limits", &body, &prep);
    assert_eq!(st, 200);
    assert!(h.contains("Idempotent-Replayed: true"), "{h}");
    assert_eq!(b, first);

    // P-251: the owner's device stops t-read; its calls get 403; the inbox lists it.
    let stop = format!(r#"{{"policy":"{policy}","id":"t-read"}}"#);
    let (st, _, b) = http_with(p, "POST", "/api/level1/tokens/stop", &stop, "");
    assert_eq!(st, 401, "{b}");
    let device = format!("X-Tameion-Device: {dev}\r\n");
    let (st, _, b) = http_with(p, "POST", "/api/level1/tokens/stop", &stop, &device);
    assert_eq!(st, 200, "{b}");
    assert!(b.contains(r#""stoppedTokens":["t-read"]"#), "{b}");
    let inbox = format!("/api/level1/inbox?policy={policy}");
    let read = "Authorization: Bearer k-read\r\n";
    let (st, _, b) = http_with(p, "GET", &inbox, "", read);
    assert_eq!(st, 403, "{b}");
    assert!(b.contains("the owner stopped this agent key"), "{b}");

    // P-252: a pass goes only to a caller that may read its account; to anyone else it is the
    // 404 of an id that does not exist. A Watch scan stays open by id.
    std::fs::create_dir_all(dir.join("jobs")).unwrap();
    std::fs::write(
        dir.join("jobs").join("6ad40000-0001.json"),
        format!(
            r#"{{"id":"6ad40000-0001","created":1,"status":"done","request":{{"level1":"run","policy":"{policy}"}},"report":{{"pendingSignature":[]}}}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("jobs").join("6ad40000-0002.json"),
        r#"{"id":"6ad40000-0002","created":1,"status":"done","request":{"chain":"arc-testnet","address":"0x1"}}"#,
    )
    .unwrap();
    let pass = "/api/watch/6ad40000-0001";
    let (st, _, b) = http_with(p, "GET", pass, "", "");
    assert_eq!(st, 404, "no credential: {b}");
    assert!(!b.contains("pendingSignature"), "{b}");
    let (st, _, _) = http_with(p, "GET", pass, "", "Authorization: Bearer not-a-key\r\n");
    assert_eq!(st, 404);
    let (st, _, b) = http_with(p, "GET", pass, "", "Authorization: Bearer k-prep\r\n");
    assert_eq!(st, 200, "a read key: {b}");
    assert!(b.contains("6ad40000-0001"), "{b}");
    let (st, _, b) = http_with(p, "GET", pass, "", &device);
    assert_eq!(st, 200, "the owner's device: {b}");
    let (st, _, _) = http_with(p, "GET", pass, "", read);
    assert_eq!(st, 404, "a stopped key reads nothing");
    let (st, _, b) = http_with(p, "GET", "/api/watch/6ad40000-0002", "", "");
    assert_eq!(st, 200, "a Watch scan stays open by id: {b}");

    // P-253: the admin token from the header only.
    let (st, _, b) = http_with(
        p,
        "GET",
        "/api/level1/tenants",
        "",
        "Authorization: Bearer t0k3n\r\n",
    );
    assert_eq!(st, 200, "{b}");
    let (st, _, _) = http_with(p, "GET", "/api/level1/tenants", "", "");
    assert_eq!(st, 401);
    let (st, _, _) = http_with(p, "GET", "/api/level1/tenants?token=t0k3n", "", "");
    assert_eq!(st, 400);

    // P-249: after a restart the reply is still kept for its caller, and the stop still holds.
    drop(srv);
    let srv = start_with(151, &env);
    let p = srv.port;
    let (st, h, b) = http_with(p, "POST", "/api/level1/prepare-limits", &body, &prep);
    assert_eq!(st, 200, "{b}");
    assert!(h.contains("Idempotent-Replayed: true"), "{h}");
    assert_eq!(b, first);
    let (st, _, _) = http_with(p, "POST", "/api/level1/prepare-limits", &body, idem);
    assert_eq!(st, 401);
    let (st, _, b) = http_with(p, "GET", &inbox, "", read);
    assert_eq!(st, 403, "{b}");
}

/// docs/121: the status lists each settlement rail with the finality it last
/// measured, read per request from `<app_dir>/rails/`; StableFX is framed.
#[test]
fn the_status_lists_each_rail_with_its_last_measured_finality() {
    let srv = start_with(171, &[]);
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-171", std::process::id()));
    let rails =
        |b: &str| serde_json::from_str::<serde_json::Value>(b).unwrap_or_default()["rails"].clone();
    let (st, _, b) = http_with(srv.port, "GET", "/api/level1/status", "", "");
    assert_eq!(st, 200, "{b}");
    let r = rails(&b);
    let ids: Vec<&str> = r
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["arc-native-usdc", "arbitrum-sepolia-usdc", "eurc-stablefx"],
        "{b}"
    );
    assert!(
        r[0]["finalitySecs"].is_null() && r[0]["finality"].is_null(),
        "{b}"
    );
    assert_eq!(
        (r[2]["currency"].as_str(), r[2]["finality"].as_str()),
        (Some("EURC"), Some("not available"))
    );

    std::fs::create_dir_all(dir.join("rails")).unwrap();
    std::fs::write(
        dir.join("rails").join("arc-native-usdc.json"),
        r#"{"id":"arc-native-usdc","currency":"USDC","sentAt":1760700000123,"includedSecs":0.6,"finalitySecs":0.9,"finality":"finalized-tag"}"#,
    )
    .unwrap();
    let (_, _, b) = http_with(srv.port, "GET", "/api/level1/status", "", "");
    let r = rails(&b);
    assert_eq!(
        (
            r[0]["finalitySecs"].as_f64(),
            r[0]["finality"].as_str(),
            r[0]["measuredAt"].as_i64()
        ),
        (Some(0.9), Some("finalized-tag"), Some(1_760_700_000)),
        "{b}"
    );
    assert!(r[1]["finalitySecs"].is_null(), "{b}");
}

/// The read-only symbolon projection: each record with its three slots. The
/// chain is unreachable here, so the witness is absent and its root listed
/// apart; the payee-signed invoice the settlement paid fills the counterparty
/// slot. On a locked account an outsider gets root, seq and leaf only.
#[test]
fn the_symbolon_route_shows_each_records_slots_and_outsiders_only_leaves() {
    use alloy::signers::SignerSync;
    use ledger::{EventBody, log::DecisionLog, money::Money, time::Timestamp};
    let srv = start_with(181, &[("TAMEION_RPC_ARC", "http://127.0.0.1:9")]);
    let p = srv.port;
    let dir = std::env::temp_dir().join(format!("tameion-app-test-{}-181", std::process::id()));
    let a = "0x00000000000000000000000000000000000000e1";
    let policy: alloy::primitives::Address = a.parse().unwrap();
    let log = dir.join("tenants").join(a).join("log");
    std::fs::create_dir_all(&log).unwrap();
    let read = operator::level1::token_hash("k-read");
    let tenant = |tokens: &str| {
        std::fs::write(
            dir.join("tenants").join(format!("{a}.json")),
            format!(
                r#"{{"chain":"arc-testnet","owner":"0x0000000000000000000000000000000000000001","policy":"{a}","agent":"0x0000000000000000000000000000000000000003","operator":"0x0000000000000000000000000000000000000004","watched":"0x0000000000000000000000000000000000000005","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"1","floor":"0","delay_secs":60,"mode":1,"created_block":0,"history_from_block":0,"created":0,"deployed":true,"agent_tokens":[{tokens}]}}"#
            ),
        )
        .unwrap();
    };
    tenant("");
    let vendor = alloy::signers::local::PrivateKeySigner::random();
    let payee = format!("{:#x}", vendor.address());
    let due = 1_792_000_000u64;
    let inv = operator::invoice::Invoice {
        payee: vendor.address(),
        policy,
        amount: 2_000,
        due_at: due,
        every_days: 30,
        discount_bps: 0,
        early_days: 0,
        reference: alloy::primitives::keccak256("INV-1"),
        issued_at: due - 86_400,
        issuer: None,
        alg: None,
        key_hash: None,
    };
    let chain_id = operator::watch::chain("arc-testnet").unwrap().chain_id;
    let s = vendor
        .sign_hash_sync(&operator::invoice::digest(chain_id, &inv))
        .unwrap();
    std::fs::write(
        dir.join("tenants").join(format!("{a}.invoices.jsonl")),
        format!("{}\n", operator::invoice::to_json(&inv, &s.as_bytes())),
    )
    .unwrap();
    let usdc = |s: &str| Money::parse(s, ledger::Currency::Usdc).unwrap();
    let mut pass = DecisionLog::new();
    pass.append(
        Timestamp::from_unix(1_792_100_000),
        EventBody::Observation {
            balance: usdc("1"),
            transfers_seen: 1,
            obligations_found: 1,
            total_due: usdc("0.002"),
            note: "a secret note".into(),
        },
    );
    pass.append(
        Timestamp::from_unix(1_792_100_001),
        EventBody::Settlement {
            counterparty: ledger::Counterparty::new(&payee),
            cycle_key: format!("{payee}@{}", due + 30 * 86_400),
            amount: usdc("0.002"),
            tx_hash: "0xfeed".into(),
            action_hash: ledger::ActionHash::from_bytes([1; 32]),
            settlement_key: ledger::ActionHash::from_bytes([2; 32]),
            note: "paid the secret vendor".into(),
        },
    );
    let file = operator::audit::persist(&log, &pass).unwrap();
    let root = operator::audit::root_from_filename(&file).unwrap();
    let tx = format!("0x{}", "ab".repeat(32));
    operator::audit::append_anchor(
        &log,
        root,
        Some(tx.clone()),
        Some("0x0000000000000000000000000000000000000003".into()),
        "0x0000000000000000000000000000000000000004".into(),
        1_792_100_002,
        2,
    )
    .unwrap();
    let get = |path: &str, who: &str| {
        let (st, _, body) = http_with(p, "GET", path, "", who);
        (
            st,
            serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        )
    };
    let path = format!("/api/level1/symbolon?policy={a}");

    let (st, v) = get(&path, "");
    assert_eq!(st, 200, "{v}");
    assert_eq!(
        (v["ok"].as_bool(), v["locked"].as_bool()),
        (Some(true), Some(false))
    );
    assert_eq!(
        v["counts"],
        serde_json::json!({ "unsigned": 0, "single": 1, "double": 0, "triple": 0 })
    );
    assert_eq!(v["chainReadFailed"], serde_json::json!([root.to_hex()]));
    let r = &v["records"][0];
    assert_eq!(
        v["records"].as_array().unwrap().len(),
        1,
        "an observation is not a record"
    );
    assert_eq!(
        (r["root"].as_str(), r["seq"].as_u64()),
        (Some(root.to_hex().as_str()), Some(1))
    );
    assert_eq!(r["kind"], "SETTLEMENT");
    assert_eq!(
        (r["counterparty"].as_str(), r["minor"].as_str()),
        (Some(payee.as_str()), Some("2000"))
    );
    assert_eq!(r["anchorTx"].as_str(), Some(tx.as_str()));
    assert!(r["agent"].is_null() && r["witness"].is_null(), "{r}");
    assert_eq!(
        r["counterpartySlot"],
        serde_json::json!({ "alg": "secp256k1-eip712", "keyId": payee, "verified": true })
    );
    let leaf = r["leaf"].as_str().unwrap().to_string();
    assert_eq!(leaf, pass.events()[1].leaf().to_hex());

    // One root by name; an unknown root and a bad policy are refused.
    let (st, v) = get(&format!("{path}&root={}", root.to_hex()), "");
    assert_eq!((st, v["records"].as_array().map(Vec::len)), (200, Some(1)));
    let (st, v) = get(&format!("{path}&root=0x{}", "11".repeat(32)), "");
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (404, Some("not_found")),
        "{v}"
    );
    let (st, v) = get("/api/level1/symbolon?policy=nope", "");
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (400, Some("invalid_body")),
        "{v}"
    );

    // Locked: an outsider gets root, seq and leaf; a read key gets the rest.
    tenant(&format!(
        r#"{{"id":"r1","label":"reader","scopes":["read"],"expires":4102444800,"issued":0,"hash":"{read}"}}"#
    ));
    let (st, v) = get(&path, "");
    assert_eq!(st, 200);
    assert_eq!(
        (v["locked"].as_bool(), v["counts"].is_null()),
        (Some(true), true),
        "{v}"
    );
    let mut keys: Vec<&String> = v["records"][0].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["leaf", "root", "seq"], "{v}");
    assert_eq!(v["records"][0]["leaf"].as_str(), Some(leaf.as_str()));
    assert!(!v.to_string().contains("secret") && !v.to_string().contains(&payee));
    let (_, v) = get(&path, "Authorization: Bearer k-read\r\n");
    assert_eq!(v["locked"].as_bool(), Some(false), "{v}");
    assert_eq!(v["records"][0]["counterpartySlot"]["verified"], true);
}
