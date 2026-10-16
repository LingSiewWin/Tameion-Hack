//! Six months of genuine on-chain payment history, built in seconds, then
//! rediscovered by the agent and acted on.
//!
//!   cargo run -p operator --features dev-anvil --example prove_e2e
//!
//! Driven by `scripts/prove_e2e.sh`, which brings up anvil and deploys the
//! contracts.
//!
//! Unit tests prove the arithmetic. They do not prove an agent can hold
//! value, read its own history off a chain, and move money, so this does that
//! part and prints transaction hashes anyone can check.
//!
//! Nothing here is simulated in memory. The history the agent reads is built
//! by actually sending transfers, with the chain clock advanced between each
//! one, so by the time the agent looks there is a real six-month subscription
//! pattern sitting in the chain's logs. The agent then discovers it the same
//! way it would on mainnet: `eth_getLogs` over ERC-20 `Transfer` events. It
//! has no way to tell the six months were compressed.

use alloy::primitives::{Address, address};
use arc::ArcClient;
use arc::dev::TestChain;
use ledger::{Currency, Money, Seconds};
use operator::{Config, Operator, reasoner::RuleBased, replay};

const SAAS_VENDOR: Address = address!("1111111111111111111111111111111111111111");
const CONTRACTOR: Address = address!("2222222222222222222222222222222222222222");
const BIG_VENDOR: Address = address!("3333333333333333333333333333333333333333");

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} must be set"))
}
fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}
fn rule(t: &str) {
    println!("\n\x1b[1m{}\x1b[0m\n{}", t, "─".repeat(t.chars().count()));
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = env("RPC_URL");
    let agent_key = env("AGENT_PK");
    let treasury_key = env("TREASURY_PK");
    let treasury: Address = env("TREASURY").parse()?;
    let token: Address = env("TOKEN").parse()?;
    let policy: Address = env("POLICY").parse()?;
    let anchor: Address = env("ANCHOR").parse()?;

    let chain = TestChain::connect(&rpc, &treasury_key)?;

    rule("STEP 1  Fund the treasury");
    chain.mint(token, treasury, usdc("20000.000000")).await?;
    println!("  treasury {treasury}");
    println!("  holding  {}", chain.balance_of(token, treasury).await?);

    rule("STEP 2  Build six months of real payment history on chain");
    println!("  Each line below is a mined transaction with a receipt and a log.");
    println!("  Between each one the chain clock moves forward.\n");

    // A fixed monthly subscription.
    for i in 1..=6 {
        let tx = chain
            .transfer(token, SAAS_VENDOR, usdc("20.000000"))
            .await?;
        println!("  subscription  cycle {i}/6   20.000000 USDC -> 0x1111…  {tx}");
        chain.advance_days(30).await?;
    }
    // A fortnightly contractor whose amount moves with hours worked.
    for (i, amount) in [
        "500.000000",
        "620.000000",
        "410.000000",
        "580.000000",
        "535.000000",
    ]
    .iter()
    .enumerate()
    {
        let tx = chain.transfer(token, CONTRACTOR, usdc(amount)).await?;
        println!(
            "  contractor    cycle {}/5  {amount} USDC -> 0x2222…  {tx}",
            i + 1
        );
        chain.advance_days(14).await?;
    }
    // A quarterly vendor large enough that the policy must refuse it.
    for i in 1..=3 {
        let tx = chain
            .transfer(token, BIG_VENDOR, usdc("1500.000000"))
            .await?;
        println!("  big vendor    cycle {i}/3  1500.000000 USDC -> 0x3333…  {tx}");
        chain.advance_days(90).await?;
    }

    let now = chain.now().await?;
    println!("\n  chain clock is now {now}, about 15 months after it started");

    rule("STEP 3  The agent reads that history back off the chain");
    let client = ArcClient::with_signer(&rpc, &agent_key)?.with_token(token);
    let mut config = Config::new(treasury, policy, anchor);
    config.horizon = Seconds::days(120).unwrap();
    // History is read for the treasury, which is who made the payments.
    config.treasury = treasury;
    let mut op = Operator::new(client, config, RuleBased)?;

    let history = op.read_history().await?;
    let out = history
        .iter()
        .filter(|t| t.direction == ledger::Direction::Out)
        .count();
    println!(
        "  eth_getLogs returned {} transfers touching the treasury",
        history.len()
    );
    println!("  {out} of them outgoing; the inbound one is the mint that funded it");
    println!("  the agent was not told what any of them were for");

    rule("STEP 4  One pass of the loop");
    let outcome = op.run_once(now, Some(history), true).await?;
    println!("{}", outcome.summary());
    println!("log root   {}", outcome.root);
    if let Some(tx) = outcome.anchor_tx {
        println!("anchored   {tx:#x}");
    }

    if !outcome.settled.is_empty() {
        println!("\n  paid:");
        for (o, tx) in &outcome.settled {
            println!("    {} {}  {tx:#x}", o.counterparty, o.typical_amount);
        }
    }
    if !outcome.escalated.is_empty() {
        println!("\n  escalated to a human:");
        for (o, v) in &outcome.escalated {
            println!("    {} {}", o.counterparty, o.typical_amount);
            for c in v.problems() {
                println!("        {c}");
            }
        }
    }

    rule("STEP 5  What the agent inferred, from addresses alone");
    let explained = outcome
        .settled
        .iter()
        .map(|(o, _)| o)
        .chain(outcome.escalated.iter().map(|(o, _)| o))
        .chain(outcome.blocked.iter().map(|(o, _)| o));
    for o in explained {
        println!("  {}", o.explain());
    }

    rule("STEP 6  The record");
    for line in replay(op.log()) {
        println!("{line}");
    }
    Ok(())
}
