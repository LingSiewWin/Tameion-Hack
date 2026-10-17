//! A ledger core where the money-representation defects found across the
//! open-source accounting stack are not expressible.
//!
//! See [`money`] for the reasoning. The short version: amounts are integer
//! counts of minor units carrying their own currency, there is no path to or
//! from a float, and anything that can fail returns `Result`.
//!
//! # The defects, as tests the compiler runs
//!
//! Invoice Ninja "marked a EUR 0.10 subscription paid with no payment, from
//! one integer cast". That cast has no syntax here:
//!
//! ```compile_fail,E0605
//! use ledger::{Currency, Money};
//! let dime = Money::parse("0.10", Currency::Eur).unwrap();
//! let lost = dime as i64;   // error[E0605]: non-primitive cast
//! ```
//!
//! ERPNext\'s ORM "casts every decimal column to float on read". There is no
//! conversion to a float to call:
//!
//! ```compile_fail,E0277
//! use ledger::{Currency, Money};
//! let dime = Money::parse("0.10", Currency::Eur).unwrap();
//! let drifted: f64 = dime.into();   // error[E0277]: f64: From<Money> unsatisfied
//! ```
//!
//! Ghostfolio stores quantity, unit price and fee as floats. A float cannot
//! become `Money` either, so the representation cannot be chosen by accident:
//!
//! ```compile_fail,E0277
//! use ledger::{Currency, Money};
//! let from_float: Money = 0.1_f64.try_into().unwrap();
//! ```
//!
//! # The complete reversal, as a test the compiler runs
//!
//! "A complete reversal swaps the debit and the credit." Debits still equal
//! credits afterwards, so no arithmetic check can see it. Here the two sides
//! are distinct types, so the swap does not typecheck:
//!
//! ```compile_fail,E0308
//! use ledger::{AccountId, Credit, Currency, Debit, DocumentKind, DocumentRef, Entry, Money};
//! let amount = Money::parse("20.000000", Currency::Usdc).unwrap();
//! let d = Debit::new(AccountId::new("expense:saas"), amount).unwrap();
//! let c = Credit::new(AccountId::new("asset:usdc"), amount).unwrap();
//! let doc = DocumentRef::new(DocumentKind::Settlement, "0xdeadbeef");
//!
//! // Arguments swapped. error[E0308]: mismatched types.
//! let reversed = Entry::post(doc, vec![c], vec![d]).unwrap();
//! ```
//!
//! # An unbalanced entry is not a value that exists
//!
//! `Entry`'s fields are private and `Entry::post` is the only constructor, so
//! there is no literal to write:
//!
//! ```compile_fail,E0451
//! use ledger::Entry;
//! let e = Entry { document: todo!(), debits: vec![], credits: vec![], total: todo!(), ccy: todo!() };
//! ```
//!
//! What does compile is the exact amount:
//!
//! ```
//! use ledger::{Currency, Money};
//! let dime = Money::parse("0.10", Currency::Eur).unwrap();
//! assert_eq!(dime.to_minor(), 10);
//! assert_eq!(dime.to_string(), "0.10 EUR");
//! ```

pub mod account;
pub mod action;
pub mod balance;
pub mod canon;
mod codec;
pub mod entry;
pub mod log;
pub mod money;
pub mod obligations;
pub mod reason;
pub mod symbolon;
pub mod time;
pub mod verify;

pub use account::{Account, AccountId, AccountKind, Side};
pub use action::{
    Action, Actor, ActorKind, Authority, Delta, Evidence, EvidenceKind, ExpectedEffect,
    Idempotency, Intent, Precondition, Principal, Provenance, Target, Validity, Verb,
};
pub use balance::{
    AssertionFailed, BalanceAssertion, BalanceError, Chart, Journal, ToleranceBreach,
};
pub use canon::{ActionHash, action_hash, canonicalize, settlement_key, settlement_key_for};
pub use entry::{Credit, Debit, DocumentKind, DocumentRef, Entry, PostError};
pub use log::{
    DecisionLog, Event, EventBody, Hash32, ProofStep, ReservationAction, verify_inclusion,
};
pub use money::{Currency, Money, MoneyError};
pub use obligations::{
    Confidence, Counterparty, DetectionConfig, Direction, Obligation, ObligationKind, Transfer,
    detect, is_dust, total_due, upcoming,
};
pub use reason::{Prioritised, ordering_divergence, rank_by_urgency};
pub use time::{Seconds, Timestamp};
pub use verify::{
    Check, Decision, DocumentRecord, Outcome, PayeeStatus, Verdict, World, looks_alike, verify,
};
