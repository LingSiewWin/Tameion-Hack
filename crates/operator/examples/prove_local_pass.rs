//! One real operator pass on anvil with the reasoner `TAMEION_REASONER`
//! picks (docs/86): proves the selection and the fallback through
//! `Operator::run_once` -- the population check, the provenance in the log,
//! the payments -- not just the reasoner on its own.
//!
//! Needs the environment `scripts/prove_e2e.sh` exports (RPC_URL, AGENT_PK,
//! TREASURY_PK, TREASURY, TOKEN, POLICY, ANCHOR) with 0x1111… and 0x2222…
//! attested and the policy funded.
//!
//!   TAMEION_REASONER=local cargo run -p operator --features dev-anvil --example prove_local_pass

// Gated so `cargo clippy --all-targets` without the feature still builds
// (prove_e2e and seed_history are not gated; docs/86 "Needs the main session").
#[cfg(not(feature = "dev-anvil"))]
fn main() {
    eprintln!("run with --features dev-anvil");
}

#[cfg(feature = "dev-anvil")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    anvil::main().await
}

#[cfg(feature = "dev-anvil")]
mod anvil {
    use std::time::Instant;

    use alloy::primitives::{Address, address};
    use arc::ArcClient;
    use arc::dev::TestChain;
    use ledger::{Currency, Money, Seconds, Timestamp};
    use operator::local::{Choice, choose};
    use operator::reasoner::{Reasoner, RuleBased};
    use operator::{Config, Operator, replay};

    const SAAS: Address = address!("1111111111111111111111111111111111111111");
    const CONTRACTOR: Address = address!("2222222222222222222222222222222222222222");

    type Res = Result<(), Box<dyn std::error::Error>>;

    fn env(k: &str) -> String {
        std::env::var(k).unwrap_or_else(|_| panic!("{k} must be set"))
    }

    fn usdc(s: &str) -> Money {
        Money::parse(s, Currency::Usdc).unwrap()
    }

    pub async fn main() -> Res {
        let rpc = env("RPC_URL");
        let chain = TestChain::connect(&rpc, &env("TREASURY_PK"))?;
        let treasury: Address = env("TREASURY").parse()?;
        let token: Address = env("TOKEN").parse()?;
        chain.mint(token, treasury, usdc("5000.000000")).await?;
        // Two weekly bills paid on the same days, so both fall due together
        // and there is an order to choose. Weekly, not monthly: 5 x 30 days
        // outlives the agent's 90-day term (BudgetPolicy AGENT_TERM) and a
        // lapsed agent only proposes (docs/86, Problems).
        for (i, hours) in ["500", "620", "410", "580", "535"].iter().enumerate() {
            let a = chain.transfer(token, SAAS, usdc("20.000000")).await?;
            let b = chain
                .transfer(token, CONTRACTOR, usdc(&format!("{hours}.000000")))
                .await?;
            println!(
                "history cycle {}: 20 USDC -> 0x1111… {a}   {hours} USDC -> 0x2222… {b}",
                i + 1
            );
            chain.advance_days(7).await?;
        }
        let now = chain.now().await?;
        let client = ArcClient::with_signer(&rpc, &env("AGENT_PK"))?.with_token(token);
        let mut config = Config::new(treasury, env("POLICY").parse()?, env("ANCHOR").parse()?);
        config.horizon = Seconds::days(120).unwrap();
        config.treasury = treasury;
        match choose() {
            Choice::Local(r) => pass(client, config, r, now).await,
            Choice::Cloud(r) => pass(client, config, r, now).await,
            Choice::CloudRedacted(r) => pass(client, config, r, now).await,
            Choice::Rule => pass(client, config, RuleBased, now).await,
        }
    }

    async fn pass<R: Reasoner>(client: ArcClient, config: Config, r: R, now: Timestamp) -> Res {
        println!("\nreasoner chosen: {} ({:?})", r.name(), r.kind());
        let mut op = Operator::new(client, config, r)?;
        let history = op.read_history().await?;
        let start = Instant::now();
        let outcome = op.run_once(now, Some(history), true).await?;
        println!(
            "pass took {:.1} s; reasoner recorded: {}",
            start.elapsed().as_secs_f64(),
            outcome.reasoner
        );
        println!("{}", outcome.summary());
        for (o, tx) in &outcome.settled {
            println!("  paid {} {}  {tx:#x}", o.counterparty, o.typical_amount);
        }
        if let Some(tx) = outcome.anchor_tx {
            println!("  anchored {tx:#x}");
        }
        println!("\nthe record:");
        for line in replay(op.log()) {
            println!("{line}");
        }
        Ok(())
    }
}
