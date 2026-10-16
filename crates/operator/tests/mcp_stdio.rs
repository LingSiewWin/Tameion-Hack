//! The MCP server answers `initialize` and `tools/list` over stdio with no
//! service behind it, and every tool says what it cannot do.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn the_mcp_server_lists_its_tools_and_says_which_one_spends() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tameion-mcp"))
        .env("TAMEION_URL", "http://127.0.0.1:9")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tameion-mcp");
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();

    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"test","version":"0"}}}}}}"#).unwrap();
    out.read_line(&mut line).unwrap();
    let init: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "tameion");
    // The instructions say what no tool can do, and name the one that spends
    // (corrected 2026-10-15, docs/60 P-75: "none can sign" was not true of
    // buy_service).
    let instructions = init["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("none of them can add a payee, raise a cap or withdraw"));
    assert!(
        instructions
            .contains("buy_service has the service pay a seller the owner already approved")
    );

    // A notification gets no reply; the next reply must be for id 2.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
    line.clear();
    out.read_line(&mut line).unwrap();
    let list: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(list["id"], 2);
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    // UAT U3 (docs/60 P-180): the gold desk was dropped; its tools are not offered.
    for gone in [
        "desk",
        "send_milestone",
        "desk_statement",
        "desk_statements_all",
        "stop_counterparty",
    ] {
        assert!(!names.contains(&gone), "{gone} is listed: {names:?}");
    }
    let tool = |n: &str| {
        list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == n)
            .unwrap()["description"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // The agent is told where everything waiting for the owner is (P-180),
    // what it may still spend (P-181) and that a prepared call says who signs
    // and when it applies (P-182).
    let inbox = tool("treasury_inbox");
    assert!(
        inbox.contains("`waiting`") && inbox.contains("spent24h") && inbox.contains("leftNow"),
        "{inbox}"
    );
    for p in ["prepare_limits", "prepare_mode", "prepare_payees"] {
        let d = tool(p);
        assert!(
            d.contains("`signer`") && d.contains("`applies`") && d.contains("reason"),
            "{p}: {d}"
        );
    }
    for t in list["result"]["tools"].as_array().unwrap() {
        let d = t["description"].as_str().unwrap();
        assert!(
            !d.contains("PLAN 5.") && !d.contains("docs/"),
            "internal references in {}: {d}",
            t["name"]
        );
    }
    for want in [
        "treasury_inbox",
        "run_pass",
        "pass_status",
        "prepare_payees",
        "prepare_limits",
        "prepare_mode",
        "import_bills",
        "records",
        "record_events",
        "watch_address",
        "invoices",
        "send_invoice",
        "purchases",
    ] {
        assert!(names.contains(&want), "missing tool {want}: {names:?}");
    }
    // Receipt v1 (docs/76 §6): the purchases tool only reads, and says what a
    // receipt carries.
    let purchases = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "purchases")
        .unwrap();
    assert_eq!(purchases["annotations"]["readOnlyHint"], true);
    assert_eq!(purchases["annotations"]["destructiveHint"], false);
    let d = purchases["description"].as_str().unwrap();
    assert!(d.contains("payment-identifier") && d.contains("sha256") && d.contains("receipt"));
    for t in list["result"]["tools"].as_array().unwrap() {
        let d = t["description"].as_str().unwrap();
        if t["name"].as_str().unwrap().starts_with("prepare_") {
            assert!(
                d.contains("Cannot sign") || d.contains("cannot sign"),
                "{d}"
            );
        }
        assert!(t["inputSchema"]["required"].is_array());
    }

    // A call with no service behind it is an error result, not a crash.
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"treasury_inbox","arguments":{{"policy":"0x0000000000000000000000000000000000000001"}}}}}}"#).unwrap();
    line.clear();
    out.read_line(&mut line).unwrap();
    let call: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(call["id"], 3);
    assert_eq!(call["result"]["isError"], true);
    // P-184: in words an agent can act on, not "error sending request".
    let words = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        words.contains("is not reachable") && words.contains("TAMEION_URL"),
        "{words}"
    );

    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":4,"method":"nope"}}"#).unwrap();
    line.clear();
    out.read_line(&mut line).unwrap();
    let nope: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(nope["error"]["code"], -32601);

    drop(stdin);
    let _ = child.wait();
}

/// docs/60 P-184: a service that takes the call and never answers is reported
/// in words an agent can act on ("the call may still complete; read the inbox
/// before retrying"), not as "error sending request".
#[test]
fn a_call_with_no_answer_says_what_to_do() {
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = silent.local_addr().unwrap().port();
    // Accept and hold the connection without answering.
    let holder = std::thread::spawn(move || {
        let (s, _) = silent.accept().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(4));
        drop(s);
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_tameion-mcp"))
        .env("TAMEION_URL", format!("http://127.0.0.1:{port}"))
        .env("TAMEION_MCP_TIMEOUT_SECS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tameion-mcp");
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"prepare_payees","arguments":{{"policy":"0x0000000000000000000000000000000000000001","payees":["0x0000000000000000000000000000000000000002"]}}}}}}"#).unwrap();
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["result"]["isError"], true);
    let words = v["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        words.contains("no answer from")
            && words.contains("may still complete")
            && words.contains("treasury_inbox"),
        "{words}"
    );
    drop(stdin);
    let _ = child.wait();
    let _ = holder.join();
}
