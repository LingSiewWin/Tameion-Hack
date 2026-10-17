//! The decision log, and the Merkle vulnerability it was built to avoid.

use ledger::log::{DecisionLog, Event, EventBody, Hash32, ReservationAction, verify_inclusion};
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

// ------------------------------- canonical bytes, pinned (tags 8 to 11)
//
// Each literal was computed from the field order alone, by a separate
// encoder written from the spec (docs/117 §11): the tag byte, then the fields
// as declared; u64 big-endian; strings with a u64 length first; money as i128
// minor units then the currency byte (USDC = 1); an Option as 0, or 1 then the
// value; an enum as one byte; i64 as its two's-complement u64; u32 widened to
// u64. An encoder change that moves one byte fails here, before it moves a
// root an auditor already holds.

const FLOWCHECK_HEX: &str = concat!(
    "000000000000000c6169722d6c6f672f76302e31", // air-log/v0.1
    "0000000000000000",                         // seq 0
    "000000006553f100",                         // at
    "08",                                       // tag 8
    "0000000068f18700",                         // period_start
    "0000000068f2d880",                         // period_end
    "00000000000000000000000005f5e10001",       // balance_start 100 USDC
    "000000000000000000000000004c4b4001",       // inflows 5
    "0000000000000000000000000185196001",       // outflows 25.5
    "00000000000000000000000004bd12e001",       // balance_end_book 79.5
    "00000000000000000000000004bd12df01",       // balance_end_chain 79.499999
    "ffffffffffffffffffffffffffffffff01",       // diff -0.000001
    "000000000000000e6f6e6520756e69742073686f7274", // note
);
const RESERVATION_HEX: &str = concat!(
    "000000000000000c6169722d6c6f672f76302e31", // air-log/v0.1
    "0000000000000000",                         // seq 0
    "000000006553f100",                         // at
    "09",                                       // tag 9
    "00000000000000057265732d31",               // reservation_id
    "000000000000000430786131",                 // issuer
    "0000000000000006746f6b656e73",             // unit
    "00000000000f4240",                         // qty 1000000
    "000000000000000430786332",                 // controller
    "01000000000000000430786331",               // prev_controller Some
    "02",                                       // TransferApplied
    "00000000000000056d6f766564",               // note
);
const RESERVATION_NONE_HEX: &str = concat!(
    "000000000000000c6169722d6c6f672f76302e31", // air-log/v0.1
    "0000000000000000",                         // seq 0
    "000000006553f100",                         // at
    "09",                                       // tag 9
    "00000000000000057265732d31",               // reservation_id
    "000000000000000430786131",                 // issuer
    "0000000000000006746f6b656e73",             // unit
    "00000000000f4240",                         // qty 1000000
    "000000000000000430786331",                 // controller
    "00",                                       // prev_controller None
    "00",                                       // Created
    "0000000000000006697373756564",             // note
);
const METERCHECK_HEX: &str = concat!(
    "000000000000000c6169722d6c6f672f76302e31", // air-log/v0.1
    "0000000000000000",                         // seq 0
    "000000006553f100",                         // at
    "0a",                                       // tag 10
    "0000000068f18700",                         // from
    "0000000068f1873c",                         // to
    "0000000000000011636c617564652d736f6e6e65742d342d35", // model
    "00000000000004b0",                         // ours_in 1200
    "0000000000000154",                         // ours_out 340
    "00000000000004b0",                         // theirs_in 1200
    "0000000000000155",                         // theirs_out 341
    "000000000000000f6f6e6520746f6b656e206170617274", // note
);
const PRICETABLE_HEX: &str = concat!(
    "000000000000000c6169722d6c6f672f76302e31", // air-log/v0.1
    "0000000000000000",                         // seq 0
    "000000006553f100",                         // at
    "0b",                                       // tag 11
    "00000000000000076c6974656c6c6d",           // source
    "0000000068f18700",                         // fetched_at
    "00000000000000083078616263313233",         // table_hash
    "000000000000002a",                         // rows 42
    "0000000000000007343220726f7773",           // note
);

fn canonical_hex(body: EventBody) -> String {
    Event {
        seq: 0,
        at: Timestamp::from_unix(T0),
        body,
    }
    .canonical()
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect()
}

#[test]
fn a_flow_check_encodes_to_its_pinned_bytes() {
    let body = EventBody::FlowCheck {
        period_start: 1_760_659_200,
        period_end: 1_760_745_600,
        balance_start: usdc("100.000000"),
        inflows: usdc("5.000000"),
        outflows: usdc("25.500000"),
        balance_end_book: usdc("79.500000"),
        balance_end_chain: usdc("79.499999"),
        diff: usdc("-0.000001"),
        note: "one unit short".into(),
    };
    assert_eq!(body.kind(), "FLOWCHECK");
    assert_eq!(body.note(), "one unit short");
    assert_eq!(canonical_hex(body), FLOWCHECK_HEX);
}

#[test]
fn a_reservation_encodes_to_its_pinned_bytes() {
    let moved = EventBody::Reservation {
        reservation_id: "res-1".into(),
        issuer: "0xa1".into(),
        unit: "tokens".into(),
        qty: 1_000_000,
        controller: "0xc2".into(),
        prev_controller: Some("0xc1".into()),
        action: ReservationAction::TransferApplied,
        note: "moved".into(),
    };
    assert_eq!(moved.kind(), "RESERVATION");
    assert_eq!(canonical_hex(moved), RESERVATION_HEX);
    let issued = EventBody::Reservation {
        reservation_id: "res-1".into(),
        issuer: "0xa1".into(),
        unit: "tokens".into(),
        qty: 1_000_000,
        controller: "0xc1".into(),
        prev_controller: None,
        action: ReservationAction::Created,
        note: "issued".into(),
    };
    assert_eq!(canonical_hex(issued), RESERVATION_NONE_HEX);
    // The action byte is the declared discriminant, never the position.
    let bytes: Vec<u8> = [
        ReservationAction::Created,
        ReservationAction::TransferProposed,
        ReservationAction::TransferApplied,
        ReservationAction::TransferCancelled,
        ReservationAction::Consumed,
        ReservationAction::Refused,
    ]
    .iter()
    .map(|a| *a as u8)
    .collect();
    assert_eq!(bytes, [0, 1, 2, 3, 4, 5]);
}

#[test]
fn a_meter_check_encodes_to_its_pinned_bytes() {
    let body = EventBody::MeterCheck {
        from: 1_760_659_200,
        to: 1_760_659_260,
        model: "claude-sonnet-4-5".into(),
        ours_in: 1200,
        ours_out: 340,
        theirs_in: 1200,
        theirs_out: 341,
        note: "one token apart".into(),
    };
    assert_eq!(body.kind(), "METERCHECK");
    assert_eq!(canonical_hex(body), METERCHECK_HEX);
}

#[test]
fn a_price_table_encodes_to_its_pinned_bytes() {
    let body = EventBody::PriceTable {
        source: "litellm".into(),
        fetched_at: 1_760_659_200,
        table_hash: "0xabc123".into(),
        rows: 42,
        note: "42 rows".into(),
    };
    assert_eq!(body.kind(), "PRICETABLE");
    assert_eq!(canonical_hex(body), PRICETABLE_HEX);
}

#[test]
fn an_event_is_cited_by_its_sequence_number() {
    let mut log = DecisionLog::new();
    for i in 0..4i64 {
        settlement(&mut log, "0xaaa", &format!("0xaaa@{i}"), "1.000000", i);
    }
    let ids: Vec<String> = log.events().iter().map(Event::id).collect();
    assert_eq!(ids, ["evt:0", "evt:1", "evt:2", "evt:3"]);
}

// ------------------------------------------------------------- decoding

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn every_kind_decodes_back_to_itself_and_nothing_else_decodes() {
    let mut log = DecisionLog::new();
    let bodies = [
        EventBody::Observation {
            balance: usdc("12.500000"),
            transfers_seen: 7,
            obligations_found: 2,
            total_due: usdc("3.000000"),
            note: "saw".into(),
        },
        EventBody::Decision {
            counterparty: cp("0x1111"),
            amount: usdc("20.000000"),
            decision: Decision::Review,
            action_hash: a_hash("act_1"),
            confidence: Confidence::new(0.85).unwrap(),
            balance_seen: usdc("12735.000000"),
            weakest_evidence: Some(Provenance::Onchain),
            evidence: vec!["0xaaa".into(), "0xbbb".into()],
            note: "due".into(),
        },
        EventBody::Escalation {
            counterparty: cp("0xccc"),
            amount: usdc("5000.000000"),
            reason: "over the cap".into(),
            note: "asking".into(),
        },
        EventBody::Skipped {
            counterparty: cp("0xddd"),
            cycle_key: "0xddd@1".into(),
            note: "paid already".into(),
        },
        EventBody::Hedge {
            coin: "ETH".into(),
            outcome: "TRADED".into(),
            size_before: "0".into(),
            size_delta: "-0.01".into(),
            size_after: "-0.01".into(),
            mark_px: "2500.5".into(),
            notional_after: usdc("25.005000"),
            oid: Some("42".into()),
            note: "hedged".into(),
        },
        EventBody::HedgeProposal {
            coin: "ETH".into(),
            advice: "widen-band".into(),
            band_bps: Some(800),
            reason: "carry".into(),
            by: "model".into(),
            note: "proposed".into(),
        },
    ];
    for (i, body) in bodies.into_iter().enumerate() {
        log.append(at(i as i64), body);
    }
    settlement(&mut log, "0xaaa", "0xaaa@1", "10.000000", 9);
    for hex in [
        FLOWCHECK_HEX,
        RESERVATION_HEX,
        RESERVATION_NONE_HEX,
        METERCHECK_HEX,
        PRICETABLE_HEX,
    ] {
        let e = Event::decode(&unhex(hex)).expect("a pinned event decodes");
        log.append(e.at, e.body);
    }
    assert_eq!(log.len(), 12);
    for e in log.events() {
        let bytes = e.canonical();
        assert_eq!(Event::decode(&bytes).as_ref(), Some(e), "{}", e.body.kind());
        assert_eq!(Event::decode(&bytes[..bytes.len() - 1]), None, "truncated");
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(Event::decode(&longer), None, "a trailing byte");
    }
    // A counterparty the encoder could never have written (upper case) is
    // refused, not silently normalised into a different record.
    let mut forged = log.events()[6].canonical();
    let at = forged.windows(5).position(|w| w == b"0xaaa").unwrap();
    forged[at..at + 5].copy_from_slice(b"0xAAA");
    assert_eq!(Event::decode(&forged), None);
}
