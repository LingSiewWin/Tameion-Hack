use hedge::policy::HedgePolicy;
use hedge::safety::{
    AccountState, Proposed, Refusal, Shrink, Tier, Verdict, check, health, maintenance_leverage,
    maintenance_rate, reduces_only,
};
use proptest::prelude::*;
use rust_decimal::Decimal;
use rust_decimal::dec;

fn acct(value: Decimal) -> AccountState {
    AccountState {
        account_value: value,
        margin_used: Decimal::ZERO,
        maintenance_margin_used: Decimal::ZERO,
        total_notional: Decimal::ZERO,
    }
}

/// Two tiers like Hyperliquid's "tiered 10x": 10x up to 3M notional, 5x above.
fn tiers() -> Vec<Tier> {
    vec![
        Tier {
            lower_bound: dec!(0),
            max_leverage: 10,
        },
        Tier {
            lower_bound: dec!(3000000),
            max_leverage: 5,
        },
    ]
}

#[test]
fn maintenance_rate_is_half_initial_at_tier_max_leverage() {
    // 10x tier: initial 10%, maintenance 5%.
    assert_eq!(maintenance_leverage(&tiers(), dec!(1000), 50), 10);
    assert_eq!(maintenance_rate(&tiers(), dec!(1000), 50), dec!(0.05));
    // 5x tier: initial 20%, maintenance 10%.
    assert_eq!(maintenance_rate(&tiers(), dec!(4000000), 50), dec!(0.1));
    // No tiers: fall back to the market's max leverage.
    assert_eq!(maintenance_rate(&[], dec!(1000), 50), dec!(0.01));
    // Never below 1x even with a zero fallback.
    assert_eq!(maintenance_leverage(&[], dec!(1), 0), 1);
}

#[test]
fn small_short_against_ample_collateral_is_allowed() {
    let p = Proposed {
        coin: "ETH".into(),
        current_size: Decimal::ZERO,
        size_delta: dec!(-0.5),
        px: dec!(3000),
    };
    // notional 1500; equity 2000 → leverage 0.75x; required = 1500 × 4 / 20 = 300.
    match check(
        &p,
        &acct(dec!(2000)),
        &tiers(),
        50,
        &HedgePolicy::default(),
        false,
    ) {
        Verdict::Allow(l) => {
            assert_eq!(l.notional_after, dec!(1500));
            assert_eq!(l.leverage_after, dec!(0.75));
            assert_eq!(l.required_equity, dec!(300));
            assert_eq!(l.maintenance_rate, dec!(0.05));
        }
        other => panic!("expected allow, got {other:?}"),
    }
}

#[test]
fn over_leverage_is_shrunk_to_the_cap() {
    let p = Proposed {
        coin: "ETH".into(),
        current_size: Decimal::ZERO,
        size_delta: dec!(-2),
        px: dec!(3000),
    };
    // notional 6000 on equity 2000 = 3x > 2x cap. Max notional 4000; size is
    // floored, so the result sits at or just under the cap, never over it.
    match check(
        &p,
        &acct(dec!(2000)),
        &tiers(),
        50,
        &HedgePolicy::default(),
        false,
    ) {
        Verdict::Reduce { why, limits } => {
            assert_eq!(why, Shrink::Leverage);
            assert!(limits.notional_after <= dec!(4000) && limits.notional_after > dec!(3999.9));
            assert!(limits.leverage_after <= dec!(2));
            assert!(limits.size_after.is_sign_negative(), "keeps the short side");
            assert!(limits.size_delta.abs() < p.size_delta.abs());
        }
        other => panic!("expected reduce, got {other:?}"),
    }
}

#[test]
fn thin_buffer_is_shrunk_by_the_maintenance_rule() {
    // Policy with a huge k so the buffer, not leverage, binds.
    let policy = HedgePolicy {
        k_maintenance: 100,
        ..HedgePolicy::default()
    };
    let p = Proposed {
        coin: "ETH".into(),
        current_size: Decimal::ZERO,
        size_delta: dec!(-0.5),
        px: dec!(3000),
    };
    // notional 1500; required = 1500 × 100 / 20 = 7500 > equity 2000 → reduce.
    // by buffer = 2000 × 20 / 100 = 400 ; by leverage = 4000 → buffer binds.
    match check(&p, &acct(dec!(2000)), &tiers(), 50, &policy, false) {
        Verdict::Reduce { why, limits } => {
            assert_eq!(why, Shrink::MaintenanceBuffer);
            assert!(limits.notional_after <= dec!(400) && limits.notional_after > dec!(399.9));
            assert!(limits.required_equity <= dec!(2000));
        }
        other => panic!("expected reduce, got {other:?}"),
    }
}

#[test]
fn refusals() {
    let base = Proposed {
        coin: "ETH".into(),
        current_size: Decimal::ZERO,
        size_delta: dec!(-0.5),
        px: dec!(3000),
    };
    let pol = HedgePolicy::default();
    assert_eq!(
        check(&base, &acct(dec!(2000)), &tiers(), 50, &pol, true),
        Verdict::Refuse(Refusal::Halted)
    );
    let doge = Proposed {
        coin: "DOGE".into(),
        ..base.clone()
    };
    assert_eq!(
        check(&doge, &acct(dec!(2000)), &tiers(), 50, &pol, false),
        Verdict::Refuse(Refusal::CoinNotAllowed("DOGE".into()))
    );
    assert_eq!(
        check(&base, &acct(Decimal::ZERO), &tiers(), 50, &pol, false),
        Verdict::Refuse(Refusal::NoCollateral)
    );
    let bad_px = Proposed {
        px: Decimal::ZERO,
        ..base.clone()
    };
    assert_eq!(
        check(&bad_px, &acct(dec!(2000)), &tiers(), 50, &pol, false),
        Verdict::Refuse(Refusal::BadPrice)
    );
    // 1 USDC of collateral can only support 2 USDC of notional: under the $10 minimum.
    match check(&base, &acct(dec!(1)), &tiers(), 50, &pol, false) {
        Verdict::Refuse(Refusal::BelowMinNotional {
            max_safe_notional,
            min_notional,
        }) => {
            assert!(max_safe_notional <= dec!(2) && max_safe_notional > dec!(1.99));
            assert_eq!(min_notional, dec!(10));
        }
        other => panic!("expected below-min refusal, got {other:?}"),
    }
}

#[test]
fn closing_to_flat_is_always_allowed() {
    let p = Proposed {
        coin: "ETH".into(),
        current_size: dec!(-5),
        size_delta: dec!(5),
        px: dec!(3000),
    };
    // Even with almost no equity: removing the position removes the risk.
    assert!(matches!(
        check(
            &p,
            &acct(dec!(1)),
            &tiers(),
            50,
            &HedgePolicy::default(),
            false
        ),
        Verdict::Allow(_)
    ));
}

#[test]
fn an_account_over_its_limits_may_still_shrink_its_position() {
    // 1 USDC of equity under a 1 ETH short at 3000: 3000x. Halving the short
    // is the order that helps; refusing it as "below the minimum" (the safe
    // size is ~2 USDC) kept the account where it was (audit 2026-10-15).
    let a = AccountState {
        account_value: dec!(1),
        margin_used: dec!(300),
        maintenance_margin_used: dec!(150),
        total_notional: dec!(3000),
    };
    let shrink = Proposed {
        coin: "ETH".into(),
        current_size: dec!(-1),
        size_delta: dec!(0.5),
        px: dec!(3000),
    };
    match check(&shrink, &a, &tiers(), 50, &HedgePolicy::default(), false) {
        Verdict::Allow(l) => assert_eq!(l.size_after, dec!(-0.5)),
        other => panic!("expected the reduction allowed, got {other:?}"),
    }
    let grow = Proposed {
        size_delta: dec!(-0.5),
        ..shrink
    };
    assert!(matches!(
        check(&grow, &a, &tiers(), 50, &HedgePolicy::default(), false),
        Verdict::Refuse(Refusal::BelowMinNotional { .. })
    ));
}

#[test]
fn health_is_equity_over_required() {
    let a = AccountState {
        account_value: dec!(1000),
        margin_used: dec!(200),
        maintenance_margin_used: dec!(100),
        total_notional: dec!(2000),
    };
    // k = 4 → required 400 → health 2.5
    assert_eq!(health(&a, &HedgePolicy::default()), Some(dec!(2.5)));
    assert_eq!(health(&acct(dec!(1000)), &HedgePolicy::default()), None);
}

#[test]
fn a_second_coin_is_judged_on_the_whole_account() {
    // An existing BTC short worth 3000 (maintenance 150 at the 10x tier)
    // already sits on 2000 of equity. ETH −0.5 @ 3000 would add 1500:
    // total 4500 / 2000 = 2.25x > 2x, even though 1500 alone is 0.75x.
    let a = AccountState {
        account_value: dec!(2000),
        margin_used: dec!(300),
        maintenance_margin_used: dec!(150),
        total_notional: dec!(3000),
    };
    let p = Proposed {
        coin: "ETH".into(),
        current_size: Decimal::ZERO,
        size_delta: dec!(-0.5),
        px: dec!(3000),
    };
    match check(&p, &a, &tiers(), 50, &HedgePolicy::default(), false) {
        Verdict::Reduce { why, limits } => {
            assert_eq!(why, Shrink::Leverage);
            // Room under the cap: 2000 × 2 − 3000 = 1000 of notional.
            assert!(limits.notional_after <= dec!(1000) && limits.notional_after > dec!(999.9));
            assert!(
                limits.leverage_after <= dec!(2),
                "{}",
                limits.leverage_after
            );
            assert!(limits.leverage_after > dec!(1.99), "counts the BTC short");
            // Required equity counts BTC's maintenance too: 4 × (150 + 1000 × 5%).
            assert!(limits.required_equity <= dec!(800) && limits.required_equity > dec!(799.9));
        }
        other => panic!("expected reduce by leverage, got {other:?}"),
    }

    // Growing the SAME coin replaces its own notional rather than adding to it:
    // current −1 ETH (3000 notional, the whole total) → −1.2 ETH is 3600 total = 1.8x.
    let a_same = AccountState {
        account_value: dec!(2000),
        margin_used: dec!(300),
        maintenance_margin_used: dec!(150),
        total_notional: dec!(3000),
    };
    let grow = Proposed {
        coin: "ETH".into(),
        current_size: dec!(-1),
        size_delta: dec!(-0.2),
        px: dec!(3000),
    };
    match check(&grow, &a_same, &tiers(), 50, &HedgePolicy::default(), false) {
        Verdict::Allow(l) => {
            assert_eq!(l.leverage_after, dec!(1.8));
            assert_eq!(l.required_equity, dec!(720)); // 4 × 3600 × 5%
        }
        other => panic!("expected allow, got {other:?}"),
    }
}

fn money() -> impl Strategy<Value = Decimal> {
    (1i64..=10_000_000i64).prop_map(|m| Decimal::new(m, 2)) // 0.01 .. 100,000.00
}
fn size() -> impl Strategy<Value = Decimal> {
    (-1_000_000i64..=1_000_000i64).prop_map(|m| Decimal::new(m, 4)) // ±100.0000
}
fn price() -> impl Strategy<Value = Decimal> {
    (1i64..=10_000_000i64).prop_map(|m| Decimal::new(m, 2)) // 0.01 .. 100,000.00
}
fn tier_set() -> impl Strategy<Value = Vec<Tier>> {
    prop_oneof![
        Just(Vec::new()),
        Just(tiers()),
        // Awkward leverages whose maintenance rate is not a terminating decimal.
        Just(vec![
            Tier {
                lower_bound: dec!(0),
                max_leverage: 3
            },
            Tier {
                lower_bound: dec!(1000),
                max_leverage: 7
            },
        ]),
    ]
}

proptest! {
    /// Whatever comes back as Allow or Reduce respects both limits exactly,
    /// and Reduce never flips the side the controller asked for.
    #[test]
    fn verdicts_respect_the_limits(
        equity in money(), current in size(), delta in size(), px in price(),
        k in 1u32..10, max_lev in 1u32..5, tiers in tier_set(), fallback in 1u32..50
    ) {
        let policy = HedgePolicy { k_maintenance: k, max_leverage: max_lev, ..HedgePolicy::default() };
        let p = Proposed { coin: "ETH".into(), current_size: current, size_delta: delta, px };
        let a = acct(equity);
        match check(&p, &a, &tiers, fallback, &policy, false) {
            Verdict::Allow(l) => {
                // An order that only shrinks the position may go through on
                // an account already over a limit: it moves it back.
                if !l.size_after.is_zero() && !reduces_only(&p) {
                    prop_assert!(l.leverage_after <= Decimal::from(max_lev));
                    prop_assert!(equity >= l.required_equity);
                    prop_assert!(l.notional_after >= policy.min_notional_usdc);
                }
                prop_assert_eq!(current + l.size_delta, l.size_after);
            }
            Verdict::Reduce { limits: l, .. } => {
                prop_assert!(l.leverage_after <= Decimal::from(max_lev), "lev {} > {}", l.leverage_after, max_lev);
                prop_assert!(equity >= l.required_equity, "equity {} < required {}", equity, l.required_equity);
                prop_assert!(l.notional_after >= policy.min_notional_usdc);
                prop_assert_eq!(current + l.size_delta, l.size_after);
                let asked_after = current + delta;
                prop_assert_eq!(l.size_after.is_sign_negative(), asked_after.is_sign_negative());
                prop_assert!(l.size_after.abs() < asked_after.abs());
            }
            Verdict::Refuse(_) => {}
        }
    }
}

/// Hyperliquid's testnet BTC table (docs: trading/margin-tiers, read
/// 2026-10-15): 40x to 10k, 25x to 50k, 10x above.
fn testnet_btc() -> Vec<Tier> {
    [(0, 40), (10_000, 25), (50_000, 10)]
        .into_iter()
        .map(|(b, l)| Tier {
            lower_bound: Decimal::from(b),
            max_leverage: l,
        })
        .collect()
}

/// The venue's own maintenance margin for one position, as its docs state
/// it: `notional × rate(tier) − maintenance_deduction(tier)`, the deduction
/// keeping the margin continuous across tiers. The engine charges
/// `notional / 2L` with no deduction, which is never less.
fn venue_mm(tiers: &[Tier], notional: Decimal) -> Decimal {
    let rate = |l: u32| Decimal::ONE / Decimal::from(2 * l);
    let (mut deduction, mut prev) = (Decimal::ZERO, None);
    let mut mm = Decimal::ZERO;
    for t in tiers {
        if t.lower_bound > notional {
            break;
        }
        if let Some(p) = prev {
            deduction += t.lower_bound * (rate(t.max_leverage) - rate(p));
        }
        prev = Some(t.max_leverage);
        mm = notional * rate(t.max_leverage) - deduction;
    }
    mm
}

proptest! {
    /// docs/68 part 4: whatever the engine lets through keeps the account at
    /// or above k × the maintenance margin **the venue** computes, deduction
    /// and all, on a cross account that also carries another position —
    /// including orders that cross a tier boundary either way.
    #[test]
    fn required_equity_covers_k_times_the_venues_own_maintenance(
        equity in (1i64..=20_000_000i64).prop_map(|m| Decimal::new(m, 2)),
        other in (0i64..=8_000_000i64).prop_map(|m| Decimal::new(m, 2)),
        current in (-200_000i64..=200_000i64).prop_map(|m| Decimal::new(m, 4)),
        delta in (-200_000i64..=200_000i64).prop_map(|m| Decimal::new(m, 4)),
        px in (50_000i64..=150_000i64).prop_map(Decimal::from),
        k in 2u32..6,
    ) {
        let tiers = testnet_btc();
        let policy = HedgePolicy { k_maintenance: k, max_leverage: 50, ..HedgePolicy::default() };
        let cur_notional = current.abs() * px;
        let a = AccountState {
            account_value: equity,
            margin_used: Decimal::ZERO,
            maintenance_margin_used: venue_mm(&tiers, other) + venue_mm(&tiers, cur_notional),
            total_notional: other + cur_notional,
        };
        let p = Proposed { coin: "BTC".into(), current_size: current, size_delta: delta, px };
        let l = match check(&p, &a, &tiers, 40, &policy, false) {
            Verdict::Allow(l) | Verdict::Reduce { limits: l, .. } => l,
            Verdict::Refuse(_) => return Ok(()),
        };
        if !l.size_after.is_zero() {
            let venue = Decimal::from(k) * (venue_mm(&tiers, other) + venue_mm(&tiers, l.notional_after));
            prop_assert!(l.required_equity >= venue, "required {} < k × venue mm {}", l.required_equity, venue);
            // An account already inside the line may still shrink its
            // position: that order moves it toward the line, not past it.
            prop_assert!(equity >= venue || reduces_only(&p));
        }
    }
}

#[test]
fn venue_maintenance_is_continuous_across_tiers() {
    let t = testnet_btc();
    // 10k at 40x: 125. Just above, 25x with deduction 10k × (1/50 − 1/80) = 75: 200 − 75 = 125.
    assert_eq!(venue_mm(&t, dec!(10000)), dec!(125));
    assert_eq!(venue_mm(&t, dec!(50000)), dec!(925));
}
