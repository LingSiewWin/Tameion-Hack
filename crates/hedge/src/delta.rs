//! The delta controller: what the hedge should be, and whether to move.
//!
//! Pure functions. The target is the negation of the spot size; the decision
//! to trade is a comparison against a band. Nothing here knows about a venue.

use crate::exposure::SpotExposure;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

/// The hedge that makes the exposure delta-neutral. Negative is short.
#[derive(Debug, Clone, PartialEq)]
pub struct HedgeTarget {
    pub coin: String,
    pub size: Decimal,
}

/// Holding `size` of a coin is neutralised by a perp position of `-size`.
pub fn target_hedge(exposure: &SpotExposure) -> HedgeTarget {
    HedgeTarget {
        coin: exposure.coin.clone(),
        size: -exposure.size,
    }
}

/// The short that covers `cover_usdc` of bills at the current mark, never
/// more than the holding: min(holding, cover / mark). `None` cover hedges
/// the whole holding. A zero or negative holding targets flat either way.
pub fn target_hedge_covering(exposure: &SpotExposure, cover_usdc: Option<Decimal>) -> HedgeTarget {
    let Some(cover) = cover_usdc else {
        return target_hedge(exposure);
    };
    if exposure.size <= Decimal::ZERO || exposure.mark_px <= Decimal::ZERO || cover <= Decimal::ZERO
    {
        return HedgeTarget {
            coin: exposure.coin.clone(),
            size: Decimal::ZERO,
        };
    }
    let needed = cover / exposure.mark_px;
    HedgeTarget {
        coin: exposure.coin.clone(),
        size: -exposure.size.min(needed),
    }
}

/// Whether the current hedge is close enough to the target.
#[derive(Debug, Clone, PartialEq)]
pub enum Rebalance {
    /// Inside the band. Do nothing.
    Hold { drift_bps: u32 },
    /// Outside the band. `size_delta` moves the position exactly onto the
    /// target; `is_buy` is its sign as the venue wants it.
    Trade {
        size_delta: Decimal,
        is_buy: bool,
        drift_bps: u32,
    },
}

/// Compare the current hedge size with the target.
///
/// Drift is `|current - target| / |target|` in basis points, saturating at
/// `u32::MAX`. A non-zero position against a zero target is infinite drift
/// and is always a trade (to flat). Zero against zero is a hold.
pub fn rebalance(current: Decimal, target: Decimal, band_bps: u32) -> Rebalance {
    let gap = target - current;
    if gap.is_zero() {
        return Rebalance::Hold { drift_bps: 0 };
    }
    let drift_bps = if target.is_zero() {
        u32::MAX
    } else {
        gap.abs()
            .checked_div(target.abs())
            .and_then(|r| r.checked_mul(Decimal::from(10_000u32)))
            .and_then(|bps| bps.to_u128())
            .map_or(u32::MAX, |bps| bps.min(u32::MAX as u128) as u32)
    };
    if drift_bps <= band_bps {
        Rebalance::Hold { drift_bps }
    } else {
        Rebalance::Trade {
            size_delta: gap,
            is_buy: gap.is_sign_positive(),
            drift_bps,
        }
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn a_cover_smaller_than_the_holding_sizes_the_short_to_the_bills() {
        let e = SpotExposure::new("ETH", dec!(10), dec!(2500)); // 25,000 USDC held
        let t = target_hedge_covering(&e, Some(dec!(5000)));
        assert_eq!(t.size, dec!(-2)); // 5,000 / 2,500
    }

    #[test]
    fn a_cover_larger_than_the_holding_is_capped_at_the_holding() {
        let e = SpotExposure::new("ETH", dec!(1), dec!(2500));
        assert_eq!(
            target_hedge_covering(&e, Some(dec!(1_000_000))).size,
            dec!(-1)
        );
    }

    #[test]
    fn no_cover_means_the_whole_holding_and_zero_cover_means_flat() {
        let e = SpotExposure::new("ETH", dec!(3), dec!(2500));
        assert_eq!(target_hedge_covering(&e, None).size, dec!(-3));
        assert_eq!(
            target_hedge_covering(&e, Some(Decimal::ZERO)).size,
            Decimal::ZERO
        );
    }
}
