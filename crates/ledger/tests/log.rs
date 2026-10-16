//! The decision log, and the Merkle vulnerability it was built to avoid.

use ledger::log::{DecisionLog, EventBody, Hash32, verify_inclusion};
use ledger::{
    ActionHash, Confidence, Counterparty, Currency, Decision, Money, Provenance, Timestamp,
};
use proptest::prelude::*;

const T0: i64 = 1_700_000_000;

fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}
fn at(offset: i64) -> Timestamp {
    Timestamp::from_unix(T0 + offset)
}
fn cp(s: &str) -> Counterparty {
    Counterparty::new(s)
}

fn a_hash(seed: &str) -> ActionHash {
    // Any 32-byte value; built through the real type so the test cannot
    // fabricate one of the wrong width.
    use ledger::*;
    let m = |s: &str| Money::parse(s, Currency::Usdc).unwrap();
    let action = Action {
        id: seed.into(),
        actor: Actor {
            id: "r".into(),
            kind: ActorKind::Rule,
        },
        principal: Principal {
            id: "o".into(),
            treasury: AccountId::new("asset:usdc"),
        },
        intent: Intent {
            verb: Verb::Settle,
            rationale: String::new(),
        },
        target: Target {
            id: "0x1".into(),
            label: None,
        },
        amount: m("1.000000"),
        evidence: vec![],
        preconditions: vec![],
        authority: Authority {
            per_action_cap: m("1.000000"),
            daily_cap: m("1.000000"),
            spent_today: m("0.000000"),
        },
        expected_effects: vec![],
        idempotency: Idempotency {
            key: "k".into(),
            scope: "s".into(),
        },
        validity: Validity {
            not_before: 0,
            expires_at: 1,
        },
    };
    action_hash(&action)
}

fn settlement(log: &mut DecisionLog, who: &str, cycle: &str, amount: &str, offset: i64) {
    log.append(
        at(offset),
        EventBody::Settlement {
            counterparty: cp(who),
            cycle_key: cycle.into(),
            amount: usdc(amount),
            tx_hash: format!("0xtx{offset}"),
            action_hash: a_hash(cycle),
            settlement_key: a_hash(&format!("settle:{cycle}")),
            note: format!("Paid {amount} USDC to {who}."),
        },
    );
}

// ------------------------------------------------- the CVE-2012-2459 fix

#[test]
fn three_events_and_four_do_not_share_a_root() {
    // The Python implementation this replaces duplicated the last node when a
    // level had an odd count, which is the Bitcoin construction. That makes
    // [A, B, C] and [A, B, C, C] produce the same root -- verified against
    // `agent/decision_log.py` on 2026-09-20, both gave
    // d21feda3c020ad890c6edf9ae1f6b5e26242e0a30fdbb03c78279909de717ff1.
    //
    // For a log whose last entry is a settlement, that means a record of one
    // payment and a record of two are indistinguishable to anything checking
    // the anchor -- exactly the double payment the log exists to prevent.
    let mut three = DecisionLog::new();
    settlement(&mut three, "0xaaa", "0xaaa@1", "10.000000", 1);
    settlement(&mut three, "0xbbb", "0xbbb@1", "20.000000", 2);
    settlement(&mut three, "0xccc", "0xccc@1", "30.000000", 3);

    let mut four = three.clone();
    // Repeat the final event exactly. Under the duplicating construction this
    // is invisible.
    settlement(&mut four, "0xccc", "0xccc@1", "30.000000", 3);

    assert_ne!(
        three.root(),
        four.root(),
        "duplicating the last event must move the root"
    );
    assert_eq!(three.len(), 3);
    assert_eq!(four.len(), 4);
}

#[test]
fn every_prefix_length_has_a_distinct_root() {
    // Stronger than the pair above: no two prefixes of a growing log may ever
    // collide, at any length, including the odd/even boundaries where the
    // duplicating construction fails.
    let mut log = DecisionLog::new();
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..64i64 {
        settlement(&mut log, "0xaaa", &format!("0xaaa@{i}"), "1.000000", i);
        assert!(
            seen.insert(log.root()),
            "root repeated at length {}",
            log.len()
        );
    }
}

#[test]
fn leaves_and_nodes_live_in_separate_domains() {
    // RFC 6962 prefixes leaves with 0x00 and internal nodes with 0x01, so an
    // internal node can never be presented as a leaf. Without the prefixes a
    // 64-byte "event" whose bytes are two concatenated hashes would hash
    // identically to the node above them.
    let mut log = DecisionLog::new();
    settlement(&mut log, "0xaaa", "0xaaa@1", "1.000000", 1);
    settlement(&mut log, "0xbbb", "0xbbb@1", "1.000000", 2);

    let root = log.root();
    let leaf_a = log.events()[0].leaf();
    let leaf_b = log.events()[1].leaf();

    // The root is a node hash over the two leaves. No leaf equals it.
    assert_ne!(root, leaf_a);
    assert_ne!(root, leaf_b);

    // And the naive concatenation, hashed as a leaf, is not the root either.
    let mut concat = Vec::new();
    concat.extend_from_slice(leaf_a.as_bytes());
    concat.extend_from_slice(leaf_b.as_bytes());
    let as_leaf = {
        use sha3::{Digest, Keccak256};
        let mut h = Keccak256::new();
        h.update([0x00u8]);
        h.update(&concat);
        let mut out = [0u8; 32];
        out.copy_from_slice(&h.finalize());
        out
    };
    assert_ne!(
        &as_leaf,
        root.as_bytes(),
        "domain separation is doing its job"
    );
}

// -------------------------------------------------------------- basics

#[test]
fn an_empty_log_commits_to_zero() {
    let log = DecisionLog::new();
    assert_eq!(log.root(), Hash32::ZERO);
    assert!(log.is_empty());
}

#[test]
fn a_single_event_root_is_its_leaf() {
    let mut log = DecisionLog::new();
    settlement(&mut log, "0xaaa", "0xaaa@1", "1.000000", 1);
    assert_eq!(log.root(), log.events()[0].leaf());
}

#[test]
fn settled_cycles_are_recovered_for_the_duplicate_guard() {
    let mut log = DecisionLog::new();
    settlement(&mut log, "0xaaa", "0xaaa@1984916082", "20.000000", 1);
    settlement(&mut log, "0xbbb", "0xbbb@1984916082", "35.000000", 2);
    log.append(
        at(3),
        EventBody::Escalation {
            counterparty: cp("0xccc"),
            amount: usdc("5000.000000"),
            reason: "over the per-action cap".into(),
            note: "Stopping and asking a human.".into(),
        },
    );

    let settled = log.settled_cycles();
    assert_eq!(settled.len(), 2, "an escalation is not a settlement");
    assert!(settled.contains("0xaaa@1984916082"));
    assert!(!settled.iter().any(|k| k.starts_with("0xccc")));
}

#[test]
fn a_decision_carries_what_it_was_decided_on() {
    let mut log = DecisionLog::new();
    log.append(
        at(1),
        EventBody::Decision {
            counterparty: cp("0x1111"),
            amount: usdc("20.000000"),
            decision: Decision::Allow,
            action_hash: a_hash("act_1"),
            confidence: Confidence::new(0.85).unwrap(),
            balance_seen: usdc("12735.000000"),
            weakest_evidence: Some(Provenance::Onchain),
            evidence: vec!["0xaaa".into(), "0xbbb".into()],
            note: "0x1111 is due.".into(),
        },
    );
    let e = &log.events()[0];
    assert_eq!(e.body.kind(), "DECISION");
    assert!(e.body.note().contains("0x1111"));
}

#[test]
fn sequence_numbers_are_reissued_on_reload() {
    // A file with gaps or duplicated sequence numbers cannot smuggle in a
    // sequence that was never issued.
    let mut log = DecisionLog::new();
    for i in 0..5i64 {
        settlement(&mut log, "0xaaa", &format!("0xaaa@{i}"), "1.000000", i);
    }
    let mut tampered: Vec<_> = log.events().to_vec();
    tampered[2].seq = 999;
    let rebuilt = DecisionLog::from_events(tampered);
    assert_eq!(rebuilt.events()[2].seq, 2);
    assert_eq!(
        rebuilt.root(),
        log.root(),
        "the root is unchanged by the fix-up"
    );
}

#[test]
fn replay_reads_as_a_record_a_person_can_follow() {
    let mut log = DecisionLog::new();
    settlement(&mut log, "0x1111", "0x1111@1", "20.000000", 1);
    let lines: Vec<String> = log.replay().collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("[0000] SETTLEMENT"), "{}", lines[0]);
    assert!(lines[0].contains("Paid 20.000000 USDC"), "{}", lines[0]);
}

// -------------------------------------------------------------- tamper

#[test]
fn editing_any_field_of_any_event_moves_the_root() {
    let mut log = DecisionLog::new();
    for i in 0..7i64 {
        settlement(&mut log, "0xaaa", &format!("0xaaa@{i}"), "10.000000", i);
    }
    let original = log.root();

    for i in 0..log.len() {
        let mut events = log.events().to_vec();
        if let EventBody::Settlement { amount, .. } = &mut events[i].body {
            // One minor unit. The smallest edit that exists.
            *amount = usdc("10.000001");
        }
        let tampered = DecisionLog::from_events(events);
        assert_ne!(
            tampered.root(),
            original,
            "editing event {i} left the root unchanged"
        );
    }
}

#[test]
fn inclusion_proofs_verify_and_fail_when_they_should() {
    let mut log = DecisionLog::new();
    for i in 0..11i64 {
        settlement(&mut log, "0xaaa", &format!("0xaaa@{i}"), "10.000000", i);
    }
    let root = log.root();

    for i in 0..log.len() {
        let proof = log.proof_for(i).expect("in range");
        let leaf = log.events()[i].leaf();
        assert!(
            verify_inclusion(leaf, &proof, root),
            "event {i} should verify"
        );

        // The same proof must not validate a different leaf.
        let other = log.events()[(i + 1) % log.len()].leaf();
        assert!(
            !verify_inclusion(other, &proof, root),
            "event {i} proof validated a stranger"
        );
    }
    assert!(
        log.proof_for(log.len()).is_none(),
        "out of range yields no proof"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Every event in a log of any size proves its own inclusion.
    #[test]
    fn inclusion_holds_at_every_size(n in 1usize..40) {
        let mut log = DecisionLog::new();
        for i in 0..n {
            settlement(&mut log, "0xaaa", &format!("0xaaa@{i}"), "1.000000", i as i64);
        }
        let root = log.root();
        for i in 0..n {
            let proof = log.proof_for(i).unwrap();
            prop_assert!(verify_inclusion(log.events()[i].leaf(), &proof, root));
        }
    }

    /// Appending always moves the root. No append is ever invisible.
    #[test]
    fn appending_always_moves_the_root(n in 1usize..40) {
        let mut log = DecisionLog::new();
        let mut previous = log.root();
        for i in 0..n {
            settlement(&mut log, "0xaaa", &format!("0xaaa@{i}"), "1.000000", i as i64);
            let now = log.root();
            prop_assert_ne!(now, previous, "append {} was invisible", i);
            previous = now;
        }
    }
}
