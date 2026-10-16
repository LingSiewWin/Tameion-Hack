//! The whole step against live testnet prices and the real account, without
//! sending an order. Shows the target, the safety verdict, the order that
//! would go out, and the books after the marks.
//!
//! `HEDGE_SPOT="ETH=0.5,SOL=20"` sets the holdings (default `ETH=0.5`).
//! `TAMEION_TREASURY_ADDR` overrides the account.

use hedge::engine::{Books, Holding, Mode, StepInput, run_once};
use hedge::hl::{Venue, parse_address};
use hedge::policy::HedgePolicy;
use ledger::Currency;
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::str::FromStr;

const DEFAULT_TREASURY: &str = "0x6c1ce01BfE3f834A5Fbfa0a9081d5d976b819A25";

fn parse_holdings(s: &str) -> Result<Vec<Holding>, String> {
    s.split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (coin, size) = p
                .split_once('=')
                .ok_or_else(|| format!("expected COIN=SIZE, got {p:?}"))?;
            Ok(Holding {
                coin: coin.trim().to_uppercase(),
                size: Decimal::from_str(size.trim()).map_err(|e| format!("{p:?}: {e}"))?,
            })
        })
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("TAMEION_TREASURY_ADDR").unwrap_or_else(|_| DEFAULT_TREASURY.into());
    let holdings =
        parse_holdings(&std::env::var("HEDGE_SPOT").unwrap_or_else(|_| "ETH=0.5".into()))?;
    let coins: Vec<&str> = holdings.iter().map(|h| h.coin.as_str()).collect();

    let input = StepInput {
        user: parse_address(&addr)?,
        holdings: holdings.clone(),
        policy: HedgePolicy::default(),
        halt_dir: PathBuf::from("runtime/hedge"),
        cover_usdc: None,
        only_listed: false,
    };
    let venue = Venue::testnet();
    let mut books = Books::new(&coins, 0);

    println!("# hedge dry run — Hyperliquid {:?}", venue.chain());
    println!("user: {}", input.user);
    println!("holdings: {holdings:?}");
    println!("policy: {:?}", input.policy);

    let report = run_once(&venue, &Mode::DryRun, &input, &mut books).await?;

    println!("\n## account");
    println!("{:?}", report.account);
    println!(
        "withdrawable={} health={:?} halted_before={}",
        report.withdrawable, report.health, report.halted_before
    );

    for c in &report.coins {
        println!("\n## {}", c.coin);
        println!(
            "spot_size={} mark={} oracle={} funding/hr={} spot_value={}",
            c.spot_size, c.mark_px, c.oracle_px, c.funding_rate, c.spot_value
        );
        println!(
            "hedge_before={} hedge_upnl={} target={}",
            c.hedge_size_before, c.hedge_upnl, c.target_size
        );
        println!("rebalance: {:?}", c.rebalance);
        println!("verdict:   {:?}", c.verdict);
        println!("action:    {:?}", c.action);
        println!("books net: {:?}", c.net);
    }

    println!(
        "\n## journal ({} entries, {} funding)",
        books.journal.len(),
        report.funding_entries_booked
    );
    for e in books.journal.entries() {
        print!("{e}");
    }
    let (d, cr) = books.journal.trial_balance(Currency::Usdc)?;
    println!(
        "trial balance: debits {d} credits {cr} {}",
        if d == cr { "BALANCED" } else { "UNBALANCED" }
    );
    Ok(())
}
