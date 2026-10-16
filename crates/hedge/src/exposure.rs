//! What the treasury is exposed to.

use rust_decimal::Decimal;

/// A spot holding the treasury carries, valued at the venue's mark.
///
/// `size` is in base units (0.5 ETH is `0.5`). The USDC value is derived,
/// never stored, so a stale valuation cannot be carried forward by accident.
#[derive(Debug, Clone, PartialEq)]
pub struct SpotExposure {
    pub coin: String,
    pub size: Decimal,
    pub mark_px: Decimal,
}

impl SpotExposure {
    pub fn new(coin: impl Into<String>, size: Decimal, mark_px: Decimal) -> Self {
        SpotExposure {
            coin: coin.into(),
            size,
            mark_px,
        }
    }

    /// USDC value at the mark. `None` on overflow.
    pub fn notional(&self) -> Option<Decimal> {
        self.size.checked_mul(self.mark_px)
    }
}
