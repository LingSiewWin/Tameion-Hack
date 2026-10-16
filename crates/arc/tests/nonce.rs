//! One key, many clients, at once: the hosted service builds a fresh client
//! per operation and signs with one relayer key for every tenant (gas
//! top-ups, deploys, mints, keyless anchors), up to three passes at a time.
//! Every send must land; none may reuse another's nonce.

use alloy::primitives::{Address, U256};
use std::process::{Child, Command, Stdio};

/// Anvil's published test mnemonic; its first key is derived when the test runs
/// (no private key is written into the repository).
const ANVIL_MNEMONIC: &str = "test test test test test test test test test test test junk";

fn anvil_key() -> Option<String> {
    let out = Command::new("cast")
        .args([
            "wallet",
            "private-key",
            "--mnemonic",
            ANVIL_MNEMONIC,
            "--mnemonic-index",
            "0",
        ])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

struct Anvil(Child);

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn anvil(port: u16) -> Option<Anvil> {
    let child = Command::new("anvil")
        .args(["--port", &port.to_string(), "--silent"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some(Anvil(child));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_sends_from_one_key_on_separate_clients_all_land() {
    let port = 20_000 + (std::process::id() % 10_000) as u16;
    let Some(_node) = anvil(port) else {
        eprintln!("anvil not found; skipped");
        return;
    };
    let Some(key) = anvil_key() else {
        eprintln!("cast not found; skipped");
        return;
    };
    let url = format!("http://127.0.0.1:{port}");
    // Spawned before any is awaited (a lazy `map` would run them in turn).
    let sends: Vec<_> = (1..=16u8)
        .map(|i| {
            let url = url.clone();
            let key = key.clone();
            tokio::spawn(async move {
                let client = arc::ArcClient::with_signer(&url, &key).expect("client");
                client
                    .send_native(Address::repeat_byte(i), U256::from(1_000u64))
                    .await
            })
        })
        .collect();
    let mut failed = Vec::new();
    for s in sends {
        if let Err(e) = s.await.expect("task") {
            failed.push(e.to_string());
        }
    }
    assert!(
        failed.is_empty(),
        "{} of 16 sends failed: {failed:?}",
        failed.len()
    );
}
