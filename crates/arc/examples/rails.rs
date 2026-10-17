//! Measure each settlement rail's finality with one real, tiny self-transfer
//! (docs/117 §7, docs/121): 0.000001 USDC native on Arc testnet and 0.000001
//! USDC (ERC-20) on Arbitrum Sepolia, both at once, signed by the relayer key.
//! Prints the three times per rail and writes `<out_dir>/<rail id>.json`,
//! which `GET /api/level1/status` serves as `rails` from `<app_dir>/rails/`.
//!
//!   TAMEION_RELAYER_KEY="$(cat ~/.tameion/treasury.key)" \
//!     cargo run -p arc --example rails -- <app_dir>/rails
//!
//! `TAMEION_RPC_ARC` / `TAMEION_RPC_ARBITRUM_SEPOLIA` override the endpoints.
use arc::rail::{ArcNativeUsdc, Erc20Usdc, Settled, SettlementRail, Transfer};
use arc::{ARC_TESTNET_RPC, ArcClient};
use ledger::{Currency, Money};
use serde_json::json;

const ARB_SEPOLIA_RPC: &str = "https://sepolia-rollup.arbitrum.io/rpc";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).ok_or("usage: rails <out_dir>")?);
    let key = std::env::var("TAMEION_RELAYER_KEY").map_err(|_| "TAMEION_RELAYER_KEY is not set")?;
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let arc_rpc = env("TAMEION_RPC_ARC", ARC_TESTNET_RPC);
    let arb_rpc = env("TAMEION_RPC_ARBITRUM_SEPOLIA", ARB_SEPOLIA_RPC);
    let arc = ArcClient::with_signer(&arc_rpc, &key)?;
    let arb = ArcClient::with_signer(&arb_rpc, &key)?.with_token(Erc20Usdc::ARBITRUM_SEPOLIA.token);
    drop(key);
    let me = arc.address().ok_or("no signer")?;
    println!("relayer {me:#x}");
    println!(
        "before: Arc native {}; Arbitrum Sepolia USDC {}, ETH wei {}",
        arc.native_balance(me).await?,
        arb.usdc_balance(me).await?,
        arb.native_wei(me).await?
    );

    let one = Money::minor(1, Currency::Usdc);
    let to_self = || Transfer {
        to: me,
        amount: one,
        policy: None,
    };
    let native = ArcNativeUsdc::TESTNET;
    let erc20 = Erc20Usdc::ARBITRUM_SEPOLIA;
    let (a, b) = tokio::join!(
        native.settle(to_self(), &arc),
        erc20.settle(to_self(), &arb)
    );
    std::fs::create_dir_all(&out)?;
    for (rail, explorer, chain, r) in [
        (
            native.id(),
            "https://explorer.testnet.arc.io/tx/",
            5_042_002u64,
            a,
        ),
        (erc20.id(), "https://sepolia.arbiscan.io/tx/", 421_614, b),
    ] {
        match r {
            Ok(s) => {
                print_settled(rail, explorer, &s);
                let v = record(rail, chain, explorer, &s);
                std::fs::write(out.join(format!("{rail}.json")), format!("{v:#}\n"))?;
            }
            Err(e) => println!("{rail}: refused: {e}"),
        }
    }
    println!(
        "after: Arc native {}; Arbitrum Sepolia USDC {}, ETH wei {}",
        arc.native_balance(me).await?,
        arb.usdc_balance(me).await?,
        arb.native_wei(me).await?
    );
    Ok(())
}

fn print_settled(rail: &str, explorer: &str, s: &Settled) {
    println!("{rail}:");
    println!("  tx            {explorer}{:#x}", s.tx);
    println!("  block         {}", s.block);
    println!("  sent_at       {} ms", s.sent_at);
    println!(
        "  included_at   {} ms  (+{} ms)",
        s.included_at,
        s.included_at - s.sent_at
    );
    match s.confirmed_at {
        Some(c) => println!("  confirmed_at  {c} ms  (+{} ms)", c - s.sent_at),
        None => println!(
            "  confirmed_at  none within {} s",
            arc::rail::FINALITY_CAP.as_secs()
        ),
    }
    println!("  finality      {}", s.finality.label());
    println!("  finalized tag last seen at block {:?}", s.finalized_seen);
}

fn record(rail: &str, chain: u64, explorer: &str, s: &Settled) -> serde_json::Value {
    json!({
        "id": rail,
        "currency": "USDC",
        "chainId": chain,
        "tx": format!("{:#x}", s.tx),
        "explorer": format!("{explorer}{:#x}", s.tx),
        "block": s.block,
        "sentAt": s.sent_at,
        "includedAt": s.included_at,
        "confirmedAt": s.confirmed_at,
        "finality": s.finality.label(),
        "finalizedSeen": s.finalized_seen,
        "includedSecs": (s.included_at - s.sent_at) as f64 / 1000.0,
        "finalitySecs": s.finality_secs(),
        "capSecs": arc::rail::FINALITY_CAP.as_secs(),
    })
}
