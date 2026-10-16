//! Read-only proof against Hyperliquid testnet. No key, no orders.
//!
//! Prints the allowlisted markets, their live mark/oracle/funding, their
//! maintenance tiers, and the treasury's account state and positions. Run
//! through `scripts/prove_hedge.sh readonly` so the output lands in `proof/`.
//!
//! `TAMEION_TREASURY_ADDR` overrides the address (default: the Arc treasury).

use hedge::hl::{Venue, VenueApi, parse_address};
use hedge::policy::HedgePolicy;
use hedge::safety;

const DEFAULT_TREASURY: &str = "0x6c1ce01BfE3f834A5Fbfa0a9081d5d976b819A25";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("TAMEION_TREASURY_ADDR").unwrap_or_else(|_| DEFAULT_TREASURY.into());
    let user = parse_address(&addr)?;
    let policy = HedgePolicy::default();
    let venue = Venue::testnet();

    println!("# hedge read-only proof — Hyperliquid {:?}", venue.chain());
    println!("user: {user}");
    println!("policy: {policy:?}");

    let markets = venue.markets().await?;
    let prices = venue.prices().await?;
    println!("\n## markets ({} live perps on venue)", markets.len());
    for coin in &policy.allowlist {
        let Some(m) = markets.get(coin) else {
            println!("{coin}: NOT LISTED");
            continue;
        };
        let ctx = prices.get(coin);
        println!(
            "{coin}: index={} sz_decimals={} max_leverage={}x",
            m.index, m.sz_decimals, m.max_leverage
        );
        if let Some(c) = ctx {
            println!(
                "    mark={} oracle={} funding/hr={} open_interest={} margin_table_id={:?}",
                c.mark_px, c.oracle_px, c.funding, c.open_interest, c.margin_table_id
            );
            let tiers = venue.tiers(c.margin_table_id).await?;
            for t in &tiers {
                println!(
                    "    tier: notional >= {} → max {}x → maintenance rate {}",
                    t.lower_bound,
                    t.max_leverage,
                    safety::maintenance_rate(
                        std::slice::from_ref(t),
                        t.lower_bound,
                        c.max_leverage
                    )
                );
            }
        }
    }

    let snap = venue.account(user).await?;
    println!("\n## account (cross)");
    println!(
        "account_value={} margin_used={} maintenance_margin_used={} withdrawable={} time={}",
        snap.state.account_value,
        snap.state.margin_used,
        snap.state.maintenance_margin_used,
        snap.withdrawable,
        snap.time_ms
    );
    println!(
        "health (equity / k×MM): {:?}",
        safety::health(&snap.state, &policy)
    );
    if snap.positions.is_empty() {
        println!("positions: none");
    }
    for (coin, p) in &snap.positions {
        println!(
            "position {coin}: szi={} entry={:?} value={} upnl={} liq={:?} margin_used={} lev={}x cum_funding(all_time)={}",
            p.szi,
            p.entry_px,
            p.position_value,
            p.unrealized_pnl,
            p.liquidation_px,
            p.margin_used,
            p.leverage.value,
            p.cum_funding.all_time
        );
    }
    if snap.state.account_value.is_zero() {
        println!(
            "\nNOTE: account value is 0 on Hyperliquid testnet. Fund it at https://app.hyperliquid-testnet.xyz/faucet before `live_once`."
        );
    }
    Ok(())
}
