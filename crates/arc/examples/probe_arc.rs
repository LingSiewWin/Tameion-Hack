//! Read-only probe against live Arc testnet. Sends nothing, costs nothing.
//!   cargo run -p arc --example probe_arc
use alloy::primitives::{U256, address};
use arc::{ARC_TESTNET_CHAIN_ID, ARC_TESTNET_RPC, ArcClient, USDC_ADDRESS, usdc_from_wei};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ArcClient::read_only(ARC_TESTNET_RPC)?;

    let chain_id = client.chain_id().await?;
    println!("chain id        {chain_id}  (expected {ARC_TESTNET_CHAIN_ID})");
    client.expect_chain(ARC_TESTNET_CHAIN_ID).await?;
    println!("chain check     ok");

    let head = client.block_number().await?;
    println!("head block      {head}");

    let t = client.block_timestamp(head).await?;
    println!("head timestamp  {t}");

    // The treasury the faucet funded.
    let treasury = address!("6c1ce01BfE3f834A5Fbfa0a9081d5d976b819A25");
    let native = client.native_balance(treasury).await?;
    println!("treasury        {native}   (read as native, converted at 1e12)");

    let erc20 = client.usdc_balance(treasury).await?;
    println!("via ERC-20      {erc20}   (USDC precompile at {USDC_ADDRESS})");

    println!("history source  {:?}", client.source());

    // The faucet payment is the only thing that has ever touched this
    // address, so finding it is the end-to-end check on the log path:
    // filter, decode, direction, and the wei -> minor-unit conversion.
    let from = head.saturating_sub(300_000);
    let found = client.transfers(treasury, from, head).await?;
    println!(
        "treasury logs   {} transfers in the last 300k blocks",
        found.len()
    );
    for t in &found {
        println!(
            "  {:?}  {}  from/to {}  {}",
            t.direction, t.amount, t.counterparty, t.tx_hash
        );
    }

    println!();
    println!("unit conversion, checked both ways:");
    for wei in [
        U256::from(1u64),
        U256::from(1_000_000_000_000u64),
        U256::from(20_000_000_000_000_000_000u128),
    ] {
        println!("  {wei:>22} wei -> {}", usdc_from_wei(wei));
    }
    Ok(())
}
