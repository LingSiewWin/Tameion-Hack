//! Recovering what an agent owes from its own payment history.
//!
//! An agent that pays bills on its own authority has to know what it owes
//! before it can decide anything, and nobody hands it that list. A card
//! network carries the merchant category for free; a USDC transfer carries a
//! destination address and nothing else. So the obligations have to be
//! recovered from the shape of the payments themselves.
//!
//! The signal that survives with no metadata at all is recurrence: the same
//! counterparty, a similar amount, a regular interval. That is enough to say
//! "this reads as a subscription, and the next one is due Thursday" without
//! anyone having labelled anything.
//!
//! # Honesty about confidence is load-bearing
//!
//! Three payments at a tight interval is a weaker claim than twelve, and the
//! caller has to be able to tell the difference, because a low-confidence
//! obligation must reach a human rather than a transfer. Every [`Obligation`]
//! therefore carries the transaction hashes it was derived from: the claim is
//! auditable back to on-chain facts rather than asserted.
//!
//! # Why floats appear here at all
//!
//! [`Money`] forbids any path to or from a float, and that is not relaxed
//! here: `typical_amount` is a `Money`, and it is a *median of amounts
//! actually paid*, never an average that might land between two payable
//! values.
//!
//! Confidence and the coefficients of variation are different. They are
//! scores, not quantities of value. Nobody is ever paid 0.85 of anything, a
//! rounding error in the fourth decimal place of a confidence score cannot
//! lose money, and the alternative -- fixed-point arithmetic for a number
//! that exists to be compared against a threshold -- buys nothing. The rule
//! is not "floats are bad", it is "money is not a float".

use crate::money::{Currency, Money};
use crate::time::{Seconds, Timestamp};
use core::fmt;
use std::collections::BTreeMap;

/// A pattern needs at least three payments. Two payments define exactly one
/// interval, and a single interval is not evidence of regularity: any two
/// dates are perfectly regular with respect to themselves.
pub const MIN_SAMPLES: usize = 3;

/// Coefficient of variation of the gaps between payments. Above this the
/// payments are not on a schedule; they merely share a payee.
pub const MAX_INTERVAL_CV: f64 = 0.35;

/// Below this CV the amounts are effectively identical, which is what a fixed
/// subscription looks like. Above it but still regular in time reads as a
/// recurring payment with a variable amount, such as a contractor billing
/// hours.
pub const SUBSCRIPTION_AMOUNT_CV: f64 = 0.02;

/// Which way value moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Value left the treasury. Only these become obligations.
    Out,
    /// Value arrived. Revenue, not an obligation.
    In,
}

/// A counterparty address, normalised once at construction.
///
/// Hex addresses arrive in mixed case -- `0xABC` from one source, `0xabc`
/// from another -- and treating those as two different payees splits a
/// six-payment history into two three-payment histories, or into none at all.
/// Normalising in the constructor means there is no un-normalised
/// `Counterparty` anywhere in the program to forget about.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Counterparty(String);

impl Counterparty {
    pub fn new(id: impl AsRef<str>) -> Self {
        Counterparty(id.as_ref().trim().to_ascii_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Counterparty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A score in `0.0..=1.0`.
///
/// A newtype because the policy engine reads it directly against a threshold,
/// and a confidence of 1.3 or of NaN would compare in ways nobody intended.
/// The constructor is the only way in and it rejects both.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Confidence(f64);

impl Confidence {
    pub const ZERO: Confidence = Confidence(0.0);

    /// Rejects NaN, infinities, and anything outside `0.0..=1.0`.
    pub fn new(v: f64) -> Option<Confidence> {
        if v.is_finite() && (0.0..=1.0).contains(&v) {
            Some(Confidence(v))
        } else {
            None
        }
    }

    /// Clamp into range. For arithmetic whose result is known to be bounded
    /// but whose last bit may drift.
    pub fn clamped(v: f64) -> Confidence {
        if v.is_nan() {
            Confidence(0.0)
        } else {
            Confidence(v.clamp(0.0, 1.0))
        }
    }

    pub const fn get(self) -> f64 {
        self.0
    }

    pub fn at_least(self, floor: Confidence) -> bool {
        self.0 >= floor.0
    }
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.2}", self.0)
    }
}

/// One observed movement of value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub tx_hash: String,
    pub counterparty: Counterparty,
    pub amount: Money,
    pub at: Timestamp,
    pub direction: Direction,
}

/// What a recurring pattern reads as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObligationKind {
    /// Fixed amount, fixed cadence.
    Subscription,
    /// Regular cadence, varying amount.
    RecurringPayment,
}

impl fmt::Display for ObligationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ObligationKind::Subscription => "subscription",
            ObligationKind::RecurringPayment => "recurring payment",
        })
    }
}

/// A recurring payment the agent believes it owes.
#[derive(Debug, Clone, PartialEq)]
pub struct Obligation {
    pub counterparty: Counterparty,
    pub kind: ObligationKind,
    /// Median of amounts actually paid. Never an interpolated value.
    pub typical_amount: Money,
    pub interval: Seconds,
    pub last_paid: Timestamp,
    pub next_due: Timestamp,
    pub confidence: Confidence,
    pub sample_count: usize,
    pub interval_cv: f64,
    pub amount_cv: f64,
    /// Transaction hashes this claim rests on, oldest first.
    pub evidence: Vec<String>,
}

impl Obligation {
    /// True once `now` has reached the predicted due date, less any grace.
    pub fn is_due(&self, now: Timestamp, grace: Seconds) -> bool {
        now >= self.next_due - grace
    }

    /// How overdue, negative if not yet due.
    pub fn overdue_by(&self, now: Timestamp) -> Seconds {
        now - self.next_due
    }

    /// Stable identifier for this counterparty's current billing cycle.
    ///
    /// Two runs that see the same obligation produce the same key, which is
    /// what lets the decision log and the on-chain policy both refuse a
    /// second payment for a cycle already settled. Keyed on the due date
    /// rather than the run time, so a retry minutes later and a retry days
    /// later are both caught.
    pub fn cycle_key(&self) -> String {
        format!("{}@{}", self.counterparty, self.next_due.as_unix())
    }

    pub fn currency(&self) -> Currency {
        self.typical_amount.currency()
    }

    /// One sentence a human can check against the evidence.
    pub fn explain(&self) -> String {
        format!(
            "{} has been paid {} times every {}, amounts varying by {:.1}% \
             and timing by {:.1}%, so this reads as a {} (confidence {}).",
            self.counterparty,
            self.sample_count,
            self.interval.human(),
            self.amount_cv * 100.0,
            self.interval_cv * 100.0,
            self.kind,
            self.confidence,
        )
    }
}

/// Whether a transfer is too small to be evidence that anyone is owed
/// anything: at or below one whole unit of its currency (1 USDC, 1 USD).
///
/// Address poisoning plants zero-value or dust transfers so a look-alike
/// address appears in a treasury's history as though it had been paid.
/// docs/21 §7 found look-alikes planted in 64 of 130 real treasuries, and the
/// detector inferred obligations to two of them. The threshold is the one that
/// scan used. A real bill of one unit or less is never paid unattended as a
/// result -- the safe direction to err in.
pub fn is_dust(amount: Money) -> bool {
    let one = 10_i128.pow(amount.currency().scale() as u32);
    amount.to_minor().abs() <= one
}

/// Thresholds for [`detect`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectionConfig {
    pub min_samples: usize,
    pub max_interval_cv: f64,
    pub subscription_amount_cv: f64,
    /// Transfers at or below this amount are not evidence of anything.
    /// `None` means one whole unit of the transfer's currency ([`is_dust`]).
    ///
    /// Set lower only where every amount is scaled down with it -- a testnet
    /// demonstration paying thousandths of a USDC -- so that what is filtered
    /// is still the poisoner's zero-value and dust transfers and not the
    /// payments themselves.
    pub dust_line: Option<Money>,
}

impl DetectionConfig {
    /// Whether `amount` is too small to count, under this configuration.
    pub fn is_dust(&self, amount: Money) -> bool {
        match self.dust_line {
            Some(line) if line.currency() == amount.currency() => {
                amount.to_minor().abs() <= line.to_minor()
            }
            _ => is_dust(amount),
        }
    }
}

impl Default for DetectionConfig {
    fn default() -> Self {
        DetectionConfig {
            min_samples: MIN_SAMPLES,
            max_interval_cv: MAX_INTERVAL_CV,
            subscription_amount_cv: SUBSCRIPTION_AMOUNT_CV,
            dust_line: None,
        }
    }
}

/// Sample standard deviation over mean, by magnitude. `0.0` is perfectly
/// uniform.
///
/// Sample rather than population -- the `n - 1` denominator -- because these
/// payments are a sample of an ongoing relationship, not the complete
/// population of every payment that will ever be made.
fn coefficient_of_variation(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    if mean == 0.0 {
        return 0.0;
    }
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    variance.sqrt() / mean.abs()
}

/// What a run of `n` payments is worth on its own.
///
/// Asymptotic toward 1.0: the third payment tells you far more than the
/// thirtieth does. Never reaches 1.0, because a pattern observed is still a
/// pattern that can break next month.
fn sample_confidence(n: usize, min_samples: usize) -> f64 {
    if n < min_samples || n < 2 {
        return 0.0;
    }
    1.0 - (0.75 / (n as f64 - 1.0))
}

/// Median of a sorted slice of money, taking the **lower** of the two middle
/// values when the count is even.
///
/// Interpolating would be the textbook median, but the result has to be an
/// amount somebody could actually be paid. Averaging 10.000000 and 11.000000
/// gives 10.500000, which is representable; averaging 10.000000 and
/// 10.000001 gives a half-unit, which is not. Taking the lower middle always
/// returns a value that was genuinely paid at least once, and errs toward
/// underpaying rather than overpaying -- the safer direction for an agent
/// acting unsupervised.
fn lower_median_money(sorted: &[Money]) -> Option<Money> {
    if sorted.is_empty() {
        return None;
    }
    Some(sorted[(sorted.len() - 1) / 2])
}

/// Median of sorted durations, interpolating on an even count.
///
/// Unlike an amount, an interval estimate is not paid to anyone, so the
/// halfway point between two observed gaps is a legitimate estimate. Integer
/// division floors, which keeps the prediction a whole second.
fn median_gap(sorted: &[Seconds]) -> Option<Seconds> {
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => Some(sorted[n / 2]),
        n => {
            let a = sorted[n / 2 - 1].as_i64();
            let b = sorted[n / 2].as_i64();
            // (a + b) / 2 without overflowing on large values.
            Some(Seconds::new(a + (b - a) / 2))
        }
    }
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

/// Recover recurring obligations from a wallet's history.
///
/// Only outgoing transfers are considered: value received is revenue, not an
/// obligation. Dust is ignored ([`is_dust`]): it is how an attacker writes
/// themselves into the history.
///
/// Payments are grouped by counterparty **and currency**. A vendor paid in
/// both USDC and EURC is two obligations, not one confused one -- and the
/// group's amounts are then guaranteed comparable, so the statistics below
/// are never taken over a mixture of scales.
///
/// Returns obligations sorted by due date, soonest first, so the caller reads
/// them in the order they will need deciding.
pub fn detect(transfers: &[Transfer], config: DetectionConfig) -> Vec<Obligation> {
    let mut groups: BTreeMap<(Counterparty, Currency), Vec<&Transfer>> = BTreeMap::new();
    for t in transfers {
        if t.direction != Direction::Out || config.is_dust(t.amount) {
            continue;
        }
        groups
            .entry((t.counterparty.clone(), t.amount.currency()))
            .or_default()
            .push(t);
    }

    let mut out = Vec::new();

    for ((counterparty, _ccy), mut payments) in groups {
        if payments.len() < config.min_samples {
            continue;
        }
        payments.sort_by_key(|p| p.at);

        // Gaps between consecutive payments. A zero or negative gap means two
        // payments landed in the same second or out of order; neither is
        // evidence of a schedule, so they are dropped rather than counted as
        // an interval of zero, which would drag the median toward nothing.
        let mut gaps: Vec<Seconds> = payments
            .windows(2)
            .filter_map(|w| {
                let d = w[1].at - w[0].at;
                (d > Seconds::ZERO).then_some(d)
            })
            .collect();

        if gaps.len() + 1 < config.min_samples {
            continue;
        }

        let gap_floats: Vec<f64> = gaps.iter().map(|g| g.as_i64() as f64).collect();
        let interval_cv = coefficient_of_variation(&gap_floats);
        if interval_cv > config.max_interval_cv {
            // Same payee, no rhythm. Refuse to invent a schedule that is not
            // there rather than report one with a low score.
            continue;
        }

        let amount_floats: Vec<f64> = payments
            .iter()
            .map(|p| p.amount.to_minor() as f64)
            .collect();
        let amount_cv = coefficient_of_variation(&amount_floats);

        gaps.sort_unstable();
        // Safe: `gaps` is non-empty, checked above.
        let Some(interval) = median_gap(&gaps) else {
            continue;
        };

        let mut sorted_amounts: Vec<Money> = payments.iter().map(|p| p.amount).collect();
        sorted_amounts.sort_by_key(|m| m.to_minor());
        let Some(typical_amount) = lower_median_money(&sorted_amounts) else {
            continue;
        };

        let kind = if amount_cv <= config.subscription_amount_cv {
            ObligationKind::Subscription
        } else {
            ObligationKind::RecurringPayment
        };

        // Confidence degrades with irregular timing, and with irregular
        // amounts. Each factor is a ratio of observed tidiness to the point
        // at which the pattern would have been rejected outright.
        let timing_factor = 1.0 - (interval_cv / config.max_interval_cv).min(1.0) * 0.5;
        let amount_factor = 1.0 - amount_cv.min(1.0) * 0.3;
        let confidence = Confidence::clamped(round4(
            sample_confidence(payments.len(), config.min_samples) * timing_factor * amount_factor,
        ));

        let last = payments[payments.len() - 1];
        let Some(next_due) = last.at.checked_add(interval) else {
            continue;
        };

        out.push(Obligation {
            counterparty,
            kind,
            typical_amount,
            interval,
            last_paid: last.at,
            next_due,
            confidence,
            sample_count: payments.len(),
            interval_cv: round4(interval_cv),
            amount_cv: round4(amount_cv),
            evidence: payments.iter().map(|p| p.tx_hash.clone()).collect(),
        });
    }

    out.sort_by_key(|o| o.next_due);
    out
}

/// Obligations falling due within `horizon` of `now`.
///
/// The "upcoming obligations" half of a liquidity check. RFB 04 asks for
/// "liquidity monitoring that reads balances and upcoming obligations
/// together", and a balance on its own means nothing without this side.
pub fn upcoming(obligations: &[Obligation], now: Timestamp, horizon: Seconds) -> Vec<&Obligation> {
    let cutoff = now + horizon;
    obligations
        .iter()
        .filter(|o| o.next_due <= cutoff)
        .collect()
}

/// Exact total of what the given obligations will cost.
///
/// Returns `None` on a currency mismatch or on overflow, rather than
/// producing a number that is wrong in a way nobody would notice.
pub fn total_due<'a>(
    obligations: impl IntoIterator<Item = &'a Obligation>,
    ccy: Currency,
) -> Option<Money> {
    let mut total = Money::zero(ccy);
    for o in obligations {
        total = total.checked_add(o.typical_amount).ok()?;
    }
    Some(total)
}
