//! A rail settles over the existing send path and measures the chain's
//! finality (docs/117 §7): on anvil with two-block epochs, `finalized` trails
//! the head by two blocks, so the wait is real and short.

use alloy::primitives::Address;
use arc::rail::{ArcNativeUsdc, EurcStableFx, FinalityKind, RailError, SettlementRail, Transfer};
use ledger::{Currency, Money};
use std::process::{Child, Command, Stdio};

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
        .args([
            "--port",
            &port.to_string(),
            "--silent",
            "--block-time",
            "1",
            "--slots-in-an-epoch",
            "1",
        ])
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
async fn native_rail_waits_for_the_finalized_tag() {
    let port = 30_000 + (std::process::id() % 10_000) as u16;
    let Some(_node) = anvil(port) else {
        eprintln!("anvil not found; skipped");
        return;
    };
    let Some(key) = anvil_key() else {
        eprintln!("cast not found; skipped");
        return;
    };
    let url = format!("http://127.0.0.1:{port}");
    let signer = arc::ArcClient::with_signer(&url, &key).expect("client");
    let rail = ArcNativeUsdc { chain_id: 31_337 };
    let to = Address::repeat_byte(7);
    let s = rail
        .settle(
            Transfer {
                to,
                amount: Money::minor(1, Currency::Usdc),
                policy: None,
            },
            &signer,
        )
        .await
        .expect("settled");
    assert_eq!(s.finality, FinalityKind::ChainFinalTag);
    let confirmed = s.confirmed_at.expect("finalized within the cap");
    assert!(
        s.sent_at <= s.included_at && s.included_at <= confirmed,
        "{s:?}"
    );
    assert!(s.finalized_seen.is_some_and(|f| f >= s.block), "{s:?}");
    // One block of interval mining is a second; two more finalize it.
    assert!(confirmed - s.included_at >= 1_000, "{s:?}");
    assert_eq!(
        signer.native_balance(to).await.unwrap(),
        Money::minor(1, Currency::Usdc)
    );

    // The wrong chain sends nothing: a native rail elsewhere would move that chain's gas token.
    let elsewhere = ArcNativeUsdc::TESTNET
        .settle(
            Transfer {
                to,
                amount: Money::minor(1, Currency::Usdc),
                policy: None,
            },
            &signer,
        )
        .await;
    assert!(matches!(
        elsewhere,
        Err(RailError::Chain(arc::ArcError::WrongChain { .. }))
    ));
    // Nor does the wrong currency.
    let eur = rail
        .settle(
            Transfer {
                to,
                amount: Money::minor(1, Currency::Eurc),
                policy: None,
            },
            &signer,
        )
        .await;
    assert!(matches!(eur, Err(RailError::WrongCurrency { .. })));
    // StableFX is framed: refused before anything is signed.
    let fx = EurcStableFx
        .settle(
            Transfer {
                to,
                amount: Money::minor(1, Currency::Eurc),
                policy: None,
            },
            &signer,
        )
        .await;
    assert!(matches!(fx, Err(RailError::NotAvailable(m)) if m.contains("StableFX")));
    assert_eq!(
        signer.native_balance(to).await.unwrap(),
        Money::minor(1, Currency::Usdc)
    );
}
