//! The hedge policy: the numbers a human set, that code enforces.
//!
//! The model never sees these and cannot change them. A change to the band
//! or the leverage cap is a human decision recorded in `CLAUDE.md`, the same
//! way `BudgetPolicy` caps are.

use rust_decimal::{Decimal, dec};

/// What the hedge is allowed to do.
#[derive(Debug, Clone, PartialEq)]
pub struct HedgePolicy {
    /// Rebalance when the hedge drifts from its target by more than this
    /// many basis points of the target. 500 = 5%.
    pub band_bps: u32,
    /// Hard cap on hedge notional / account value. The hedge is a short
    /// against a holding the treasury already owns; it never needs more
    /// than 1x, and 2x leaves room for a fill at a worse price.
    pub max_leverage: u32,
    /// Account value must stay at or above this multiple of the venue's
    /// maintenance margin after every order. Hyperliquid's backstop
    /// liquidation fires at 2/3 of maintenance margin; `k = 4` keeps the
    /// account six times further from that line than the venue requires.
    pub k_maintenance: u32,
    /// Markets the hedge may trade. BTC, ETH and SOL have the deepest books
    /// and are backstopped by the liquidator vault; HIP-3 markets are not,
    /// and go straight to auto-deleveraging. PAXG (one fine troy ounce of
    /// gold) is a core perp with the same backstop: the desk's gold offset
    /// (`docs/83` D4, 2026-10-15, under the founder's delegation).
    pub allowlist: Vec<String>,
    /// Hyperliquid rejects orders under 10 USDC of notional.
    pub min_notional_usdc: Decimal,
    /// Worst acceptable price for a hedge order, as basis points away from
    /// the mark. A hedge that does not fill is worse than one filled 0.5%
    /// off, so this is generous.
    pub slippage_bps: u32,
    /// The most of the treasury's USDC that may sit on the venue as margin,
    /// in percent (docs/46 #7, CFO seat). The venue can only ever touch the
    /// margin placed there, so this number *is* the venue-side maximum loss,
    /// and it must not grow quietly. Enforced where USDC moves to the venue
    /// (`scripts/hedge_fund_from_evm.sh`, which reads the same default), not
    /// in `safety`, which sees only venue state. 50 = half.
    pub max_margin_share_pct: u32,
}

impl Default for HedgePolicy {
    fn default() -> Self {
        HedgePolicy {
            band_bps: 500,
            max_leverage: 2,
            k_maintenance: 4,
            allowlist: vec!["BTC".into(), "ETH".into(), "SOL".into(), "PAXG".into()],
            min_notional_usdc: dec!(10),
            slippage_bps: 50,
            max_margin_share_pct: 50,
        }
    }
}

impl HedgePolicy {
    pub fn allows(&self, coin: &str) -> bool {
        self.allowlist.iter().any(|c| c == coin)
    }

    /// Refuse a policy that cannot do what it says, listing every problem
    /// rather than the first: a caller who fixes one and re-runs to find the
    /// next learns the list one round trip at a time.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        if self.band_bps == 0 || self.band_bps >= 10_000 {
            problems.push(format!(
                "band_bps {} must be between 1 and 9999",
                self.band_bps
            ));
        }
        if self.max_leverage == 0 {
            problems.push("max_leverage must be at least 1".to_string());
        }
        if self.max_leverage > 5 {
            problems.push(format!(
                "max_leverage {}x: a hedge of a holding the treasury owns never needs more than 2x; 5x is the ceiling",
                self.max_leverage
            ));
        }
        if self.k_maintenance < 2 {
            problems.push(format!(
                "k_maintenance {} leaves no buffer above the venue's own maintenance line",
                self.k_maintenance
            ));
        }
        if self.allowlist.is_empty() {
            problems.push("allowlist is empty: nothing can be hedged".to_string());
        }
        if self.min_notional_usdc <= Decimal::ZERO {
            problems.push("min_notional_usdc must be positive".to_string());
        }
        if self.slippage_bps >= 1_000 {
            problems.push(format!(
                "slippage_bps {} is 10% or more; a hedge filled that far off the mark is a loss, not a hedge",
                self.slippage_bps
            ));
        }
        if self.max_margin_share_pct == 0 || self.max_margin_share_pct > 100 {
            problems.push(format!(
                "max_margin_share_pct {} must be between 1 and 100",
                self.max_margin_share_pct
            ));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_margin_share_matches_the_funding_script() {
        // scripts/hedge_fund_from_evm.sh carries MAX_MARGIN_SHARE=50 as its
        // default; the two must say the same number or one of them lies.
        let script = include_str!("../../../scripts/hedge_fund_from_evm.sh");
        assert!(
            script.contains("MAX_MARGIN_SHARE=${MAX_MARGIN_SHARE:-50}"),
            "script default drifted"
        );
        assert_eq!(HedgePolicy::default().max_margin_share_pct, 50);
        assert!(HedgePolicy::default().validate().is_ok());
    }

    #[test]
    fn a_margin_share_outside_one_to_one_hundred_is_refused() {
        for bad in [0u32, 101, 500] {
            let p = HedgePolicy {
                max_margin_share_pct: bad,
                ..HedgePolicy::default()
            };
            let problems = p.validate().unwrap_err();
            assert!(
                problems.iter().any(|m| m.contains("max_margin_share_pct")),
                "{problems:?}"
            );
        }
    }
}
