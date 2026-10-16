//! Level 0 of the trust ladder (docs/46 #8): watch an address, infer what it
//! owes, say what the agent would pay and what it would ask about -- with no
//! key, no policy of its own, and nothing sent. The first-user motion from
//! `docs/45`: "give me a public address and I run it read-only for two weeks."
//!
//!   TAMEION_TREASURY_ADDR=0x... FROM=<block> cargo run --locked -q -p operator --example watch
//!
//! Environment:
//!   TAMEION_TREASURY_ADDR  the address whose outgoing payments are the history (required)
//!   FROM                   first block to read (required; a public RPC refuses wide scans)
//!   ALSO_HISTORY           other wallets of the same owner, comma-separated
//!   RPC_URL / CHAIN_ID     default Arc testnet
//!   TOKEN                  ERC-20 USDC on chains where USDC is not native
//!   POLICY                 a BudgetPolicy to read the replay guard and payee list from
//!                          (default runtime/arc_policy.addr). For a stranger's address
//!                          none of their payees are attested there, so every payment
//!                          shows as "would ask you". That is the point of level 0.
//!   DUST                   smallest transfer that counts as evidence (default 0.001000)
//!
//! Prints the obligations with their evidence, which fall due within 30 days,
//! which the agent would pay now if it were trusted, and which it would hand
//! to the owner. Exit 0 always; this is a report.

use alloy::primitives::Address;
use arc::{ARC_TESTNET_CHAIN_ID, ARC_TESTNET_RPC, ArcClient};
use ledger::{Currency, Money, Seconds};
use operator::reasoner::RuleBased;
use operator::{Config, Operator};

fn env(k: &str) -> Result<String, String> {
    std::env::var(k).map_err(|_| format!("{k} must be set"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let treasury: Address = env("TAMEION_TREASURY_ADDR")?.parse()?;
    let from: u64 = env("FROM")?.parse()?;
    let policy: Address = match std::env::var("POLICY") {
        Ok(p) => p.parse()?,
        Err(_) => std::fs::read_to_string("runtime/arc_policy.addr")
            .map_err(|_| "POLICY not set and runtime/arc_policy.addr missing")?
            .trim()
            .parse()?,
    };
    let rpc = std::env::var("RPC_URL").unwrap_or_else(|_| ARC_TESTNET_RPC.to_string());
    let chain: u64 = match std::env::var("CHAIN_ID") {
        Ok(c) => c.parse()?,
        Err(_) => ARC_TESTNET_CHAIN_ID,
    };

    // Read-only by type: this client cannot sign, so nothing here can move.
    let mut client = ArcClient::read_only(&rpc)?;
    if let Ok(token) = std::env::var("TOKEN") {
        client = client.with_token(token.parse()?);
    }
    client.expect_chain(chain).await?;

    println!("# watch {treasury:#x}  (level 0: read-only, no key, nothing sent)");
    println!(
        "chain      {chain}  history {:?}  from block {from}",
        client.source()
    );
    println!(
        "policy     {policy:#x}  (replay guard and payee list are read here; a stranger has none attested)"
    );

    let mut config = Config::new(treasury, policy, Address::ZERO);
    config.history_from_block = from;
    for a in std::env::var("ALSO_HISTORY").unwrap_or_default().split(',') {
        let a = a.trim();
        if !a.is_empty() {
            config.also_history.push(a.parse()?);
            println!("also reads {a}");
        }
    }
    config.horizon = Seconds::days(30).unwrap();
    let dust = std::env::var("DUST").unwrap_or_else(|_| "0.001000".into());
    config.detection.dust_line = Some(Money::parse(&dust, Currency::Usdc)?);

    let op = Operator::new_unchecked(client, config, RuleBased);
    let history = op.read_history().await?;
    let out = history
        .iter()
        .filter(|t| t.direction == ledger::Direction::Out)
        .count();
    println!(
        "history    {} transfers read, {out} outgoing",
        history.len()
    );

    let head = op.client_ref().block_number().await?;
    let now = op.client_ref().block_timestamp(head).await?;
    let plan = op.obligations_due(&history, now).await?;

    let candidates: Vec<Address> = plan
        .obligations
        .iter()
        .filter_map(|o| o.counterparty.as_str().parse().ok())
        .collect();
    // `None` when the policy has no payee list to read (not a BudgetPolicy):
    // then nothing is attested, which is the honest answer for a stranger.
    let attested = op
        .client_ref()
        .attested_among(policy, &candidates)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    println!();
    println!(
        "## what this address owes, inferred from its own payments ({} obligations)",
        plan.obligations.len()
    );
    for o in &plan.obligations {
        let overdue = o.overdue_by(now);
        let when = if overdue.is_negative() {
            format!("due in {:.1} days", -overdue.as_days())
        } else {
            format!("overdue by {:.1} days", overdue.as_days())
        };
        let ok = attested.contains(&o.counterparty.to_string());
        println!(
            "- {}  {} every {}  confidence {}  {when}  evidence {} tx\n    {}\n    {}",
            o.counterparty,
            o.typical_amount,
            o.interval.human(),
            o.confidence,
            o.evidence.len(),
            o.explain(),
            if ok {
                "payee attested on the policy: Autopilot would pay this inside the caps"
            } else {
                "not attested: the agent would ASK YOU before ever paying this (one passkey tap approves it)"
            }
        );
    }
    println!();
    println!(
        "## next 30 days: {} obligations, {} within the pay lead right now",
        plan.upcoming.len(),
        plan.due.len()
    );
    let total = ledger::total_due(&plan.upcoming, Currency::Usdc)
        .unwrap_or_else(|| Money::zero(Currency::Usdc));
    println!("liquidity needed within 30 days: {total}");
    println!();
    println!("Nothing was sent, signed or stored. To add the hedge view for the same address:");
    println!("  HEDGE_USER={treasury:#x} HEDGE_SPOT=ETH=<size> scripts/prove_hedge.sh dry");
    Ok(())
}
