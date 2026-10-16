//! The action hash is only useful if the digest this crate computes is the
//! digest an EVM contract computes. `BudgetPolicy` records the hash and keys
//! its replay guard on it, so a mismatch between the two implementations
//! would mean the contract is protecting a different value than the one the
//! verifier decided on.
//!
//! Checked against published Keccak-256 vectors rather than against our own
//! output, so agreement is with the standard and not with ourselves.

use ledger::{action_hash, canonicalize};
use sha3::{Digest, Keccak256};

fn keccak_hex(input: &[u8]) -> String {
    let mut h = Keccak256::new();
    h.update(input);
    format!(
        "0x{}",
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

#[test]
fn matches_published_keccak256_vectors() {
    // These are the values `cast keccak` and Solidity's keccak256 return.
    // Keccak-256 as used by Ethereum, NOT NIST SHA3-256: the two differ in
    // padding and produce entirely different digests for the same input.
    assert_eq!(
        keccak_hex(b""),
        "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470",
        "empty string vector"
    );
    assert_eq!(
        keccak_hex(b"abc"),
        "0x4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45",
        "\"abc\" vector"
    );
    assert_eq!(
        keccak_hex(b"hello"),
        "0x1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8",
        "\"hello\" vector"
    );
}

#[test]
fn sha3_256_would_have_been_the_wrong_choice() {
    // Guards against someone "fixing" the import to sha3::Sha3_256 later.
    // SHA3-256("") is a completely different digest, and a contract keyed on
    // it would silently never match.
    use sha3::Sha3_256;
    let mut h = Sha3_256::new();
    h.update(b"");
    let sha3 = format!(
        "0x{}",
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    assert_eq!(
        sha3,
        "0xa7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a"
    );
    assert_ne!(
        sha3,
        keccak_hex(b""),
        "keccak and sha3 must not be confused"
    );
}

#[test]
fn the_action_hash_is_keccak_over_the_canonical_bytes() {
    // The two paths must agree: hashing the canonical encoding by hand gives
    // the same value action_hash returns. This is what lets an auditor
    // recompute the digest without this crate.
    let a = sample_action();
    let bytes = canonicalize(&a);
    assert_eq!(action_hash(&a).to_hex(), keccak_hex(&bytes));
}

#[test]
fn the_domain_separator_is_present_in_the_encoding() {
    // A digest over some other structure must never be mistakable for an
    // action hash, so the encoding opens with a length-prefixed tag.
    let bytes = canonicalize(&sample_action());
    assert_eq!(&bytes[0..8], &8u64.to_be_bytes(), "8-byte length prefix");
    assert_eq!(&bytes[8..16], b"air/v0.1");
}

fn sample_action() -> ledger::Action {
    use ledger::*;
    let m = |s: &str| Money::parse(s, Currency::Usdc).unwrap();
    Action {
        id: "act_0001".into(),
        actor: Actor {
            id: "claude-reasoner".into(),
            kind: ActorKind::Model,
        },
        principal: Principal {
            id: "org".into(),
            treasury: AccountId::new("asset:usdc"),
        },
        intent: Intent {
            verb: Verb::Settle,
            rationale: "due".into(),
        },
        target: Target {
            id: "0x1111".into(),
            label: None,
        },
        amount: m("20.000000"),
        evidence: vec![Evidence {
            kind: EvidenceKind::OnchainTransfer,
            reference: "0xaa".into(),
            provenance: Provenance::Onchain,
        }],
        preconditions: vec![Precondition::NotAlreadySettled],
        authority: Authority {
            per_action_cap: m("100.000000"),
            daily_cap: m("500.000000"),
            spent_today: m("0.000000"),
        },
        expected_effects: vec![ExpectedEffect {
            account: AccountId::new("asset:usdc"),
            delta: Delta::Decrease(m("20.000000")),
        }],
        idempotency: Idempotency {
            key: "k".into(),
            scope: "org".into(),
        },
        validity: Validity {
            not_before: 0,
            expires_at: u64::MAX,
        },
    }
}

#[test]
fn the_canonical_encoding_is_frozen() {
    // A golden value. The canonical encoding defines the action hash, the
    // action hash is what BudgetPolicy keys its replay guard on, and a
    // previously-recorded hash must keep meaning the same action forever.
    //
    // If this test fails after a refactor, the refactor changed the wire
    // format. That is a breaking change to anything already anchored on
    // chain, and needs a version bump in the domain separator rather than a
    // new golden value here.
    let a = sample_action();
    assert_eq!(
        action_hash(&a).to_hex(),
        "0x78ee416d03addf9336152ed31b554f6d9dd970f614845bc61da52fe096570ac5"
    );
    assert_eq!(
        canonicalize(&a).len(),
        291,
        "encoded length is part of the format"
    );
}
