//! Parity with the Python implementation this module replaces.
//!
//! `agent/obligations.py` is the original. Before it is deleted, its output
//! on a fixed history is pinned here, so the port stays verified once the
//! Python is gone and nobody can re-run it for comparison.
//!
//! The expected values below were produced by running the fixture through
//! `agent/obligations.py` on 2026-09-20 and are transcribed verbatim.
//!
//! One value differs on purpose, and it is called out below.

use ledger::{
    Counterparty, Currency, DetectionConfig, Direction, Money, ObligationKind, Timestamp, Transfer,
    detect,
};

const T0: i64 = 1_700_000_000;

fn tx(h: &str, who: &str, minor: i128, at: i64, dir: Direction) -> Transfer {
    Transfer {
        tx_hash: h.into(),
        counterparty: Counterparty::new(who),
        amount: Money::minor(minor, Currency::Usdc),
        at: Timestamp::from_unix(at),
        direction: dir,
    }
}

fn fixture() -> Vec<Transfer> {
    let mut txs = Vec::new();
    // Clean monthly subscription: six payments of 20 USDC.
    for i in 0..6i64 {
        txs.push(tx(
            &format!("0xaaa{i}"),
            "0x1111",
            20_000000,
            T0 + i * 30 * 86400,
            Direction::Out,
        ));
    }
    // Contractor billing hours: fortnightly, rising amounts.
    for (i, amt) in [800i128, 900, 1000, 1100, 1200, 1300].iter().enumerate() {
        txs.push(tx(
            &format!("0xbbb{i}"),
            "0xC0DE",
            amt * 1_000_000,
            T0 + i as i64 * 14 * 86400,
            Direction::Out,
        ));
    }
    // Weekly, five payments.
    for i in 0..5i64 {
        txs.push(tx(
            &format!("0xccc{i}"),
            "0xSOON",
            3_500000,
            T0 + i * 7 * 86400,
            Direction::Out,
        ));
    }
    // Incoming: revenue, must be ignored entirely.
    for i in 0..6i64 {
        txs.push(tx(
            &format!("0xddd{i}"),
            "0xFEED",
            500_000000,
            T0 + i * 30 * 86400,
            Direction::In,
        ));
    }
    txs
}

#[test]
fn matches_the_python_implementation_field_for_field() {
    let found = detect(&fixture(), DetectionConfig::default());
    assert_eq!(found.len(), 3, "python found 3 obligations");

    // Ordering: soonest due first. Python produced exactly this order.
    let expected = [
        // (counterparty, kind, interval_days, next_due, confidence, n, interval_cv, amount_cv)
        (
            "0xsoon",
            ObligationKind::Subscription,
            7.0,
            1703024000i64,
            0.8125,
            5usize,
            0.0,
            0.0,
        ),
        (
            "0xc0de",
            ObligationKind::RecurringPayment,
            14.0,
            1707257600,
            0.8046,
            6,
            0.0,
            0.1782,
        ),
        (
            "0x1111",
            ObligationKind::Subscription,
            30.0,
            1715552000,
            0.85,
            6,
            0.0,
            0.0,
        ),
    ];

    for (o, e) in found.iter().zip(expected) {
        assert_eq!(o.counterparty.as_str(), e.0);
        assert_eq!(o.kind, e.1);
        assert!(
            (o.interval.as_days() - e.2).abs() < 1e-9,
            "{}: interval",
            e.0
        );
        assert_eq!(o.next_due.as_unix(), e.3, "{}: next_due", e.0);
        assert!(
            (o.confidence.get() - e.4).abs() < 1e-9,
            "{}: confidence {}",
            e.0,
            o.confidence
        );
        assert_eq!(o.sample_count, e.5, "{}: sample_count", e.0);
        assert!((o.interval_cv - e.6).abs() < 1e-9, "{}: interval_cv", e.0);
        assert!((o.amount_cv - e.7).abs() < 1e-9, "{}: amount_cv", e.0);
    }
}

#[test]
fn the_typical_amount_deliberately_differs_from_python() {
    // The contractor was paid 800, 900, 1000, 1100, 1200, 1300 USDC.
    //
    // Python uses `statistics.median`, which interpolates on an even count:
    // (1000 + 1100) / 2 = 1050. That amount was never paid. For a figure the
    // agent will hand to a transfer, an interpolated value is wrong twice
    // over: it may not be representable at the currency's scale, and it
    // claims a payment history that does not exist.
    //
    // `lower_median_money` returns the lower of the two middle values: an
    // amount genuinely paid at least once, erring toward underpaying, which
    // is the safer direction for an agent acting without supervision.
    let found = detect(&fixture(), DetectionConfig::default());
    let contractor = found
        .iter()
        .find(|o| o.counterparty.as_str() == "0xc0de")
        .unwrap();

    assert_eq!(
        contractor.typical_amount,
        Money::parse("1000.000000", Currency::Usdc).unwrap(),
        "rust returns an amount that was actually paid"
    );
    assert_ne!(
        contractor.typical_amount,
        Money::parse("1050.000000", Currency::Usdc).unwrap(),
        "python returned 1050, which nobody was ever paid"
    );

    // And the value returned is one of the observed amounts.
    let paid = [800i128, 900, 1000, 1100, 1200, 1300].map(|a| a * 1_000_000);
    assert!(paid.contains(&contractor.typical_amount.to_minor()));
}

#[test]
fn incoming_transfers_were_ignored_by_both() {
    let found = detect(&fixture(), DetectionConfig::default());
    assert!(
        !found.iter().any(|o| o.counterparty.as_str() == "0xfeed"),
        "0xFEED only ever sent value in; it is revenue, not an obligation"
    );
}
