//! The conversion between native wei and USDC minor units.
//!
//! Arc carries eighteen decimals on the native scale and six on the
//! settlement scale. Every defect the Canteen survey found in the
//! open-source accounting stack was a scale or representation mistake, so
//! this boundary gets more attention than its four lines suggest.
//!
//! Confirmed against live Arc testnet on 2026-09-20: the treasury's native
//! balance and its ERC-20 `balanceOf` both read 20.000000 USDC. Two
//! independent paths, one answer.

use alloy::primitives::U256;
use arc::{WEI_PER_USDC_UNIT, usdc_from_wei, wei_from_usdc};
use ledger::{Currency, Money};
use proptest::prelude::*;

fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}

#[test]
fn one_minor_unit_is_ten_to_the_twelve_wei() {
    assert_eq!(WEI_PER_USDC_UNIT, 1_000_000_000_000);
    assert_eq!(
        usdc_from_wei(U256::from(WEI_PER_USDC_UNIT)),
        usdc("0.000001")
    );
    assert_eq!(
        wei_from_usdc(usdc("0.000001")).unwrap(),
        U256::from(WEI_PER_USDC_UNIT)
    );
}

#[test]
fn twenty_usdc_is_twenty_times_ten_to_the_eighteen() {
    // The figure the faucet sent, and the figure the chain reported back.
    let twenty = usdc("20.000000");
    assert_eq!(
        wei_from_usdc(twenty).unwrap(),
        U256::from(20_000_000_000_000_000_000u128)
    );
    assert_eq!(
        usdc_from_wei(U256::from(20_000_000_000_000_000_000u128)),
        twenty
    );
}

#[test]
fn sub_minor_dust_truncates_to_zero_rather_than_rounding_up() {
    // A fraction of a minor unit is not a small amount of USDC. It is an
    // amount that has no representation on the settlement scale at all, so
    // it becomes zero rather than being rounded into existence.
    for wei in [1u64, 999, 999_999_999_999] {
        assert_eq!(
            usdc_from_wei(U256::from(wei)),
            usdc("0.000000"),
            "wei={wei}"
        );
    }
    // And one wei above the threshold is exactly one minor unit, not two.
    assert_eq!(
        usdc_from_wei(U256::from(1_000_000_000_001u64)),
        usdc("0.000001")
    );
}

#[test]
fn a_negative_amount_cannot_become_wei() {
    // U256 has no negative values; a negative Money reaching this boundary
    // is a bug upstream, and it must surface as an error rather than wrap
    // into an enormous positive number.
    assert!(wei_from_usdc(Money::minor(-1, Currency::Usdc)).is_err());
}

#[test]
fn a_balance_too_large_to_exist_saturates_rather_than_panicking() {
    // ~1.7e32 USDC cannot arise -- total USDC supply is ~1e10 -- but a
    // malicious or broken RPC can return anything, and a panic in the read
    // path would take the agent down.
    let absurd = U256::MAX;
    let m = usdc_from_wei(absurd);
    assert!(m.to_minor() > 0, "saturates high, does not wrap negative");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// wei -> usdc -> wei is the identity on exact multiples of a minor unit.
    #[test]
    fn round_trip_is_exact_on_representable_amounts(minor in 0i128..1_000_000_000_000_000i128) {
        let m = Money::minor(minor, Currency::Usdc);
        let wei = wei_from_usdc(m).unwrap();
        prop_assert_eq!(usdc_from_wei(wei), m);
    }

    /// usdc -> wei -> usdc never gains value. Truncation only ever loses the
    /// sub-minor remainder, and only in that direction.
    #[test]
    fn conversion_never_invents_value(wei_raw in 0u128..u128::MAX) {
        let wei = U256::from(wei_raw);
        let m = usdc_from_wei(wei);
        let back = wei_from_usdc(m).unwrap();
        prop_assert!(back <= wei, "converting back must not exceed the original");
        let lost = wei - back;
        prop_assert!(
            lost < U256::from(WEI_PER_USDC_UNIT),
            "at most one minor unit's worth of dust is ever dropped"
        );
    }

    /// Conversion is monotonic: more wei is never less USDC.
    #[test]
    fn more_wei_is_never_less_usdc(a in 0u128..u128::MAX, b in 0u128..u128::MAX) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        prop_assert!(
            usdc_from_wei(U256::from(lo)).to_minor() <= usdc_from_wei(U256::from(hi)).to_minor()
        );
    }
}
