//! The four properties of the brief (docs/117 §13): the score stays in
//! [0, 10000]; a closed loop never raises it; a triple-signed record never
//! lowers survival; recency falls with age. And a fifth: a record counted
//! twice counts once.

use pistis::{Flow, Params, Record, SlotSig, Symbolon, SymbolonKind, score};
use proptest::prelude::*;

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;
// A small pool of counterparties and amounts, so loops and disputes happen.
const PARTIES: [&str; 5] = ["a", "b", "c", "d", "fresh"];
const AMOUNTS: [u128; 4] = [100_000, 200_000, 500_000, 1_000_000];

fn slot(on: bool) -> Option<SlotSig> {
    on.then(|| SlotSig {
        alg: "chain-receipt".into(),
        key_id: "chain:5042002".into(),
        sig: vec![7],
        at: 0,
    })
}

/// Record `id` is its own record: the hash-first id dedups equal hashes.
fn record(id: u64, flow: Flow, cp: &str, amount: u128, at: i64, [a, c, w]: [bool; 3]) -> Record {
    let mut record_hash = [0; 32];
    record_hash[..8].copy_from_slice(&id.to_be_bytes());
    Record {
        sym: Symbolon {
            record_hash,
            kind: SymbolonKind::Settlement,
            agent: slot(a),
            counterparty: slot(c),
            witness: slot(w),
        },
        at,
        counterparty: cp.into(),
        amount,
        flow,
    }
}

prop_compose! {
    fn any_record()(
        flow in prop_oneof![
            Just(Flow::Paid), Just(Flow::Received), Just(Flow::Claimed),
            Just(Flow::Refused), Just(Flow::Other),
        ],
        cp in 0..4usize,
        amount in 0..AMOUNTS.len(),
        days_ago in 0i64..200,
        slots in any::<[bool; 3]>(),
    ) -> Record {
        record(0, flow, PARTIES[cp], AMOUNTS[amount], NOW - days_ago * DAY, slots)
    }
}

fn records() -> impl Strategy<Value = Vec<Record>> {
    prop::collection::vec(any_record(), 0..30).prop_map(|mut rs| {
        for (i, r) in rs.iter_mut().enumerate() {
            r.sym.record_hash[..8].copy_from_slice(&(i as u64 + 1).to_be_bytes());
        }
        rs
    })
}

proptest! {
    #[test]
    fn the_score_stays_in_range(rs in records(), ahead in 0i64..400) {
        let s = score(&rs, NOW + ahead * DAY, &Params::default());
        prop_assert!(s.bps <= 10_000);
        for f in [s.survival, s.independence, s.recency] {
            prop_assert!((0.0..=1.0).contains(&f));
        }
    }

    #[test]
    fn a_closed_loop_never_raises_the_score(
        rs in records(),
        cp in 0..PARTIES.len(),
        out in 0..AMOUNTS.len(),
        back in 0..AMOUNTS.len(),
        days_ago in 0i64..200,
        back_after in 0i64..=30,
        out_slots in any::<[bool; 3]>(),
        back_slots in any::<[bool; 3]>(),
    ) {
        let p = Params::default();
        let before = score(&rs, NOW, &p);
        let at = NOW - days_ago * DAY;
        let mut looped = rs.clone();
        looped.push(record(1_000, Flow::Paid, PARTIES[cp], AMOUNTS[out], at, out_slots));
        looped.push(record(1_001, Flow::Received, PARTIES[cp], AMOUNTS[back], at + back_after * DAY, back_slots));
        let after = score(&looped, NOW, &p);
        prop_assert!(after.bps <= before.bps, "{before:?} -> {after:?}");
        prop_assert!(after.survival <= before.survival);
        prop_assert!(after.independence <= before.independence);
        prop_assert!(after.recency <= before.recency);
    }

    #[test]
    fn a_triple_signed_record_never_lowers_survival(
        rs in records(),
        cp in 0..PARTIES.len(),
        amount in 0..AMOUNTS.len(),
        days_ago in 0i64..200,
        claim in any::<bool>(),
    ) {
        let p = Params::default();
        let (cp, amount, at) = (PARTIES[cp], AMOUNTS[amount], NOW - days_ago * DAY);
        let flow = if claim { Flow::Claimed } else { Flow::Paid };
        // The two deliberate exceptions (unit tests in lib.rs): a record that
        // something later refuses is a dispute, and a payment whose money
        // comes back closes a loop, which takes the counterparty's evidence
        // away.
        let refused_later = rs.iter().any(|q| {
            q.flow == Flow::Refused && q.counterparty == cp && q.amount == amount && q.at > at
        });
        let closes_loop = flow == Flow::Paid
            && rs.iter().any(|i| {
                i.flow == Flow::Received
                    && i.counterparty == cp
                    && i.at >= at
                    && i.at <= at + i64::from(p.loop_window_days) * DAY
            });
        prop_assume!(!refused_later && !closes_loop);
        let before = score(&rs, NOW, &p).survival;
        let mut more = rs.clone();
        more.push(record(2_000, flow, cp, amount, at, [true; 3]));
        prop_assert!(score(&more, NOW, &p).survival >= before);
    }

    #[test]
    fn a_record_counted_twice_counts_once(rs in records(), ahead in 0i64..400) {
        let p = Params::default();
        let twice: Vec<Record> = rs.iter().chain(&rs).cloned().collect();
        prop_assert_eq!(score(&twice, NOW + ahead * DAY, &p), score(&rs, NOW + ahead * DAY, &p));
    }

    #[test]
    fn recency_falls_with_age(rs in records(), later in 0i64..400) {
        let p = Params::default();
        prop_assert!(score(&rs, NOW + later * DAY, &p).recency <= score(&rs, NOW, &p).recency);
    }
}
