//! Money that cannot silently become a different kind of number.
//!
//! The Canteen survey of nine open-source accounting systems found four
//! distinct money-representation defects: Invoice Ninja marked a EUR 0.10
//! subscription paid from one integer cast, ERPNext's ORM casts every decimal
//! column to float on read, Ghostfolio stores quantity and price as floats,
//! and GnuCash caps a commodity fraction at 10^9 because 10^10 would have
//! required widening a C `int64_t`.
//!
//! Every one of those is a representation choice, so this module makes the
//! wrong representations unconstructable rather than discouraged:
//!
//! - the amount is an integer count of minor units, never a float
//! - there is no `From<f64>`, no `Into<f64>`, and no deref to a primitive,
//!   so the Invoice Ninja cast has no syntax available to it
//! - the currency travels with the amount, so USDC cannot be added to USD
//! - `i128` holds ~1.7e38 minor units, which at USDC's six decimals is
//!   ~1.7e32 USDC, so the GnuCash ceiling is not reachable
//! - every operation that can fail returns `Result`, so a caller cannot
//!   ignore an overflow by accident

use core::fmt;

/// A currency, carrying the number of decimal places it settles at.
///
/// Scale is a property of the currency rather than of a database column,
/// because the mismatch between a two-decimal currency and six-decimal
/// settlement is exactly what produces a Round Off account full of residue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Currency {
    /// Six decimals. The settlement currency on Arc, where it is also gas.
    Usdc,
    /// Six decimals.
    Eurc,
    /// Two decimals.
    Usd,
    /// Two decimals.
    Eur,
    /// Grams of fine gold, to the milligram (three decimals). Goods are
    /// booked like a currency, as GnuCash books commodities, so a gold
    /// trader's book balances in grams the way a cash book balances in
    /// dollars, and grams cannot be added to USDC by accident.
    GoldGram,
}

impl Currency {
    /// Decimal places this currency settles at.
    pub const fn scale(self) -> u8 {
        match self {
            Currency::Usdc | Currency::Eurc => 6,
            Currency::Usd | Currency::Eur => 2,
            Currency::GoldGram => 3,
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Currency::Usdc => "USDC",
            Currency::Eurc => "EURC",
            Currency::Usd => "USD",
            Currency::Eur => "EUR",
            Currency::GoldGram => "g gold",
        }
    }

    /// 10^scale, the number of minor units in one major unit.
    const fn units_per_major(self) -> i128 {
        // scale is at most 6, so this cannot overflow.
        10_i128.pow(self.scale() as u32)
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Why an operation on money could not be completed.
///
/// These are returned rather than panicked so that a ledger can refuse an
/// entry and say why, which is the behaviour GnuCash's visible `Imbalance`
/// account gets right and ERPNext's silent Round Off account gets wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    /// Two amounts in different currencies were combined.
    CurrencyMismatch { left: Currency, right: Currency },
    /// The result did not fit in `i128`.
    Overflow,
    /// A decimal string could not be read.
    Malformed,
    /// A decimal string carried more precision than the currency settles at.
    ///
    /// Refusing here is deliberate. Rounding 0.1234567 USDC to 0.123457 is
    /// how a residue account starts to fill up, and the caller is the only
    /// one who knows whether losing that unit is acceptable.
    TooPrecise { got: u32, max: u8 },
}

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoneyError::CurrencyMismatch { left, right } => {
                write!(f, "cannot combine {left} with {right}")
            }
            MoneyError::Overflow => f.write_str("amount does not fit in i128"),
            MoneyError::Malformed => f.write_str("not a decimal amount"),
            MoneyError::TooPrecise { got, max } => {
                write!(f, "{got} decimal places given, currency settles at {max}")
            }
        }
    }
}

impl std::error::Error for MoneyError {}

/// An exact amount of one currency, counted in minor units.
///
/// `Money` is deliberately not `Ord` across currencies and deliberately has
/// no conversion to or from a floating point number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Money {
    minor: i128,
    ccy: Currency,
}

impl Money {
    /// Build from a count of minor units: `Money::minor(20_000_000, Usdc)`
    /// is 20 USDC, because USDC settles at six decimals.
    pub const fn minor(minor: i128, ccy: Currency) -> Self {
        Money { minor, ccy }
    }

    /// Zero in the given currency.
    pub const fn zero(ccy: Currency) -> Self {
        Money { minor: 0, ccy }
    }

    /// The raw count of minor units. This is the only way out, and it is
    /// still an integer.
    pub const fn to_minor(self) -> i128 {
        self.minor
    }

    pub const fn currency(self) -> Currency {
        self.ccy
    }

    pub const fn is_zero(self) -> bool {
        self.minor == 0
    }

    pub const fn is_negative(self) -> bool {
        self.minor < 0
    }

    /// Parse a decimal string exactly: `"20.00"`, `"-1.234567"`, `"0"`.
    ///
    /// Fails rather than rounds when the string carries more precision than
    /// the currency settles at, so `"0.1234567"` is a `TooPrecise` error for
    /// USDC rather than a silent loss of one minor unit.
    pub fn parse(s: &str, ccy: Currency) -> Result<Self, MoneyError> {
        let s = s.trim();
        let (negative, digits) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s.strip_prefix('+').unwrap_or(s)),
        };
        if digits.is_empty() {
            return Err(MoneyError::Malformed);
        }

        let (whole, frac) = match digits.split_once('.') {
            Some((w, f)) => (w, f),
            None => (digits, ""),
        };
        if whole.is_empty() && frac.is_empty() {
            return Err(MoneyError::Malformed);
        }
        if !whole.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return Err(MoneyError::Malformed);
        }

        let scale = ccy.scale();
        if frac.len() > scale as usize {
            // Only an error if the extra places carry value. "20.000000000"
            // is still 20 to a six-decimal currency.
            if frac[scale as usize..].bytes().any(|b| b != b'0') {
                return Err(MoneyError::TooPrecise {
                    got: frac.len() as u32,
                    max: scale,
                });
            }
        }

        let whole_part: i128 = if whole.is_empty() {
            0
        } else {
            whole.parse().map_err(|_| MoneyError::Overflow)?
        };

        let mut frac_part: i128 = 0;
        for i in 0..scale as usize {
            let digit = frac.as_bytes().get(i).map_or(0, |b| (b - b'0') as i128);
            frac_part = frac_part
                .checked_mul(10)
                .and_then(|v| v.checked_add(digit))
                .ok_or(MoneyError::Overflow)?;
        }

        let minor = whole_part
            .checked_mul(ccy.units_per_major())
            .and_then(|v| v.checked_add(frac_part))
            .ok_or(MoneyError::Overflow)?;

        Ok(Money {
            minor: if negative { -minor } else { minor },
            ccy,
        })
    }

    /// Add, refusing a currency mismatch and refusing to wrap on overflow.
    pub fn checked_add(self, rhs: Money) -> Result<Money, MoneyError> {
        self.same_currency(rhs)?;
        let minor = self
            .minor
            .checked_add(rhs.minor)
            .ok_or(MoneyError::Overflow)?;
        Ok(Money {
            minor,
            ccy: self.ccy,
        })
    }

    /// Subtract, with the same guarantees as [`Money::checked_add`].
    pub fn checked_sub(self, rhs: Money) -> Result<Money, MoneyError> {
        self.same_currency(rhs)?;
        let minor = self
            .minor
            .checked_sub(rhs.minor)
            .ok_or(MoneyError::Overflow)?;
        Ok(Money {
            minor,
            ccy: self.ccy,
        })
    }

    /// Negate. Always exact, because negation of an integer loses nothing.
    pub fn negate(self) -> Result<Money, MoneyError> {
        let minor = self.minor.checked_neg().ok_or(MoneyError::Overflow)?;
        Ok(Money {
            minor,
            ccy: self.ccy,
        })
    }

    pub fn abs(self) -> Result<Money, MoneyError> {
        if self.minor < 0 {
            self.negate()
        } else {
            Ok(self)
        }
    }

    fn same_currency(self, rhs: Money) -> Result<(), MoneyError> {
        if self.ccy == rhs.ccy {
            Ok(())
        } else {
            Err(MoneyError::CurrencyMismatch {
                left: self.ccy,
                right: rhs.ccy,
            })
        }
    }

    /// Sum a sequence exactly, refusing mixed currencies.
    ///
    /// Returns zero in `ccy` for an empty sequence, so the caller always
    /// states the currency rather than having it inferred from the first
    /// element of a list that might be empty.
    pub fn sum<I: IntoIterator<Item = Money>>(
        items: I,
        ccy: Currency,
    ) -> Result<Money, MoneyError> {
        let mut total = Money::zero(ccy);
        for item in items {
            total = total.checked_add(item)?;
        }
        Ok(total)
    }
}

impl fmt::Display for Money {
    /// Renders at exactly the currency's scale: `20.000000 USDC`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scale = self.ccy.scale() as usize;
        let per_major = self.ccy.units_per_major();
        let negative = self.minor < 0;
        // Work in unsigned space so i128::MIN does not overflow on negation.
        let magnitude = self.minor.unsigned_abs();
        let whole = magnitude / per_major as u128;
        let frac = magnitude % per_major as u128;
        if negative {
            f.write_str("-")?;
        }
        if scale == 0 {
            write!(f, "{whole} {}", self.ccy)
        } else {
            write!(f, "{whole}.{frac:0>scale$} {}", self.ccy)
        }
    }
}
