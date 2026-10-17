//! Level 0 of the trust ladder as a service (docs/46 #8, docs/47): watch any
//! address on a chain, read its USDC transfers, infer what it owes, say what
//! the agent would pay and what it would ask about, and print the maximum a
//! stolen key could lose under caps the reader chooses. No key, no policy,
//! nothing sent. The treasurer's first contact with the product.
//!
//! Everything here is read-only by type: [`ArcClient::read_only`] cannot sign.

use alloy::primitives::Address;
use arc::{ARC_TESTNET_CHAIN_ID, ARC_TESTNET_RPC, ArcClient, ArcError};
use ledger::obligations::{DetectionConfig, Direction, Obligation, Transfer, detect, upcoming};
use ledger::{Currency, Money, Seconds, Timestamp, rank_by_urgency};
use serde_json::{Value, json};
use std::str::FromStr;

/// A chain the watch service can read. USDC is native on Arc; an ERC-20
/// elsewhere.
#[derive(Debug, Clone, Copy)]
pub struct ChainPreset {
    pub key: &'static str,
    pub name: &'static str,
    pub chain_id: u64,
    /// Environment variable that overrides the RPC URL (a keyed endpoint
    /// scans faster and without rate limits).
    pub rpc_env: &'static str,
    pub default_rpc: &'static str,
    /// `None` = native USDC (Arc); otherwise the token contract.
    pub usdc: Option<&'static str>,
    pub explorer: &'static str,
    /// Transfers below this are not evidence (`DetectionConfig::dust_line`).
    pub dust: &'static str,
    /// The public endpoint refuses log scans at any useful volume (measured),
    /// so a scan without a keyed RPC is refused up front instead of after
    /// minutes of backoff. Balance reads still work.
    pub needs_key: bool,
    /// Blocks per `eth_getLogs` the public endpoint answers (measured; a
    /// refusal still halves it). Arbitrum takes millions, Arc ten thousand.
    pub log_span: u64,
    /// An Etherscan-compatible explorer API (Blockscout `tokentx`) that lists
    /// a wallet's token transfers in one call, tried before the log walk.
    pub indexer: Option<&'static str>,
}

pub const CHAINS: &[ChainPreset] = &[
    ChainPreset {
        key: "arc-testnet",
        name: "Arc testnet",
        chain_id: ARC_TESTNET_CHAIN_ID,
        rpc_env: "TAMEION_RPC_ARC",
        default_rpc: ARC_TESTNET_RPC,
        usdc: None,
        explorer: "https://explorer.testnet.arc.io",
        // The Arc demonstration pays thousandths of a USDC.
        dust: "0.001000",
        needs_key: false,
        // 10,000 is the node's own limit; its history also stops a few days
        // back (-32014), so a 90-day read comes from the explorer's index
        // (50 transfers in 0.9 s, measured 2026-10-14; docs/60 P-38).
        log_span: 10_000,
        indexer: Some("https://explorer.testnet.arc.io/api"),
    },
    ChainPreset {
        key: "arbitrum",
        name: "Arbitrum One",
        chain_id: 42161,
        rpc_env: "TAMEION_RPC_ARBITRUM",
        default_rpc: "https://arb1.arbitrum.io/rpc",
        usdc: Some("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
        explorer: "https://arbiscan.io",
        dust: "1.000000",
        needs_key: false,
        // 5,000,000 blocks of a wallet's USDC logs answered in 0.5 s
        // (measured 2026-10-14); the endpoint refuses above 10,000,000.
        log_span: 5_000_000,
        indexer: None,
    },
    ChainPreset {
        key: "ethereum",
        name: "Ethereum",
        chain_id: 1,
        rpc_env: "TAMEION_RPC_ETHEREUM",
        default_rpc: "https://ethereum-rpc.publicnode.com",
        usdc: Some("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"),
        explorer: "https://etherscan.io",
        dust: "1.000000",
        // publicnode: "Archive requests require a personal token".
        needs_key: true,
        log_span: 10_000,
        indexer: None,
    },
    ChainPreset {
        key: "base",
        name: "Base",
        chain_id: 8453,
        rpc_env: "TAMEION_RPC_BASE",
        default_rpc: "https://mainnet.base.org",
        usdc: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
        explorer: "https://basescan.org",
        dust: "1.000000",
        // mainnet.base.org: -32011 "request limit reached" on eth_getLogs.
        needs_key: true,
        log_span: 10_000,
        indexer: None,
    },
    // Accounts, not only reports, on a second testnet (PLAN 5.58): USDC is an
    // ERC-20 here and gas is ETH; the owner's contract lands at the same
    // address as on Arc (the EIP-7997 factory is on both).
    ChainPreset {
        key: "arbitrum-sepolia",
        name: "Arbitrum Sepolia",
        chain_id: 421_614,
        rpc_env: "TAMEION_RPC_ARBITRUM_SEPOLIA",
        default_rpc: "https://sepolia-rollup.arbitrum.io/rpc",
        usdc: Some("0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d"),
        explorer: "https://sepolia.arbiscan.io",
        dust: "0.001000",
        needs_key: false,
        // Same limits as Arbitrum One (0.58 s for 5,000,000 blocks, measured
        // 2026-10-14). Its Blockscout sits behind a Cloudflare challenge.
        log_span: 5_000_000,
        indexer: None,
    },
];

/// A test network: its balances are faucet money, never exposure.
pub fn is_testnet(c: &ChainPreset) -> bool {
    c.key.ends_with("testnet") || c.key.contains("sepolia")
}

/// Whether a keyed RPC is configured for this chain (public endpoints throttle
/// log scans; Base's refuses them outright at this volume).
pub fn keyed(c: &ChainPreset) -> bool {
    std::env::var(c.rpc_env).is_ok()
}

/// ETH held by `address` on every EVM mainnet preset, read live (`docs/50`
/// #2: Kyriba pulls exposure from the ERP; we read it off the chains). One
/// `eth_getBalance` per chain; a chain that does not answer is reported as
/// an error string, never as zero.
pub async fn eth_holdings(address: Address) -> Vec<(String, Result<hedge::Decimal, String>)> {
    let mut out = Vec::new();
    // Mainnet holdings only: a testnet preset (PLAN 5.58) holds faucet ETH,
    // which must never size a hedge.
    for c in CHAINS.iter().filter(|c| c.usdc.is_some() && !is_testnet(c)) {
        let r = async {
            let client = ArcClient::read_only(&rpc_for(c)).map_err(|e| e.to_string())?;
            let wei = client
                .native_wei(address)
                .await
                .map_err(|e| e.to_string())?;
            let s = wei.to_string();
            let d = hedge::Decimal::from_str(&s).map_err(|e| e.to_string())?;
            Ok::<_, String>(d / hedge::Decimal::from(1_000_000_000_000_000_000u128))
        }
        .await;
        out.push((c.name.to_string(), r));
    }
    out
}

pub fn chain(key: &str) -> Option<&'static ChainPreset> {
    CHAINS.iter().find(|c| c.key == key)
}

// ---- Circle Gateway: the company's USDC on other chains (PLAN 5.14, docs/56 #2)

/// Circle Gateway's balance read: `POST {host}/v1/balances` with
/// `{"token":"USDC","sources":[{"depositor"}]}` (no domain = every domain),
/// no API key (the Gateway OpenAPI declares no security on this path).
/// Testnet and mainnet are separate deployments with their own hosts.
pub fn gateway_host(c: &ChainPreset) -> &'static str {
    if c.chain_id == ARC_TESTNET_CHAIN_ID {
        "https://gateway-api-testnet.circle.com"
    } else {
        "https://gateway-api.circle.com"
    }
}

/// Gateway domain → chain name (`GET /v1/info`; the same ids on both
/// networks, the testnet names here when `testnet`).
pub fn gateway_domain_name(domain: u64, testnet: bool) -> String {
    let (main, test) = match domain {
        0 => ("Ethereum", "Ethereum Sepolia"),
        1 => ("Avalanche", "Avalanche Fuji"),
        2 => ("OP", "OP Sepolia"),
        3 => ("Arbitrum", "Arbitrum Sepolia"),
        5 => ("Solana", "Solana Devnet"),
        6 => ("Base", "Base Sepolia"),
        7 => ("Polygon PoS", "Polygon PoS Amoy"),
        10 => ("Unichain", "Unichain Sepolia"),
        13 => ("Sonic", "Sonic Testnet"),
        14 => ("World Chain", "World Chain Sepolia"),
        16 => ("Sei", "Sei Atlantic"),
        19 => ("HyperEVM", "HyperEVM Testnet"),
        26 => ("Arc", "Arc Testnet"),
        other => return format!("Gateway domain {other}"),
    };
    (if testnet { test } else { main }).to_string()
}

/// The EVM chains a browser wallet can deposit into Circle Gateway from, for
/// the funding card on `/` (PLAN 5.33): Gateway domain, EVM chain id, the
/// USDC contract there, and the Gateway wallet contract (one address on every
/// chain). Testnet today; the mainnet table lands with the Base preset (5.9).
pub fn gateway_sources(testnet: bool) -> Vec<Value> {
    if !testnet {
        return Vec::new();
    }
    let rows: [(u64, u64, &str); 5] = [
        (0, 11_155_111, "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238"),
        (2, 11_155_420, "0x5fd84259d66Cd46123540766Be93DFE6D43130D7"),
        (3, 421_614, "0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d"),
        (6, 84_532, "0x036CbD53842c5426634e7929541eC2318f3dCF7e"),
        (26, 5_042_002, "0x3600000000000000000000000000000000000000"),
    ];
    rows.iter()
        .map(|(domain, chain_id, usdc)| {
            json!({
                "domain": domain,
                "chainId": chain_id,
                "name": gateway_domain_name(*domain, true),
                "usdc": usdc,
                "wallet": GATEWAY_WALLET_TESTNET,
            })
        })
        .collect()
}

/// The non-zero Gateway balances in a `/v1/balances` body, plus the total.
/// Pure, so the shape is pinned by a test without the network. A body that
/// does not parse, or a balance the currency cannot hold, is an error.
pub fn parse_gateway_balances(body: &str, testnet: bool) -> Result<Value, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("gateway: {e}"))?;
    let mut chains = Vec::new();
    let mut total: i128 = 0;
    for b in v["balances"].as_array().ok_or("gateway: no balances")? {
        let m = Money::parse(b["balance"].as_str().unwrap_or("0"), Currency::Usdc)
            .map_err(|e| format!("gateway: {e}"))?;
        if m.to_minor() <= 0 {
            continue;
        }
        let domain = b["domain"].as_u64().unwrap_or(u64::MAX);
        total += m.to_minor();
        chains.push(json!({
            "domain": domain,
            "chain": gateway_domain_name(domain, testnet),
            "balance": m.to_string(),
        }));
    }
    Ok(json!({
        "source": "Circle Gateway",
        "total": Money::minor(total, Currency::Usdc).to_string(),
        "chains": chains,
    }))
}

/// What the bill wallet holds outside dollars (PLAN 5.49): ETH on the
/// mainnet presets whose native asset is ETH, valued at Hyperliquid's public
/// mid price (read-only, no key). Display numbers, not money movement: the
/// hedge engine (crate `hedge`) does its own sizing from the venue's marks.
pub async fn wallet_exposure(wallet: Address) -> Value {
    let mut rows = Vec::new();
    let mut eth_total = 0f64;
    // A chain that did not answer is named, never counted as zero (audit
    // 2026-10-15, docs/60 P-73; the same rule as `eth_holdings`).
    let mut unread: Vec<&str> = Vec::new();
    for key in ["ethereum", "base", "arbitrum"] {
        let Some(c) = chain(key) else { continue };
        let Ok(client) = arc::ArcClient::read_only(&rpc_for(c)) else {
            unread.push(c.name);
            continue;
        };
        let Ok(wei) = client.native_wei(wallet).await else {
            unread.push(c.name);
            continue;
        };
        let eth = wei.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
        if eth > 0.0 {
            eth_total += eth;
            rows.push(json!({ "chain": c.name, "coin": "ETH", "size": format!("{eth:.6}") }));
        }
    }
    let mark = match gateway_http(10) {
        Ok(http) => match http
            .post("https://api.hyperliquid.xyz/info")
            .json(&json!({ "type": "allMids" }))
            .send()
            .await
        {
            Ok(r) => r
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v["ETH"].as_str().and_then(|p| p.parse::<f64>().ok())),
            Err(_) => None,
        },
        Err(_) => None,
    };
    json!({
        "holdings": rows,
        "ethTotal": format!("{eth_total:.6}"),
        "ethMark": mark.map(|m| format!("{m:.2}")),
        "notionalUsd": mark.map(|m| format!("{:.2}", eth_total * m)),
        "unread": unread,
        "complete": unread.is_empty(),
        "source": "ETH on Ethereum, Base and Arbitrum One read from public RPCs; priced at Hyperliquid's mid (api.hyperliquid.xyz, read-only)",
    })
}

/// One HTTP client for Gateway calls: a User-Agent (Cloudflare in front of
/// the API refuses some anonymous clients) and the whole error chain in the
/// message, not reqwest's one-line "error sending request".
fn gateway_http(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("tameion/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| e.to_string())
}

fn http_err(e: reqwest::Error) -> String {
    use std::error::Error;
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(x) = src {
        s.push_str(": ");
        s.push_str(&x.to_string());
        src = x.source();
    }
    s
}

/// USDC `depositor` holds in Circle Gateway, on every chain Gateway serves,
/// as one line: `{source, total, chains:[{domain, chain, balance}]}`, or
/// `{source, error}` when Gateway did not answer (never a silent zero).
/// Reading needs no key and nothing is sent; the depositor's own signature
/// moves it, so the agent cannot count it until it reaches the policy.
pub async fn gateway_holdings(c: &ChainPreset, depositor: Address) -> Value {
    let testnet = c.chain_id == ARC_TESTNET_CHAIN_ID;
    let r = async {
        let http = gateway_http(8)?;
        let resp = http
            .post(format!("{}/v1/balances", gateway_host(c)))
            .json(&json!({ "token": "USDC", "sources": [{ "depositor": format!("{depositor:#x}") }] }))
            .send()
            .await
            .map_err(|e| format!("gateway: {}", http_err(e)))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("gateway: {e}"))?;
        if !status.is_success() {
            return Err(format!("gateway: {status}: {text}"));
        }
        let mut out = parse_gateway_balances(&text, testnet)?;
        // Deposits Gateway has seen but not yet credited (an Arbitrum or Base
        // deposit waits ~65 Ethereum blocks): "arriving", so the page can say
        // so instead of showing nothing for a quarter of an hour.
        let resp = http
            .post(format!("{}/v1/deposits", gateway_host(c)))
            .json(&json!({ "token": "USDC", "sources": [{ "depositor": format!("{depositor:#x}") }] }))
            .send()
            .await
            .map_err(|e| format!("gateway deposits: {}", http_err(e)))?;
        if resp.status().is_success() {
            let text = resp.text().await.map_err(|e| format!("gateway: {e}"))?;
            out["arriving"] = json!(parse_gateway_deposits(&text, testnet)?);
        }
        Ok(out)
    }
    .await;
    // ponytail: two Gateway calls per inbox read (the page polls every few
    // seconds while a pass runs); a 60 s cache when Circle rate-limits us.
    r.unwrap_or_else(|e| json!({ "source": "Circle Gateway", "error": e }))
}

/// The deposits in a `/v1/deposits` body that Gateway has not credited yet:
/// `[{domain, chain, amount, tx, since}]`. Amounts there are minor units
/// (`"1000000"` = 1 USDC), unlike `/v1/balances`.
pub fn parse_gateway_deposits(body: &str, testnet: bool) -> Result<Vec<Value>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("gateway deposits: {e}"))?;
    Ok(v["deposits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["status"].as_str() == Some("pending"))
        .filter_map(|d| {
            let minor: i128 = d["amount"].as_str()?.parse().ok()?;
            let domain = d["domain"].as_u64()?;
            Some(json!({
                "domain": domain,
                "chain": gateway_domain_name(domain, testnet),
                "amount": Money::minor(minor, Currency::Usdc).to_string(),
                "tx": d["transactionHash"],
                "since": d["blockTimestamp"],
            }))
        })
        .collect())
}

// ---- Fund the policy from Gateway (PLAN 5.15, docs/56 #3)
//
// Source, read 2026-10-14: developers.circle.com/gateway/quickstarts/unified-balance-evm
// (EIP-712 domain {name:"GatewayWallet",version:"1"}, no chainId; BurnIntent
// {maxBlockHeight, maxFee, spec}; TransferSpec with bytes32 left-padded
// addresses; POST /v1/estimate gives maxFee and maxBlockHeight; POST
// /v1/transfer takes [{burnIntent, signature}] and answers {attestation,
// signature, transferId, fees}; GatewayMinter.gatewayMint(bytes,bytes) on the
// destination), /openapi/gateway.yaml (field types) and the contract
// addresses page (one wallet and one minter address on every EVM testnet).

/// Gateway's wallet and minter on every EVM testnet (Arc included).
pub const GATEWAY_WALLET_TESTNET: &str = "0x0077777d7EBA4688BDeF3E311b846F25870A19B9";
pub const GATEWAY_MINTER_TESTNET: &str = "0x0022222ABE238Cc2C7Bb1f21003F0a260052475B";

/// The USDC Gateway holds on each testnet domain (quickstart table).
pub fn gateway_testnet_usdc(domain: u64) -> Option<&'static str> {
    Some(match domain {
        0 => "0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238",
        1 => "0x5425890298aed601595a70ab815c96711a31bc65",
        3 => "0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d",
        6 => "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
        13 => "0x0BA304580ee7c9a980CF72e55f5Ed2E9fd30Bc51",
        14 => "0x66145f38cBAC35Ca6F1Dfb4914dF98F1614aeA88",
        16 => "0x4fCF1784B31630811181f670Aea7A7bEF803eaED",
        19 => "0x2B3370eE501B4a559b57D449569354196457D8Ab",
        26 => "0x3600000000000000000000000000000000000000",
        _ => return None,
    })
}

/// The Gateway domain a preset's chain lives on.
pub fn gateway_domain_of(c: &ChainPreset) -> Option<u64> {
    match c.key {
        "arc-testnet" => Some(26),
        "ethereum" => Some(0),
        "arbitrum" | "arbitrum-sepolia" => Some(3),
        "base" => Some(6),
        _ => None,
    }
}

/// An address as Gateway's bytes32: left-padded, as viem's `pad` does.
pub fn bytes32_of(a: Address) -> String {
    format!(
        "{:#x}",
        alloy::primitives::B256::left_padding_from(a.as_slice())
    )
}

/// The EIP-712 typed data the bill wallet signs to move `value` minor units
/// of its Gateway USDC from `source_domain` to `recipient` on the preset's
/// chain, with Circle's fee and expiry filled from `/v1/estimate`; with `all`,
/// `value_minor` is the balance and the fee cap is taken off it. Returns
/// `{typedData, fees, describe}`; nothing here can sign.
pub async fn gateway_fund_typed_data(
    c: &ChainPreset,
    source_domain: u64,
    depositor: Address,
    recipient: Address,
    value_minor: i128,
    all: bool,
) -> Result<Value, String> {
    if c.chain_id != ARC_TESTNET_CHAIN_ID {
        return Err("Gateway funding is configured for Arc testnet policies only".into());
    }
    if value_minor <= 0 {
        return Err("amount must be positive".into());
    }
    let dest_domain = gateway_domain_of(c).ok_or("no Gateway domain for this chain")?;
    let source_token = gateway_testnet_usdc(source_domain).ok_or_else(|| {
        format!("Gateway testnet domain {source_domain} is not one this service knows")
    })?;
    let dest_token = gateway_testnet_usdc(dest_domain).ok_or("no destination USDC")?;
    let a = |s: &str| {
        Address::from_str(s)
            .map(bytes32_of)
            .map_err(|e| e.to_string())
    };
    let spec = json!({
        "version": 1,
        "sourceDomain": source_domain,
        "destinationDomain": dest_domain,
        "sourceContract": a(GATEWAY_WALLET_TESTNET)?,
        "destinationContract": a(GATEWAY_MINTER_TESTNET)?,
        "sourceToken": a(source_token)?,
        "destinationToken": a(dest_token)?,
        "sourceDepositor": bytes32_of(depositor),
        "destinationRecipient": bytes32_of(recipient),
        "sourceSigner": bytes32_of(depositor),
        "destinationCaller": format!("{:#x}", alloy::primitives::B256::ZERO),
        "value": value_minor.to_string(),
        "salt": format!("{:#x}", alloy::primitives::B256::random()),
        "hookData": "0x",
    });
    let http = gateway_http(15)?;
    let resp = http
        .post(format!("{}/v1/estimate", gateway_host(c)))
        .json(&json!([{ "spec": spec }]))
        .send()
        .await
        .map_err(|e| format!("gateway estimate: {}", http_err(e)))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("gateway estimate: {status}: {text}"));
    }
    let est: Value = serde_json::from_str(&text).map_err(|e| format!("gateway estimate: {e}"))?;
    // The OpenAPI says `{body:[{burnIntent}], fees}`; the testnet API answered
    // a bare `[{burnIntent}]` on 2026-10-14. Take either.
    let items = est["body"]
        .as_array()
        .or(est.as_array())
        .ok_or("gateway estimate: no body")?;
    let bi = &items.first().ok_or("gateway estimate: empty")?["burnIntent"];
    let max_fee = bi["maxFee"]
        .as_str()
        .ok_or("gateway estimate: no maxFee")?
        .to_string();
    let max_height = bi["maxBlockHeight"]
        .as_str()
        .ok_or("gateway estimate: no maxBlockHeight")?
        .to_string();
    let fee_minor: i128 = max_fee
        .parse()
        .map_err(|_| "gateway estimate: maxFee not a number")?;
    // Gateway charges the fee on top of the value ("available 0.05, required
    // 0.0535", 2026-10-14). "All of it" therefore means the balance less the
    // fee cap; the estimate was taken on the larger value, so the cap holds.
    let mut spec = spec;
    let value_minor = if all {
        let v = value_minor - fee_minor;
        if v <= 0 {
            return Err(format!(
                "the balance {} does not cover Circle's fee of up to {}",
                Money::minor(value_minor, Currency::Usdc),
                Money::minor(fee_minor, Currency::Usdc)
            ));
        }
        spec["value"] = json!(v.to_string());
        v
    } else {
        value_minor
    };
    let typed = json!({
        "types": {
            "EIP712Domain": [ { "name": "name", "type": "string" }, { "name": "version", "type": "string" } ],
            "TransferSpec": [
                { "name": "version", "type": "uint32" }, { "name": "sourceDomain", "type": "uint32" }, { "name": "destinationDomain", "type": "uint32" },
                { "name": "sourceContract", "type": "bytes32" }, { "name": "destinationContract", "type": "bytes32" },
                { "name": "sourceToken", "type": "bytes32" }, { "name": "destinationToken", "type": "bytes32" },
                { "name": "sourceDepositor", "type": "bytes32" }, { "name": "destinationRecipient", "type": "bytes32" },
                { "name": "sourceSigner", "type": "bytes32" }, { "name": "destinationCaller", "type": "bytes32" },
                { "name": "value", "type": "uint256" }, { "name": "salt", "type": "bytes32" }, { "name": "hookData", "type": "bytes" }
            ],
            "BurnIntent": [ { "name": "maxBlockHeight", "type": "uint256" }, { "name": "maxFee", "type": "uint256" }, { "name": "spec", "type": "TransferSpec" } ]
        },
        "primaryType": "BurnIntent",
        "domain": { "name": "GatewayWallet", "version": "1" },
        "message": { "maxBlockHeight": max_height, "maxFee": max_fee, "spec": spec },
    });
    let describe = format!(
        "Move {} of your USDC in Circle Gateway on {} to the policy {:#x} on {}. Circle keeps a fee of at most {}. This signature authorises nothing else.",
        Money::minor(value_minor, Currency::Usdc),
        gateway_domain_name(source_domain, true),
        recipient,
        c.name,
        Money::minor(fee_minor, Currency::Usdc),
    );
    let fees = if est["fees"].is_null() {
        json!({ "total": max_fee, "token": "USDC", "note": "Gateway's estimate carried no fee summary; total = maxFee in minor units" })
    } else {
        est["fees"].clone()
    };
    Ok(json!({ "typedData": typed, "fees": fees, "describe": describe }))
}

/// Hand a signed burn intent to Gateway; returns `{attestation, signature,
/// transferId, fees, expirationBlock}` for the mint on the destination.
pub async fn gateway_transfer(
    c: &ChainPreset,
    burn_intent: &Value,
    signature: &str,
) -> Result<Value, String> {
    let http = gateway_http(60)?;
    let resp = http
        .post(format!("{}/v1/transfer", gateway_host(c)))
        .json(&json!([{ "burnIntent": burn_intent, "signature": signature }]))
        .send()
        .await
        .map_err(|e| format!("gateway transfer: {}", http_err(e)))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("gateway transfer: {status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("gateway transfer: {e}"))
}

/// `gatewayMint(bytes attestationPayload, bytes signature)` calldata:
/// selector, two offsets, then each `bytes` as length + right-padded data.
pub fn gateway_mint_calldata(attestation: &[u8], signature: &[u8]) -> Vec<u8> {
    fn word(n: usize) -> [u8; 32] {
        let mut w = [0u8; 32];
        w[24..].copy_from_slice(&(n as u64).to_be_bytes());
        w
    }
    fn tail(b: &[u8]) -> Vec<u8> {
        let mut t = word(b.len()).to_vec();
        t.extend_from_slice(b);
        t.resize(32 + b.len().div_ceil(32) * 32, 0);
        t
    }
    let a = tail(attestation);
    let mut out = vec![0x9f, 0xb0, 0x1c, 0xc5];
    out.extend_from_slice(&word(64));
    out.extend_from_slice(&word(64 + a.len()));
    out.extend_from_slice(&a);
    out.extend_from_slice(&tail(signature));
    out
}

/// The caps a reader chooses to see the maximum loss under.
#[derive(Debug, Clone)]
pub struct Caps {
    pub per_action: Money,
    pub daily: Money,
    pub floor: Money,
}

#[derive(Debug, Clone)]
pub struct WatchParams {
    pub chain: &'static ChainPreset,
    pub address: Address,
    pub also_history: Vec<Address>,
    pub days: u32,
    pub caps: Option<Caps>,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub phase: String,
    pub scanned_blocks: u64,
    pub total_blocks: u64,
    pub transfers_so_far: usize,
    /// Seconds left in the scan's time budget, if it has one: the ETA a
    /// page shows never exceeds it (a 90-day rate estimate said "78 min"
    /// to a client whose pass would stop at ten; docs/60 P-38).
    pub eta_cap_secs: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error(
        "{chain}'s public RPC refuses log scans at this volume ({detail}). Set {env} to a keyed endpoint (an Alchemy or QuickNode free tier is enough) and run again."
    )]
    RpcLimited {
        chain: &'static str,
        env: &'static str,
        detail: String,
    },
    #[error(transparent)]
    Chain(#[from] ArcError),
    #[error("the chain reports a clock before the Unix epoch")]
    Clock,
    #[error("money: {0}")]
    Money(String),
}

pub fn rpc_for(c: &ChainPreset) -> String {
    std::env::var(c.rpc_env).unwrap_or_else(|_| c.default_rpc.to_string())
}

/// A read-only client for `c`: its token, its log window.
pub fn client_for(c: &ChainPreset) -> Result<ArcClient, ArcError> {
    let mut client = ArcClient::read_only(&rpc_for(c))?.with_log_chunk(c.log_span);
    if let Some(t) = c.usdc {
        client = client.with_token(Address::from_str(t).expect("preset token address"));
    }
    Ok(client)
}

/// Pay now if due within three days, or within a tenth of the interval when
/// that is shorter. Same rule as the operator's; copied rather than shared
/// because the operator's is private and this crate's public surface should
/// not grow for one line.
fn within_pay_lead(o: &Obligation, now: Timestamp) -> bool {
    let three_days = Seconds::days(3).expect("3 days");
    let tenth = Seconds::new(o.interval.as_i64() / 10);
    let lead = if tenth < three_days {
        tenth
    } else {
        three_days
    };
    o.next_due - now <= lead
}

/// What `o` costs in `now..=now + horizon`, in USDC minor units: every due
/// date at its cadence, the next one counted even if just past — except a
/// bill already a full cycle or more behind (nobody paid the next one
/// either: "it may have ended", the agent's cycles-behind check), which
/// counts once. Counting each bill once under-counted a weekly bill four
/// times over in "the next 30 days come to"; projecting a bill that may have
/// ended at its cadence told a client with 0.35 USDC of bills that the
/// account needed 15,084 USDC more (docs/70 C8).
///
/// The dates are the one bill walk's (`ledger::momentum::due_dates`).
pub fn due_within(o: &Obligation, now: Timestamp, horizon: Seconds) -> i128 {
    ledger::momentum::due_dates(&[o.into()], now.as_unix(), horizon.as_i64())
        .iter()
        .map(|d| d.1)
        .sum()
}

/// Read, infer, judge, and say so. `progress` is called between scan slices.
/// The sliced scan Watch and a Level 1 pass share: every USDC transfer
/// touching `wallets` in `from..=head`, one progress tick per slice (about
/// forty per run), so a six-minute wait on a public RPC shows a bar and an
/// ETA instead of a spinner.
pub async fn scan_transfers(
    client: &ArcClient,
    c: &ChainPreset,
    wallets: &[Address],
    from: u64,
    head: u64,
    progress: &impl Fn(Progress),
) -> Result<Vec<Transfer>, WatchError> {
    scan_transfers_budgeted(client, c, wallets, from, head, None, progress)
        .await
        .map(|(t, _)| t)
}

/// The same scan, newest block first, stopping at `deadline` if there is one.
/// Returns the transfers read and the oldest block actually reached: on a
/// public node that answers one 10k-block window every few seconds, a
/// 90-day window is an hour, and a first pass that fails after twenty
/// minutes is worse than one that reads the newest weeks and says so. The
/// newest history is what the bills are inferred from; the rest can wait.
pub async fn scan_transfers_budgeted(
    client: &ArcClient,
    c: &ChainPreset,
    wallets: &[Address],
    from: u64,
    head: u64,
    deadline: Option<std::time::Instant>,
    progress: &impl Fn(Progress),
) -> Result<(Vec<Transfer>, u64), WatchError> {
    let total_blocks = head.saturating_sub(from) + 1;
    let cap = || {
        deadline.map(|d| {
            d.saturating_duration_since(std::time::Instant::now())
                .as_secs()
        })
    };
    // The explorer's index answers the whole window in about a second where
    // the node answers it in an hour or not at all (Arc's public node keeps
    // a few days of logs). The index is a reader's convenience, not
    // authority: what it lists only proposes bills, and paying a payee still
    // needs the owner's passkey and the contract's caps. Any failure falls
    // back to the chain.
    if let Some(api) = c.indexer.filter(|_| indexer_usable(c)) {
        match indexed_window(client, c, api, wallets, from, head).await {
            Ok((transfers, reached)) => {
                progress(Progress {
                    phase: format!("read blocks {reached}–{head} from the explorer index"),
                    scanned_blocks: total_blocks,
                    total_blocks,
                    transfers_so_far: transfers.len(),
                    eta_cap_secs: None,
                });
                return Ok((transfers, reached));
            }
            Err(e) => eprintln!(
                "watch: {} index unavailable ({e}); reading the chain",
                c.name
            ),
        }
    }
    let floor = c.log_span.max(10_000);
    let slice = (total_blocks / 40).clamp(floor, floor.max(200_000));
    let mut transfers: Vec<Transfer> = Vec::new();
    let mut end = head;
    let mut reached;
    loop {
        let start = end.saturating_sub(slice - 1).max(from);
        for w in wallets {
            match client.transfers(*w, start, end).await {
                Ok(t) => transfers.extend(t),
                Err(ArcError::RangeTooWide { detail, .. })
                    if detail.to_ascii_lowercase().contains("limit") =>
                {
                    return Err(WatchError::RpcLimited {
                        chain: c.name,
                        env: c.rpc_env,
                        detail,
                    });
                }
                Err(e) => return Err(e.into()),
            }
        }
        reached = start;
        progress(Progress {
            phase: format!("reading newest first: blocks {start}–{end} of {from}–{head}"),
            scanned_blocks: head - start + 1,
            total_blocks,
            transfers_so_far: transfers.len(),
            eta_cap_secs: cap(),
        });
        if start == from {
            break;
        }
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            break;
        }
        // Leave the node room for everyone else: a back-to-back scan tripped
        // the public endpoint's -32005 "rate limit exceeded" for the relayer's
        // own transactions (seen 2026-10-14 while a first pass ran). A slice
        // that is one call per wallet (Arbitrum's millions of blocks) is not
        // that scan and does not wait.
        if slice > c.log_span {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
        end = start - 1;
    }
    Ok((transfers, reached))
}

/// Whether the explorer index may stand in for the node: not when the RPC
/// is a local fork (the index describes the public chain, not the fork),
/// and not when `TAMEION_INDEXER=off`.
fn indexer_usable(c: &ChainPreset) -> bool {
    let rpc = rpc_for(c);
    std::env::var("TAMEION_INDEXER").map_or(true, |v| v != "off")
        && !rpc.contains("127.0.0.1")
        && !rpc.contains("localhost")
}

/// Every wallet's transfers in `from..=head` from the index, plus the newest
/// two log windows read from the node itself (the index trails the chain
/// by a few blocks). Returns the oldest block the index reached.
async fn indexed_window(
    client: &ArcClient,
    c: &ChainPreset,
    api: &str,
    wallets: &[Address],
    from: u64,
    head: u64,
) -> Result<(Vec<Transfer>, u64), String> {
    let mut all = Vec::new();
    let mut reached = from;
    for w in wallets {
        let (t, r) = indexed_transfers(api, client.token(), *w, from, head).await?;
        all.extend(t);
        reached = reached.max(r);
    }
    let tail = head.saturating_sub(2 * c.log_span).max(from);
    for w in wallets {
        all.extend(
            client
                .transfers(*w, tail, head)
                .await
                .map_err(|e| e.to_string())?,
        );
    }
    dedup_transfers(&mut all);
    Ok((all, reached))
}

/// One row per (tx, direction, counterparty, amount), oldest first: the
/// same transfer read from the index and from the node, or from two
/// overlapping passes, counts once.
pub fn dedup_transfers(v: &mut Vec<Transfer>) {
    let mut seen = std::collections::BTreeSet::new();
    v.retain(|t| {
        seen.insert((
            t.tx_hash.to_lowercase(),
            t.direction == Direction::Out,
            t.counterparty.as_str().to_string(),
            t.amount.to_minor(),
        ))
    });
    v.sort_by_key(|t| t.at);
}

/// Rows the index returns per page; pages are walked newest first.
const INDEX_PAGE: usize = 1_000;
/// Pages read before stopping (20,000 transfers): a wallet busier than that
/// keeps its newest history and says how far back it reached.
const INDEX_PAGES: usize = 20;

/// `wallet`'s transfers of `token` in `from..=to` from an Etherscan-compatible
/// `tokentx` endpoint, and the oldest block reached.
async fn indexed_transfers(
    api: &str,
    token: Address,
    wallet: Address,
    from: u64,
    to: u64,
) -> Result<(Vec<Transfer>, u64), String> {
    let http = gateway_http(20)?;
    let mut out = Vec::new();
    let mut end = to;
    for _ in 0..INDEX_PAGES {
        let url = format!(
            "{api}?module=account&action=tokentx&address={wallet:#x}&contractaddress={token:#x}&startblock={from}&endblock={end}&page=1&offset={INDEX_PAGE}&sort=desc"
        );
        let v: Value = http
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let rows = v["result"]
            .as_array()
            .ok_or_else(|| format!("index answered {}", v["message"]))?;
        let mut oldest = end;
        for r in rows {
            let t = index_row(r, wallet)?;
            oldest = oldest.min(t.0);
            if let Some(t) = t.1 {
                out.push(t);
            }
        }
        if rows.len() < INDEX_PAGE {
            return Ok((out, from));
        }
        if oldest == end {
            // A whole page inside one block: paging by block cannot move.
            return Err(format!("more than {INDEX_PAGE} transfers in block {end}"));
        }
        // The oldest block may be cut mid-page: ask for it again (dedup
        // keeps one copy).
        end = oldest;
    }
    // Block `end` was only partly read (its page was cut there): the history
    // reached is everything after it, so a later pass re-reads that block.
    Ok((out, end + 1))
}

/// One `tokentx` row as (block, transfer touching `wallet`); a self-transfer
/// is no transfer.
fn index_row(r: &Value, wallet: Address) -> Result<(u64, Option<Transfer>), String> {
    let field = |k: &str| {
        r[k].as_str()
            .ok_or_else(|| format!("index row without {k}"))
    };
    let block: u64 = field("blockNumber")?
        .parse()
        .map_err(|_| "bad blockNumber")?;
    if field("tokenDecimal")? != "6" {
        return Err(format!(
            "token has {} decimals, not 6",
            field("tokenDecimal")?
        ));
    }
    let me = format!("{wallet:#x}");
    let (src, dst) = (field("from")?.to_lowercase(), field("to")?.to_lowercase());
    let (direction, other) = match (src == me, dst == me) {
        (true, false) => (Direction::Out, dst),
        (false, true) => (Direction::In, src),
        _ => return Ok((block, None)),
    };
    let minor: i128 = field("value")?.parse().map_err(|_| "bad value")?;
    let at: i64 = field("timeStamp")?.parse().map_err(|_| "bad timeStamp")?;
    Ok((
        block,
        Some(Transfer {
            tx_hash: field("hash")?.to_lowercase(),
            counterparty: ledger::obligations::Counterparty::new(other),
            amount: Money::minor(minor, Currency::Usdc),
            at: Timestamp::from_unix(at),
            direction,
        }),
    ))
}

pub async fn watch(p: &WatchParams, progress: impl Fn(Progress)) -> Result<Value, WatchError> {
    let started = std::time::Instant::now();
    let c = p.chain;
    // Base's public endpoint answers eth_getLogs with -32011 "request limit
    // reached" at any volume (measured 2026-10-13); say so at once instead
    // of after minutes of backoff.
    if c.needs_key && !keyed(c) {
        return Err(WatchError::RpcLimited {
            chain: c.name,
            env: c.rpc_env,
            detail: format!("{} refuses log scans without a key", c.default_rpc),
        });
    }
    let client = client_for(c)?;

    progress(Progress {
        phase: "reading the chain clock".into(),
        scanned_blocks: 0,
        total_blocks: 0,
        transfers_so_far: 0,
        eta_cap_secs: None,
    });
    // Independent reads together: each is a round trip (0.4 s from here to
    // Arbitrum's public node), and a first check is judged in seconds.
    let (chain_ok, head) = tokio::join!(client.expect_chain(c.chain_id), client.block_number());
    chain_ok?;
    let head = head?;
    // Seconds per block from the chain itself, so "days" means days on every
    // chain this runs on.
    let probe = head.saturating_sub(10_000);
    let (now, then) = tokio::join!(client.block_timestamp(head), client.block_timestamp(probe));
    let (now, then) = (now?, then?);
    let span = (now - then).as_i64().max(1) as f64;
    let secs_per_block = span / (head - probe).max(1) as f64;
    let total_blocks = ((p.days as f64) * 86_400.0 / secs_per_block).ceil() as u64;
    let from = head.saturating_sub(total_blocks);

    let wallets: Vec<Address> = std::iter::once(p.address)
        .chain(p.also_history.iter().copied())
        .collect();
    // The scan, the Gateway balances and the exposure prices do not depend
    // on each other: read them at once.
    let (scanned, elsewhere, mut exposure) = tokio::join!(
        scan_transfers(&client, c, &wallets, from, head, &progress),
        gateway_holdings(c, p.address),
        wallet_exposure(p.address),
    );
    let mut transfers = scanned?;
    // Transfers among the watched wallets are the owner moving their own
    // money, not evidence of a payee.
    let own: std::collections::BTreeSet<String> =
        wallets.iter().map(|a| format!("{a:#x}")).collect();
    transfers.retain(|t| !own.contains(&t.counterparty.to_string().to_lowercase()));
    transfers.sort_by_key(|t| t.at);

    progress(Progress {
        phase: "inferring obligations".into(),
        scanned_blocks: head - from + 1,
        total_blocks: head - from + 1,
        transfers_so_far: transfers.len(),
        eta_cap_secs: None,
    });
    let cfg = DetectionConfig {
        dust_line: Some(
            Money::parse(c.dust, Currency::Usdc).map_err(|e| WatchError::Money(e.to_string()))?,
        ),
        ..DetectionConfig::default()
    };
    let obligations = detect(&transfers, cfg);
    let horizon = Seconds::days(30).expect("30 days");
    let upcoming: Vec<&Obligation> = upcoming(&obligations, now, horizon);
    let due: Vec<&Obligation> = upcoming
        .iter()
        .copied()
        .filter(|o| within_pay_lead(o, now))
        .collect();
    let balance = client.usdc_balance(p.address).await?;
    // What the same address holds in Circle Gateway on other chains (PLAN
    // 5.14): shown beside the balance, never counted as the balance.

    let liquidity_30d = Money::minor(
        obligations
            .iter()
            .map(|o| due_within(o, now, horizon))
            .sum(),
        Currency::Usdc,
    );

    // The order the deterministic rule would act in; the model's ordering is
    // a level-2 matter and needs a key.
    let ranked = rank_by_urgency(
        &due.iter().map(|o| (*o).clone()).collect::<Vec<_>>(),
        balance,
        now,
    );

    let outgoing = transfers
        .iter()
        .filter(|t| t.direction == Direction::Out)
        .count();
    let counterparties = transfers
        .iter()
        .filter(|t| t.direction == Direction::Out)
        .map(|t| t.counterparty.to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .len();

    // The range each payee was actually paid in, and the timing spread, so a
    // reader sees "between X and Y, around the 1st ± 2 days" rather than one
    // typical figure (Modern Treasury's expected payments carry bounds the
    // operator declares; here they come from the chain, `docs/50`).
    let range_of = |o: &Obligation| -> (Money, Money) {
        let mut amounts: Vec<Money> = transfers
            .iter()
            .filter(|t| t.direction == Direction::Out && t.counterparty == o.counterparty)
            .map(|t| t.amount)
            .collect();
        amounts.sort_by_key(|m| m.to_minor());
        match (amounts.first(), amounts.last()) {
            (Some(a), Some(b)) => (*a, *b),
            _ => (o.typical_amount, o.typical_amount),
        }
    };
    let obligation_json = |o: &Obligation| -> Value {
        let overdue = o.overdue_by(now);
        let (lo, hi) = range_of(o);
        let due_window_days = o.interval.as_days() * o.interval_cv;
        let is_due = due
            .iter()
            .any(|d| d.counterparty == o.counterparty && d.next_due == o.next_due);
        let (verdict, why) = match (&p.caps, is_due) {
            (Some(caps), true) => {
                if o.confidence.get()
                    < f64::from(ledger::action::CONFIDENCE_FLOOR_PROPOSE_BP) / 10_000.0
                {
                    (
                        "ask",
                        format!("confidence {} is below the 0.60 floor", o.confidence),
                    )
                } else if o.confidence.get()
                    < f64::from(ledger::action::CONFIDENCE_FLOOR_AUTOPILOT_BP) / 10_000.0
                {
                    // docs/76 §7 row 7: between the two floors the agent brings
                    // the bill to the owner in Propose and asks in Autopilot.
                    (
                        "ask",
                        format!(
                            "confidence {} is below the 0.75 Autopilot floor; in Propose (floor 0.60) the agent would bring it to you",
                            o.confidence
                        ),
                    )
                } else if o.typical_amount.to_minor() > caps.per_action.to_minor() {
                    (
                        "ask",
                        format!(
                            "{} is over the per-action cap {}",
                            o.typical_amount, caps.per_action
                        ),
                    )
                } else {
                    (
                        "pay",
                        "due now, inside the cap, once you have approved this payee".into(),
                    )
                }
            }
            (None, true) => ("pay-or-ask", "due now; choose caps to see which".into()),
            (_, false) => ("wait", "not due yet".into()),
        };
        json!({
            "counterparty": o.counterparty.to_string(),
            "kind": format!("{:?}", o.kind),
            "amount": o.typical_amount.to_string(),
            "amountMin": lo.to_string(),
            "amountMax": hi.to_string(),
            "dueWindowDays": due_window_days,
            "every": o.interval.human(),
            "intervalSecs": o.interval.as_i64(),
            "confidence": o.confidence.to_string(),
            "lastPaid": o.last_paid.as_unix(),
            "nextDue": o.next_due.as_unix(),
            "overdueDays": overdue.as_days(),
            "samples": o.sample_count,
            "evidence": o.evidence,
            "explain": o.explain(),
            "verdict": verdict,
            "why": why,
            "attested": false,
        })
    };

    let max_loss = p.caps.as_ref().map(|caps| {
        let bal = balance.to_minor();
        let agent_24h = (caps.daily.to_minor()).min((bal - caps.floor.to_minor()).max(0)).max(0);
        json!({
            "perActionCap": caps.per_action.to_string(),
            "dailyCap": caps.daily.to_string(),
            "reserveFloor": caps.floor.to_string(),
            "agentKey24h": Money::minor(agent_24h, Currency::Usdc).to_string(),
            "agentKeyPerCall": caps.per_action.to_string(),
            "operatorKeyNoDelay": balance.to_string(),
            "operatorKeyWith48h": Money::minor(caps.daily.to_minor() * 2, Currency::Usdc).to_string(),
            "principal": balance.to_string(),
            "onlyTo": "payees you approved with your passkey",
        })
    });

    // PLAN 5.49: what the address holds outside dollars, and what a 30% fall
    // would cost in days of the bills this report found.

    {
        let notional = exposure["notionalUsd"]
            .as_str()
            .and_then(|x| x.parse::<f64>().ok())
            .unwrap_or(0.0);
        let per_day = liquidity_30d
            .to_string()
            .trim_end_matches(" USDC")
            .parse::<f64>()
            .unwrap_or(0.0)
            / 30.0;
        let fall = notional * 0.3;
        exposure["fall30Usd"] = json!(format!("{fall:.2}"));
        exposure["fall30Days"] = if per_day > 0.0 {
            json!((fall / per_day).floor() as u64)
        } else {
            Value::Null
        };
    }

    Ok(json!({
        "chain": { "key": c.key, "name": c.name, "chainId": c.chain_id, "explorer": c.explorer, "usdc": c.usdc },
        "address": format!("{:#x}", p.address),
        "exposure": exposure,
        "alsoHistory": p.also_history.iter().map(|a| format!("{a:#x}")).collect::<Vec<_>>(),
        "days": p.days,
        "fromBlock": from,
        "toBlock": head,
        "now": now.as_unix(),
        "secsPerBlock": secs_per_block,
        "transfers": transfers.len(),
        "outgoing": outgoing,
        "counterparties": counterparties,
        "balance": balance.to_string(),
        "holdingsElsewhere": elsewhere,
        "obligations": obligations.iter().map(obligation_json).collect::<Vec<_>>(),
        "upcoming30d": upcoming.len(),
        "dueNow": due.len(),
        "liquidity30d": liquidity_30d.to_string(),
        "orderRule": ranked.iter().map(|r| json!({ "counterparty": r.obligation.counterparty.to_string(), "rationale": r.rationale })).collect::<Vec<_>>(),
        "maxLoss": max_loss,
        "readOnly": true,
        "elapsedMs": started.elapsed().as_millis(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_row_reads_as_the_node_would_and_duplicates_count_once() {
        let me = Address::from_str("0x6c1ce01bfe3f834a5fbfa0a9081d5d976b819a25").unwrap();
        // A real row from explorer.testnet.arc.io (2026-10-14), trimmed.
        let row = json!({ "value": "10000", "blockNumber": "67173633", "from": "0x6c1ce01bfe3f834a5fbfa0a9081d5d976b819a25",
            "to": "0xB227c2a801d02e99bc7052255dd8b4279a41bd55", "timeStamp": "1791988210", "tokenDecimal": "6",
            "hash": "0xE2468DCF77101bd7517e16be22c22842a252270bef66aaf5c8198924e34bd75a" });
        let (block, t) = index_row(&row, me).unwrap();
        let t = t.unwrap();
        assert_eq!(block, 67_173_633);
        assert_eq!(t.direction, Direction::Out);
        assert_eq!(
            t.counterparty.as_str(),
            "0xb227c2a801d02e99bc7052255dd8b4279a41bd55"
        );
        assert_eq!(t.amount.to_string(), "0.010000 USDC");
        assert_eq!(t.at.as_unix(), 1_791_988_210);
        // The same transfer read from the node (lowercase hash) counts once.
        let mut v = vec![
            t.clone(),
            Transfer {
                tx_hash: t.tx_hash.to_uppercase(),
                ..t.clone()
            },
        ];
        dedup_transfers(&mut v);
        assert_eq!(v.len(), 1);
        // A self-transfer is no transfer; another token's decimals are refused.
        let selfie = json!({ "value": "1", "blockNumber": "1", "from": format!("{me:#x}"), "to": format!("{me:#x}"),
            "timeStamp": "1", "tokenDecimal": "6", "hash": "0x1" });
        assert!(index_row(&selfie, me).unwrap().1.is_none());
        let other = json!({ "value": "1", "blockNumber": "1", "from": "0x1", "to": format!("{me:#x}"),
            "timeStamp": "1", "tokenDecimal": "18", "hash": "0x1" });
        assert!(index_row(&other, me).is_err());
    }

    #[test]
    fn a_bill_counts_at_its_cadence_unless_it_may_have_ended() {
        use ledger::obligations::{Counterparty, ObligationKind};
        let now = Timestamp::from_unix(1_000_000);
        let month = Seconds::days(30).unwrap();
        let mk = |every_days: i64, next_in: i64| Obligation {
            counterparty: Counterparty::new("0xabc"),
            kind: ObligationKind::Subscription,
            typical_amount: Money::minor(100, Currency::Usdc),
            interval: Seconds::days(every_days).unwrap(),
            interval_cv: 0.0,
            amount_cv: 0.0,
            last_paid: now,
            next_due: Timestamp::from_unix(now.as_unix() + next_in),
            sample_count: 4,
            confidence: ledger::obligations::Confidence::new(0.9).unwrap(),
            evidence: Vec::new(),
        };
        // A weekly bill due tomorrow: five dates in 30 days, not one.
        assert_eq!(due_within(&mk(7, 86_400), now, month), 500);
        // A monthly bill due in ten days: once.
        assert_eq!(due_within(&mk(30, 10 * 86_400), now, month), 100);
        // Due after the horizon: nothing.
        assert_eq!(due_within(&mk(30, 40 * 86_400), now, month), 0);
        // Overdue by less than a cycle: still counted at its cadence.
        assert_eq!(due_within(&mk(7, -86_400), now, month), 500);
        // Two cycles behind: it may have ended — once, not 30 days of it.
        assert_eq!(due_within(&mk(1, -3 * 86_400), now, month), 100);
    }

    #[test]
    fn every_preset_reads_logs_in_measured_windows() {
        for c in CHAINS {
            assert!(c.log_span >= 10_000, "{}", c.key);
        }
        assert_eq!(chain("arbitrum-sepolia").unwrap().log_span, 5_000_000);
        assert!(chain("arc-testnet").unwrap().indexer.is_some());
    }

    #[test]
    fn gateway_sources_name_real_domains_and_one_wallet() {
        let rows = gateway_sources(true);
        assert_eq!(rows.len(), 5);
        for r in &rows {
            assert_eq!(r["wallet"], GATEWAY_WALLET_TESTNET);
            assert!(!r["name"].as_str().unwrap().starts_with("Gateway domain"));
            assert_eq!(r["usdc"].as_str().unwrap().len(), 42);
        }
        assert_eq!(rows[2]["chainId"], 421_614);
        assert!(gateway_sources(false).is_empty());
    }

    #[test]
    fn gateway_balances_keep_only_what_is_there_and_total_it() {
        // The shape Circle's testnet API answered on 2026-10-14 (every
        // domain, zero or not, plus a `pendingBatch` we ignore).
        let body = r#"{"token":"USDC","balances":[
            {"domain":0,"depositor":"0xabc","balance":"0","pendingBatch":"0"},
            {"domain":6,"depositor":"0xabc","balance":"1.25","pendingBatch":"0"},
            {"domain":26,"depositor":"0xabc","balance":"0.050000","pendingBatch":"0"},
            {"domain":99,"depositor":"0xabc","balance":"2","pendingBatch":"0"}]}"#;
        let v = parse_gateway_balances(body, true).unwrap();
        assert_eq!(v["source"], "Circle Gateway");
        assert_eq!(v["total"], "3.300000 USDC");
        let chains = v["chains"].as_array().unwrap();
        assert_eq!(chains.len(), 3, "zero balances are not listed");
        assert_eq!(chains[0]["chain"], "Base Sepolia");
        assert_eq!(chains[0]["balance"], "1.250000 USDC");
        assert_eq!(chains[1]["chain"], "Arc Testnet");
        assert_eq!(chains[2]["chain"], "Gateway domain 99");
        assert_eq!(gateway_domain_name(6, false), "Base");
        assert!(parse_gateway_balances("{}", true).is_err());
        assert!(
            parse_gateway_balances(r#"{"balances":[{"domain":6,"balance":"0.1234567"}]}"#, true)
                .is_err(),
            "more precision than USDC settles at is an error, not a rounding"
        );
        assert_eq!(
            gateway_host(chain("arc-testnet").unwrap()),
            "https://gateway-api-testnet.circle.com"
        );
        assert_eq!(
            gateway_host(chain("base").unwrap()),
            "https://gateway-api.circle.com"
        );
    }

    #[test]
    fn pending_gateway_deposits_are_listed_as_arriving() {
        // Shape answered on 2026-10-14 for a 1 USDC deposit on Arbitrum Sepolia.
        let body = r#"{"token":"USDC","deposits":[{"depositor":"0xabc","domain":3,"transactionHash":"0x764b","amount":"1000000","status":"pending","blockHeight":"318967330","blockHash":"0x25","blockTimestamp":"2026-10-14T03:56:16.000Z"},{"depositor":"0xabc","domain":26,"transactionHash":"0x6a4b","amount":"50000","status":"finalized"}]}"#;
        let a = parse_gateway_deposits(body, true).unwrap();
        assert_eq!(a.len(), 1, "only pending ones");
        assert_eq!(a[0]["chain"], "Arbitrum Sepolia");
        assert_eq!(a[0]["amount"], "1.000000 USDC");
        assert_eq!(a[0]["tx"], "0x764b");
        assert!(parse_gateway_deposits("{}", true).unwrap().is_empty());
    }

    #[test]
    fn gateway_mint_calldata_matches_cast() {
        // `cast calldata 'gatewayMint(bytes,bytes)' 0x1234 0xabcdef`, 2026-10-14.
        let want = "0x9fb01cc5\
            0000000000000000000000000000000000000000000000000000000000000040\
            0000000000000000000000000000000000000000000000000000000000000080\
            0000000000000000000000000000000000000000000000000000000000000002\
            1234000000000000000000000000000000000000000000000000000000000000\
            0000000000000000000000000000000000000000000000000000000000000003\
            abcdef0000000000000000000000000000000000000000000000000000000000";
        let got =
            alloy::hex::encode_prefixed(gateway_mint_calldata(&[0x12, 0x34], &[0xab, 0xcd, 0xef]));
        assert_eq!(got, want);
        // A 32-byte payload takes one word, no padding word after it.
        let d = gateway_mint_calldata(&[1u8; 32], &[]);
        assert_eq!(d.len(), 4 + 32 * 2 + 32 + 32 + 32);
        assert_eq!(
            bytes32_of(Address::from_str("0x1645951793f7ca2ae36d7a3eeae90ef93e1932ab").unwrap()),
            "0x0000000000000000000000001645951793f7ca2ae36d7a3eeae90ef93e1932ab"
        );
        assert_eq!(gateway_domain_of(chain("arc-testnet").unwrap()), Some(26));
        assert_eq!(
            gateway_testnet_usdc(3),
            Some("0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d")
        );
        assert!(gateway_testnet_usdc(99).is_none());
    }

    #[test]
    fn testnet_eth_never_counts_as_holdings() {
        assert!(is_testnet(chain("arbitrum-sepolia").unwrap()));
        assert!(is_testnet(chain("arc-testnet").unwrap()));
        assert!(!is_testnet(chain("arbitrum").unwrap()));
    }

    #[test]
    fn every_preset_is_well_formed_and_distinct() {
        let mut keys = std::collections::BTreeSet::new();
        for c in CHAINS {
            assert!(keys.insert(c.key), "duplicate chain key {}", c.key);
            if let Some(t) = c.usdc {
                Address::from_str(t).unwrap_or_else(|_| panic!("{}: bad token address", c.key));
            }
            Money::parse(c.dust, Currency::Usdc).unwrap_or_else(|_| panic!("{}: bad dust", c.key));
            assert!(c.default_rpc.starts_with("https://"));
            assert!(chain(c.key).is_some());
        }
        assert!(chain("nope").is_none());
    }

    #[test]
    fn the_pay_lead_is_a_tenth_of_the_interval_or_three_days() {
        use ledger::obligations::{Confidence, Counterparty, ObligationKind};
        let now = Timestamp::from_unix(1_800_000_000);
        let mk = |interval_days: i64, due_in_secs: i64| Obligation {
            counterparty: Counterparty::new("0x0000000000000000000000000000000000000001"),
            kind: ObligationKind::Subscription,
            typical_amount: Money::parse("10", Currency::Usdc).unwrap(),
            interval: Seconds::days(interval_days).unwrap(),
            last_paid: now - Seconds::days(interval_days).unwrap(),
            next_due: now + Seconds::new(due_in_secs),
            confidence: Confidence::clamped(0.9),
            sample_count: 4,
            interval_cv: 0.0,
            amount_cv: 0.0,
            evidence: vec![],
        };
        // Monthly: a tenth is three days; due in two days is inside the lead.
        assert!(within_pay_lead(&mk(30, 2 * 86_400), now));
        assert!(!within_pay_lead(&mk(30, 4 * 86_400), now));
        // Ten-day cycle: a tenth is one day, shorter than three.
        assert!(
            !within_pay_lead(&mk(10, 2 * 86_400), now),
            "ten-day cycle: lead is one day"
        );
        assert!(within_pay_lead(&mk(10, 12 * 3_600), now));
    }
}
