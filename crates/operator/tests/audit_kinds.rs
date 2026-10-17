//! The four kinds of docs/117 §11 (FlowCheck, Reservation, MeterCheck,
//! PriceTable) go through the audit record like every other kind: persisted
//! as canonical bytes, read back, and the root recomputed from those bytes
//! alone equals the log's root and the file's name.

use ledger::log::{path_from_leaves, verify_inclusion};
use ledger::{Currency, DecisionLog, EventBody, Money, ReservationAction, Timestamp};
use operator::audit;

fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}

#[test]
fn the_four_new_kinds_persist_load_and_recompute_to_the_root() {
    let t = |i: i64| Timestamp::from_unix(1_760_659_200 + i);
    let mut log = DecisionLog::new();
    log.append(
        t(0),
        EventBody::FlowCheck {
            period_start: 1_760_572_800,
            period_end: 1_760_659_200,
            balance_start: usdc("100.000000"),
            inflows: usdc("5.000000"),
            outflows: usdc("25.500000"),
            balance_end_book: usdc("79.500000"),
            balance_end_chain: usdc("79.500000"),
            diff: usdc("0"),
            note: "The book and the chain agree.".into(),
        },
    );
    log.append(
        t(1),
        EventBody::Reservation {
            reservation_id: "res-1".into(),
            issuer: "0x00000000000000000000000000000000000000a1".into(),
            unit: "tokens".into(),
            qty: 1_000_000,
            controller: "0x00000000000000000000000000000000000000c2".into(),
            prev_controller: Some("0x00000000000000000000000000000000000000c1".into()),
            action: ReservationAction::TransferApplied,
            note: "Control moved after the delay.".into(),
        },
    );
    log.append(
        t(2),
        EventBody::MeterCheck {
            from: 1_760_659_140,
            to: 1_760_659_200,
            model: "claude-sonnet-4-5".into(),
            ours_in: 1200,
            ours_out: 340,
            theirs_in: 1200,
            theirs_out: 340,
            note: "Our meter and the provider's agree.".into(),
        },
    );
    log.append(
        t(3),
        EventBody::PriceTable {
            source: "litellm".into(),
            fetched_at: 1_760_659_000,
            table_hash: "0xabc123".into(),
            rows: 42,
            note: "Priced against this table.".into(),
        },
    );

    let dir = std::env::temp_dir().join(format!("tameion-audit-kinds-{}", std::process::id()));
    let path = audit::persist(&dir, &log).unwrap();
    let lines = audit::load(&path).unwrap();
    let kinds: Vec<&str> = lines.iter().map(|l| l.kind.as_str()).collect();
    assert_eq!(
        kinds,
        ["FLOWCHECK", "RESERVATION", "METERCHECK", "PRICETABLE"]
    );
    for (line, event) in lines.iter().zip(log.events()) {
        assert_eq!(line.canonical, event.canonical());
        assert_eq!(line.at, event.at.as_unix());
        assert_eq!(line.note, event.body.note());
        assert_eq!(format!("evt:{}", line.seq), event.id());
    }
    let (leaves, root) = audit::recompute(&lines);
    assert_eq!(root, log.root());
    assert_eq!(audit::root_from_filename(&path), Some(root));
    for (i, leaf) in leaves.iter().enumerate() {
        let p = path_from_leaves(&leaves, i).unwrap();
        assert!(verify_inclusion(*leaf, &p, root), "event {i}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
