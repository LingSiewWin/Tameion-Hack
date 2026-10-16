//! How many RPC round trips one agent pass costs, by phase (docs/68 §3).
//! Read-only: the pass runs with `execute = false` (no payment, no anchor).
//!
//! ```text
//! python3 rpc_count_proxy.py 8547 <public rpc> /dev/null &
//! cargo run -p operator --example pass_rpc_cost -- <chain> <policy> <watched> \
//!     <created_block> <history_from_block> http://0.0.0.0:8547 [warm_blocks]
//! ```
//!
//! Point the client at the counting proxy; `/stats` and `/reset` on the proxy
//! split the count by phase. `warm_blocks` > 0 measures a steady-state pass
//! (the history cache only reads that many new blocks).

use alloy::primitives::Address;
use arc::ArcClient;
use ledger::{Currency, Money, Seconds, Timestamp};
use operator::reasoner::RuleBased;
use operator::{Config, Operator};
use std::str::FromStr;

/// The proxy's counts; null when pointed straight at a node (timing only).
async fn stats(proxy: &str, reset: bool) -> serde_json::Value {
    let url = format!("{proxy}/{}", if reset { "reset" } else { "stats" });
    match reqwest::get(url).await {
        Ok(r) => r.json().await.unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    }
}

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = operator::watch::chain(&a[0]).expect("chain key");
    let policy = Address::from_str(&a[1]).unwrap();
    let watched = Address::from_str(&a[2]).unwrap();
    let created: u64 = a[3].parse().unwrap();
    let from: u64 = a[4].parse().unwrap();
    let proxy = a[5].trim_end_matches('/').to_string();
    let warm: u64 = a.get(6).and_then(|x| x.parse().ok()).unwrap_or(0);

    let mut client = ArcClient::read_only(&proxy)
        .unwrap()
        .with_log_chunk(c.log_span);
    if let Some(t) = c.usdc {
        client = client.with_token(Address::from_str(t).unwrap());
    }
    let anchor = operator::level1::anchor_for(c).unwrap();

    stats(&proxy, true).await;
    let t0 = std::time::Instant::now();
    let head = client.block_number().await.unwrap();
    let now: Timestamp = client.block_timestamp(head).await.unwrap();
    let hist_from = if warm > 0 { head - warm } else { from };
    let (mut history, _) = operator::watch::scan_transfers_budgeted(
        &client,
        c,
        &[watched],
        hist_from,
        head,
        None,
        &|_| {},
    )
    .await
    .unwrap();
    history.extend(
        client
            .transfers(policy, created.min(head), head)
            .await
            .unwrap(),
    );
    operator::watch::dedup_transfers(&mut history);
    let t_hist = t0.elapsed();
    let s_hist = stats(&proxy, false).await;

    stats(&proxy, true).await;
    let mut config = Config::new(policy, policy, anchor);
    config.also_history.push(watched);
    config.history_from_block = from;
    config.policy_from_block = Some(created);
    config.horizon = Seconds::days(30).unwrap();
    config.detection.dust_line = Money::parse(c.dust, Currency::Usdc).ok();
    config.audit_dir = Some(std::env::temp_dir().join(format!("pass-cost-{}", std::process::id())));
    let mut op = Operator::new(client, config, RuleBased).unwrap();
    let t1 = std::time::Instant::now();
    let out = op
        .run_once(now, Some(history.clone()), false)
        .await
        .unwrap();
    let t_pass = t1.elapsed();
    let s_pass = stats(&proxy, false).await;

    println!(
        "{}",
        serde_json::json!({
            "chain": c.key, "policy": format!("{policy:#x}"),
            "historyBlocks": head - hist_from, "transfers": history.len(),
            "obligations": out.obligations_found,
            "history": { "secs": t_hist.as_secs_f64(), "rpc": s_hist },
            "decide": { "secs": t_pass.as_secs_f64(), "rpc": s_pass },
        })
    );
}
