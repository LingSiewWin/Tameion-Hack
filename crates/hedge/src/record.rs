//! Turning a step into the record: decision-log events the operator anchors,
//! and the JSON the dashboard renders.
//!
//! One `EventBody::Hedge` per coin, whether or not an order went out. A
//! refusal is a decision; a hold is a decision. The note is the sentence a
//! reviewer reads; every number in it is also a typed field on the event.

use crate::book::{self, BookError};
use crate::delta::Rebalance;
use crate::engine::{Action, Books, CoinReport, StepReport};
use crate::safety::Verdict;
use hypersdk::hypercore::OrderResponseStatus;
use ledger::{Currency, EventBody};
use rust_decimal::Decimal;
use serde_json::{Value, json};

/// The outcome word for a coin's action. Stable strings: the log is read by
/// people and by tests, so these do not change casually.
pub fn outcome(c: &CoinReport) -> &'static str {
    match &c.action {
        Action::None(reason) if reason.starts_with("refused") => "REFUSE",
        Action::None(_) => "HOLD",
        Action::WouldTrade { .. } => "DRY-RUN",
        Action::Traded { .. } => "TRADED",
        Action::Halted { .. } => "HALTED",
    }
}

fn dec(d: Decimal) -> String {
    d.normalize().to_string()
}

fn first_oid(statuses: &[OrderResponseStatus]) -> Option<String> {
    statuses.iter().find_map(|s| match s {
        OrderResponseStatus::Resting { oid, .. } | OrderResponseStatus::Filled { oid, .. } => {
            Some(oid.to_string())
        }
        _ => None,
    })
}

/// Signed size change and resulting size for the event.
fn sizes(c: &CoinReport) -> (Decimal, Decimal, Option<String>) {
    match &c.action {
        Action::WouldTrade { size_delta, .. } => {
            (*size_delta, c.hedge_size_before + *size_delta, None)
        }
        Action::Traded {
            filled_size,
            read_back,
            statuses,
            ..
        } => (*filled_size, *read_back, first_oid(statuses)),
        Action::None(_) | Action::Halted { .. } => (Decimal::ZERO, c.hedge_size_before, None),
    }
}

/// One anchored event per coin.
pub fn events_for(report: &StepReport) -> Result<Vec<EventBody>, BookError> {
    report
        .coins
        .iter()
        .map(|c| {
            let (size_delta, size_after, oid) = sizes(c);
            let notional_after = book::usdc(size_after.abs() * c.mark_px)?;
            Ok(EventBody::Hedge {
                coin: c.coin.clone(),
                outcome: outcome(c).to_string(),
                size_before: dec(c.hedge_size_before),
                size_delta: dec(size_delta),
                size_after: dec(size_after),
                mark_px: dec(c.mark_px),
                notional_after,
                oid,
                note: note(c, report),
            })
        })
        .collect()
}

/// The sentence a reviewer reads for one coin.
pub fn note(c: &CoinReport, report: &StepReport) -> String {
    let drift = match &c.rebalance {
        Rebalance::Hold { drift_bps } => format!("drift {drift_bps} bps, inside band"),
        Rebalance::Trade { drift_bps, .. } => format!("drift {drift_bps} bps, outside band"),
    };
    let verdict = match &c.verdict {
        None => String::new(),
        Some(Verdict::Allow(l)) => format!(
            "; allowed: leverage after {} , required equity {}",
            dec(l.leverage_after),
            dec(l.required_equity)
        ),
        Some(Verdict::Reduce { why, limits }) => {
            format!("; reduced ({why:?}) to delta {}", dec(limits.size_delta))
        }
        Some(Verdict::Refuse(r)) => format!("; refused: {r:?}"),
    };
    let action = match &c.action {
        // A refusal is already in the verdict text; do not say it twice.
        Action::None(r) if r.starts_with("refused") && c.verdict.is_some() => {
            "no order".to_string()
        }
        Action::None(r) => r.clone(),
        Action::WouldTrade {
            size_delta,
            worst_px,
            ..
        } => format!(
            "would send {} at worst {}",
            dec(*size_delta),
            dec(*worst_px)
        ),
        Action::Traded {
            filled_size,
            read_back,
            worst_px,
            ..
        } => format!(
            "sent at worst {}, filled {}, read back {}",
            dec(*worst_px),
            dec(*filled_size),
            dec(*read_back)
        ),
        Action::Halted { reason } => format!("HALTED: {reason}"),
    };
    format!(
        "{}: spot {} @ {} = {} USDC; hedge {} -> target {}; {drift}{verdict}; {action}; equity {}",
        c.coin,
        dec(c.spot_size),
        dec(c.mark_px),
        dec(c.spot_value),
        dec(c.hedge_size_before),
        dec(c.target_size),
        dec(report.account.account_value)
    )
}

/// What the dashboard renders. `mode` is `"dry-run"` or `"live"`.
pub fn json(report: &StepReport, books: &Books, mode: &str) -> Value {
    let (d, cr) = books
        .journal
        .trial_balance(Currency::Usdc)
        .map(|(d, c)| (d.to_string(), c.to_string()))
        .unwrap_or_else(|e| (e.to_string(), String::new()));
    json!({
        "mode": mode,
        "haltedBefore": report.halted_before,
        "coverUsdc": report.cover_usdc.map(dec),
        "account": {
            "accountValue": dec(report.account.account_value),
            "marginUsed": dec(report.account.margin_used),
            "maintenanceMarginUsed": dec(report.account.maintenance_margin_used),
            "withdrawable": dec(report.withdrawable),
        },
        "health": report.health.map(dec),
        "coins": report.coins.iter().map(|c| {
            let (size_delta, size_after, oid) = sizes(c);
            json!({
                "coin": c.coin,
                "spotSize": dec(c.spot_size),
                "markPx": dec(c.mark_px),
                "oraclePx": dec(c.oracle_px),
                "fundingRate": dec(c.funding_rate),
                "spotValue": dec(c.spot_value),
                "hedgeBefore": dec(c.hedge_size_before),
                "hedgeUpnl": dec(c.hedge_upnl),
                "target": dec(c.target_size),
                "outcome": outcome(c),
                "sizeDelta": dec(size_delta),
                "sizeAfter": dec(size_after),
                "oid": oid,
                "reason": note(c, report),
                "net": c.net.as_ref().map(|n| json!({
                    "spot": n.spot.to_string(),
                    "hedge": n.hedge.to_string(),
                    "margin": n.margin.to_string(),
                    "net": n.net.to_string(),
                })),
            })
        }).collect::<Vec<_>>(),
        "entries": books.journal.len(),
        "fundingEntries": report.funding_entries_booked,
        "cashResidual": report.cash_residual.map(dec),
        "trialBalance": if d == cr { format!("{d} = {cr}") } else { format!("UNBALANCED {d} vs {cr}") },
    })
}
