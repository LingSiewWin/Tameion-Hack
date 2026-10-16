//! Validation probe: our treasury has no positions on testnet, so the
//! position-parsing path of `Venue::account` would otherwise go untested.
//! This finds a recently active testnet address from the public trade tape
//! and reads its account, so the parse runs against real, non-empty data.
//! Read-only; no key.

use hedge::hl::{Venue, VenueApi};
use hedge::policy::HedgePolicy;
use hedge::safety;
use hypersdk::hypercore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let venue = Venue::testnet();
    let raw = hypercore::testnet();
    let trades = raw.recent_trades("ETH".into()).await?;
    println!(
        "# position-parse probe — {} recent ETH trades on testnet",
        trades.len()
    );

    let mut seen = std::collections::HashSet::new();
    let mut checked = 0;
    for t in trades.iter().take(60) {
        for user in t.users {
            if !seen.insert(user) {
                continue;
            }
            let snap = venue.account(user).await?;
            if snap.positions.is_empty() {
                continue;
            }
            checked += 1;
            println!(
                "\nuser {user}: account_value={} margin_used={} mm_used={} health={:?}",
                snap.state.account_value,
                snap.state.margin_used,
                snap.state.maintenance_margin_used,
                safety::health(&snap.state, &HedgePolicy::default())
            );
            for (coin, p) in &snap.positions {
                println!(
                    "  {coin}: szi={} entry={:?} value={} upnl={} liq={:?} margin_used={} lev={}x/{:?} cum_funding={}",
                    p.szi,
                    p.entry_px,
                    p.position_value,
                    p.unrealized_pnl,
                    p.liquidation_px,
                    p.margin_used,
                    p.leverage.value,
                    p.leverage.leverage_type,
                    p.cum_funding.all_time
                );
            }
            if checked >= 3 {
                break;
            }
        }
        if checked >= 3 {
            break;
        }
    }
    if checked == 0 {
        println!(
            "no address with open positions found in the recent tape; parse path NOT exercised"
        );
        std::process::exit(5);
    }
    println!("\nparsed {checked} accounts with open positions: OK");
    Ok(())
}
