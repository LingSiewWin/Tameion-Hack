//! The client retries Arc's intermittent "-32601 method not supported", and
//! nothing it should not.
//!
//! A local JSON-RPC server stands in for the endpoint: it answers
//! `eth_blockNumber` with a scripted sequence of errors before the real
//! answer, and counts what it was asked.

use arc::ArcClient;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serve `failures` copies of the error `code`, then block 0x2a, forever.
async fn endpoint(code: i64, failures: usize) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let counter = counter.clone();
            tokio::spawn(async move {
                // One request per connection is enough for this client; read
                // headers, then exactly Content-Length bytes of body.
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
                let req: serde_json::Value = serde_json::from_str(&body).unwrap();
                let id = req["id"].clone();
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let reply = if n < failures {
                    serde_json::json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": code, "message": "method not supported"}})
                } else {
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": "0x2a"})
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
    (url, asked)
}

#[tokio::test]
async fn method_not_supported_is_retried_until_the_answer_arrives() {
    let (url, asked) = endpoint(-32601, 2).await;
    let client = ArcClient::read_only(&url).unwrap();
    assert_eq!(client.block_number().await.unwrap(), 42);
    assert_eq!(
        asked.load(Ordering::SeqCst),
        3,
        "two refusals, then the answer"
    );
}

#[tokio::test]
async fn other_errors_are_not_retried() {
    // -32000 is a generic server error: the node may have done something.
    // Repeating it blindly is how one payment becomes two, so it surfaces.
    let (url, asked) = endpoint(-32000, 1).await;
    let client = ArcClient::read_only(&url).unwrap();
    assert!(client.block_number().await.is_err());
    assert_eq!(asked.load(Ordering::SeqCst), 1, "asked once, not repeated");
}
