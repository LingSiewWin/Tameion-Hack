//! One full pass of the agent loop against a chain.
//!
//! Driven by `scripts/prove_operator.sh`, which brings up anvil and deploys
//! the contracts. Reads its configuration from the environment so the same
//! binary runs against a local node or Arc testnet with nothing recompiled.
//!
//! History is supplied rather than read from logs here. Reading history off
//! chain is exercised separately by `cargo run -p arc --example probe_arc`
//! against live Arc; this example is about what happens *after* the history
//! is known -- detect, rank, propose, verify, pay, record, anchor.

use alloy::primitives::Address;
use arc::ArcClient;
use ledger::{Counterparty, Currency, Direction, Money, Seconds, Timestamp, Transfer};
use operator::claude::ClaudeReasoner;
use operator::{Config, Operator, reasoner::RuleBased, replay};

const DAY: i64 = 86_400;

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} must be set"))
}

fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}

/// `n` payments to `who`, `every_days` apart, ending `ends_days_ago` before `now`.
fn schedule(
    who: &str,
    amount: &str,
    every_days: i64,
    n: i64,
    now: Timestamp,
    ends_days_ago: i64,
) -> Vec<Transfer> {
    (0..n)
        .map(|i| {
            let ago = ends_days_ago + (n - 1 - i) * every_days;
            Transfer {
                tx_hash: format!("0xseed{}{:02}", &who[2..6], i),
                counterparty: Counterparty::new(who),
                amount: usdc(amount),
                at: Timestamp::from_unix(now.as_unix() - ago * DAY),
                direction: Direction::Out,
            }
        })
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rpc = env("RPC_URL");
    let agent_key = env("AGENT_PK");
    let policy: Address = env("POLICY").parse()?;
    let anchor: Address = env("ANCHOR").parse()?;

    let client = ArcClient::with_signer(&rpc, &agent_key)?;
    println!("agent      {}", client.address().expect("signer"));
    println!("policy     {policy}");
    println!("anchor     {anchor}");

    let mut config = Config::new(policy, policy, anchor);
    config.horizon = Seconds::days(30).unwrap(); // Which reasoner drives this pass is chosen by the environment, so one
    // proof script exercises the deterministic control and the model without
    // a code change. `CLAUDE_BASE_URL` points the client at a dead endpoint
    // on purpose: that is how the fallback gets demonstrated rather than
    // merely asserted.
    match ClaudeReasoner::from_env() {
        Some(claude) => {
            let claude = match std::env::var("CLAUDE_BASE_URL") {
                Ok(u) => claude.with_url(u),
                Err(_) => claude,
            };
            drive(Operator::new(client, config, claude)?, policy).await
        }
        None => {
            eprintln!("(no ANTHROPIC_API_KEY set -- using the deterministic control)");
            drive(Operator::new(client, config, RuleBased)?, policy).await
        }
    }
}

/// The pass itself, generic over whatever is doing the reasoning.
async fn drive<R: operator::reasoner::Reasoner>(
    mut op: Operator<R>,
    policy: alloy::primitives::Address,
) -> Result<(), Box<dyn std::error::Error>> {
    // A treasury with three relationships, all overdue by design so every
    // one of them reaches a decision in a single pass.
    let now = Timestamp::from_unix(1_800_000_000);
    let mut history = Vec::new();
    // Clean monthly subscription, well inside the cap.
    history.extend(schedule(
        "0x1111111111111111111111111111111111111111",
        "20.000000",
        30,
        6,
        now,
        31,
    ));
    // Fortnightly contractor, also inside the cap.
    history.extend(schedule(
        "0x2222222222222222222222222222222222222222",
        "35.000000",
        14,
        6,
        now,
        15,
    ));
    // Monthly, but above the per-action cap: this one must reach a human.
    history.extend(schedule(
        "0x3333333333333333333333333333333333333333",
        "150.000000",
        30,
        6,
        now,
        31,
    ));

    println!(
        "\nseeded {} historical transfers across 3 counterparties\n",
        history.len()
    );

    // Hedge first (when HEDGE_SPOT is set), so the pass anchors both.
    if let Some(henv) = hedge::driver::from_env(policy) {
        let mut henv = henv?;
        // docs/46 #4: live only in Autopilot; a level that could not be read
        // is not Autopilot (audit 2026-10-15).
        if matches!(henv.mode, hedge::engine::Mode::Live(_))
            && !operator::hedge_may_trade_live(op.client_ref(), policy).await
        {
            println!(
                "policy not in Autopilot (or its level unreadable): live hedge demoted to dry run"
            );
            henv.mode = hedge::engine::Mode::DryRun;
        }
        let (report, _books) = hedge::driver::step(&henv).await?;
        println!("── hedge ({}) ──", henv.mode_label());
        for body in hedge::record::events_for(&report)? {
            println!("  {}", body.note());
            op.log_mut().append(now, body);
        }
        println!();
    }

    let outcome = op.run_once(now, Some(history), true).await?;

    println!("── outcome ──");
    println!("{}", outcome.summary());
    println!("reasoner   {}", outcome.reasoner);
    println!("log root   {}", outcome.root);
    if let Some(tx) = outcome.anchor_tx {
        println!("anchored   {tx:#x}");
    }

    if !outcome.settled.is_empty() {
        println!("\n── paid ──");
        for (o, tx) in &outcome.settled {
            println!("  {} {}  {tx:#x}", o.counterparty, o.typical_amount);
        }
    }
    if !outcome.escalated.is_empty() {
        println!("\n── escalated to a human ──");
        for (o, v) in &outcome.escalated {
            println!("  {} {}", o.counterparty, o.typical_amount);
            for c in v.problems() {
                println!("      {c}");
            }
        }
    }
    if !outcome.blocked.is_empty() {
        println!("\n── blocked ──");
        for (o, v) in &outcome.blocked {
            println!("  {} {}", o.counterparty, o.typical_amount);
            for c in v.problems() {
                println!("      {c}");
            }
        }
    }

    println!("\n── the record ──");
    for line in replay(op.log()) {
        println!("{line}");
    }

    // Every settlement's key must be spent on chain. This is the cross-check
    // that the record and the chain agree, and it is the one thing here that
    // would catch the log claiming a payment the chain never saw.
    //
    // It asserts rather than prints. The earlier version printed, compared
    // the *action hash* against a guard keyed on the *settlement key*, and so
    // reported `spent on chain: false` for every payment it had just made.
    // A false that nobody fails on is not a check.
    println!("\n── record vs chain ──");
    let mut unconfirmed = Vec::new();
    for (cycle, key) in operator::settlement_hashes(op.log()) {
        let spent = op
            .client_ref()
            .already_executed(policy, alloy::primitives::B256::from(*key.as_bytes()))
            .await?;
        println!("  {cycle}  {key}  spent on chain: {spent}");
        if !spent {
            unconfirmed.push(cycle);
        }
    }
    if !unconfirmed.is_empty() {
        return Err(format!(
            "the record claims {} settlement(s) the chain has no record of: {}",
            unconfirmed.len(),
            unconfirmed.join(", ")
        )
        .into());
    }
    println!("  every settlement in the record is spent on chain.");
    Ok(())
}
