//! The service's `/mcp` (Streamable HTTP, MCP 2025-06-18) serves the same tool
//! table as `tameion-mcp`, its tool calls reach the service's own routes, and
//! `POST /api/level1/pay` refuses by code. Through the real binary on a free
//! port; no RPC is called.

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
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
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let dir = std::env::temp_dir().join(format!("tameion-mcp-http-{}", std::process::id()));
    let child = Command::new(env!("CARGO_BIN_EXE_tameion-app"))
        .env("PORT", port.to_string())
        .env("TAMEION_APP_DIR", &dir)
        .env_remove("HOST")
        .env_remove("TAMEION_PUBLIC_URL")
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

/// One request; the status, the head and the body.
fn http(port: u16, method: &str, path: &str, extra: &str, body: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    (status, head.to_string(), body.to_string())
}

const ACCEPT: &str = "Accept: application/json, text/event-stream\r\n";

fn rpc(port: u16, extra: &str, msg: Value) -> (u16, String, Value) {
    let (st, head, body) = http(
        port,
        "POST",
        "/mcp",
        &format!("{ACCEPT}{extra}"),
        &msg.to_string(),
    );
    (st, head, serde_json::from_str(&body).unwrap_or(Value::Null))
}

#[test]
fn the_service_serves_the_mcp_tools_over_streamable_http() {
    let srv = start();
    let p = srv.port;
    let (st, head, init) = rpc(
        p,
        "",
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } } }),
    );
    assert_eq!(st, 200, "{init}");
    assert!(head.contains("Content-Type: application/json"), "{head}");
    assert!(
        !head.to_ascii_lowercase().contains("mcp-session-id"),
        "no session is issued"
    );
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        init["result"]["serverInfo"]["version"],
        operator::refusal::CONTRACT_VERSION
    );

    // A notification is accepted with 202 and no body.
    let (st, _, body) = http(
        p,
        "POST",
        "/mcp",
        ACCEPT,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    assert_eq!((st, body.as_str()), (202, ""));

    // The one table both transports serve.
    let v = "MCP-Protocol-Version: 2025-06-18\r\n";
    let (st, _, list) = rpc(
        p,
        v,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    assert_eq!(st, 200);
    assert_eq!(list["result"]["tools"], json!(operator::mcp::tools()));
    assert!(
        list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "pay"),
        "pay is offered"
    );

    // A tool call goes to this service's own route, which answers in the envelope.
    let (st, _, call) = rpc(
        p,
        v,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "treasury_inbox", "arguments": { "policy": "0x00000000000000000000000000000000000000c4" } } }),
    );
    assert_eq!(st, 200);
    assert_eq!(call["result"]["isError"], true, "{call}");
    let error: Value =
        serde_json::from_str(call["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["code"], "not_found");
    assert_eq!(call["result"]["structuredContent"]["error"], error);

    // pay over MCP: refused by the route, by its code.
    let (_, _, paid) = rpc(
        p,
        v,
        json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "pay", "arguments": { "policy": "0x00000000000000000000000000000000000000c4", "payee": "0x00000000000000000000000000000000000000c5", "amount": "0.001", "why": "test" } } }),
    );
    assert_eq!(paid["result"]["isError"], true);
    assert_eq!(
        paid["result"]["structuredContent"]["error"]["code"], "invalid_body",
        "no operationId, no payment: {paid}"
    );

    // What the transport refuses.
    let (st, _, bad) = rpc(
        p,
        "MCP-Protocol-Version: 1999-01-01\r\n",
        json!({ "jsonrpc": "2.0", "id": 5, "method": "ping" }),
    );
    assert_eq!(st, 400, "{bad}");
    let (st, _, _) = rpc(
        p,
        "Origin: https://evil.example\r\n",
        json!({ "jsonrpc": "2.0", "id": 6, "method": "ping" }),
    );
    assert_eq!(st, 403, "a foreign browser origin (DNS rebinding)");
    let (st, _, ping) = rpc(
        p,
        "Origin: http://localhost:3000\r\n",
        json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" }),
    );
    assert_eq!((st, &ping["result"]), (200, &json!({})));
    let (st, _, _) = rpc(
        p,
        "",
        json!([{ "jsonrpc": "2.0", "id": 8, "method": "ping" }]),
    );
    assert_eq!(st, 400, "no batches in 2025-06-18");
    let (st, head, _) = http(p, "GET", "/mcp", "Accept: text/event-stream\r\n", "");
    assert_eq!(st, 405, "no server stream");
    assert!(head.contains("Allow: POST"), "{head}");
}

#[test]
fn pay_refuses_by_code_before_anything_is_sent() {
    let srv = start();
    let p = srv.port;
    let body = json!({ "policy": "0x00000000000000000000000000000000000000c4", "payee": "0x00000000000000000000000000000000000000c5", "amount": "0.001", "why": "rent" }).to_string();
    let code = |st: u16, b: &str| {
        let v: Value = serde_json::from_str(b).unwrap();
        assert_eq!(v["ok"], false, "{b}");
        assert_eq!(v["error"]["executed"], "no", "{b}");
        (st, v["error"]["code"].as_str().unwrap().to_string())
    };
    let (st, _, b) = http(p, "POST", "/api/level1/pay", "", &body);
    assert_eq!(
        code(st, &b),
        (400, "invalid_body".into()),
        "no Idempotency-Key"
    );
    let no_why = body.replace("\"rent\"", "\"\"");
    let (st, _, b) = http(
        p,
        "POST",
        "/api/level1/pay",
        "Idempotency-Key: op_test-0001\r\n",
        &no_why,
    );
    assert_eq!(code(st, &b), (400, "invalid_body".into()), "no why");
    let (st, _, b) = http(
        p,
        "POST",
        "/api/level1/pay",
        "Idempotency-Key: op_test-0002\r\n",
        &body,
    );
    assert_eq!(code(st, &b), (404, "not_found".into()));
}
