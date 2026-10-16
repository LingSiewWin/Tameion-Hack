//! Validation probe: the funding-booking path against real venue data.
//!
//! Our treasury has never held a position, so `user_funding` for it is
//! empty. This finds an active testnet address from the public trade tape,
//! fetches its funding payments for the last seven days, and books them
//! through `book::funding` into a journal, so the parse and the double-entry
//! legs are exercised against payloads the venue actually sends. Read-only.

use hedge::book;
use hedge::hl::{Venue, VenueApi};
use hypersdk::hypercore;
use ledger::{Currency, Journal};
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let venue = Venue::testnet();
    let raw = hypercore::testnet();
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let since = now_ms.saturating_sub(7 * 24 * 3600 * 1000);

    let mut users = Vec::new();
    for coin in ["BTC", "ETH"] {
        for t in raw.recent_trades(coin.into()).await? {
            for u in t.users {
                if !users.contains(&u) {
                    users.push(u);
                }
            }
        }
    }
    println!(
        "# funding-booking probe — {} distinct addresses from the BTC/ETH tape",
        users.len()
    );

    for user in users.iter().take(40) {
        let entries = venue.funding_since(*user, since).await?;
        if entries.is_empty() {
            continue;
        }
        let coins: Vec<String> = entries
            .iter()
            .map(|e| e.delta.coin.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let coin_refs: Vec<&str> = coins.iter().map(String::as_str).collect();
        let chart = book::chart(&coin_refs);
        let mut journal = Journal::new();
        let mut booked = 0;
        for e in &entries {
            let doc_id = format!("{}@{}", e.delta.coin, e.time);
            if let Some(entry) = book::funding(&e.delta.coin, e.delta.usdc, &doc_id)? {
                journal.append(entry);
                booked += 1;
            }
        }
        println!(
            "\nuser {user}: {} funding payments in 7d on {coins:?}; booked {booked}",
            entries.len()
        );
        for e in entries.iter().take(3) {
            println!(
                "  t={} {} usdc={} szi={} rate={} type={} hash={}",
                e.time,
                e.delta.coin,
                e.delta.usdc,
                e.delta.szi,
                e.delta.funding_rate,
                e.delta.delta_type,
                e.hash
            );
        }
        let income = journal.balance(&chart, &book::accounts::funding_income())?;
        let expense = journal.balance(&chart, &book::accounts::funding_expense())?;
        let margin = journal.balance(&chart, &book::accounts::hedge_margin())?;
        let (d, c) = journal.trial_balance(Currency::Usdc)?;
        println!(
            "  books: funding income {income} · funding expense {expense} · margin net {margin}"
        );
        println!(
            "  trial balance: {d} = {c} {}",
            if d == c { "BALANCED" } else { "UNBALANCED" }
        );
        println!("\nfunding path exercised against real payloads: OK");
        return Ok(());
    }
    println!("no address with funding in the last 7 days among the first 40; path NOT exercised");
    std::process::exit(5);
}
