//! Canonical encoding and the action hash.
//!
//! An action is verified at one moment and executed at another. Between
//! those two moments the action must not change, or the verification was of
//! something else. The defence is a hash taken over a *canonical* encoding:
//! one action has exactly one byte string, so any edit -- a different
//! recipient, one more minor unit, a weaker precondition quietly dropped --
//! produces a different hash and the executor refuses.
//!
//! Canonical means there is no freedom in the encoding. Every field is
//! written in a fixed order, every variable-length value is length-prefixed
//! before its bytes, and every enum is written as a fixed tag. Length
//! prefixes matter more than they look: without them `("ab", "c")` and
//! `("a", "bc")` encode identically, and an attacker who controls two
//! adjacent strings can move a boundary without moving the hash.
//!
//! Keccak-256 is used rather than SHA-256 so the digest can be handed
//! straight to an EVM contract, which is where the root of the decision log
//! is anchored.

use crate::action::Action;
use crate::codec::Encoder;
use sha3::{Digest, Keccak256};

/// A 32-byte Keccak-256 digest over an action's canonical encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActionHash([u8; 32]);

impl ActionHash {
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Rebuild a hash from 32 bytes that came back from somewhere else --
    /// a chain call, a stored log, a receipt.
    ///
    /// Deliberately not `From<[u8; 32]>`: an implicit conversion would let
    /// any thirty-two bytes become an `ActionHash` wherever one is expected,
    /// and the type exists to make "this digest and that digest are the same
    /// kind of thing" a claim someone had to write down.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        ActionHash(bytes)
    }

    /// Hex with the `0x` prefix, ready for an EVM call or a log line.
    pub fn to_hex(self) -> String {
        let mut s = String::with_capacity(66);
        s.push_str("0x");
        for b in self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }
}

impl core::fmt::Display for ActionHash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// The canonical byte encoding of an action.
///
/// Exposed so a test can show two actions differing in one field producing
/// different bytes, and so an auditor can recompute the hash by hand.
pub fn canonicalize(action: &Action) -> Vec<u8> {
    let mut e = Encoder::new();
    // Domain separator. Prevents a digest over some other structure from
    // ever being mistaken for an action hash.
    e.str("air/v0.1");
    e.str(&action.id);
    e.actor(&action.actor);
    e.principal(&action.principal);
    e.intent(&action.intent);
    e.target(&action.target);
    e.money(action.amount);
    e.seq(&action.evidence, |enc, ev| {
        enc.evidence(ev);
    });
    e.seq(&action.preconditions, |enc, p| {
        enc.precondition(p);
    });
    e.authority(&action.authority);
    e.seq(&action.expected_effects, |enc, ef| {
        enc.effect(ef);
    });
    e.idempotency(&action.idempotency);
    e.validity(&action.validity);
    e.buf
}

/// The stable identity of *what this action settles*, independent of how it
/// was explained.
///
/// [`action_hash`] covers the whole action, prose included, which is right for
/// provenance: the log should be able to prove exactly what was proposed and
/// why. It is wrong for a replay key. The rationale is free text written by
/// whoever reasoned, and a language model writes a different sentence every
/// time it is asked; hashing that into the idempotency key means the same
/// economic action -- pay this counterparty for this billing cycle -- produces
/// a different key on every run, and an `executed[key]` guard never matches.
///
/// That is not hypothetical. With a model-backed reasoner it caused the same
/// two obligations to be paid twice, in separate transactions, both accepted
/// on chain. Deterministic reasoners hid the bug because their prose does not
/// vary.
///
/// So the replay key is derived from [`Idempotency`] alone -- the scope and
/// the cycle key -- which is the same pair
/// [`crate::verify`] already uses for `NotAlreadySettled`. Two runs that
/// disagree about wording still agree about which bill they are paying.
///
/// The domain separator differs from the action one so a settlement key can
/// never collide with an action hash.
pub fn settlement_key(action: &Action) -> ActionHash {
    settlement_key_for(&action.idempotency)
}

/// The settlement key of a billing cycle, without building an action for it.
///
/// The agent asks the chain whether a cycle is already settled before it has
/// any reason to propose paying it -- that is how it steps past the cycles it
/// paid on earlier passes. Same bytes as [`settlement_key`], by construction.
pub fn settlement_key_for(idempotency: &crate::action::Idempotency) -> ActionHash {
    let mut e = Encoder::new();
    e.str("air/v0.1/settlement");
    e.idempotency(idempotency);
    let mut hasher = Keccak256::new();
    hasher.update(e.buf);
    let out = hasher.finalize();
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&out);
    ActionHash(bytes)
}

/// Keccak-256 over the canonical encoding.
pub fn action_hash(action: &Action) -> ActionHash {
    let mut hasher = Keccak256::new();
    hasher.update(canonicalize(action));
    let out = hasher.finalize();
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&out);
    ActionHash(bytes)
}

#[cfg(test)]
mod settlement_key_tests {
    use super::*;
    use crate::action::*;
    use crate::money::{Currency, Money};

    fn action_with(rationale: &str, cycle: &str) -> Action {
        Action {
            id: "act_1".into(),
            actor: Actor {
                id: "claude-opus-5".into(),
                kind: ActorKind::Model,
            },
            principal: Principal {
                id: "org_tameion".into(),
                treasury: crate::AccountId::new("asset:usdc"),
            },
            intent: Intent {
                verb: Verb::Settle,
                rationale: rationale.into(),
            },
            target: Target {
                id: "0x1111".into(),
                label: None,
            },
            amount: Money::minor(20_000_000, Currency::Usdc),
            evidence: vec![],
            authority: Authority {
                per_action_cap: Money::minor(100_000_000, Currency::Usdc),
                daily_cap: Money::minor(500_000_000, Currency::Usdc),
                spent_today: Money::zero(Currency::Usdc),
            },
            preconditions: vec![],
            expected_effects: vec![],
            idempotency: Idempotency {
                key: cycle.into(),
                scope: "org_tameion".into(),
            },
            validity: Validity {
                not_before: 0,
                expires_at: 3600,
            },
        }
    }

    /// The bug this exists to prevent: a model writes a different sentence
    /// each run, the action hash moves with it, and the on-chain replay guard
    /// stops recognising a bill it has already paid. Observed in a live run --
    /// two counterparties were paid twice.
    #[test]
    fn wording_changes_the_action_hash_but_not_the_settlement_key() {
        let a = action_with("vendor cuts service on day one", "0x1111@1786544000");
        let b = action_with(
            "largest claim, most likely to be crowded out",
            "0x1111@1786544000",
        );

        assert_ne!(
            action_hash(&a),
            action_hash(&b),
            "provenance must track the prose"
        );
        assert_eq!(
            settlement_key(&a),
            settlement_key(&b),
            "the replay key must not move when only the explanation does"
        );
    }

    #[test]
    fn a_different_billing_cycle_is_a_different_key() {
        let a = action_with("same words", "0x1111@1786544000");
        let b = action_with("same words", "0x1111@1789136000");
        assert_ne!(settlement_key(&a), settlement_key(&b));
    }

    #[test]
    fn a_different_scope_is_a_different_key() {
        let mut b = action_with("same words", "0x1111@1786544000");
        b.idempotency.scope = "org_other".into();
        assert_ne!(
            settlement_key(&action_with("same words", "0x1111@1786544000")),
            settlement_key(&b)
        );
    }

    /// Separate domain separators, so the two hashes can never collide.
    #[test]
    fn a_settlement_key_is_never_an_action_hash() {
        let a = action_with("x", "0x1111@1");
        assert_ne!(settlement_key(&a), action_hash(&a));
    }
}
