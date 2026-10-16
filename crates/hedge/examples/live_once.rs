//! One live step on Hyperliquid **testnet**: place the hedge, read it back,
//! book it. Refuses to run without an API-wallet key file and a funded
//! account. Never uses the treasury key.
//!
//! Env:
//! - `HEDGE_AGENT_KEY_FILE`  path to the hex private key of the approved API
//!   wallet (default `~/.tameion/hedge-agent.key`, mode 600).
//! - `TAMEION_TREASURY_ADDR` the master account the agent trades for.
//! - `HEDGE_SPOT`            holdings, e.g. `ETH=0.01` (default).
//! - `HEDGE_HALT_DIR`        where `HALT`, the books and the pass record live
//!   (default `runtime/hedge`, as in `hedge::driver`).
//!
//! Run through `scripts/prove_hedge.sh live` so the output lands in `proof/`.

use hedge::engine::{Action, Books, Holding, Mode, StepInput, run_once};
use hedge::hl::{Venue, VenueApi, parse_address, parse_signer};
use hedge::policy::HedgePolicy;
use ledger::Currency;
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::str::FromStr;

const DEFAULT_TREASURY: &str = "0x6c1ce01BfE3f834A5Fbfa0a9081d5d976b819A25";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let key_file = std::env::var("HEDGE_AGENT_KEY_FILE").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/.tameion/hedge-agent.key")
    });
    let key = match std::fs::read_to_string(&key_file) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("refusing: no API wallet key at {key_file}: {e}");
            eprintln!(
                "approve an API wallet for the treasury on testnet first (see docs/43 §8), save its key there, mode 600."
            );
            std::process::exit(2);
        }
    };
    let signer = parse_signer(&key)?;
    drop(key);

    let addr = std::env::var("TAMEION_TREASURY_ADDR").unwrap_or_else(|_| DEFAULT_TREASURY.into());
    let spot = std::env::var("HEDGE_SPOT").unwrap_or_else(|_| "ETH=0.01".into());
    let holdings: Vec<Holding> = spot
        .split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (c, s) = p.split_once('=').expect("COIN=SIZE");
            Holding {
                coin: c.trim().to_uppercase(),
                size: Decimal::from_str(s.trim()).expect("decimal size"),
            }
        })
        .collect();
    let coins: Vec<&str> = holdings.iter().map(|h| h.coin.as_str()).collect();

    let venue = Venue::testnet();
    let input = StepInput {
        user: parse_address(&addr)?,
        holdings: holdings.clone(),
        policy: HedgePolicy::default(),
        halt_dir: PathBuf::from(
            std::env::var("HEDGE_HALT_DIR").unwrap_or_else(|_| "runtime/hedge".into()),
        ),
        cover_usdc: hedge::driver::cover_from_env()?,
        only_listed: false,
    };
    let snap = venue.account(input.user).await?;
    if snap.state.account_value.is_zero() {
        eprintln!(
            "refusing: account {} has 0 value on testnet; fund it at https://app.hyperliquid-testnet.xyz/faucet",
            input.user
        );
        std::process::exit(3);
    }

    println!("# hedge LIVE step — Hyperliquid {:?}", venue.chain());
    println!("user: {}  agent: {}", input.user, signer.address());
    println!("holdings: {holdings:?}");

    // Real runs keep their books: the journal and marks persist next to HALT.
    // One set of books per venue account (docs/46, 2026-10-13); HALT stays at the root.
    let books_dir = hedge::driver::books_dir(&input.halt_dir, input.user);
    let mut books = match hedge::store::load_or_new(&books_dir, &coins)? {
        Some(b) => b,
        None => Books::new(&coins, snap.time_ms.saturating_sub(24 * 3600 * 1000)),
    };
    let report = run_once(&venue, &Mode::Live(signer), &input, &mut books).await?;
    let appended = hedge::store::save(&books_dir, &mut books)?;
    println!(
        "books: {appended} new entries written to {}",
        books_dir.display()
    );

    println!("\n## account after");
    println!("{:?} health={:?}", report.account, report.health);
    let mut halted = false;
    for c in &report.coins {
        println!("\n## {}", c.coin);
        println!(
            "mark={} target={} before={} rebalance={:?}",
            c.mark_px, c.target_size, c.hedge_size_before, c.rebalance
        );
        println!("verdict: {:?}", c.verdict);
        match &c.action {
            Action::Traded {
                statuses,
                filled_size,
                read_back,
                worst_px,
                ..
            } => {
                println!("TRADED worst_px={worst_px} filled={filled_size} read_back={read_back}");
                for s in statuses {
                    println!("  status: {s:?}");
                }
            }
            Action::Halted { reason } => {
                halted = true;
                println!("HALTED: {reason}");
            }
            other => println!("action: {other:?}"),
        }
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
    if halted {
        std::process::exit(4);
    }
    Ok(())
}
