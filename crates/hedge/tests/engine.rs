//! The engine against a scripted venue: the dry-run path, the live path with a
//! matching read-back, and the live path with a mismatch -- the one branch a
//! funded account should never have to exercise.

use hedge::engine::{Action, Books, Holding, Mode, StepInput, run_once};
use hedge::halt;
use hedge::hl::{AccountSnapshot, AssetCtx, MarketInfo, VenueApi, VenueError};
use hedge::policy::HedgePolicy;
use hedge::safety::{AccountState, Shrink, Tier, Verdict};
use hypersdk::Address;
use hypersdk::hypercore::{
    CumulativeFunding, Leverage, LeverageType, OrderResponseStatus, PositionData, PrivateKeySigner,
    UserFundingEntry,
};
use ledger::Currency;
use rust_decimal::Decimal;
use rust_decimal::dec;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// A venue that answers from a script and records what it was asked to do.
struct Fake {
    equity: Decimal,
    /// Account value reported after the order (fees, realised PnL); `None`
    /// means unchanged.
    equity_after: Option<Decimal>,
    /// Signed position reported *before* the order.
    position_before: Decimal,
    /// What the order reply says was filled (positive base units).
    fill: Decimal,
    /// Signed position reported *after* the order.
    position_after: Decimal,
    orders: Mutex<Vec<(bool, Decimal, Decimal)>>,
    traded: Mutex<bool>,
    /// The order call errors after it may have reached the venue.
    place_fails: bool,
}

fn position(coin: &str, szi: Decimal, px: Decimal) -> PositionData {
    PositionData {
        coin: coin.into(),
        szi,
        leverage: Leverage {
            leverage_type: LeverageType::Cross,
            value: 2,
            raw_usd: None,
        },
        entry_px: Some(px),
        position_value: szi.abs() * px,
        unrealized_pnl: Decimal::ZERO,
        return_on_equity: Decimal::ZERO,
        liquidation_px: None,
        margin_used: szi.abs() * px / dec!(2),
        max_leverage: 25,
        cum_funding: CumulativeFunding {
            all_time: Decimal::ZERO,
            since_open: Decimal::ZERO,
            since_change: Decimal::ZERO,
        },
    }
}

impl VenueApi for Fake {
    async fn markets(&self) -> Result<HashMap<String, MarketInfo>, VenueError> {
        Ok(HashMap::from([
            (
                "ETH".to_string(),
                MarketInfo {
                    name: "ETH".into(),
                    index: 4,
                    sz_decimals: 4,
                    max_leverage: 25,
                },
            ),
            (
                "BTC".to_string(),
                MarketInfo {
                    name: "BTC".into(),
                    index: 3,
                    sz_decimals: 5,
                    max_leverage: 40,
                },
            ),
        ]))
    }
    async fn prices(&self) -> Result<HashMap<String, AssetCtx>, VenueError> {
        Ok(HashMap::from([
            (
                "ETH".to_string(),
                AssetCtx {
                    mark_px: dec!(2500),
                    oracle_px: dec!(2501),
                    funding: dec!(-0.0001),
                    open_interest: dec!(100),
                    max_leverage: 25,
                    margin_table_id: None,
                },
            ),
            (
                "BTC".to_string(),
                AssetCtx {
                    mark_px: dec!(50000),
                    oracle_px: dec!(50010),
                    funding: dec!(-0.0001),
                    open_interest: dec!(100),
                    max_leverage: 40,
                    margin_table_id: None,
                },
            ),
        ]))
    }
    async fn account(&self, _user: Address) -> Result<AccountSnapshot, VenueError> {
        let szi = if *self.traded.lock().unwrap() {
            self.position_after
        } else {
            self.position_before
        };
        let mut positions = HashMap::new();
        if !szi.is_zero() {
            positions.insert("ETH".to_string(), position("ETH", szi, dec!(2500)));
        }
        let equity = if *self.traded.lock().unwrap() {
            self.equity_after.unwrap_or(self.equity)
        } else {
            self.equity
        };
        Ok(AccountSnapshot {
            state: AccountState {
                account_value: equity,
                margin_used: Decimal::ZERO,
                maintenance_margin_used: Decimal::ZERO,
                total_notional: szi.abs() * dec!(2500),
            },
            withdrawable: equity,
            positions,
            time_ms: 1_800_000_000_000,
        })
    }
    async fn tiers(&self, _id: Option<u32>) -> Result<Vec<Tier>, VenueError> {
        Ok(Vec::new())
    }
    async fn funding_since(
        &self,
        _user: Address,
        _start_ms: u64,
    ) -> Result<Vec<UserFundingEntry>, VenueError> {
        Ok(Vec::new())
    }
    async fn set_leverage(
        &self,
        _signer: &PrivateKeySigner,
        _market: &MarketInfo,
        _leverage: u32,
    ) -> Result<(), VenueError> {
        Ok(())
    }
    async fn place_market(
        &self,
        _signer: &PrivateKeySigner,
        _market: &MarketInfo,
        is_buy: bool,
        size: Decimal,
        worst_px: Decimal,
    ) -> Result<Vec<OrderResponseStatus>, VenueError> {
        self.orders.lock().unwrap().push((is_buy, size, worst_px));
        *self.traded.lock().unwrap() = true;
        if self.place_fails {
            return Err(VenueError::Shape("connection reset after send".into()));
        }
        Ok(vec![OrderResponseStatus::Filled {
            total_sz: self.fill,
            avg_px: dec!(2499),
            oid: 42,
        }])
    }
}

fn input(halt_dir: PathBuf) -> StepInput {
    StepInput {
        user: Address::ZERO,
        holdings: vec![Holding {
            coin: "ETH".into(),
            size: dec!(0.5),
        }],
        policy: HedgePolicy::default(),
        halt_dir,
        cover_usdc: None,
        only_listed: false,
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hedge-engine-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[tokio::test]
async fn dry_run_books_the_mark_and_reports_the_order_it_would_send() {
    let venue = Fake {
        equity: dec!(2000),
        equity_after: None,
        position_before: Decimal::ZERO,
        fill: Decimal::ZERO,
        position_after: Decimal::ZERO,
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let dir = temp_dir("dry");
    let mut books = Books::new(&["ETH"], 0);
    let r = run_once(&venue, &Mode::DryRun, &input(dir), &mut books)
        .await
        .unwrap();
    let c = &r.coins[0];
    match &c.action {
        Action::WouldTrade {
            size_delta,
            is_buy,
            worst_px,
            ..
        } => {
            assert_eq!(*size_delta, dec!(-0.5));
            assert!(!is_buy);
            // 50 bps below the mark for a sell.
            assert_eq!(*worst_px, dec!(2487.5));
        }
        other => panic!("expected WouldTrade, got {other:?}"),
    }
    assert!(
        venue.orders.lock().unwrap().is_empty(),
        "dry run sent nothing"
    );
    // The spot mark was booked: 0.5 ETH × 2500.
    let net = c.net.as_ref().unwrap();
    assert_eq!(
        net.spot,
        ledger::Money::minor(1_250_000_000, Currency::Usdc)
    );
    let (d, cr) = books.journal.trial_balance(Currency::Usdc).unwrap();
    assert_eq!(d, cr);
    assert!(!halt::is_halted(&temp_dir("dry")));
}

#[tokio::test]
async fn live_with_matching_read_back_is_a_trade() {
    let venue = Fake {
        equity: dec!(2000),
        equity_after: None,
        position_before: Decimal::ZERO,
        fill: dec!(0.5),
        position_after: dec!(-0.5),
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let dir = temp_dir("live-ok");
    let mut books = Books::new(&["ETH"], 0);
    let signer = PrivateKeySigner::random();
    let r = run_once(&venue, &Mode::Live(signer), &input(dir.clone()), &mut books)
        .await
        .unwrap();
    match &r.coins[0].action {
        Action::Traded {
            filled_size,
            read_back,
            statuses,
            ..
        } => {
            assert_eq!(
                *filled_size,
                dec!(-0.5),
                "a sell fill is a negative size change"
            );
            assert_eq!(*read_back, dec!(-0.5));
            assert_eq!(statuses.len(), 1);
        }
        other => panic!("expected Traded, got {other:?}"),
    }
    let orders = venue.orders.lock().unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0], (false, dec!(0.5), dec!(2487.5)));
    assert!(!halt::is_halted(&dir), "a matching read-back never halts");
    // One pass on record: the order as sent, its id, and the read-back.
    let pass = last_pass(&dir);
    assert_eq!(pass["mode"], "live");
    assert!(pass["ms"].is_u64());
    let o = &pass["orders"][0];
    assert_eq!(
        (
            &o["coin"],
            &o["side"],
            &o["size"],
            &o["oid"],
            &o["readBack"]
        ),
        (
            &serde_json::json!("ETH"),
            &serde_json::json!("sell"),
            &serde_json::json!("0.5"),
            &serde_json::json!(42),
            &serde_json::json!("match")
        )
    );
}

/// docs/103 S3-3: the pass record is for watching; when it cannot be written
/// (disk full, read-only `runtime/`), the step still returns its report, so the
/// caller saves the books and no fill is lost from them.
#[tokio::test]
async fn a_pass_record_that_cannot_be_written_never_loses_the_step() {
    let venue = Fake {
        equity: dec!(2000),
        equity_after: None,
        position_before: Decimal::ZERO,
        fill: dec!(0.5),
        position_after: dec!(-0.5),
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let dir = temp_dir("record-fails");
    std::fs::create_dir_all(&dir).unwrap();
    // A file where the record's folder should be: the append cannot happen.
    std::fs::write(
        hedge::driver::books_dir(&dir, Address::ZERO),
        b"not a folder",
    )
    .unwrap();
    let mut books = Books::new(&["ETH"], 0);
    let r = run_once(
        &venue,
        &Mode::Live(PrivateKeySigner::random()),
        &input(dir.clone()),
        &mut books,
    )
    .await
    .expect("a record that fails does not fail the step");
    assert!(matches!(r.coins[0].action, Action::Traded { .. }));
    assert!(
        books.unsaved() > 0,
        "the books hold the pass, ready for store::save"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The newest line of the pass record the engine keeps beside the books.
fn last_pass(halt_dir: &std::path::Path) -> serde_json::Value {
    let p = hedge::driver::books_dir(halt_dir, Address::ZERO).join(hedge::store::PASSES_FILE);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(text.lines().last().expect("one pass")).unwrap()
}

#[tokio::test]
async fn live_with_a_mismatching_read_back_halts_and_stays_halted() {
    let venue = Fake {
        equity: dec!(2000),
        equity_after: None,
        position_before: Decimal::ZERO,
        fill: dec!(0.5),
        // The venue says it filled 0.5 but the account shows 0.3 short.
        position_after: dec!(-0.3),
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let dir = temp_dir("live-halt");
    let mut books = Books::new(&["ETH"], 0);
    let r = run_once(
        &venue,
        &Mode::Live(PrivateKeySigner::random()),
        &input(dir.clone()),
        &mut books,
    )
    .await
    .unwrap();
    match &r.coins[0].action {
        Action::Halted { reason } => {
            assert!(reason.contains("read back -0.3, expected -0.5"), "{reason}");
        }
        other => panic!("expected Halted, got {other:?}"),
    }
    // The pass record keeps the order and the mismatch the HALT names.
    let o = &last_pass(&dir)["orders"][0];
    assert_eq!(o["readBack"], "mismatch");
    assert_eq!(
        (o["expected"].as_str(), o["read"].as_str()),
        (Some("-0.5"), Some("-0.3"))
    );
    assert!(halt::is_halted(&dir), "HALT file written");
    let written = std::fs::read_to_string(halt::path(&dir)).unwrap();
    assert!(written.contains("read back -0.3"), "{written}");

    // The next step sees the file and refuses before touching the venue.
    let r2 = run_once(
        &venue,
        &Mode::Live(PrivateKeySigner::random()),
        &input(dir.clone()),
        &mut books,
    )
    .await
    .unwrap();
    assert!(r2.halted_before);
    assert!(
        matches!(&r2.coins[0].action, Action::None(s) if s.contains("Halted")),
        "{:?}",
        r2.coins[0].action
    );
    assert_eq!(
        venue.orders.lock().unwrap().len(),
        1,
        "no second order while halted"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_position_without_a_holding_is_closed_to_flat() {
    // The treasury sold its ETH; the venue still shows a 0.2 ETH short.
    let venue = Fake {
        equity: dec!(2000),
        equity_after: None,
        position_before: dec!(-0.2),
        fill: Decimal::ZERO,
        position_after: Decimal::ZERO,
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let dir = temp_dir("leftover");
    let input = StepInput {
        user: Address::ZERO,
        holdings: vec![],
        policy: HedgePolicy::default(),
        halt_dir: dir.clone(),
        cover_usdc: None,
        only_listed: false,
    };
    // No coins asked for: the chart starts empty and must grow to fit ETH.
    let mut books = Books::new(&[], 0);
    let r = run_once(&venue, &Mode::DryRun, &input, &mut books)
        .await
        .unwrap();
    assert_eq!(r.coins.len(), 1);
    let c = &r.coins[0];
    assert_eq!(c.coin, "ETH");
    assert_eq!(c.spot_size, Decimal::ZERO);
    assert_eq!(c.target_size, Decimal::ZERO);
    match &c.action {
        Action::WouldTrade {
            size_delta, is_buy, ..
        } => {
            assert_eq!(*size_delta, dec!(0.2), "buy back the short");
            assert!(is_buy);
        }
        other => panic!("expected a close, got {other:?}"),
    }
    assert!(
        matches!(c.verdict, Some(hedge::safety::Verdict::Allow(_))),
        "closing to flat is always allowed: {:?}",
        c.verdict
    );
    assert!(
        c.net.is_some(),
        "the books answer for a coin that was never requested"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// docs/46 (found 2026-10-13): the books must carry the venue's cash. Margin
/// that arrived outside the books is booked as contributed equity on a step
/// that did not trade; on a step that did, the cash left over after marks and
/// funding is realised PnL and fees. Either way margin + marks = account value.
#[tokio::test]
async fn the_books_track_the_venue_cash_and_say_where_it_came_from() {
    use hedge::book::accounts;
    let dir = temp_dir("cash");
    let coins = ["ETH"];
    let margin = |books: &Books| {
        books
            .journal
            .balance(&books.chart, &accounts::hedge_margin())
            .unwrap()
    };

    // Step 1: dry run on an account that already holds 1000 USDC nobody booked.
    let venue = Fake {
        equity: dec!(1000),
        equity_after: None,
        position_before: Decimal::ZERO,
        fill: Decimal::ZERO,
        position_after: Decimal::ZERO,
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let mut books = Books::new(&coins, 0);
    let r = run_once(&venue, &Mode::DryRun, &input(dir.clone()), &mut books)
        .await
        .unwrap();
    assert_eq!(
        r.cash_residual,
        Some(dec!(1000)),
        "the unbooked margin is the residual"
    );
    assert_eq!(
        margin(&books),
        ledger::Money::minor(1_000_000_000, Currency::Usdc)
    );
    assert_eq!(
        books
            .journal
            .balance(&books.chart, &accounts::margin_contributed())
            .unwrap(),
        ledger::Money::minor(1_000_000_000, Currency::Usdc),
        "booked as contributed, not as paid by the treasury"
    );

    // Step 2: live trade; the venue reports 999.5 after fees. The 0.5 is
    // realised loss/fees on margin, and the books agree with the venue again.
    let venue = Fake {
        equity: dec!(1000),
        equity_after: Some(dec!(999.5)),
        position_before: Decimal::ZERO,
        fill: dec!(0.5),
        position_after: dec!(-0.5),
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let signer = PrivateKeySigner::random();
    let r = run_once(&venue, &Mode::Live(signer), &input(dir.clone()), &mut books)
        .await
        .unwrap();
    assert!(matches!(r.coins[0].action, Action::Traded { .. }));
    assert_eq!(r.cash_residual, Some(dec!(-0.5)));
    assert_eq!(
        margin(&books),
        ledger::Money::minor(999_500_000, Currency::Usdc)
    );
    assert_eq!(
        books
            .journal
            .balance(&books.chart, &accounts::hedge_loss())
            .unwrap(),
        ledger::Money::minor(500_000, Currency::Usdc)
    );
    // Position shows zero unrealised PnL in the fake, so margin + mtm = value.
    let mtm = books
        .journal
        .balance(&books.chart, &accounts::hedge_mtm("ETH"))
        .unwrap();
    assert_eq!(
        margin(&books).checked_add(mtm).unwrap(),
        ledger::Money::minor(999_500_000, Currency::Usdc)
    );
    let (d, cr) = books.journal.trial_balance(Currency::Usdc).unwrap();
    assert_eq!(d, cr);

    // Step 3: nothing changed, nothing traded: no residual, no entry.
    let before = books.journal.len();
    let venue = Fake {
        equity: dec!(999.5),
        equity_after: None,
        position_before: dec!(-0.5),
        fill: Decimal::ZERO,
        position_after: dec!(-0.5),
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let r = run_once(&venue, &Mode::DryRun, &input(dir), &mut books)
        .await
        .unwrap();
    assert_eq!(r.cash_residual, None);
    assert_eq!(books.journal.len(), before);
}

#[tokio::test]
async fn a_dry_run_judges_the_next_coin_as_if_the_first_had_filled() {
    // 1,000 USDC of margin, 2x cap = 2,000 of notional for the whole account.
    // ETH 0.5 × 2,500 = 1,250 fits alone; BTC 0.02 × 50,000 = 1,000 fits
    // alone; together they do not. A live step re-reads the account after
    // ETH fills and shrinks BTC to what is left (750); the dry run must print
    // that same plan, not two orders the venue account could not carry.
    let venue = Fake {
        equity: dec!(1000),
        equity_after: None,
        position_before: Decimal::ZERO,
        fill: Decimal::ZERO,
        position_after: Decimal::ZERO,
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let dir = temp_dir("dry-two");
    let mut inp = input(dir);
    inp.holdings.push(Holding {
        coin: "BTC".into(),
        size: dec!(0.02),
    });
    let mut books = Books::new(&["ETH", "BTC"], 0);
    let r = run_once(&venue, &Mode::DryRun, &inp, &mut books)
        .await
        .unwrap();
    let sizes: Vec<Decimal> = r
        .coins
        .iter()
        .map(|c| match &c.action {
            Action::WouldTrade { size_delta, .. } => *size_delta,
            other => panic!("expected WouldTrade, got {other:?}"),
        })
        .collect();
    // 750 of room at mark, less the 50 bps a 1,000 BTC fill may cost the
    // equity it is judged on: (1,000 - 5) × 2 - 1,250 = 740 → 0.0148 BTC.
    assert_eq!(sizes, vec![dec!(-0.5), dec!(-0.0148)]);
    assert!(matches!(
        r.coins[1].verdict,
        Some(Verdict::Reduce {
            why: Shrink::Leverage,
            ..
        })
    ));
}

/// Audit 2026-10-15 (docs/60 P-74): the halt flag was read once per step, so a
/// mismatch on ETH still let BTC trade live in the same step.
#[tokio::test]
async fn a_halt_on_one_coin_stops_the_next_coin_in_the_same_step() {
    let venue = Fake {
        equity: dec!(2000),
        equity_after: None,
        position_before: Decimal::ZERO,
        fill: dec!(0.5),
        // ETH filled 0.5 but reads back 0.3: halt.
        position_after: dec!(-0.3),
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let dir = temp_dir("halt-two");
    let mut inp = input(dir.clone());
    inp.holdings.push(Holding {
        coin: "BTC".into(),
        size: dec!(0.02),
    });
    let mut books = Books::new(&["ETH", "BTC"], 0);
    let r = run_once(
        &venue,
        &Mode::Live(PrivateKeySigner::random()),
        &inp,
        &mut books,
    )
    .await
    .unwrap();
    assert!(matches!(r.coins[0].action, Action::Halted { .. }));
    assert!(
        matches!(&r.coins[1].action, Action::None(s) if s.contains("Halted")),
        "{:?}",
        r.coins[1].action
    );
    assert_eq!(
        venue.orders.lock().unwrap().len(),
        1,
        "no BTC order after the ETH halt"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Audit 2026-10-15 (docs/60 P-74): an error after the order was sent left no
/// HALT, so the next step traded on a position nobody had read back.
#[tokio::test]
async fn an_order_whose_outcome_is_unknown_halts() {
    let venue = Fake {
        equity: dec!(2000),
        equity_after: None,
        position_before: Decimal::ZERO,
        fill: dec!(0.5),
        position_after: dec!(-0.5),
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: true,
    };
    let dir = temp_dir("unknown");
    let mut books = Books::new(&["ETH"], 0);
    let r = run_once(
        &venue,
        &Mode::Live(PrivateKeySigner::random()),
        &input(dir.clone()),
        &mut books,
    )
    .await;
    assert!(r.is_err(), "the step fails");
    assert!(halt::is_halted(&dir), "HALT written");
    let written = std::fs::read_to_string(halt::path(&dir)).unwrap();
    assert!(written.contains("order sent, outcome unknown"), "{written}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_desk_step_never_touches_a_coin_it_did_not_list() {
    // docs/89 M3: the desk offsets PAXG on an account that also holds the
    // treasury's 0.2 ETH short; its step must leave the ETH alone.
    let venue = Fake {
        equity: dec!(2000),
        equity_after: None,
        position_before: dec!(-0.2),
        fill: Decimal::ZERO,
        position_after: Decimal::ZERO,
        orders: Mutex::new(vec![]),
        traded: Mutex::new(false),
        place_fails: false,
    };
    let dir = temp_dir("only-listed");
    let input = StepInput {
        user: Address::ZERO,
        holdings: vec![],
        policy: HedgePolicy::default(),
        halt_dir: dir.clone(),
        cover_usdc: None,
        only_listed: true,
    };
    let mut books = Books::new(&[], 0);
    let r = run_once(&venue, &Mode::DryRun, &input, &mut books)
        .await
        .unwrap();
    assert!(
        r.coins.is_empty(),
        "nothing listed, nothing touched: {:?}",
        r.coins
    );
    assert!(venue.orders.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
