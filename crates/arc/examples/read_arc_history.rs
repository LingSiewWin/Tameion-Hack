//! Read the policy's own payment history off live Arc.
use alloy::primitives::Address;
use arc::{ARC_TESTNET_RPC, ArcClient};
use ledger::{DetectionConfig, detect};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let policy: Address = std::env::var("POLICY")?.parse()?;
    let from: u64 = std::env::var("FROM")?.parse()?;
    let client = ArcClient::read_only(ARC_TESTNET_RPC)?;
    let head = client.block_number().await?;

    println!("读 {policy} 在 {from}..{head} 的历史\n");
    let history = client.transfers(policy, from, head).await?;
    println!("找到 {} 笔:", history.len());
    for t in &history {
        println!(
            "  {:?}  {}  {}  {}",
            t.direction, t.amount, t.counterparty, t.at
        );
    }

    println!("\n推断出的周期性义务:");
    let obs = detect(&history, DetectionConfig::default());
    if obs.is_empty() {
        println!("  (没有)");
    }
    for o in &obs {
        println!("  {}", o.explain());
        println!("    下次到期 {}  cycle_key {}", o.next_due, o.cycle_key());
    }
    Ok(())
}
