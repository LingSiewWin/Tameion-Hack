//! The margin-safety engine. The part of the product that keeps the hedge
//! alive through a spike, stated against the venue's own numbers.
//!
//! Hyperliquid's liquidation waterfall (`docs/42`): when account equity drops
//! below maintenance margin, positions are market-sold; below 2/3 of
//! maintenance margin the liquidator vault takes the position; below zero,
//! the other side is auto-deleveraged. Maintenance margin is half the initial
//! margin at the max leverage of the tier the position's notional falls in.
//!
//! This module never lets an order through that would leave the account
//! closer to that line than the policy's `k × maintenance margin`, or above
//! the policy's leverage cap. When the requested size breaks a limit it
//! returns the largest size that does not, so the caller shrinks the order
//! rather than skipping the hedge. Pure; tested by property.
//!
//! Cross margin is one account, so both limits are judged on the **whole**
//! account: the other open positions' notional and maintenance margin are
//! carried in [`AccountState`] and added to this order's. A second coin is
//! never allowed to pretend the first one is not there.
//!
//! Arithmetic note: the maintenance rate is `1 / (2L)` and `L` is often not
//! a divisor of a power of ten (3x → 1/6). Every limit below is therefore
//! computed with the integer `2L`, with one division at the end, and sizes
//! derived from a limit are rounded *toward zero*, so a limit this module
//! reports as satisfied is satisfied exactly, not up to a rounding error.

use crate::policy::HedgePolicy;
use rust_decimal::{Decimal, RoundingStrategy};

/// Sizes derived from a limit are floored at this many places. Venue size
/// ticks are coarser (2–5 places); the engine rounds again to the tick.
const SIZE_PLACES: u32 = 8;

/// The venue's view of the account, in USDC.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountState {
    /// Equity: collateral plus unrealised PnL.
    pub account_value: Decimal,
    /// Initial margin currently committed to open positions.
    pub margin_used: Decimal,
    /// Maintenance margin the venue currently requires, all positions.
    pub maintenance_margin_used: Decimal,
    /// Sum of `|size| × mark` over every open position. Cross-margin
    /// leverage is judged on this total, not on one coin.
    pub total_notional: Decimal,
}

/// One tier of a venue margin table: above `lower_bound` of position
/// notional, at most `max_leverage` applies.
#[derive(Debug, Clone, PartialEq)]
pub struct Tier {
    pub lower_bound: Decimal,
    pub max_leverage: u32,
}

/// The order the controller wants to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposed {
    pub coin: String,
    /// Current signed position size (negative = short).
    pub current_size: Decimal,
    /// Signed change the controller asked for.
    pub size_delta: Decimal,
    /// Price the order will be valued at (mark, or the worst fill price).
    pub px: Decimal,
}

/// Why the engine would not send the order as asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// `runtime/hedge/HALT` exists. A human has to clear it.
    Halted,
    /// Not in the policy allowlist.
    CoinNotAllowed(String),
    /// Account value is zero or negative; nothing can be margined.
    NoCollateral,
    /// The price is zero or negative.
    BadPrice,
    /// Even the largest safe size is under the venue minimum; the hedge
    /// cannot be placed at all with this collateral (or what is left of it
    /// after the other positions).
    BelowMinNotional {
        max_safe_notional: Decimal,
        min_notional: Decimal,
    },
    /// Decimal arithmetic overflowed. Refuse rather than guess.
    Overflow,
}

/// Why the engine shrank the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shrink {
    /// Would exceed `policy.max_leverage` on the whole account.
    Leverage,
    /// Account value would fall under `k × maintenance margin`.
    MaintenanceBuffer,
}

/// The engine's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Allow(Limits),
    /// Send a smaller order. `limits.size_delta` is the one to send.
    Reduce {
        why: Shrink,
        limits: Limits,
    },
    Refuse(Refusal),
}

/// The numbers behind an `Allow` or `Reduce`, kept so the decision log can
/// carry them. Leverage and required equity are for the whole account.
#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    pub size_delta: Decimal,
    pub size_after: Decimal,
    /// This coin's notional after the order.
    pub notional_after: Decimal,
    /// (other positions + this coin) / equity.
    pub leverage_after: Decimal,
    /// `1 / (2 × tier max leverage)`, for display.
    pub maintenance_rate: Decimal,
    /// `k × (other maintenance margin + this coin's)`.
    pub required_equity: Decimal,
}

/// The tier max leverage `L` that applies to a position of this notional.
///
/// Tiers are matched by the highest `lower_bound` not above `notional`. With
/// no tiers the market's own max leverage is used. Never below 1.
pub fn maintenance_leverage(tiers: &[Tier], notional: Decimal, fallback_max_leverage: u32) -> u32 {
    tiers
        .iter()
        .filter(|t| t.lower_bound <= notional)
        .max_by(|a, b| a.lower_bound.cmp(&b.lower_bound))
        .map_or(fallback_max_leverage, |t| t.max_leverage)
        .max(1)
}

/// Maintenance margin as a fraction of notional: half the initial margin at
/// the tier's max leverage, `1 / (2L)`. For display; limits use `2L` directly.
pub fn maintenance_rate(tiers: &[Tier], notional: Decimal, fallback_max_leverage: u32) -> Decimal {
    Decimal::ONE / Decimal::from(2 * maintenance_leverage(tiers, notional, fallback_max_leverage))
}

/// The venue's own maintenance margin for one position (Hyperliquid, "Margin
/// tiers"): `notional × rate(tier) − deduction(tier)`, the deduction keeping
/// the margin continuous across tiers. Never more than `notional / 2L`, which
/// is what this engine charges for the order it is judging.
pub fn venue_maintenance(tiers: &[Tier], notional: Decimal, fallback_max_leverage: u32) -> Decimal {
    let rate = |l: u32| Decimal::ONE / Decimal::from(2 * l.max(1));
    let mut sorted: Vec<&Tier> = tiers.iter().collect();
    sorted.sort_by_key(|t| t.lower_bound);
    let (mut mm, mut deduction, mut prev) =
        (notional * rate(fallback_max_leverage), Decimal::ZERO, None);
    for t in sorted.into_iter().take_while(|t| t.lower_bound <= notional) {
        if let Some(p) = prev {
            deduction += t.lower_bound * (rate(t.max_leverage) - rate(p));
        }
        prev = Some(t.max_leverage);
        mm = notional * rate(t.max_leverage) - deduction;
    }
    mm
}

/// Equity divided by the equity the policy requires right now. Above 1 is
/// inside policy; 1/k is the venue's maintenance line; 2/(3k) is the
/// backstop. `None` with no open position.
pub fn health(acct: &AccountState, policy: &HedgePolicy) -> Option<Decimal> {
    if acct.maintenance_margin_used.is_zero() {
        return None;
    }
    acct.account_value
        .checked_div(acct.maintenance_margin_used * Decimal::from(policy.k_maintenance))
}

/// Decide whether `p` may be sent, shrunk, or must be refused.
pub fn check(
    p: &Proposed,
    acct: &AccountState,
    tiers: &[Tier],
    fallback_max_leverage: u32,
    policy: &HedgePolicy,
    halted: bool,
) -> Verdict {
    if halted {
        return Verdict::Refuse(Refusal::Halted);
    }
    if !policy.allows(&p.coin) {
        return Verdict::Refuse(Refusal::CoinNotAllowed(p.coin.clone()));
    }
    if p.px <= Decimal::ZERO {
        return Verdict::Refuse(Refusal::BadPrice);
    }
    if acct.account_value <= Decimal::ZERO {
        return Verdict::Refuse(Refusal::NoCollateral);
    }

    let size_after = p.current_size + p.size_delta;
    let Some(notional_after) = size_after.abs().checked_mul(p.px) else {
        return Verdict::Refuse(Refusal::Overflow);
    };
    let two_l =
        Decimal::from(2 * maintenance_leverage(tiers, notional_after, fallback_max_leverage));
    let k = Decimal::from(policy.k_maintenance);
    let max_lev = Decimal::from(policy.max_leverage);

    // What the rest of the account carries once this coin's current position
    // is taken out of the totals (it is being replaced by `size_after`).
    // Clamped at zero: a venue total can lag a position by a tick.
    let Some(current_notional) = p.current_size.abs().checked_mul(p.px) else {
        return Verdict::Refuse(Refusal::Overflow);
    };
    let other = Other {
        notional: (acct.total_notional - current_notional).max(Decimal::ZERO),
        // At the venue's own figure for the current position (its tier, its
        // deduction), not at the rate of the size asked for: an order that
        // crosses a tier and is then shrunk below the current size would
        // otherwise understate the other positions (docs/68 part 4).
        maintenance: (acct.maintenance_margin_used
            - venue_maintenance(tiers, current_notional, fallback_max_leverage))
        .max(Decimal::ZERO),
    };

    let Some(limits) = limits_for(
        p.size_delta,
        size_after,
        notional_after,
        two_l,
        k,
        acct,
        &other,
    ) else {
        return Verdict::Refuse(Refusal::Overflow);
    };

    // Closing to flat removes this coin's risk; it is always allowed.
    if size_after.is_zero() {
        return Verdict::Allow(limits);
    }

    let within_leverage = limits.leverage_after <= max_lev;
    let within_buffer = acct.account_value >= limits.required_equity;
    if within_leverage && within_buffer {
        if notional_after < policy.min_notional_usdc {
            return Verdict::Refuse(Refusal::BelowMinNotional {
                max_safe_notional: notional_after,
                min_notional: policy.min_notional_usdc,
            });
        }
        return Verdict::Allow(limits);
    }

    // Over a limit. The largest notional for THIS coin that both limits
    // accept once the other positions are counted, using the tier at the
    // requested size: a smaller notional can only land in a tier with a
    // higher or equal max leverage, so this is conservative.
    //   by leverage: equity × max_lev − other notional
    //   by buffer:   (equity − k × other maintenance) × 2L / k
    let by_leverage = acct
        .account_value
        .checked_mul(max_lev)
        .map(|v| (v - other.notional).max(Decimal::ZERO));
    let by_buffer = other
        .maintenance
        .checked_mul(k)
        .map(|km| (acct.account_value - km).max(Decimal::ZERO))
        .and_then(|free| free.checked_mul(two_l))
        .and_then(|v| v.checked_div(k));
    let (Some(by_leverage), Some(by_buffer)) = (by_leverage, by_buffer) else {
        return Verdict::Refuse(Refusal::Overflow);
    };
    let (max_notional, why) = if by_leverage <= by_buffer {
        (by_leverage, Shrink::Leverage)
    } else {
        (by_buffer, Shrink::MaintenanceBuffer)
    };
    // Size from notional, floored so size × px never exceeds the limit.
    let Some(max_size) = max_notional
        .checked_div(p.px)
        .map(|s| s.round_dp_with_strategy(SIZE_PLACES, RoundingStrategy::ToZero))
    else {
        return Verdict::Refuse(Refusal::Overflow);
    };
    let Some(reduced_notional) = max_size.checked_mul(p.px) else {
        return Verdict::Refuse(Refusal::Overflow);
    };
    if reduced_notional < policy.min_notional_usdc {
        // No room left for this coin, but an order that only shrinks the
        // position moves the account back toward both limits; refusing it
        // would keep the account where it is.
        if reduces_only(p) {
            return Verdict::Allow(limits);
        }
        return Verdict::Refuse(Refusal::BelowMinNotional {
            max_safe_notional: reduced_notional,
            min_notional: policy.min_notional_usdc,
        });
    }
    // Keep the side the controller asked for; shrink the magnitude.
    let reduced_after = if size_after.is_sign_negative() {
        -max_size
    } else {
        max_size
    };
    let reduced_delta = reduced_after - p.current_size;
    let Some(reduced) = limits_for(
        reduced_delta,
        reduced_after,
        reduced_notional,
        two_l,
        k,
        acct,
        &other,
    ) else {
        return Verdict::Refuse(Refusal::Overflow);
    };
    Verdict::Reduce {
        why,
        limits: reduced,
    }
}

/// The order only shrinks the position, on the same side: it lowers both the
/// account's leverage and its maintenance margin.
pub fn reduces_only(p: &Proposed) -> bool {
    let after = p.current_size + p.size_delta;
    after.abs() < p.current_size.abs()
        && (after.is_zero() || after.is_sign_negative() == p.current_size.is_sign_negative())
}

/// The rest of the account, with this coin's current position removed.
struct Other {
    notional: Decimal,
    maintenance: Decimal,
}

/// `required = k × (other_mm + notional × / 2L)`; `leverage = (other + notional) / equity`.
fn limits_for(
    size_delta: Decimal,
    size_after: Decimal,
    notional_after: Decimal,
    two_l: Decimal,
    k: Decimal,
    acct: &AccountState,
    other: &Other,
) -> Option<Limits> {
    let total = other.notional.checked_add(notional_after)?;
    let leverage_after = total.checked_div(acct.account_value)?;
    let this_mm = notional_after.checked_mul(k)?.checked_div(two_l)?;
    let required_equity = other.maintenance.checked_mul(k)?.checked_add(this_mm)?;
    Some(Limits {
        size_delta,
        size_after,
        notional_after,
        leverage_after,
        maintenance_rate: Decimal::ONE / two_l,
        required_equity,
    })
}
