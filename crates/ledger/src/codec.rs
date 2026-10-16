//! Canonical byte encoding.
//!
//! Shared by the action hash and the decision log, because both commit to
//! structured data that must produce one byte string and only one.
//!
//! Canonical means there is no freedom in the encoding. Every field is
//! written in a fixed order, every variable-length value is length-prefixed
//! before its bytes, and every enum is written as a fixed one-byte tag.
//!
//! Length prefixes carry more weight than they look. Without them `("ab",
//! "c")` and `("a", "bc")` encode identically, so an attacker controlling two
//! adjacent strings could move a field boundary without moving the hash.

use crate::action::{
    Actor, ActorKind, Authority, Delta, Evidence, EvidenceKind, ExpectedEffect, Idempotency,
    Intent, Precondition, Principal, Provenance, Target, Validity, Verb,
};
use crate::money::{Currency, Money};

/// Accumulates the canonical byte string.
pub(crate) struct Encoder {
    pub(crate) buf: Vec<u8>,
}

impl Encoder {
    pub(crate) fn new() -> Self {
        Encoder {
            buf: Vec::with_capacity(512),
        }
    }

    /// A single-byte discriminant. Every enum writes exactly one of these,
    /// so adding a variant later cannot silently collide with an old one.
    pub(crate) fn tag(&mut self, t: u8) -> &mut Self {
        self.buf.push(t);
        self
    }

    pub(crate) fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub(crate) fn i128(&mut self, v: i128) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Length-prefixed bytes. The prefix is what makes concatenation
    /// unambiguous.
    pub(crate) fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.u64(b.len() as u64);
        self.buf.extend_from_slice(b);
        self
    }

    pub(crate) fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }

    /// Length-prefixed sequence. The count is written before the elements so
    /// a truncated list cannot be confused with a complete shorter one.
    pub(crate) fn seq<T>(&mut self, items: &[T], mut f: impl FnMut(&mut Self, &T)) -> &mut Self {
        self.u64(items.len() as u64);
        for item in items {
            f(self, item);
        }
        self
    }

    pub(crate) fn opt_str(&mut self, s: Option<&str>) -> &mut Self {
        match s {
            None => self.tag(0),
            Some(v) => self.tag(1).str(v),
        }
    }

    pub(crate) fn currency(&mut self, c: Currency) -> &mut Self {
        self.tag(match c {
            Currency::Usdc => 1,
            Currency::Eurc => 2,
            Currency::Usd => 3,
            Currency::Eur => 4,
            Currency::GoldGram => 5,
        })
    }

    /// Money is written as its minor units plus its currency tag. The
    /// decimal string is never used, so "20.0" and "20.000000" cannot
    /// produce different hashes for the same amount.
    pub(crate) fn money(&mut self, m: Money) -> &mut Self {
        self.i128(m.to_minor()).currency(m.currency())
    }

    pub(crate) fn provenance(&mut self, p: Provenance) -> &mut Self {
        self.tag(match p {
            Provenance::Inferred => 1,
            Provenance::CounterpartyAsserted => 2,
            Provenance::HumanConfirmed => 3,
            Provenance::Onchain => 4,
        })
    }

    pub(crate) fn actor(&mut self, a: &Actor) -> &mut Self {
        self.str(&a.id).tag(match a.kind {
            ActorKind::Model => 1,
            ActorKind::Rule => 2,
            ActorKind::Human => 3,
        })
    }

    pub(crate) fn principal(&mut self, p: &Principal) -> &mut Self {
        self.str(&p.id).str(p.treasury.as_str())
    }

    pub(crate) fn intent(&mut self, i: &Intent) -> &mut Self {
        self.tag(match i.verb {
            Verb::Settle => 1,
            Verb::Transfer => 2,
            Verb::Purchase => 3,
            Verb::Refund => 4,
        })
        .str(&i.rationale)
    }

    pub(crate) fn target(&mut self, t: &Target) -> &mut Self {
        self.str(&t.id).opt_str(t.label.as_deref())
    }

    pub(crate) fn evidence(&mut self, e: &Evidence) -> &mut Self {
        self.tag(match e.kind {
            EvidenceKind::OnchainTransfer => 1,
            EvidenceKind::Invoice => 2,
            EvidenceKind::Receipt => 3,
            EvidenceKind::PurchaseOrder => 4,
            EvidenceKind::HumanConfirmation => 5,
            EvidenceKind::InferredPattern => 6,
        })
        .str(&e.reference)
        .provenance(e.provenance)
    }

    pub(crate) fn precondition(&mut self, p: &Precondition) -> &mut Self {
        match p {
            Precondition::BalanceAtLeast(m) => self.tag(1).money(*m),
            Precondition::ReserveFloor(m) => self.tag(2).money(*m),
            Precondition::NotAlreadySettled => self.tag(3),
            Precondition::CounterpartyKnown => self.tag(4),
            Precondition::PayeeUnchanged => self.tag(5),
            Precondition::EvidenceAtLeast(n) => self.tag(6).u64(*n as u64),
            Precondition::ProvenanceAtLeast(p) => {
                self.tag(7);
                self.provenance(*p)
            }
            Precondition::ThreeWayMatch => self.tag(8),
            Precondition::ConfidenceAtLeast(bp) => self.tag(9).u64(u64::from(*bp)),
        }
    }

    pub(crate) fn authority(&mut self, a: &Authority) -> &mut Self {
        self.money(a.per_action_cap)
            .money(a.daily_cap)
            .money(a.spent_today)
    }

    pub(crate) fn effect(&mut self, e: &ExpectedEffect) -> &mut Self {
        self.str(e.account.as_str());
        match &e.delta {
            Delta::Decrease(m) => self.tag(1).money(*m),
            Delta::Increase(m) => self.tag(2).money(*m),
        }
    }

    pub(crate) fn idempotency(&mut self, i: &Idempotency) -> &mut Self {
        self.str(&i.key).str(&i.scope)
    }

    pub(crate) fn validity(&mut self, v: &Validity) -> &mut Self {
        self.u64(v.not_before).u64(v.expires_at)
    }
}
