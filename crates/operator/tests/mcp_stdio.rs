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

    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":4,"method":"nope"}}"#).unwrap();
    line.clear();
    out.read_line(&mut line).unwrap();
    let nope: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(nope["error"]["code"], -32601);

    drop(stdin);
    let _ = child.wait();
}
