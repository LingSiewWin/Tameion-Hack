//! Run the level-0 watch once from the terminal, printing each phase.
//!   WATCH_CHAIN=base WATCH_ADDRESS=0x... WATCH_DAYS=30 cargo run --locked -q -p operator --example watch_once
use operator::watch::{WatchParams, chain, watch};
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let key = std::env::var("WATCH_CHAIN").unwrap_or_else(|_| "arc-testnet".into());
    let preset = chain(&key).ok_or("unknown WATCH_CHAIN")?;
    let address = alloy::primitives::Address::from_str(&std::env::var("WATCH_ADDRESS")?)?;
    let days: u32 = std::env::var("WATCH_DAYS")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(30);
    // WATCH_ETH=1: only read the ETH held on each mainnet preset (docs/50 #2).
    if std::env::var("WATCH_ETH").is_ok_and(|v| v == "1") {
        for (chain, r) in operator::watch::eth_holdings(address).await {
            match r {
                Ok(eth) => println!("{chain:<12} {eth} ETH"),
                Err(e) => println!("{chain:<12} unreadable: {e}"),
            }
        }
        return Ok(());
    }
    let t0 = std::time::Instant::now();
    let report = watch(
        &WatchParams {
            chain: preset,
            address,
            also_history: vec![],
            days,
            caps: None,
        },
        |p| {
            eprintln!(
                "[{:>6.1}s] {} ({}/{} blocks, {} transfers)",
                t0.elapsed().as_secs_f64(),
                p.phase,
                p.scanned_blocks,
                p.total_blocks,
                p.transfers_so_far
            )
        },
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
