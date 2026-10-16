//! Instants and durations, in whole seconds.
//!
//! Separate types rather than a pair of bare integers, because the two are
//! not interchangeable and the compiler is willing to say so: adding two
//! instants together is meaningless, subtracting them yields a duration, and
//! a duration added to an instant yields another instant. Written as bare
//! `i64` those three expressions are indistinguishable.
//!
//! Whole seconds, signed. Signed because the interesting quantity is almost
//! always a *difference* -- how overdue, how far apart -- and an unsigned
//! difference that underflows is how a payment three seconds early becomes
//! one 584 billion years late.

use core::fmt;
use core::ops::{Add, Sub};

/// A duration in whole seconds. May be negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Seconds(i64);

impl Seconds {
    pub const ZERO: Seconds = Seconds(0);
    pub const MINUTE: Seconds = Seconds(60);
    pub const HOUR: Seconds = Seconds(3_600);
    pub const DAY: Seconds = Seconds(86_400);

    pub const fn new(secs: i64) -> Self {
        Seconds(secs)
    }

    /// Build from a whole number of days.
    pub const fn days(d: i64) -> Option<Self> {
        match d.checked_mul(86_400) {
            Some(s) => Some(Seconds(s)),
            None => None,
        }
    }

    pub const fn as_i64(self) -> i64 {
        self.0
    }

    /// Length in days, fractional. A presentation quantity: it is fine for
    /// this to be a float because nothing is ever paid in days.
    /// A cadence a person can read. Days are the natural unit for a billing
    /// cycle, but a demonstration compresses months into seconds, and
    /// "every 0.0 days" reads as a bug rather than as 40 seconds.
    pub fn human(self) -> String {
        let s = self.as_i64().abs();
        match s {
            0..=90 => format!("{s}s"),
            91..=5399 => format!("{:.1}m", s as f64 / 60.0),
            5400..=86_399 => format!("{:.1}h", s as f64 / 3600.0),
            _ => format!("{:.1} days", s as f64 / 86_400.0),
        }
    }

    pub fn as_days(self) -> f64 {
        self.0 as f64 / 86_400.0
    }

    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub fn checked_add(self, rhs: Seconds) -> Option<Seconds> {
        self.0.checked_add(rhs.0).map(Seconds)
    }

    pub fn checked_mul(self, factor: i64) -> Option<Seconds> {
        self.0.checked_mul(factor).map(Seconds)
    }

    pub fn abs(self) -> Option<Seconds> {
        self.0.checked_abs().map(Seconds)
    }
}

impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.as_days();
        if d.abs() >= 1.0 {
            write!(f, "{d:.1} days")
        } else {
            write!(f, "{}s", self.0)
        }
    }
}

/// A point in time: whole seconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const EPOCH: Timestamp = Timestamp(0);

    pub const fn from_unix(secs: i64) -> Self {
        Timestamp(secs)
    }

    pub const fn as_unix(self) -> i64 {
        self.0
    }

    /// For the Action IR boundary, which carries unsigned seconds.
    /// Returns `None` before the epoch.
    pub const fn as_u64(self) -> Option<u64> {
        if self.0 < 0 {
            None
        } else {
            Some(self.0 as u64)
        }
    }

    pub fn checked_add(self, d: Seconds) -> Option<Timestamp> {
        self.0.checked_add(d.0).map(Timestamp)
    }

    pub fn checked_sub(self, d: Seconds) -> Option<Timestamp> {
        self.0.checked_sub(d.0).map(Timestamp)
    }

    /// Distance from `earlier` to `self`. Negative if `self` is earlier.
    pub fn since(self, earlier: Timestamp) -> Option<Seconds> {
        self.0.checked_sub(earlier.0).map(Seconds)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t={}", self.0)
    }
}

// Only the meaningful combinations get operators. `Timestamp + Timestamp` has
// no impl and so does not compile, which is the point of the two types.
impl Add<Seconds> for Timestamp {
    type Output = Timestamp;
    fn add(self, rhs: Seconds) -> Timestamp {
        Timestamp(self.0.saturating_add(rhs.0))
    }
}

impl Sub<Seconds> for Timestamp {
    type Output = Timestamp;
    fn sub(self, rhs: Seconds) -> Timestamp {
        Timestamp(self.0.saturating_sub(rhs.0))
    }
}

impl Sub<Timestamp> for Timestamp {
    type Output = Seconds;
    fn sub(self, rhs: Timestamp) -> Seconds {
        Seconds(self.0.saturating_sub(rhs.0))
    }
}

impl Add<Seconds> for Seconds {
    type Output = Seconds;
    fn add(self, rhs: Seconds) -> Seconds {
        Seconds(self.0.saturating_add(rhs.0))
    }
}
