//! Accounts, and the side each kind increases on.
//!
//! The kind is carried on the account rather than inferred at posting time
//! because an error of principle -- the right amount in the wrong *kind* of
//! account, an asset booked as an expense -- is invisible to the debits-equal-
//! credits check. Naming the kind is what lets anything downstream notice.

use crate::money::Currency;
use core::fmt;

/// Which side of the ledger an account's balance naturally sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Debit,
    Credit,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Side::Debit => "debit",
            Side::Credit => "credit",
        })
    }
}

/// The five account kinds of double-entry bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountKind {
    Asset,
    Liability,
    Equity,
    Revenue,
    Expense,
}

impl AccountKind {
    /// The side this kind increases on.
    ///
    /// Assets and expenses grow with debits; liabilities, equity and revenue
    /// grow with credits. This is the rule that turns a pile of postings into
    /// a balance with a sign that means something.
    pub const fn normal_side(self) -> Side {
        match self {
            AccountKind::Asset | AccountKind::Expense => Side::Debit,
            AccountKind::Liability | AccountKind::Equity | AccountKind::Revenue => Side::Credit,
        }
    }
}

impl fmt::Display for AccountKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AccountKind::Asset => "asset",
            AccountKind::Liability => "liability",
            AccountKind::Equity => "equity",
            AccountKind::Revenue => "revenue",
            AccountKind::Expense => "expense",
        })
    }
}

/// A stable identifier for an account.
///
/// Wrapped rather than a bare `String` so that an account id cannot be passed
/// where a document id or a counterparty address was meant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(String);

impl AccountId {
    pub fn new(id: impl Into<String>) -> Self {
        AccountId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An account in the chart: what it is called, what kind it is, and the one
/// currency it is denominated in.
///
/// Pinning the currency to the account means a posting into it can be checked
/// against the account rather than only against the other legs of its entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    id: AccountId,
    kind: AccountKind,
    ccy: Currency,
}

impl Account {
    pub fn new(id: AccountId, kind: AccountKind, ccy: Currency) -> Self {
        Account { id, kind, ccy }
    }

    pub fn id(&self) -> &AccountId {
        &self.id
    }

    pub const fn kind(&self) -> AccountKind {
        self.kind
    }

    pub const fn currency(&self) -> Currency {
        self.ccy
    }

    pub const fn normal_side(&self) -> Side {
        self.kind.normal_side()
    }
}
