//! Exercises the real HTTP path of [`ClaudeReasoner`] against a local server.
//!
//! The unit tests in `claude.rs` cover the pure parts: they prove the parser
//! and the population guard behave, but they never build a request or read a
//! socket. This file does, so the things only a transport can get wrong --
//! the auth header, the API version header, the JSON body actually sent --
//! are checked against bytes a server received rather than against a struct.
//!
//! It stubs the transport, not the judgment. What it cannot tell you is
//! whether Claude returns a sensible ordering; only a real key can.

use std::sync::{Arc, Mutex};

use ledger::{
    Confidence, Counterparty, Currency, Money, Obligation, ObligationKind, Seconds, Timestamp,
};
use operator::claude::{Auth, ClaudeReasoner};
use operator::reasoner::{Reasoner, ReasonerError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const NOW: Timestamp = Timestamp::from_unix(1_700_000_000);

fn obligation(who: &str, due_in_days: i64, amount: i128) -> Obligation {
    let interval = Seconds::days(30).unwrap();
    let next_due = NOW
        .checked_add(Seconds::days(due_in_days).unwrap())
        .unwrap();
    Obligation {
        counterparty: Counterparty::new(who),
        kind: ObligationKind::Subscription,
        typical_amount: Money::minor(amount, Currency::Usdc),
        interval,
        last_paid: next_due.checked_sub(interval).unwrap(),
        next_due,
        confidence: Confidence::new(0.9).unwrap(),
        sample_count: 4,
        interval_cv: 0.01,
        amount_cv: 0.0,
        evidence: vec!["0xaa".into()],
    }
}

/// Serve exactly one request, hand back `status`/`body`, and record what the
/// client sent.
async fn stub(status: u16, body: String) -> (String, Arc<Mutex<Option<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);

    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
        // Read until the body is complete: headers, then Content-Length bytes.
        loop {
            let n = sock.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            req.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&req).to_string();
            if let Some(head_end) = text.find("\r\n\r\n") {
                let len = text
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if text.len() - (head_end + 4) >= len {
                    break;
                }
            }
        }
        *sink.lock().unwrap() = Some(String::from_utf8_lossy(&req).to_string());

        let reason = if status == 200 { "OK" } else { "Error" };
        let resp = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        sock.write_all(resp.as_bytes()).await.unwrap();
        sock.flush().await.unwrap();
    });

    (url, seen)
}

fn ok_body(keys: &[&str]) -> String {
    let ordering: Vec<_> = keys
        .iter()
        .map(|k| serde_json::json!({ "cycle_key": k, "rationale": "because" }))
        .collect();
    let inner = serde_json::json!({ "ordering": ordering }).to_string();
    serde_json::json!({
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": inner }]
    })
    .to_string()
}

#[tokio::test]
async fn sends_the_documented_request_and_applies_the_reply() {
    let obs = vec![
        obligation("0xaaa", -2, 1_000_000),
        obligation("0xbbb", 4, 2_000_000),
    ];
    let keys: Vec<String> = obs.iter().map(|o| o.cycle_key()).collect();

    // Reply flips the order relative to urgency.
    let (url, seen) = stub(200, ok_body(&[keys[1].as_str(), keys[0].as_str()])).await;

    let got = ClaudeReasoner::new(Auth::ApiKey("sk-ant-test-key".into()))
        .with_url(url)
        .prioritise(&obs, Money::minor(3_000_000, Currency::Usdc), NOW)
        .await
        .unwrap();

    // The reply was applied, in the order the model gave.
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].obligation.counterparty.as_str(), "0xbbb");
    assert_eq!(got[1].obligation.counterparty.as_str(), "0xaaa");

    // And the wire looked the way the API documents.
    let raw = seen.lock().unwrap().clone().unwrap();
    let lower = raw.to_ascii_lowercase();
    assert!(
        lower.contains("x-api-key: sk-ant-test-key"),
        "auth header missing"
    );
    assert!(
        lower.contains("anthropic-version: 2023-06-01"),
        "version header missing"
    );
    assert!(
        !lower.contains("authorization:"),
        "bearer must not be sent with an api key"
    );

    let body: serde_json::Value =
        serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["model"], "claude-opus-5");
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    assert!(body.get("temperature").is_none());
    // The obligations really are in the prompt.
    let user = body["messages"][0]["content"].as_str().unwrap();
    assert!(user.contains("0xaaa") && user.contains("0xbbb"));
}

#[tokio::test]
async fn bearer_auth_adds_the_oauth_beta_header() {
    let obs = vec![
        obligation("0xaaa", -2, 1_000_000),
        obligation("0xbbb", 4, 2_000_000),
    ];
    let keys: Vec<String> = obs.iter().map(|o| o.cycle_key()).collect();
    let (url, seen) = stub(200, ok_body(&[keys[0].as_str(), keys[1].as_str()])).await;

    ClaudeReasoner::new(Auth::Bearer("oat-token".into()))
        .with_url(url)
        .prioritise(&obs, Money::minor(3_000_000, Currency::Usdc), NOW)
        .await
        .unwrap();

    let raw = seen.lock().unwrap().clone().unwrap().to_ascii_lowercase();
    assert!(raw.contains("authorization: bearer oat-token"));
    // Without this header an OAuth token is rejected by /v1/messages.
    assert!(raw.contains("anthropic-beta: oauth-2025-04-20"));
    assert!(!raw.contains("x-api-key"));
}

#[tokio::test]
async fn an_http_error_surfaces_the_api_explanation() {
    let obs = vec![
        obligation("0xaaa", -2, 1_000_000),
        obligation("0xbbb", 4, 2_000_000),
    ];
    let body = serde_json::json!({
        "type": "error",
        "error": { "type": "authentication_error", "message": "invalid x-api-key" }
    })
    .to_string();
    let (url, _seen) = stub(401, body).await;

    let err = ClaudeReasoner::new(Auth::ApiKey("bad".into()))
        .with_url(url)
        .prioritise(&obs, Money::minor(3_000_000, Currency::Usdc), NOW)
        .await
        .unwrap_err();

    match err {
        // 401 is a configuration problem, not a blip: retrying the identical
        // request cannot succeed, so it must classify as permanent.
        ReasonerError::Permanent(ref m) => {
            assert!(m.contains("401"), "{m}");
            assert!(m.contains("invalid x-api-key"), "{m}");
            assert!(!err.retryable());
            assert_eq!(err.code(), "reasoner-permanent");
        }
        other => panic!("expected Permanent, got {other:?}"),
    }
}

#[tokio::test]
async fn an_invented_payee_over_the_wire_is_still_refused() {
    // Same guard as the unit test, but reached through the real transport --
    // proving nothing in the HTTP path bypasses it.
    let obs = vec![
        obligation("0xaaa", -2, 1_000_000),
        obligation("0xbbb", 4, 2_000_000),
    ];
    let keys: Vec<String> = obs.iter().map(|o| o.cycle_key()).collect();
    let (url, _seen) = stub(200, ok_body(&["0xattacker@1700000000", keys[0].as_str()])).await;

    let err = ClaudeReasoner::new(Auth::ApiKey("k".into()))
        .with_url(url)
        .prioritise(&obs, Money::minor(3_000_000, Currency::Usdc), NOW)
        .await
        .unwrap_err();
    assert!(matches!(err, ReasonerError::WrongPopulation));
}
