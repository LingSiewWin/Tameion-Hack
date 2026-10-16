//! One full pass against live Arc testnet.
//!
//! The agent reads its own payment history off the chain, infers what it
//! owes, proposes an action per obligation, has each one verified, pays what
//! clears, escalates what does not, and anchors a root of the record.
//!
//!   POLICY=… ANCHOR=… FROM=… cargo run -p operator --example run_arc
//!
//! The same pass runs on any EVM chain where USDC is an ERC-20 and the policy
//! is a `BudgetPolicyToken`: set RPC_URL, CHAIN_ID and TOKEN (the USDC
//! contract), and history is read from that token's Transfer events instead of
//! Arc's native-transfer emitter.

use alloy::primitives::Address;
use arc::{ARC_TESTNET_CHAIN_ID, ARC_TESTNET_RPC, ArcClient};
use ledger::Seconds;
use operator::claude::ClaudeReasoner;
use operator::reasoner::Reasoner;
use operator::{Config, Operator, reasoner::RuleBased, replay};

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} must be set"))
}
fn rule(t: &str) {
    println!("\n\x1b[1m{t}\x1b[0m\n{}", "─".repeat(t.chars().count()));
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let policy: Address = env("POLICY").parse()?;
    let anchor: Address = env("ANCHOR").parse()?;
    let from: u64 = env("FROM").parse()?;
    // AGENT_PK for a local chain's development key; otherwise the agent's
    // own key file, which never leaves ~/.tameion.
    let agent_key = match std::env::var("AGENT_PK") {
        Ok(k) => k,
        Err(_) => std::fs::read_to_string(
            std::path::Path::new(&std::env::var("HOME")?).join(".tameion/agent.key"),
        )?,
    };

    let rpc = std::env::var("RPC_URL").unwrap_or_else(|_| ARC_TESTNET_RPC.to_string());
    let chain: u64 = match std::env::var("CHAIN_ID") {
        Ok(c) => c.parse()?,
        Err(_) => ARC_TESTNET_CHAIN_ID,
    };
    let mut client = ArcClient::with_signer(&rpc, agent_key.trim())?;
    if let Ok(token) = std::env::var("TOKEN") {
        client = client.with_token(token.parse()?);
    }
    client.expect_chain(chain).await?;

    rule("链");
    println!("  chain id   {}", client.chain_id().await?);
    println!("  history    {:?}", client.source());
    println!("  agent      {}", client.address().unwrap());
    println!("  policy     {policy}");
    println!("  anchor     {anchor}");

    let authority = client.authority(policy).await?;
    println!("  单笔上限   {}", authority.per_action_cap);
    println!(
        "  每日上限   {}  已花 {}",
        authority.daily_cap, authority.spent_today
    );
    println!("  合约余额   {}", client.policy_balance(policy).await?);

    let mut config = Config::new(policy, policy, anchor);
    config.history_from_block = from;
    // The owner's other wallets, comma-separated: where a payee the policy
    // has never been told about first shows up.
    for a in std::env::var("ALSO_HISTORY").unwrap_or_default().split(',') {
        let a = a.trim();
        if !a.is_empty() {
            config.also_history.push(a.parse()?);
            println!("  also reads {a}");
        }
    }
    config.horizon = Seconds::days(30).unwrap();
    // The Arc demonstration pays thousandths of a USDC (prove_arc.sh pays
    // 0.002, 0.005 and 0.030), so the dust line scales down with it: one
    // thousandth of a USDC instead of one USDC. What it filters is still the
    // poisoner's zero-value and dust transfers, not the payments themselves.
    config.detection.dust_line = Some(ledger::Money::parse("0.001000", ledger::Currency::Usdc)?);
    // Same selection rule as the local demo: a key means the model reasons,
    // no key means the deterministic control does. Hard-coding RuleBased here
    // meant the one run that happened on real Arc was the one run the model
    // never touched.
    //
    // Two call sites rather than a boxed trait object: `Reasoner::prioritise`
    // returns `impl Future`, which is not dyn-safe.
    match ClaudeReasoner::from_env() {
        Some(c) => {
            println!("  reasoner   {} (model)", c.name());
            drive(Operator::new(client, config, c)?).await
        }
        None => {
            println!("  reasoner   rule-based (no ANTHROPIC_API_KEY)");
            drive(Operator::new(client, config, RuleBased)?).await
        }
    }
}

/// The pass itself, generic over whatever is reasoning.
async fn drive<R: operator::reasoner::Reasoner>(
    mut op: Operator<R>,
) -> Result<(), Box<dyn std::error::Error>> {
    rule("读历史");
    let history = op.read_history().await?;
    let history_for_onboarding = history.clone();
    let out = history
        .iter()
        .filter(|t| t.direction == ledger::Direction::Out)
        .count();
    println!("  链上读到 {} 笔,其中 {out} 笔是付出去的", history.len());

    // The chain's clock, not this machine's. The contract measures its
    // 24-hour window in block time, so the agent reasons in block time too;
    // on a live chain the two agree to within a block, and on a local chain
    // fast-forwarded through months of history only block time is right.
    let head = op.client_ref().block_number().await?;
    let now = op.client_ref().block_timestamp(head).await?;

    // The hedge step first, so its verdicts sit under the same root as the
    // payments below. Only when HEDGE_SPOT is set; dry run unless HEDGE_LIVE=1
    // and an API-wallet key exists. See `hedge::driver`.
    let hedge_env = match hedge::driver::from_env(op.config().treasury) {
        Some(r) => Some(r),
        None if std::env::var("HEDGE_FROM_CHAIN").is_ok_and(|v| v == "1") => {
            let read = operator::watch::eth_holdings(op.config().treasury).await;
            for (chain, r) in &read {
                match r {
                    Ok(eth) => println!("  exposure   {chain}: {eth} ETH"),
                    Err(e) => println!("  exposure   {chain}: unreadable ({e})"),
                }
            }
            let total: hedge::Decimal = read.iter().filter_map(|(_, r)| r.as_ref().ok()).sum();
            Some(hedge::driver::from_holdings(
                op.config().treasury,
                vec![hedge::engine::Holding {
                    coin: "ETH".into(),
                    size: total,
                }],
            ))
        }
        None => None,
    };
    if let Some(henv) = hedge_env {
        let mut henv = henv?;
        rule("对冲");
        // docs/50 #3: the short covers the bills within the horizon, not the
        // whole holding, unless HEDGE_COVER_USDC=full.
        if henv.input.cover_usdc.is_none() && hedge::driver::cover_follows_obligations() {
            let plan = op.obligations_due(&history, now).await?;
            let cover: hedge::Decimal = plan
                .upcoming
                .iter()
                .map(|o| hedge::Decimal::from_i128_with_scale(o.typical_amount.to_minor(), 6))
                .sum();
            println!("  cover      {cover} USDC of bills within the horizon sizes the short");
            henv.input.cover_usdc = Some(cover);
        }
        // docs/46 #4: live trading only in Autopilot; otherwise this pass is
        // a dry run whatever HEDGE_LIVE says.
        // A level that could not be read is not Autopilot (audit 2026-10-15).
        if matches!(henv.mode, hedge::engine::Mode::Live(_))
            && !operator::hedge_may_trade_live(op.client_ref(), op.config().policy).await
        {
            println!(
                "  policy not in Autopilot (or its level unreadable): live hedge demoted to dry run"
            );
            henv.mode = hedge::engine::Mode::DryRun;
        }
        let (report, _books) = hedge::driver::step(&henv).await?;
        println!("  mode       {}", henv.mode_label());
        println!(
            "  account    equity {} margin {} MM {}",
            report.account.account_value,
            report.account.margin_used,
            report.account.maintenance_margin_used
        );
        for body in hedge::record::events_for(&report)? {
            println!("  {}", body.note());
            op.log_mut().append(now, body);
        }
        if let Some(c) = ClaudeReasoner::from_env() {
            match operator::hedge_advice::propose(&c, &report, &henv.input.policy).await {
                Ok(ps) => {
                    for p in &ps {
                        let body = operator::hedge_advice::event_for(
                            p,
                            operator::hedge_advice::model_name(),
                        );
                        println!("  {}", body.note());
                        op.log_mut().append(now, body);
                    }
                }
                Err(e) => println!("  model proposal unavailable: {e}"),
            }
        }
    }

    rule("跑一轮");
    let outcome = op.run_once(now, Some(history), true).await?;
    println!("  {}", outcome.summary());
    if let Some(m) = outcome.mode {
        println!("  policy mode {}", m.label());
    }
    println!("  log root   {}", outcome.root);
    if let Some(tx) = outcome.anchor_tx {
        println!("  anchored   {tx:#x}");
    }

    if !outcome.settled.is_empty() {
        println!("\n  付掉的:");
        for (o, tx) in &outcome.settled {
            println!("    {} {}  {tx:#x}", o.counterparty, o.typical_amount);
        }
    }
    if !outcome.escalated.is_empty() {
        println!("\n  交给人的:");
        for (o, v) in &outcome.escalated {
            println!("    {} {}", o.counterparty, o.typical_amount);
            for c in v.problems() {
                println!("        {c}");
            }
        }
    }
    if !outcome.blocked.is_empty() {
        println!("\n  拒绝的:");
        for (o, v) in &outcome.blocked {
            println!("    {} {}", o.counterparty, o.typical_amount);
            for c in v.problems() {
                println!("        {c}");
            }
        }
    }

    // What the principal can approve, as ONE request. Every payee the owner
    // pays on a schedule and the policy does not yet allow -- the ones
    // escalated this pass and the ones not due yet -- goes into a single
    // `setPayees` (or `setPayee` for one), so onboarding is one tap rather
    // than one per payee. When the principal is a passkey (a contract that
    // answers nonce()), that request is a link: open it, approve with Face ID
    // or Touch ID, and the relayer submits.
    let to_approve = op
        .payees_to_approve(&history_for_onboarding, &outcome.own_payments)
        .await?;
    if !to_approve.is_empty() {
        rule("等主人批准的");
        let policy = op.config().policy;
        let call = if to_approve.len() == 1 {
            arc::prepare_payee_attestation(policy, to_approve[0])
        } else {
            arc::prepare_payee_batch(policy, &to_approve)
        };
        for a in &to_approve {
            println!("  {a}");
        }
        let principal = op.client_ref().principal(policy).await?;
        match op.client_ref().passkey_nonce(principal).await {
            Ok(n) => {
                let chain = op.client_ref().chain_id().await?;
                let c = arc::passkey_challenge(chain, principal, n, &call)?;
                println!("  principal  {principal} (passkey), approval #{n}");
                println!("  {}", call.summary);
                // The call itself, not just its hash: the page shows it in
                // words and recomputes the challenge before it signs.
                println!(
                    "  http://localhost:8787/?owner={principal:#x}&chain={chain}&n={n}&to={:#x}&data={}",
                    call.to, call.data
                );
                println!("  signs      {c:#x}");
                println!("  calldata   {}", call.data);
            }
            Err(_) => {
                println!("  principal  {principal} (a key, not a passkey): sign this call");
                println!("  {}  to {:#x}  data {}", call.summary, call.to, call.data);
            }
        }
    }

    rule("记录");
    for line in replay(op.log()) {
        println!("{line}");
    }
    Ok(())
}
