//! Entries that cannot exist unbalanced, and cannot exist without a document.
//!
//! Two decisions from the Canteen survey are encoded here.
//!
//! First, *the document is the entry*. Odoo has no invoice separate from its
//! journal entry, and ERPNext forbids creating a GL entry directly -- entries
//! appear only as a side effect of submitting a voucher. Both can therefore
//! ask "which document is this entry for?", and the three-way match and
//! vendor change-control checks have somewhere to attach. Systems where the
//! ledger is a running balance beside the invoice cannot ask it at all. So
//! [`Entry::post`] requires a [`DocumentRef`]; there is no way to post
//! without pointing at something.
//!
//! Second, *sign lives on the side, not on the amount*. A leg carries a
//! positive amount and is either a [`Debit`] or a [`Credit`]. They are
//! distinct types, so the complete reversal -- swapping the debit and the
//! credit, which leaves debits equal to credits and passes every arithmetic
//! check -- cannot be written by getting an argument order wrong.

use crate::account::AccountId;
use crate::money::{Currency, Money, MoneyError};
use core::fmt;

/// What kind of thing an entry is evidence of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocumentKind {
    /// A recurring obligation inferred from payment history.
    Obligation,
    /// A payment the agent executed on chain.
    Settlement,
    /// A bill received from a vendor.
    Invoice,
    /// Confirmation that goods or services arrived.
    Receipt,
    /// A correction, which is always a new entry and never an edit.
    Adjustment,
    /// A perpetual-futures hedge opened, adjusted or closed against a spot
    /// holding. The id is the venue order id, or the read-back position key
    /// when the venue reports a fill the agent did not place.
    Hedge,
    /// A funding payment received or paid on an open hedge.
    Funding,
    /// A mark-to-market of a holding or a hedge at a stated price and time.
    /// Always a new entry; the previous mark is never edited.
    Mark,
    /// A priced contract for goods: a purchase from a supplier or a sale to
    /// a buyer. The id is the contract's id on the desk.
    Contract,
    /// A logistics document that moves goods between stages: a bill of
    /// lading, an arrival notice. The id is the document's own number.
    Shipping,
}

impl fmt::Display for DocumentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DocumentKind::Obligation => "obligation",
            DocumentKind::Settlement => "settlement",
            DocumentKind::Invoice => "invoice",
            DocumentKind::Receipt => "receipt",
            DocumentKind::Adjustment => "adjustment",
            DocumentKind::Hedge => "hedge",
            DocumentKind::Funding => "funding",
            DocumentKind::Mark => "mark",
            DocumentKind::Contract => "contract",
            DocumentKind::Shipping => "shipping",
        })
    }
}

/// The document an entry books. Required, never optional.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentRef {
    kind: DocumentKind,
    id: String,
}

impl DocumentRef {
    pub fn new(kind: DocumentKind, id: impl Into<String>) -> Self {
        DocumentRef {
            kind,
            id: id.into(),
        }
    }

    pub const fn kind(&self) -> DocumentKind {
        self.kind
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

impl fmt::Display for DocumentRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, self.id)
    }
}

/// One side of one leg. Amount is always positive; direction is the type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Debit {
    account: AccountId,
    amount: Money,
}

/// One side of one leg. Amount is always positive; direction is the type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    account: AccountId,
    amount: Money,
}

macro_rules! impl_leg {
    ($name:ident, $label:literal) => {
        impl $name {
            /// Build a leg. Rejects zero and negative amounts: a reduction is
            /// expressed by posting to the other side, not by negating.
            pub fn new(account: AccountId, amount: Money) -> Result<Self, PostError> {
                if amount.is_negative() {
                    return Err(PostError::NegativeLeg { account, amount });
                }
                if amount.is_zero() {
                    return Err(PostError::ZeroLeg { account });
                }
                Ok($name { account, amount })
            }

            pub fn account(&self) -> &AccountId {
                &self.account
            }

            pub const fn amount(&self) -> Money {
                self.amount
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{} {} {}", $label, self.account, self.amount)
            }
        }
    };
}

impl_leg!(Debit, "debit");
impl_leg!(Credit, "credit");

/// Why an entry could not be posted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostError {
    /// Debits did not equal credits. Carries both totals and the gap, so the
    /// caller can report the difference rather than absorb it.
    ///
    /// ERPNext books a gap of up to half a unit of currency to a Round Off
    /// account and lets the entry through. This refuses instead. With
    /// six-decimal settlement posted into a two-decimal world, that
    /// allowance is how a residue account fills with a thousand entries
    /// whose only job is to make the trial balance close.
    Unbalanced {
        debits: Money,
        credits: Money,
        difference: Money,
    },
    /// An entry with no legs on one side or the other.
    Empty { side: &'static str },
    /// A leg carried a negative amount.
    NegativeLeg { account: AccountId, amount: Money },
    /// A leg carried zero.
    ZeroLeg { account: AccountId },
    /// Legs of more than one currency in a single entry.
    MixedCurrency { expected: Currency, found: Currency },
    /// Arithmetic failed while totalling.
    Arithmetic(MoneyError),
}

impl From<MoneyError> for PostError {
    fn from(e: MoneyError) -> Self {
        PostError::Arithmetic(e)
    }
}

impl fmt::Display for PostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PostError::Unbalanced {
                debits,
                credits,
                difference,
            } => write!(
                f,
                "unbalanced: debits {debits}, credits {credits}, difference {difference}"
            ),
            PostError::Empty { side } => write!(f, "entry has no {side} legs"),
            PostError::NegativeLeg { account, amount } => {
                write!(
                    f,
                    "negative leg on {account}: {amount}; post to the other side instead"
                )
            }
            PostError::ZeroLeg { account } => write!(f, "zero leg on {account}"),
            PostError::MixedCurrency { expected, found } => {
                write!(f, "entry is in {expected} but a leg is in {found}")
            }
            PostError::Arithmetic(e) => write!(f, "arithmetic: {e}"),
        }
    }
}

impl std::error::Error for PostError {}

/// A balanced entry, pointing at the document it books.
///
/// Every field is private and the only constructor is [`Entry::post`], which
/// validates before it builds. An `Entry` value therefore *is* the proof that
/// its debits equal its credits -- there is no path that produces an
/// unbalanced one, so nothing downstream has to re-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    document: DocumentRef,
    debits: Vec<Debit>,
    credits: Vec<Credit>,
    total: Money,
    ccy: Currency,
}

impl Entry {
    /// Validate and post. The only way to build an [`Entry`].
    pub fn post(
        document: DocumentRef,
        debits: Vec<Debit>,
        credits: Vec<Credit>,
    ) -> Result<Entry, PostError> {
        if debits.is_empty() {
            return Err(PostError::Empty { side: "debit" });
        }
        if credits.is_empty() {
            return Err(PostError::Empty { side: "credit" });
        }

        // The first leg fixes the currency for the whole entry.
        let ccy = debits[0].amount().currency();
        for d in &debits {
            if d.amount().currency() != ccy {
                return Err(PostError::MixedCurrency {
                    expected: ccy,
                    found: d.amount().currency(),
                });
            }
        }
        for c in &credits {
            if c.amount().currency() != ccy {
                return Err(PostError::MixedCurrency {
                    expected: ccy,
                    found: c.amount().currency(),
                });
            }
        }

        let debit_total = Money::sum(debits.iter().map(|d| d.amount()), ccy)?;
        let credit_total = Money::sum(credits.iter().map(|c| c.amount()), ccy)?;

        if debit_total != credit_total {
            let difference = debit_total.checked_sub(credit_total)?;
            return Err(PostError::Unbalanced {
                debits: debit_total,
                credits: credit_total,
                difference,
            });
        }

        Ok(Entry {
            document,
            debits,
            credits,
            total: debit_total,
            ccy,
        })
    }

    pub fn document(&self) -> &DocumentRef {
        &self.document
    }

    pub fn debits(&self) -> &[Debit] {
        &self.debits
    }

    pub fn credits(&self) -> &[Credit] {
        &self.credits
    }

    /// The entry's total, which is both the debit total and the credit total
    /// because the two are equal by construction.
    pub const fn total(&self) -> Money {
        self.total
    }

    pub const fn currency(&self) -> Currency {
        self.ccy
    }
}

impl fmt::Display for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} ({})", self.document, self.total)?;
        for d in &self.debits {
            writeln!(f, "  {d}")?;
        }
        for c in &self.credits {
            writeln!(f, "  {c}")?;
        }
        Ok(())
    }
}
