use hedge::delta::{Rebalance, rebalance, target_hedge};
use hedge::exposure::SpotExposure;
use proptest::prelude::*;
use rust_decimal::Decimal;
use rust_decimal::dec;

#[test]
fn target_is_the_negation_of_the_holding() {
    let e = SpotExposure::new("ETH", dec!(0.5), dec!(3000));
    let t = target_hedge(&e);
    assert_eq!(t.coin, "ETH");
    assert_eq!(t.size, dec!(-0.5));
    assert_eq!(e.notional(), Some(dec!(1500)));
}

#[test]
fn inside_band_holds() {
    // target -1.0, current -0.97: drift 3% < 5%
    match rebalance(dec!(-0.97), dec!(-1.0), 500) {
        Rebalance::Hold { drift_bps } => assert_eq!(drift_bps, 300),
        other => panic!("expected hold, got {other:?}"),
    }
}

#[test]
fn outside_band_trades_exactly_onto_target() {
    match rebalance(dec!(-0.4), dec!(-1.0), 500) {
        Rebalance::Trade {
            size_delta,
            is_buy,
            drift_bps,
        } => {
            assert_eq!(size_delta, dec!(-0.6));
            assert!(!is_buy, "growing a short is a sell");
            assert_eq!(drift_bps, 6000);
        }
        other => panic!("expected trade, got {other:?}"),
    }
}

#[test]
fn position_against_zero_target_is_always_a_trade_to_flat() {
    match rebalance(dec!(-0.3), Decimal::ZERO, 500) {
        Rebalance::Trade {
            size_delta,
            is_buy,
            drift_bps,
        } => {
            assert_eq!(size_delta, dec!(0.3));
            assert!(is_buy);
            assert_eq!(drift_bps, u32::MAX);
        }
        other => panic!("expected trade, got {other:?}"),
    }
}

#[test]
fn flat_against_flat_holds() {
    assert_eq!(
        rebalance(Decimal::ZERO, Decimal::ZERO, 500),
        Rebalance::Hold { drift_bps: 0 }
    );
}

fn small_decimal() -> impl Strategy<Value = Decimal> {
    // -100.0000 .. 100.0000 at four places, the size precision of most perps.
    (-1_000_000i64..=1_000_000i64).prop_map(|m| Decimal::new(m, 4))
}

proptest! {
    /// Applying the trade the controller asks for lands exactly on target.
    #[test]
    fn trade_lands_on_target(current in small_decimal(), target in small_decimal(), band in 0u32..5_000) {
        match rebalance(current, target, band) {
            Rebalance::Trade { size_delta, is_buy, .. } => {
                prop_assert_eq!(current + size_delta, target);
                prop_assert_eq!(is_buy, size_delta.is_sign_positive());
            }
            Rebalance::Hold { drift_bps } => {
                prop_assert!(drift_bps <= band);
            }
        }
    }

    /// Hold iff drift is within the band (for a non-zero target).
    #[test]
    fn hold_iff_within_band(current in small_decimal(), target in small_decimal(), band in 0u32..5_000) {
        prop_assume!(!target.is_zero());
        let drift = ((target - current).abs() / target.abs()) * Decimal::from(10_000u32);
        let within = drift <= Decimal::from(band);
        let held = matches!(rebalance(current, target, band), Rebalance::Hold { .. });
        // Truncation to whole bps can only make a borderline case hold.
        if within { prop_assert!(held); }
        if !held { prop_assert!(!within); }
    }
}
