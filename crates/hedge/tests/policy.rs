use hedge::policy::HedgePolicy;
use rust_decimal::dec;

#[test]
fn the_default_policy_validates() {
    assert_eq!(HedgePolicy::default().validate(), Ok(()));
    assert!(HedgePolicy::default().allows("ETH"));
    assert!(!HedgePolicy::default().allows("DOGE"));
}

#[test]
fn a_bad_policy_lists_every_problem_at_once() {
    let bad = HedgePolicy {
        band_bps: 0,
        max_leverage: 50,
        k_maintenance: 1,
        allowlist: vec![],
        min_notional_usdc: dec!(0),
        slippage_bps: 5_000,
        max_margin_share_pct: 0,
    };
    let problems = bad.validate().unwrap_err();
    assert_eq!(problems.len(), 7, "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("band_bps")));
    assert!(problems.iter().any(|p| p.contains("max_leverage 50x")));
    assert!(problems.iter().any(|p| p.contains("k_maintenance 1")));
    assert!(problems.iter().any(|p| p.contains("allowlist is empty")));
    assert!(problems.iter().any(|p| p.contains("min_notional_usdc")));
    assert!(problems.iter().any(|p| p.contains("slippage_bps 5000")));
}
