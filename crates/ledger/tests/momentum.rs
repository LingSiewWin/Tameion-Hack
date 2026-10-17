//! The fold's two laws (docs/117 §1), over any record: flow is conserved, and
//! more money never means less runway.

use ledger::momentum::{Bill, DAY, flow_check, fold};
use ledger::{ActionHash, Counterparty, Currency, Event, EventBody, Money, Timestamp};
use proptest::prelude::*;

fn usdc(minor: i128) -> Money {
    Money::minor(minor, Currency::Usdc)
}

fn paid(at: i64, who: u8, minor: i128) -> Event {
    Event {
        seq: 0,
        at: Timestamp::from_unix(at),
        body: EventBody::Settlement {
            counterparty: Counterparty::new(format!("0x{who:040x}")),
            cycle_key: format!("{who}@{at}"),
            amount: usdc(minor),
            tx_hash: format!("0x{at:064x}"),
            action_hash: ActionHash::from_bytes([who; 32]),
            settlement_key: ActionHash::from_bytes([who; 32]),
            note: String::new(),
        },
    }
}

fn check(body: &EventBody) -> (Money, Money, Money, Money, Money) {
    match body {
        EventBody::FlowCheck {
            balance_start,
            inflows,
            outflows,
            balance_end_book,
            diff,
            ..
        } => (
            *balance_start,
            *inflows,
            *outflows,
            *balance_end_book,
            *diff,
        ),
        _ => panic!("not a flow check"),
    }
}

const T: i64 = 30 * DAY;

proptest! {
    /// Σ inflows − Σ outflows = Δ balance: when the chain moved exactly what
    /// the book and the deposits say, every period's difference is zero, a
    /// period split in two ends where the whole one ends, and a payment the
    /// book never recorded is the difference to the minor unit.
    #[test]
    fn flow_is_conserved(
        pays in prop::collection::vec((0..T, 0u8..4, 1i128..5_000_000), 0..40),
        deposits in prop::collection::vec((0..T, 1i128..50_000_000), 0..10),
        cut in 1..T,
        missing in 1i128..1_000_000,
    ) {
        let events: Vec<Event> = pays.iter().map(|&(at, who, m)| paid(at, who, m)).collect();
        let inflow = |a: i64, b: i64| usdc(deposits.iter().filter(|d| d.0 >= a && d.0 < b).map(|d| d.1).sum());
        let outflow = |a: i64, b: i64| pays.iter().filter(|p| p.0 >= a && p.0 < b).map(|p| p.2).sum::<i128>();
        let chain_at = |b: i64| usdc(inflow(0, b).to_minor() - outflow(0, b));

        let whole = flow_check(&events, 0, T, inflow(0, T), chain_at(T));
        let (start, inflows, outflows, book, diff) = check(&whole);
        prop_assert_eq!(start, usdc(0));
        prop_assert_eq!(inflows.to_minor() - outflows.to_minor(), book.to_minor() - start.to_minor());
        prop_assert_eq!(diff, usdc(0));

        let first = flow_check(&events, 0, cut, inflow(0, cut), chain_at(cut));
        prop_assert_eq!(check(&first).4, usdc(0));
        let mut with_first = events.clone();
        with_first.push(Event { seq: 99, at: Timestamp::from_unix(cut), body: first });
        let second = flow_check(&with_first, cut, T, inflow(cut, T), chain_at(T));
        prop_assert_eq!(check(&second).3, book);
        prop_assert_eq!(check(&second).4, usdc(0));

        let short = flow_check(&events, 0, T, inflow(0, T), usdc(chain_at(T).to_minor() - missing));
        prop_assert_eq!(check(&short).4, usdc(-missing));
        // What the record keeps is what the fold reads back.
        prop_assert_eq!(Event::decode(&Event { seq: 1, at: Timestamp::from_unix(T), body: whole.clone() }.canonical()).map(|e| e.body), Some(whole));
    }

    /// More money above the floor never shortens either runway.
    #[test]
    fn runway_is_monotone_in_balance(
        pays in prop::collection::vec((0..T, 0u8..4, 1i128..5_000_000), 0..40),
        bills in prop::collection::vec((0..60 * DAY, 0..40 * DAY, 1i128..20_000_000), 0..6),
        low in 0i128..100_000_000,
        more in 0i128..100_000_000,
        floor in 0i128..10_000_000,
        window in 1u32..40,
    ) {
        let events: Vec<Event> = pays.iter().map(|&(at, who, m)| paid(at, who, m)).collect();
        let bills: Vec<Bill> = bills.iter().map(|&(due, every, m)| Bill { next_due: due, every_secs: every, amount_minor: m }).collect();
        let a = fold(&events, &bills, usdc(low), usdc(floor), T, window);
        let b = fold(&events, &bills, usdc(low + more), usdc(floor), T, window);
        prop_assert_eq!(a.rate_per_day, b.rate_per_day);
        let never = |d: Option<i64>| d.unwrap_or(i64::MAX);
        prop_assert!(never(a.runway_by_rate_days) <= never(b.runway_by_rate_days));
        prop_assert!(never(a.first_shortfall_days) <= never(b.first_shortfall_days));
    }
}
