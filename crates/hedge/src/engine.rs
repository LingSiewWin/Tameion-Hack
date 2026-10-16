//! One step of the hedge loop: read, decide, (maybe) trade, read back, book.
//!
//! The order of operations is the safety argument:
//!
//! 1. Read markets, prices and the account from the venue. Book the marks.
//! 2. For each exposure, compute the target and whether to move.
//! 3. Ask the safety engine. It may shrink the order or refuse it.
//! 4. In `Live` mode, place the order, then **read the position back**. If
//!    it is not where the fill said it would be, write `HALT` and stop. In
//!    `DryRun` mode, report what would have been sent.
//! 5. Book funding received or paid since the last step.
//!
//! Nothing here is a model's decision. The reasoner can read the report.
//! Written against [`VenueApi`] so the tests can script a venue and drive
//! the halt path, which no funded account should ever have to.

use crate::book::{self, BookError, NetView};
use crate::delta::{self, Rebalance};
use crate::exposure::SpotExposure;
use crate::halt;
use crate::hl::{self, AccountSnapshot, MarketInfo, VenueApi, VenueError};
use crate::policy::HedgePolicy;
use crate::safety::{self, Limits, Proposed, Verdict};
use hypersdk::Address;
use hypersdk::hypercore::{OrderResponseStatus, PrivateKeySigner};
use ledger::{Chart, Journal};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::PathBuf;

/// Whether orders are sent.
pub enum Mode {
    /// Decide and report; send nothing.
    DryRun,
    /// Send orders signed by this API wallet. It can trade; it cannot
    /// withdraw. It is never the treasury key.
    Live(PrivateKeySigner),
}

/// What the caller wants hedged, in base units. Marks come from the venue.
#[derive(Debug, Clone, PartialEq)]
pub struct Holding {
    pub coin: String,
    pub size: Decimal,
}

/// Everything the step needs that is not the venue.
pub struct StepInput {
    pub user: Address,
    pub holdings: Vec<Holding>,
    pub policy: HedgePolicy,
    pub halt_dir: PathBuf,
    /// USDC of upcoming obligations the hedge must cover. `Some` sizes the
    /// short to the bills, not the whole holding: "protect the next 90 days
    /// of payroll" (`docs/45`, Kyriba's cash-flow exposure in `docs/50`).
    /// `None` hedges the full holding.
    pub cover_usdc: Option<Decimal>,
    /// Trade only the coins in `holdings`. The desk sets it: its step must
    /// never close the treasury's ETH/BTC/SOL hedge on a shared account
    /// (docs/89 M3); the treasury leaves it false, so a coin it no longer
    /// holds is closed as before.
    pub only_listed: bool,
}

/// The books the engine writes into, and the last marks it booked against.
pub struct Books {
    pub chart: Chart,
    pub journal: Journal,
    pub(crate) last_spot_value: HashMap<String, Decimal>,
    pub(crate) last_hedge_upnl: HashMap<String, Decimal>,
    /// Funding is fetched from here (ms); advanced after each step.
    pub funding_cursor_ms: u64,
    /// How many journal entries are already on disk. `store::save` appends
    /// from here, so an entry is written once however often it is saved.
    pub(crate) persisted: usize,
}

impl Books {
    pub fn new(coins: &[&str], funding_cursor_ms: u64) -> Self {
        Books {
            chart: book::chart(coins),
            journal: Journal::new(),
            last_spot_value: HashMap::new(),
            last_hedge_upnl: HashMap::new(),
            funding_cursor_ms,
            persisted: 0,
        }
    }

    /// Entries appended since the books were last saved (or created).
    pub fn unsaved(&self) -> usize {
        self.journal.len().saturating_sub(self.persisted)
    }

    /// Add a coin's accounts to the chart if they are not there yet, so a
    /// position the venue reports in a coin nobody asked about can still be
    /// marked and closed with its books in order.
    pub fn ensure_coin(&mut self, coin: &str) {
        if self.chart.get(&book::accounts::spot(coin)).is_some() {
            return;
        }
        let u = ledger::Currency::Usdc;
        let chart = std::mem::take(&mut self.chart);
        self.chart = chart
            .with(ledger::Account::new(
                book::accounts::spot(coin),
                ledger::AccountKind::Asset,
                u,
            ))
            .with(ledger::Account::new(
                book::accounts::hedge_mtm(coin),
                ledger::AccountKind::Asset,
                u,
            ));
    }
}

/// What happened to one coin in one step.
#[derive(Debug, Clone)]
pub struct CoinReport {
    pub coin: String,
    pub spot_size: Decimal,
    pub mark_px: Decimal,
    pub oracle_px: Decimal,
    pub funding_rate: Decimal,
    pub spot_value: Decimal,
    pub hedge_size_before: Decimal,
    pub hedge_upnl: Decimal,
    pub target_size: Decimal,
    pub rebalance: Rebalance,
    pub verdict: Option<Verdict>,
    pub action: Action,
    pub net: Option<NetView>,
}

/// What the engine did about a coin.
#[derive(Debug, Clone)]
pub enum Action {
    /// Nothing to send. The string says why.
    None(String),
    /// Dry run: this is the order that would have gone out.
    WouldTrade {
        size_delta: Decimal,
        is_buy: bool,
        worst_px: Decimal,
        limits: Limits,
    },
    /// Live: sent, filled, read back, and the read-back matched.
    Traded {
        size_delta: Decimal,
        is_buy: bool,
        worst_px: Decimal,
        statuses: Vec<OrderResponseStatus>,
        filled_size: Decimal,
        read_back: Decimal,
    },
    /// Live: the read-back did not match. `HALT` has been written.
    Halted { reason: String },
}

/// The whole step.
#[derive(Debug, Clone)]
pub struct StepReport {
    pub account: safety::AccountState,
    pub withdrawable: Decimal,
    pub health: Option<Decimal>,
    pub halted_before: bool,
    pub coins: Vec<CoinReport>,
    pub funding_entries_booked: usize,
    pub entries_booked: usize,
    /// Venue cash (account value less unrealised PnL) minus the books' margin
    /// balance before this step's reconciliation entry; `None` when they
    /// already agreed. Booked as realised PnL and fees on a step that traded,
    /// as an external margin movement otherwise (docs/46, found 2026-10-13).
    pub cash_residual: Option<Decimal>,
    /// The cover the targets were sized to, when one was given.
    pub cover_usdc: Option<Decimal>,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Venue(#[from] VenueError),
    #[error(transparent)]
    Book(#[from] BookError),
    #[error("halt file: {0}")]
    Halt(#[from] std::io::Error),
    #[error("books: {0}")]
    Balance(#[from] ledger::BalanceError),
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
}

/// Run one step. See the module docs for the order and why.
pub async fn run_once<V: VenueApi>(
    venue: &V,
    mode: &Mode,
    input: &StepInput,
    books: &mut Books,
) -> Result<StepReport, EngineError> {
    let started = std::time::Instant::now();
    let halted_before = halt::is_halted(&input.halt_dir);
    let markets = venue.markets().await?;
    let prices = venue.prices().await?;
    let mut snapshot = venue.account(input.user).await?;
    let health = safety::health(&snapshot.state, &input.policy);
    let entries_at_start = books.journal.len();

    // A position in a coin the treasury no longer holds is a naked short, not
    // a hedge. It is treated as a holding of zero, so the controller's target
    // is flat and the safety engine -- which always allows closing -- lets
    // the close through. Only for markets the venue still lists.
    let mut holdings = input.holdings.clone();
    for coin in snapshot.positions.keys() {
        if input.only_listed {
            break;
        }
        if !holdings.iter().any(|h| &h.coin == coin) && markets.contains_key(coin) {
            holdings.push(Holding {
                coin: coin.clone(),
                size: Decimal::ZERO,
            });
            books.ensure_coin(coin);
        }
    }

    let mut coins = Vec::with_capacity(holdings.len());
    // A halt written by one coin stops every coin after it in the same step,
    // not only the next step (audit 2026-10-15, docs/60 P-74).
    let mut halted = halted_before;
    // Every order sent in this pass with its read-back, for the pass record.
    let mut orders = Vec::new();
    for h in &holdings {
        let (report, updated) = step_coin(
            venue,
            mode,
            input,
            books,
            &markets,
            &prices,
            &snapshot,
            h,
            halted,
            &mut orders,
        )
        .await?;
        halted = halted || matches!(report.action, Action::Halted { .. });
        // A live trade changed the account; the next coin is judged on the
        // state read back after it, never on the snapshot from before.
        if let Some(fresh) = updated {
            snapshot = fresh;
        } else if let Action::WouldTrade { limits, .. } = &report.action {
            // A dry run judges the next coin as if this order had filled, so
            // the plan it prints is one a live step would be allowed to send
            // (docs/68 part 4: ETH and BTC each passed on the empty account,
            // 2.85x together under a 2x cap).
            let current = (limits.size_after - limits.size_delta).abs() * report.mark_px;
            let s = &mut snapshot.state;
            s.total_notional =
                (s.total_notional - current).max(Decimal::ZERO) + limits.notional_after;
            s.maintenance_margin_used =
                (s.maintenance_margin_used - current * limits.maintenance_rate).max(Decimal::ZERO)
                    + limits.notional_after * limits.maintenance_rate;
        }
        coins.push(report);
    }

    let funding_entries_booked = book_funding(venue, input, books).await?;
    // Funding first (from the venue's own feed), then whatever cash movement
    // is left: realised PnL and fees if something traded, else a deposit or
    // withdrawal these books did not see. After this line the books' margin
    // plus marks equal the venue's account value, and `audit_books` can say so.
    let traded = coins
        .iter()
        .any(|c| matches!(c.action, Action::Traded { .. }));
    let cash_residual = reconcile_cash(&snapshot, books, traded)?;
    // Best effort, never `?`: the orders are sent and the books updated in
    // memory; failing here would skip the caller's `store::save` and lose the
    // pass from the books (docs/103 S3-3). The record is for watching.
    if let Err(e) = crate::store::append_pass(
        &input.halt_dir,
        input.user,
        &serde_json::json!({
            "at": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            "ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "mode": match mode {
                Mode::DryRun => "dry-run",
                Mode::Live(_) => "live",
            },
            "orders": orders,
        }),
    ) {
        eprintln!("hedge: the pass record was not written ({e}); the books are kept");
    }

    Ok(StepReport {
        account: snapshot.state.clone(),
        withdrawable: snapshot.withdrawable,
        health,
        halted_before,
        coins,
        funding_entries_booked,
        entries_booked: books.journal.len() - entries_at_start,
        cash_residual,
        cover_usdc: input.cover_usdc,
    })
}

/// Money as the venue's decimal.
fn dec_of(m: ledger::Money) -> Decimal {
    Decimal::from_i128_with_scale(m.to_minor(), 6)
}

fn reconcile_cash(
    snapshot: &AccountSnapshot,
    books: &mut Books,
    traded: bool,
) -> Result<Option<Decimal>, EngineError> {
    let upnl: Decimal = snapshot.positions.values().map(|p| p.unrealized_pnl).sum();
    let cash_venue = snapshot.state.account_value - upnl;
    let cash_books = dec_of(
        books
            .journal
            .balance(&books.chart, &book::accounts::hedge_margin())?,
    );
    let residual = cash_venue - cash_books;
    if residual.is_zero() {
        return Ok(None);
    }
    let doc_id = format!("cash@{}", snapshot.time_ms);
    let entry = if traded {
        book::settle(residual, &doc_id)?
    } else {
        book::external(residual, &doc_id)?
    };
    match entry {
        Some(e) => {
            books.journal.append(e);
            Ok(Some(residual))
        }
        // Rounded to nothing at six places.
        None => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
async fn step_coin<V: VenueApi>(
    venue: &V,
    mode: &Mode,
    input: &StepInput,
    books: &mut Books,
    markets: &HashMap<String, MarketInfo>,
    prices: &HashMap<String, hl::AssetCtx>,
    snapshot: &AccountSnapshot,
    h: &Holding,
    halted: bool,
    orders: &mut Vec<serde_json::Value>,
) -> Result<(CoinReport, Option<AccountSnapshot>), EngineError> {
    let coin = h.coin.as_str();
    let market = markets
        .get(coin)
        .ok_or_else(|| VenueError::UnknownMarket(coin.to_string()))?;
    let ctx = prices
        .get(coin)
        .ok_or_else(|| VenueError::UnknownMarket(coin.to_string()))?;

    // 1. Marks. Book the change in spot value and in hedge unrealised PnL.
    let exposure = SpotExposure::new(coin, h.size, ctx.mark_px);
    let spot_value = exposure.notional().unwrap_or(Decimal::ZERO);
    let last_spot = books
        .last_spot_value
        .get(coin)
        .copied()
        .unwrap_or(Decimal::ZERO);
    if let Some(e) = book::mark_spot(
        coin,
        spot_value - last_spot,
        &mark_id(coin, snapshot.time_ms),
    )? {
        books.journal.append(e);
    }
    books.last_spot_value.insert(coin.to_string(), spot_value);

    let hedge_before = snapshot.size(coin);
    let hedge_upnl = snapshot
        .positions
        .get(coin)
        .map_or(Decimal::ZERO, |p| p.unrealized_pnl);
    let last_upnl = books
        .last_hedge_upnl
        .get(coin)
        .copied()
        .unwrap_or(Decimal::ZERO);
    if let Some(e) = book::mark_hedge(
        coin,
        hedge_upnl - last_upnl,
        &mark_id(coin, snapshot.time_ms),
    )? {
        books.journal.append(e);
    }
    books.last_hedge_upnl.insert(coin.to_string(), hedge_upnl);

    // 2. Target and drift. With a cover, the short is only as large as the
    // bills it protects, at today's mark.
    let target = delta::target_hedge_covering(&exposure, input.cover_usdc);
    let rebalance = delta::rebalance(hedge_before, target.size, input.policy.band_bps);

    let mut report = CoinReport {
        coin: coin.to_string(),
        spot_size: h.size,
        mark_px: ctx.mark_px,
        oracle_px: ctx.oracle_px,
        funding_rate: ctx.funding,
        spot_value,
        hedge_size_before: hedge_before,
        hedge_upnl,
        target_size: target.size,
        rebalance: rebalance.clone(),
        verdict: None,
        action: Action::None("inside band".into()),
        net: None,
    };

    let Rebalance::Trade { size_delta, .. } = rebalance else {
        report.net = Some(book::net_view(&books.journal, &books.chart, coin)?);
        return Ok((report, None));
    };

    // 3. Safety. Round the requested size to the venue tick first so the
    // engine judges the order that would actually be sent.
    let size_delta = hl::round_size(market, size_delta);
    if size_delta.is_zero() {
        report.action = Action::None("size delta rounds to zero at venue tick".into());
        report.net = Some(book::net_view(&books.journal, &books.chart, coin)?);
        return Ok((report, None));
    }
    let tiers = venue.tiers(ctx.margin_table_id).await?;
    let proposed = Proposed {
        coin: coin.to_string(),
        current_size: hedge_before,
        size_delta,
        px: ctx.mark_px,
    };
    // A fill up to `slippage_bps` off the mark costs that much equity as it
    // lands; judged on equity at mark, a 2x cap ended at 2.02x after a 50 bps
    // fill. An order that grows the position is judged on what is left after
    // the worst fill; one that only shrinks it lowers both limits anyway.
    let mut judged = snapshot.state.clone();
    if !safety::reduces_only(&proposed) {
        judged.account_value -=
            size_delta.abs() * ctx.mark_px * Decimal::from(input.policy.slippage_bps)
                / Decimal::from(10_000u32);
    }
    let verdict = safety::check(
        &proposed,
        &judged,
        &tiers,
        ctx.max_leverage,
        &input.policy,
        halted,
    );
    report.verdict = Some(verdict.clone());

    let limits = match verdict {
        Verdict::Allow(l) => l,
        Verdict::Reduce { limits, .. } => limits,
        Verdict::Refuse(r) => {
            report.action = Action::None(format!("refused: {r:?}"));
            report.net = Some(book::net_view(&books.journal, &books.chart, coin)?);
            return Ok((report, None));
        }
    };
    let send_delta = hl::round_size(market, limits.size_delta);
    if send_delta.is_zero() {
        report.action = Action::None("safe size rounds to zero at venue tick".into());
        report.net = Some(book::net_view(&books.journal, &books.chart, coin)?);
        return Ok((report, None));
    }
    let is_buy = send_delta.is_sign_positive();
    let worst_px = hl::worst_price(ctx.mark_px, is_buy, input.policy.slippage_bps);

    // 4. Send, or say what would be sent.
    let mut updated: Option<AccountSnapshot> = None;
    report.action = match mode {
        Mode::DryRun => Action::WouldTrade {
            size_delta: send_delta,
            is_buy,
            worst_px,
            limits,
        },
        Mode::Live(signer) => {
            venue
                .set_leverage(signer, market, input.policy.max_leverage)
                .await?;
            // From here the order may have reached the venue. An error leaves
            // the position unknown, so HALT is written before returning: no
            // later step trades on a book nobody read back.
            let sent = async {
                let statuses = venue
                    .place_market(signer, market, is_buy, send_delta.abs(), worst_px)
                    .await?;
                let after = venue.account(input.user).await?;
                Ok::<_, VenueError>((statuses, after))
            }
            .await;
            let (statuses, after) = match sent {
                Ok(v) => v,
                Err(e) => {
                    halt::halt(
                        &input.halt_dir,
                        &format!("{coin}: order sent, outcome unknown: {e}"),
                    )?;
                    return Err(e.into());
                }
            };
            let filled = filled_size(&statuses);
            let signed_fill = if is_buy { filled } else { -filled };
            let expected = hedge_before + signed_fill;
            updated = Some(after.clone());
            let read_back = after.size(coin);
            // The order and what the venue showed after it, kept for the pass
            // record whatever the outcome (the HALT below, if any, comes first
            // in effect: nothing here can fail).
            orders.push(serde_json::json!({
                "at": after.time_ms / 1000,
                "coin": coin,
                "side": if is_buy { "buy" } else { "sell" },
                "size": send_delta.abs().normalize().to_string(),
                "oid": statuses.iter().find_map(|s| match s {
                    OrderResponseStatus::Filled { oid, .. }
                    | OrderResponseStatus::Resting { oid, .. } => Some(*oid),
                    _ => None,
                }),
                "readBack": if read_back == expected { "match" } else { "mismatch" },
                "expected": expected.normalize().to_string(),
                "read": read_back.normalize().to_string(),
            }));
            if read_back != expected {
                let reason = format!(
                    "{coin}: read back {read_back}, expected {expected} (before {hedge_before}, fill {signed_fill}); statuses {statuses:?}"
                );
                halt::halt(&input.halt_dir, &reason)?;
                Action::Halted { reason }
            } else {
                // Re-mark on the account read back after the fill, so the
                // books carry the post-trade unrealised PnL and the cash
                // reconciliation below sees only realised PnL and fees.
                let upnl_after = after
                    .positions
                    .get(coin)
                    .map_or(Decimal::ZERO, |p| p.unrealized_pnl);
                let prev = books
                    .last_hedge_upnl
                    .get(coin)
                    .copied()
                    .unwrap_or(Decimal::ZERO);
                let doc_id = format!("{coin}@{}/fill", after.time_ms);
                if let Some(e) = book::mark_hedge(coin, upnl_after - prev, &doc_id)? {
                    books.journal.append(e);
                }
                books.last_hedge_upnl.insert(coin.to_string(), upnl_after);
                Action::Traded {
                    size_delta: send_delta,
                    is_buy,
                    worst_px,
                    statuses,
                    filled_size: signed_fill,
                    read_back,
                }
            }
        }
    };
    report.net = Some(book::net_view(&books.journal, &books.chart, coin)?);
    Ok((report, updated))
}

/// Sum of filled sizes in a batch reply. Resting orders contribute nothing:
/// a hedge that rests is not a hedge yet.
pub fn filled_size(statuses: &[OrderResponseStatus]) -> Decimal {
    statuses
        .iter()
        .map(|s| match s {
            OrderResponseStatus::Filled { total_sz, .. } => *total_sz,
            _ => Decimal::ZERO,
        })
        .sum()
}

async fn book_funding<V: VenueApi>(
    venue: &V,
    input: &StepInput,
    books: &mut Books,
) -> Result<usize, EngineError> {
    let entries = venue
        .funding_since(input.user, books.funding_cursor_ms)
        .await?;
    let mut booked = 0;
    let mut newest = books.funding_cursor_ms;
    for f in entries {
        // Funding lands on the shared margin account whatever the coin, so a
        // payment on a position we did not open is still money and still
        // booked, named by the venue's hash.
        // Named by coin and time, not by the venue hash: testnet funding
        // entries carry an all-zero hash, and a document id must point at
        // one payment.
        let doc_id = format!("{}@{}", f.delta.coin, f.time);
        if let Some(e) = book::funding(&f.delta.coin, f.delta.usdc, &doc_id)? {
            books.journal.append(e);
            booked += 1;
        }
        newest = newest.max(f.time.saturating_add(1));
    }
    // Advance only past what was seen. With no entries the cursor stays put,
    // so a payment that lands between two steps is never skipped.
    books.funding_cursor_ms = newest;
    Ok(booked)
}

fn mark_id(coin: &str, time_ms: u64) -> String {
    format!("{coin}@{time_ms}")
}
