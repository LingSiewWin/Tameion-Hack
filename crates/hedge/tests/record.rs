use hedge::delta::Rebalance;
use hedge::engine::{Action, Books, CoinReport, StepReport};
use hedge::record::{events_for, json, outcome};
use hedge::safety::{AccountState, Limits, Refusal, Verdict};
use ledger::{Currency, DecisionLog, EventBody, Money, Timestamp};
use rust_decimal::Decimal;
use rust_decimal::dec;

fn coin(action: Action, verdict: Option<Verdict>) -> CoinReport {
    CoinReport {
        coin: "ETH".into(),
        spot_size: dec!(0.5),
        mark_px: dec!(2500),
        oracle_px: dec!(2501),
        funding_rate: dec!(-0.0001),
        spot_value: dec!(1250),
        hedge_size_before: Decimal::ZERO,
        hedge_upnl: Decimal::ZERO,
        target_size: dec!(-0.5),
        rebalance: Rebalance::Trade {
            size_delta: dec!(-0.5),
            is_buy: false,
            drift_bps: 10_000,
        },
        verdict,
        action,
        net: None,
    }
}

fn report(coins: Vec<CoinReport>, equity: Decimal) -> StepReport {
    StepReport {
        account: AccountState {
            account_value: equity,
            margin_used: Decimal::ZERO,
            maintenance_margin_used: Decimal::ZERO,
            total_notional: Decimal::ZERO,
        },
        withdrawable: equity,
        health: None,
        halted_before: false,
        coins,
        funding_entries_booked: 0,
        entries_booked: 0,
        cash_residual: None,
        cover_usdc: None,
    }
}

fn limits() -> Limits {
    Limits {
        size_delta: dec!(-0.5),
        size_after: dec!(-0.5),
        notional_after: dec!(1250),
        leverage_after: dec!(0.625),
        maintenance_rate: dec!(0.02),
        required_equity: dec!(100),
    }
}

#[test]
fn outcome_words_are_stable() {
    assert_eq!(
        outcome(&coin(Action::None("inside band".into()), None)),
        "HOLD"
    );
    assert_eq!(
        outcome(&coin(
            Action::None("refused: NoCollateral".into()),
            Some(Verdict::Refuse(Refusal::NoCollateral))
        )),
        "REFUSE"
    );
    assert_eq!(
        outcome(&coin(
            Action::WouldTrade {
                size_delta: dec!(-0.5),
                is_buy: false,
                worst_px: dec!(2487.5),
                limits: limits(),
            },
            Some(Verdict::Allow(limits()))
        )),
        "DRY-RUN"
    );
    assert_eq!(
        outcome(&coin(
            Action::Halted {
                reason: "mismatch".into()
            },
            Some(Verdict::Allow(limits()))
        )),
        "HALTED"
    );
}

#[test]
fn a_dry_run_becomes_one_typed_event_with_usdc_notional() {
    let r = report(
        vec![coin(
            Action::WouldTrade {
                size_delta: dec!(-0.5),
                is_buy: false,
                worst_px: dec!(2487.5),
                limits: limits(),
            },
            Some(Verdict::Allow(limits())),
        )],
        dec!(2000),
    );
    let events = events_for(&r).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        EventBody::Hedge {
            coin,
            outcome,
            size_before,
            size_delta,
            size_after,
            mark_px,
            notional_after,
            oid,
            note,
        } => {
            assert_eq!(coin, "ETH");
            assert_eq!(outcome, "DRY-RUN");
            assert_eq!(size_before, "0");
            assert_eq!(size_delta, "-0.5");
            assert_eq!(size_after, "-0.5");
            assert_eq!(mark_px, "2500");
            assert_eq!(*notional_after, Money::minor(1_250_000_000, Currency::Usdc));
            assert!(oid.is_none());
            assert!(note.contains("would send -0.5 at worst 2487.5"), "{note}");
            assert!(note.contains("equity 2000"), "{note}");
        }
        other => panic!("expected Hedge, got {other:?}"),
    }
    assert_eq!(events[0].kind(), "HEDGE");
}

#[test]
fn hedge_events_change_the_anchored_root_and_encode_deterministically() {
    let r = report(
        vec![coin(
            Action::None("refused: NoCollateral".into()),
            Some(Verdict::Refuse(Refusal::NoCollateral)),
        )],
        Decimal::ZERO,
    );
    let body = events_for(&r).unwrap().remove(0);
    let at = Timestamp::from_unix(1_800_000_000);

    let mut a = DecisionLog::new();
    a.append(at, body.clone());
    let mut b = DecisionLog::new();
    b.append(at, body.clone());
    assert_eq!(a.root(), b.root(), "same event, same root, on any machine");
    assert_eq!(a.events()[0].canonical(), b.events()[0].canonical());

    // The same verdict on a different coin is a different record.
    let mut sol = report(
        vec![coin(
            Action::None("refused: NoCollateral".into()),
            Some(Verdict::Refuse(Refusal::NoCollateral)),
        )],
        Decimal::ZERO,
    );
    sol.coins[0].coin = "SOL".into();
    let mut c = DecisionLog::new();
    c.append(at, events_for(&sol).unwrap().remove(0));
    assert_ne!(a.root(), c.root(), "a different coin is a different record");
    assert!(a.replay().next().unwrap().contains("HEDGE"));
}

#[test]
fn dashboard_json_carries_the_fields_the_page_reads() {
    let r = report(
        vec![coin(Action::None("inside band".into()), None)],
        dec!(10),
    );
    let books = Books::new(&["ETH"], 0);
    let v = json(&r, &books, "dry-run");
    assert_eq!(v["mode"], "dry-run");
    assert_eq!(v["account"]["accountValue"], "10");
    assert_eq!(v["coins"][0]["coin"], "ETH");
    assert_eq!(v["coins"][0]["outcome"], "HOLD");
    assert_eq!(v["coins"][0]["sizeDelta"], "0");
    assert_eq!(v["entries"], 0);
    assert!(v["trialBalance"].as_str().unwrap().contains("="));
}
