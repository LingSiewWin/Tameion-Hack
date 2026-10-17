//! The daily flow check replayed by an outsider (docs/117 §1), read-only: an
//! account's anchored record against the chain, one UTC day per line, with
//! the balance read from two RPCs. Writes nothing, sends nothing.
//!
//!   cargo run -p operator --example flow_check -- <app_dir> <policy> <second_rpc>
//!
//! Each period is the pass's own `ledger::momentum::flow_check` over the
//! record (`operator::audit::book`): the day's opening balance is the last
//! check's chain balance, money in is the chain's transfers into the account,
//! money out is the book's settlements, and the chain's balance at the day's
//! end is the account's transfer log summed to that instant (it opened
//! empty). The last line ends now, against `balanceOf` from both RPCs.

use alloy::primitives::Address;
use ledger::momentum::{DAY, flow_check, fold};
use ledger::{Currency, Direction, Event, EventBody, Money, Timestamp};
use operator::{audit, level1};
use std::path::Path;
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (dir, policy, rpc2) = match args.as_slice() {
        [_, d, p, r] => (Path::new(d), p.to_lowercase(), r.clone()),
        _ => return Err("usage: flow_check <app_dir> <policy> <second_rpc>".into()),
    };
    let t = level1::load(dir, &policy).ok_or("no such account in that app dir")?;
    let c = operator::watch::chain(&t.chain).ok_or("chain unknown")?;
    let addr = Address::from_str(&policy)?;
    let one = level1::read_client(c)?;
    let mut two = arc::ArcClient::read_only(&rpc2)?.with_log_chunk(c.log_span);
    if let Some(u) = c.usdc {
        two = two.with_token(Address::from_str(u)?);
    }
    let head = one.block_number().await?;
    let now = one.block_timestamp(head).await?.as_unix();
    let created = one.block_timestamp(t.created_block).await?.as_unix();
    let transfers = one.transfers(addr, t.created_block, head).await?;
    let (b1, b2, h2) = (
        one.usdc_balance(addr).await?,
        two.usdc_balance(addr).await?,
        two.block_number().await?,
    );
    let mut book = audit::book(&level1::tenants_dir(dir).join(&policy).join("log"));
    println!(
        "account {policy} on {} · created block {} ({}) · record: {} events",
        c.name,
        t.created_block,
        operator::statement::iso8601(created),
        book.len()
    );
    println!(
        "RPC 1 {} head {head}: balanceOf {b1}\nRPC 2 {rpc2} head {h2}: balanceOf {b2}",
        operator::watch::rpc_for(c)
    );
    let net_before = |at: i64| -> i128 {
        transfers
            .iter()
            .filter(|tr| tr.at.as_unix() < at)
            .map(|tr| match tr.direction {
                Direction::In => tr.amount.to_minor(),
                Direction::Out => -tr.amount.to_minor(),
            })
            .sum()
    };
    let flow = |dir: Direction, a: i64, b: i64| -> i128 {
        transfers
            .iter()
            .filter(|tr| tr.direction == dir && tr.at.as_unix() >= a && tr.at.as_unix() < b)
            .map(|tr| tr.amount.to_minor())
            .sum()
    };
    println!(
        "the transfer log sums to {} now; balanceOf says {b1} (RPC 1) and {b2} (RPC 2)",
        Money::minor(net_before(now + 1), Currency::Usdc)
    );
    let usdc = |m: i128| Money::minor(m, Currency::Usdc);
    let mut start = created;
    while start < now {
        let end = ((start.div_euclid(DAY) + 1) * DAY).min(now);
        let chain_end = if end == now {
            b1
        } else {
            usdc(net_before(end))
        };
        let check = flow_check(
            &book,
            start,
            end,
            usdc(flow(Direction::In, start, end)),
            chain_end,
        );
        if let EventBody::FlowCheck {
            balance_start,
            inflows,
            outflows,
            balance_end_book,
            balance_end_chain,
            diff,
            ..
        } = &check
        {
            println!(
                "FLOWCHECK {} → {} | start {balance_start} + in {inflows} − out(book) {outflows} = book {balance_end_book} | chain {balance_end_chain} (out on chain {}) | diff {diff}",
                operator::statement::iso8601(start),
                operator::statement::iso8601(end),
                usdc(flow(Direction::Out, start, end))
            );
        }
        book.push(Event {
            seq: 0,
            at: Timestamp::from_unix(end),
            body: check,
        });
        start = end;
    }
    // Every payment the chain shows leaving the account that the book does
    // not hold, by transaction: the difference, explained one line each.
    let in_book: std::collections::BTreeSet<String> = book
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Settlement { tx_hash, .. } => Some(tx_hash.to_lowercase()),
            _ => None,
        })
        .collect();
    let by_owner: std::collections::BTreeSet<String> = one
        .principal_settlements(addr, t.created_block, head)
        .await
        .map(|v| v.iter().map(|s| format!("{:#x}", s.tx)).collect())
        .unwrap_or_default();
    for tr in transfers.iter().filter(|tr| {
        tr.direction == Direction::Out && !in_book.contains(&tr.tx_hash.to_lowercase())
    }) {
        println!(
            "  not in the book: {} {} to {} in {} — {}",
            operator::statement::iso8601(tr.at.as_unix()),
            tr.amount,
            tr.counterparty,
            tr.tx_hash,
            if by_owner.contains(&tr.tx_hash.to_lowercase()) {
                "settled by the owner's passkey (SettledByPrincipal)"
            } else {
                "not an agent settlement (owner withdrawal, standing order or other call)"
            }
        );
    }
    let floor = one
        .governance(addr)
        .await
        .map(|g| g.reserve_floor)
        .unwrap_or(usdc(0));
    let m = fold(&book, &[], b1, floor, now, level1::burn_window_days());
    println!(
        "momentum now: rate {} a day over {} days · runway by rate {:?} days · floor {floor} · forces {}",
        m.rate_per_day,
        m.window_days,
        m.runway_by_rate_days,
        m.forces.len()
    );
    Ok(())
}
