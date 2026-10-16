//! Re-post the hedge journal from disk and set it beside the venue's own
//! figures (`docs/46` #1, CFO seat: "an accountant needs closing balances
//! matched to the venue at one timestamp, not a hash chain").
//!
//!   HEDGE_USER=0x... cargo run --locked -q -p hedge --example audit_books
//!
//! Environment:
//!   HEDGE_USER            venue account to compare against (or TAMEION_TREASURY_ADDR)
//!   HEDGE_HALT_DIR        where journal.jsonl lives (default runtime/hedge)
//!   AUDIT_TOLERANCE_USDC  allowed gap per line before FAIL (default 0.50:
//!                         funding and marks accrue between a pass and now)
//!
//! Prints: trial balance (debits = credits or the loader has already refused
//! the file), every account balance, and per line books-vs-venue with the
//! difference. Exit 0 on PASS, 2 on FAIL, 3 when there are no books yet.

use hedge::Decimal;
use hedge::book::accounts;
use hedge::hl::{Venue, VenueApi, parse_address};
use ledger::money::Currency;
use std::path::PathBuf;
use std::str::FromStr;

fn dec(m: ledger::money::Money) -> Decimal {
    // `Money` displays with its unit ("998.976571 USDC"); go by minor units.
    Decimal::from_i128_with_scale(m.to_minor(), 6)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let halt_dir =
        PathBuf::from(std::env::var("HEDGE_HALT_DIR").unwrap_or_else(|_| "runtime/hedge".into()));
    let user = std::env::var("HEDGE_USER")
        .or_else(|_| std::env::var("TAMEION_TREASURY_ADDR"))
        .map_err(|_| "set HEDGE_USER (the venue account the books are kept against)")?;
    // One set of books per venue account (docs/46, found 2026-10-13).
    let dir = hedge::driver::books_dir(&halt_dir, parse_address(&user)?);
    let tol = std::env::var("AUDIT_TOLERANCE_USDC")
        .ok()
        .and_then(|s| Decimal::from_str(&s).ok())
        .unwrap_or_else(|| Decimal::new(50, 2));

    println!(
        "# audit_books {} vs venue {user}",
        dir.join("journal.jsonl").display()
    );
    let Some(books) = hedge::store::load_or_new(&dir, &[])? else {
        println!(
            "no journal at {}: nothing to reconcile yet (exit 3)",
            dir.display()
        );
        std::process::exit(3);
    };
    let (debits, credits) = books.journal.trial_balance(Currency::Usdc)?;
    println!(
        "entries         : {}   trial balance debits {} = credits {}   {}",
        books.journal.len(),
        debits,
        credits,
        if debits == credits {
            "BALANCED"
        } else {
            "UNBALANCED"
        }
    );

    let mut ids: Vec<_> = books.chart.ids().cloned().collect();
    ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let mut coins: Vec<String> = Vec::new();
    for id in &ids {
        let bal = books.journal.balance(&books.chart, id)?;
        println!("  {:<28} {}", id.as_str(), bal);
        // Account ids carry a kind prefix (`asset:hedge:mtm:ETH`), so match
        // the segment, not the start of the string.
        if let Some((_, coin)) = id.as_str().split_once("hedge:mtm:") {
            coins.push(coin.to_string());
        }
    }

    let venue = Venue::testnet();
    let snap = venue.account(parse_address(&user)?).await?;
    let margin = dec(books
        .journal
        .balance(&books.chart, &accounts::hedge_margin())?);
    let mut mtm_total = Decimal::ZERO;
    let mut fail = false;
    println!();
    println!(
        "{:<34} {:>16} {:>16} {:>12}",
        "line", "books", "venue", "diff"
    );
    for coin in &coins {
        let mtm = dec(books
            .journal
            .balance(&books.chart, &accounts::hedge_mtm(coin))?);
        mtm_total += mtm;
        let venue_upnl = snap
            .positions
            .get(coin.as_str())
            .map(|p| p.unrealized_pnl)
            .unwrap_or(Decimal::ZERO);
        let diff = mtm - venue_upnl;
        if diff.abs() > tol {
            fail = true;
        }
        println!(
            "{:<34} {:>16} {:>16} {:>12}   size {}",
            format!("{coin} unrealised (hedge:mtm vs upnl)"),
            mtm,
            venue_upnl,
            diff,
            snap.size(coin)
        );
    }
    let carrying = margin + mtm_total;
    let diff = carrying - snap.state.account_value;
    if diff.abs() > tol {
        fail = true;
    }
    println!(
        "{:<34} {:>16} {:>16} {:>12}",
        "margin + mtm vs accountValue", carrying, snap.state.account_value, diff
    );
    println!(
        "{:<34} {:>16} {:>16}",
        "venue marginUsed / maintenance",
        snap.state.margin_used,
        snap.state.maintenance_margin_used
    );
    println!(
        "venue snapshot  : t={} ms   tolerance {tol} USDC",
        snap.time_ms
    );
    if !coins.is_empty() && margin.is_zero() && snap.state.account_value.is_zero() {
        println!(
            "note            : books carry marks but no margin was ever deposited and the venue account is empty — a dry run; the mtm lines reconcile against zero positions."
        );
    }
    println!("RESULT          : {}", if fail { "FAIL" } else { "PASS" });
    if fail {
        std::process::exit(2);
    }
    Ok(())
}
