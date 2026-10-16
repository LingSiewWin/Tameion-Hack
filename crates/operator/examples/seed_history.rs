//! Build real payment history on a local chain, so the demo can read it back.
//!
//! The interactive demo used to hand the agent a synthetic array of transfers
//! and call that "reading the chain". It was not. Inferring obligations from
//! raw on-chain movements -- with nobody tagging anything -- is the whole
//! claim of this project, and a demo that fakes its input does not
//! demonstrate it.
//!
//! So this sends actual transfers, advancing the chain clock between them,
//! and leaves behind logs that `eth_getLogs` returns. From that point the
//! agent has exactly what it would have on Arc: addresses, amounts,
//! timestamps, and no labels.
//!
//! A local EVM emits nothing for a native transfer, which is why this uses
//! the `TestUSDC` stand-in: on Arc the same movements arrive as Transfer
//! events from `0xffff…fffe`, so the agent runs the same code path and only
//! the emitter address differs.

use alloy::primitives::{Address, address};
use arc::dev::TestChain;
use ledger::{Currency, Money};

const SAAS_VENDOR: Address = address!("1111111111111111111111111111111111111111");
const CONTRACTOR: Address = address!("2222222222222222222222222222222222222222");
const BIG_VENDOR: Address = address!("3333333333333333333333333333333333333333");
const IGNORED: Address = address!("4444444444444444444444444444444444444444");

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} not set"))
}

fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = env("RPC_URL");
    let treasury_key = env("TREASURY_PK");
    let treasury: Address = env("TREASURY").parse()?;
    let token: Address = env("TOKEN").parse()?;

    let chain = TestChain::connect(&rpc, &treasury_key)?;
    chain.mint(token, treasury, usdc("40000.000000")).await?;

    // The bills run side by side, as a real treasury's do. Each entry is
    // (day, payee, amount); they are written in day order and the clock stops
    // at NOW_DAY. Until 2026-10-02 each bill was written after the previous
    // one had finished, so by "now" the subscription had been unpaid for 470
    // days -- and the agent was being tested on paying it. A bill nobody paid
    // for fifteen months is a question for its owner (`cycles-behind`), so
    // the fixture now puts every bill where a live one would be.
    const NOW_DAY: u64 = 400;
    let mut schedule: Vec<(u64, Address, &str)> = Vec::new();
    // Fixed monthly subscription: the easy case. Next due tomorrow.
    for i in 0..6u64 {
        schedule.push((221 + 30 * i, SAAS_VENDOR, "20.000000"));
    }
    // Fortnightly contractor whose amount moves with hours worked. Tests that
    // detection keys on rhythm rather than on a repeated number. Next due
    // tomorrow.
    for (i, amount) in [
        "500.000000",
        "620.000000",
        "410.000000",
        "580.000000",
        "535.000000",
    ]
    .into_iter()
    .enumerate()
    {
        schedule.push((331 + 14 * i as u64, CONTRACTOR, amount));
    }
    // Quarterly, and large enough that the policy must refuse it. Next due
    // tomorrow.
    for i in 0..3u64 {
        schedule.push((131 + 90 * i, BIG_VENDOR, "1500.000000"));
    }
    // Over the cap and never attested: the escalation nobody answered. Twelve
    // days overdue at NOW_DAY -- past the seven-day escalation deadline, and
    // not yet a whole cycle behind.
    for i in 0..4u64 {
        schedule.push((268 + 30 * i, IGNORED, "1200.000000"));
    }
    // Two payments to an address that never settles into a rhythm. The agent
    // should decline to call this an obligation at all, and a demo that only
    // shows successful inferences is hiding the interesting half.
    let irregular = address!("5555555555555555555555555555555555555555");
    schedule.push((380, irregular, "77.000000"));
    schedule.push((383, irregular, "13.000000"));
    schedule.sort_by_key(|(day, _, _)| *day);

    let mut today = schedule.first().map(|(d, _, _)| *d).unwrap_or(0);
    let mut sent = 0;
    for (day, payee, amount) in &schedule {
        if *day > today {
            chain.advance_days(day - today).await?;
            today = *day;
        }
        chain.transfer(token, *payee, usdc(amount)).await?;
        sent += 1;
    }
    chain.advance_days(NOW_DAY - today).await?;

    println!(
        "{sent} transfers written to chain; clock at {}",
        chain.now().await?
    );
    Ok(())
}
