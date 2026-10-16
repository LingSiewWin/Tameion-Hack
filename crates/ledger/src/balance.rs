//! Balances, and assertions about them whose tolerance is written down.
//!
//! Two systems in the Canteen survey handle a disagreement between what the
//! ledger says and what the world says, and they handle it in opposite ways.
//!
//! beancount makes tolerance a declared property of the claim: a `Balance`
//! directive carries the amount of tolerance to use in the verification, and
//! a `diff_amount` that is `None` only when the check succeeds. A ledger
//! written to the cent tolerates half a cent because the *author* wrote it
//! that way.
//!
//! ERPNext makes it a hidden property of the column: a voucher may be out by
//! half a unit of currency, and the difference is posted to the company's
//! Round Off account -- a normal expense account with a normal balance, where
//! nobody looks. GnuCash at least names its version `Imbalance-CUR` and puts
//! it in the account tree where a person sees it on the next screen.
//!
//! This module takes the beancount position. [`BalanceAssertion::exact`] is
//! the default and tolerates nothing. Any slack has to be typed out by the
//! caller, in the currency, at the call site, where a reviewer reads it.

use crate::account::{Account, AccountId, Side};
use crate::entry::Entry;
use crate::money::{Currency, Money, MoneyError};
use core::fmt;
use std::collections::BTreeMap;

/// The chart of accounts: which accounts exist, and what kind each one is.
#[derive(Debug, Clone, Default)]
pub struct Chart {
    accounts: BTreeMap<AccountId, Account>,
}

impl Chart {
    pub fn new() -> Self {
        Chart {
            accounts: BTreeMap::new(),
        }
    }

    pub fn with(mut self, account: Account) -> Self {
        self.accounts.insert(account.id().clone(), account);
        self
    }

    pub fn get(&self, id: &AccountId) -> Option<&Account> {
        self.accounts.get(id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &AccountId> {
        self.accounts.keys()
    }
}

/// Why a balance could not be computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BalanceError {
    /// An entry referenced an account that is not in the chart.
    ///
    /// Refused rather than auto-created. The OCA invoice importer has a
    /// setting that creates a vendor bank account from an IBAN read off a
    /// PDF; creating financial records as a side effect of reading an
    /// untrusted document is the shape of mistake worth refusing outright.
    UnknownAccount(AccountId),
    /// A leg's currency did not match the account's declared currency.
    WrongCurrency {
        account: AccountId,
        declared: Currency,
        posted: Currency,
    },
    Arithmetic(MoneyError),
}

impl From<MoneyError> for BalanceError {
    fn from(e: MoneyError) -> Self {
        BalanceError::Arithmetic(e)
    }
}

impl fmt::Display for BalanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BalanceError::UnknownAccount(id) => write!(f, "account {id} is not in the chart"),
            BalanceError::WrongCurrency {
                account,
                declared,
                posted,
            } => {
                write!(
                    f,
                    "account {account} is in {declared} but a leg posted {posted}"
                )
            }
            BalanceError::Arithmetic(e) => write!(f, "arithmetic: {e}"),
        }
    }
}

impl std::error::Error for BalanceError {}

/// An append-only sequence of posted entries.
///
/// Kill Bill refuses to change a committed invoice and records every
/// correction as a new adjustment item of a named kind. That is the right
/// shape for an agent's ledger too: there is no `remove`, and a mistake is
/// fixed by posting its reversal, which leaves both in the record.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    entries: Vec<Entry>,
}

impl Journal {
    pub fn new() -> Self {
        Journal {
            entries: Vec::new(),
        }
    }

    /// Append an entry. There is no corresponding removal.
    pub fn append(&mut self, entry: Entry) {
        self.entries.push(entry);
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The balance of one account, signed so that a positive number means the
    /// account is on its normal side.
    ///
    /// Assets and expenses grow with debits, so their balance is debits minus
    /// credits. Liabilities, equity and revenue grow with credits, so theirs
    /// is the other way round. Without this the sign of a balance would be an
    /// accident of which column happened to be bigger.
    pub fn balance(&self, chart: &Chart, id: &AccountId) -> Result<Money, BalanceError> {
        let account = chart
            .get(id)
            .ok_or_else(|| BalanceError::UnknownAccount(id.clone()))?;
        let ccy = account.currency();

        let mut debits = Money::zero(ccy);
        let mut credits = Money::zero(ccy);

        for entry in &self.entries {
            for d in entry.debits() {
                if d.account() == id {
                    Self::check_currency(account, d.amount())?;
                    debits = debits.checked_add(d.amount())?;
                }
            }
            for c in entry.credits() {
                if c.account() == id {
                    Self::check_currency(account, c.amount())?;
                    credits = credits.checked_add(c.amount())?;
                }
            }
        }

        let balance = match account.normal_side() {
            Side::Debit => debits.checked_sub(credits)?,
            Side::Credit => credits.checked_sub(debits)?,
        };
        Ok(balance)
    }

    fn check_currency(account: &Account, amount: Money) -> Result<(), BalanceError> {
        if amount.currency() != account.currency() {
            return Err(BalanceError::WrongCurrency {
                account: account.id().clone(),
                declared: account.currency(),
                posted: amount.currency(),
            });
        }
        Ok(())
    }

    /// Total debits and total credits across the whole journal.
    ///
    /// These are equal whenever every entry balanced, which [`Entry`]
    /// guarantees. It is computed anyway, because a trial balance that is
    /// only ever asserted in a comment is not a trial balance.
    pub fn trial_balance(&self, ccy: Currency) -> Result<(Money, Money), BalanceError> {
        let mut debits = Money::zero(ccy);
        let mut credits = Money::zero(ccy);
        for entry in &self.entries {
            for d in entry.debits() {
                debits = debits.checked_add(d.amount())?;
            }
            for c in entry.credits() {
                credits = credits.checked_add(c.amount())?;
            }
        }
        Ok((debits, credits))
    }
}

/// A claim that an account holds a particular amount, with the slack the
/// author is willing to accept written into the claim itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceAssertion {
    account: AccountId,
    expected: Money,
    tolerance: Money,
}

impl BalanceAssertion {
    /// Assert an exact balance. Tolerates nothing.
    ///
    /// This is the constructor to reach for. Slack should be an unusual,
    /// visible decision rather than a default someone inherits.
    pub fn exact(account: AccountId, expected: Money) -> Self {
        BalanceAssertion {
            tolerance: Money::zero(expected.currency()),
            account,
            expected,
        }
    }

    /// Assert a balance within a stated tolerance.
    ///
    /// The tolerance is a `Money` in the same currency, so it appears in the
    /// source as an amount a reviewer can read -- `usdc("0.000001")`, not a
    /// bare `0.5` whose units are whatever the column happens to be.
    pub fn within(
        account: AccountId,
        expected: Money,
        tolerance: Money,
    ) -> Result<Self, MoneyError> {
        let tolerance = tolerance.abs()?;
        // Force the currency check now rather than at verification time.
        expected.checked_sub(tolerance)?;
        Ok(BalanceAssertion {
            account,
            expected,
            tolerance,
        })
    }

    pub fn account(&self) -> &AccountId {
        &self.account
    }

    pub const fn expected(&self) -> Money {
        self.expected
    }

    pub const fn tolerance(&self) -> Money {
        self.tolerance
    }

    /// Check the assertion against a journal.
    ///
    /// On failure the error carries the difference, mirroring beancount's
    /// `diff_amount`, so the caller reports the gap instead of absorbing it.
    pub fn verify(&self, journal: &Journal, chart: &Chart) -> Result<Money, AssertionFailed> {
        let actual = journal
            .balance(chart, &self.account)
            .map_err(AssertionFailed::Balance)?;

        let difference = actual
            .checked_sub(self.expected)
            .map_err(|e| AssertionFailed::Balance(BalanceError::Arithmetic(e)))?;

        let magnitude = difference
            .abs()
            .map_err(|e| AssertionFailed::Balance(BalanceError::Arithmetic(e)))?;

        if magnitude.to_minor() > self.tolerance.to_minor() {
            return Err(AssertionFailed::OutOfTolerance(Box::new(ToleranceBreach {
                account: self.account.clone(),
                expected: self.expected,
                actual,
                difference,
                tolerance: self.tolerance,
            })));
        }
        Ok(actual)
    }
}

/// The detail of a balance assertion that did not hold.
///
/// `difference` mirrors beancount's `diff_amount`: the gap is reported, never
/// absorbed. Boxed inside [`AssertionFailed`] so that the success path does
/// not carry the weight of the failure path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToleranceBreach {
    pub account: AccountId,
    pub expected: Money,
    pub actual: Money,
    pub difference: Money,
    pub tolerance: Money,
}

/// A balance assertion that did not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertionFailed {
    OutOfTolerance(Box<ToleranceBreach>),
    Balance(BalanceError),
}

impl fmt::Display for AssertionFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssertionFailed::OutOfTolerance(b) => write!(
                f,
                "{}: expected {}, found {}, off by {} (tolerance {})",
                b.account, b.expected, b.actual, b.difference, b.tolerance
            ),
            AssertionFailed::Balance(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AssertionFailed {}
