//! Is "the API wallet cannot withdraw" true? Hyperliquid's own pages do not
//! say it; third-party integrator docs do (`docs/46` §6). This asks the venue
//! directly: the trade-only agent key signs a `withdraw3` of 1 USDC to the
//! master's **own** address. Either the venue refuses (claim proved) or the
//! money lands in the master's own wallet on L1 (claim false, nothing lost).
//!
//!   HEDGE_USER=0x<master> cargo run --locked -q -p hedge --example prove_no_withdraw
//!
//! Environment:
//!   HEDGE_USER            the master account the agent was approved for (required)
//!   HEDGE_AGENT_KEY_FILE  default ~/.tameion/hedge-agent.key
//!   AMOUNT                default 1 (USDC)
//!
//! Exit 0 when refused and the master's account value is unchanged; exit 2 when
//! the venue accepted it. Prints the venue's answer verbatim; never a key.

use hedge::Decimal;
use hedge::hl::{Venue, VenueApi, parse_address, parse_signer};
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let master = parse_address(
        &std::env::var("HEDGE_USER").map_err(|_| "HEDGE_USER (master address) must be set")?,
    )?;
    let key_file = std::env::var("HEDGE_AGENT_KEY_FILE").unwrap_or_else(|_| {
        format!(
            "{}/.tameion/hedge-agent.key",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let key_hex = std::fs::read_to_string(&key_file)
        .map_err(|e| format!("no API wallet key at {key_file}: {e}"))?;
    let agent = parse_signer(&key_hex)?;
    drop(key_hex);
    let amount = Decimal::from_str(&std::env::var("AMOUNT").unwrap_or_else(|_| "1".into()))?;

    let venue = Venue::testnet();
    let before = venue.account(master).await?;
    println!("# prove_no_withdraw — Hyperliquid {:?}", venue.chain());
    println!(
        "master           : {master:#x}   accountValue before {}",
        before.state.account_value
    );
    println!("agent (signer)   : {:#x}", agent.address());
    println!(
        "action           : withdraw3 {amount} USDC -> {master:#x} (the master's own address)"
    );

    let verdict = venue.withdraw_as(&agent, master, amount).await;
    let after = venue.account(master).await?;
    println!("accountValue after: {}", after.state.account_value);
    match verdict {
        Err(e) => {
            println!("venue answer     : REFUSED — {e}");
            if after.state.account_value == before.state.account_value {
                println!(
                    "RESULT           : PASS — the API wallet cannot withdraw; the master's balance did not move"
                );
                Ok(())
            } else {
                println!(
                    "RESULT           : FAIL — refused, yet the balance moved; investigate before saying anything"
                );
                std::process::exit(2);
            }
        }
        Ok(()) => {
            println!("venue answer     : ACCEPTED");
            println!(
                "RESULT           : FAIL — the venue accepted a withdraw signed by the API wallet. The claim is false; say \"the venue wallet can touch the margin\" and nothing stronger. Funds went to the master's own address."
            );
            std::process::exit(2);
        }
    }
}
