//! How fast the account spends, how long until the floor at that pace, and
//! what moved the pace (docs/117 §1).
//!
//! Double entry conserves balances; Ijiri's momentum accounting conserves
//! flows: the rate of spend is a quantity the book carries, and each event
//! that changes it is recorded with the change it made (Y. Ijiri, "A
//! Framework for Triple-Entry Bookkeeping", *The Accounting Review* 61(4),
//! 1986, 745–759; "Momentum Accounting and Triple-Entry Bookkeeping", AAA
//! Studies in Accounting Research 31, 1989). Here, one fold:
//!
//! - **rate**: the settlements in a fixed trailing window, divided by the
//!   window's days. Only settled payments of the record count, never a
//!   forecast, so a wrong bill cannot move the number the owner sees.
//! - **runway by rate**: what the account holds above its floor, divided by
//!   the rate. `None` while nothing was spent in the window.
//! - **first shortfall**: the bill walk. Every bill's due dates in the next
//!   [`HORIZON_DAYS`], at its cadence, in date order, until the money above
//!   the floor runs out. The one implementation the decider, the inbox and
//!   the watch report all call.
//! - **forces**: the events that changed the rate, read from typed fields
//!   only: a refusal, a payment held for the owner by the mode or a cap, a
//!   payee paid for the first time, a payee no longer paid.
//! - **flow check**: per period, `start + inflows − outflows = end` by the
//!   book, against the balance the chain holds. The difference is recorded,
//!   not hidden.
//!
//! Pure: the operator reads the record and the chain and passes them in.

use crate::log::{Event, EventBody};
use crate::money::{Currency, Money};
use crate::obligations::Obligation;
use crate::verify::Decision;
use std::collections::{BTreeMap, BTreeSet};

pub const DAY: i64 = 86_400;

/// How far ahead the bill walk looks.
pub const HORIZON_DAYS: i64 = 90;

/// The shortest cadence the walk repeats a bill at. A bill typed or found
/// with a shorter interval is walked once a minute, so a 90-day horizon is
/// at most 129,600 dates per bill.
pub const MIN_STEP_SECS: i64 = 60;

/// A bill as the walk needs it: when it is next due, how often, how much.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bill {
    pub next_due: i64,
    /// 0 for a bill that falls once.
    pub every_secs: i64,
    pub amount_minor: i128,
}

impl From<&Obligation> for Bill {
    fn from(o: &Obligation) -> Self {
        Bill {
            next_due: o.next_due.as_unix(),
            every_secs: o.interval.as_i64(),
            amount_minor: o.typical_amount.to_minor(),
        }
    }
}

/// What moved the rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ForceKind {
    /// A payee paid for the first time in the record, inside the window.
    BillAdded,
    /// A payee paid in the window before this one and not in this one.
    BillRemoved,
    /// A payment held for the owner because the policy is not on Autopilot.
    ModeChanged,
    /// A payment held for the owner because a cap would not take it.
    CapChanged,
    /// A payment the agent refused.
    Refused,
}

impl ForceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ForceKind::BillAdded => "bill_added",
            ForceKind::BillRemoved => "bill_removed",
            ForceKind::ModeChanged => "mode_changed",
            ForceKind::CapChanged => "cap_changed",
            ForceKind::Refused => "refused",
        }
    }
}

/// One event that changed the rate, and by how much per day: what it added
/// to or kept out of the window's spend, divided by the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Force {
    pub at: i64,
    pub kind: ForceKind,
    pub counterparty: String,
    pub delta_rate_per_day: Money,
}

/// The fold's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Momentum {
    pub rate_per_day: Money,
    pub window_days: u32,
    pub runway_by_rate_days: Option<i64>,
    pub first_shortfall_days: Option<i64>,
    /// Oldest first, one per (kind, payee): the latest time it happened.
    pub forces: Vec<Force>,
}

/// Every due date of `bills` in `now ..= now + horizon_secs`, at each bill's
/// cadence, as `(at, amount, bill index)` sorted by date then index. A bill a
/// full interval or more overdue may have ended (nobody paid the next one
/// either), so it counts once, now: projecting it at its cadence told a
/// client with 0.35 USDC of bills that the account needed 15,084 USDC more
/// (docs/70 C8).
pub fn due_dates(bills: &[Bill], now: i64, horizon_secs: i64) -> Vec<(i64, i128, usize)> {
    let end = now.saturating_add(horizon_secs);
    let mut out = Vec::new();
    for (i, b) in bills.iter().enumerate() {
        if b.next_due > end {
            continue;
        }
        let step = if b.every_secs > 0 {
            b.every_secs.max(MIN_STEP_SECS)
        } else {
            0
        };
        if step == 0 || now.saturating_sub(b.next_due) >= step {
            out.push((b.next_due, b.amount_minor, i));
            continue;
        }
        let mut t = b.next_due;
        while t <= end {
            out.push((t, b.amount_minor, i));
            t += step;
        }
    }
    out.sort_by_key(|&(t, _, i)| (t, i));
    out
}

/// The first dated bill `available_minor` (the balance above the floor)
/// cannot cover, walking `dated` in order: whole days until it (toward zero,
/// never negative: a minute overdue is due today) and its position in
/// `dated`. `None` when it covers them all.
pub fn first_shortfall(
    dated: &[(i64, i128, usize)],
    available_minor: i128,
    now: i64,
) -> Option<(i64, usize)> {
    let mut left = available_minor;
    for (pos, &(at, amount, _)) in dated.iter().enumerate() {
        left -= amount;
        if left < 0 {
            return Some((((at - now) / DAY).max(0), pos));
        }
    }
    None
}

/// Days until the first bill in the horizon the money above the floor cannot
/// cover; `None` when it covers every one.
pub fn first_shortfall_days(bills: &[Bill], available_minor: i128, now: i64) -> Option<i64> {
    first_shortfall(
        &due_dates(bills, now, HORIZON_DAYS * DAY),
        available_minor,
        now,
    )
    .map(|(d, _)| d)
}

/// The settlements of `events` in `(from, to]`, by payee, in minor units of
/// `ccy` (another currency is not this account's spend).
fn spend_by_payee(events: &[Event], from: i64, to: i64, ccy: Currency) -> BTreeMap<String, i128> {
    let mut out = BTreeMap::new();
    for e in events {
        let at = e.at.as_unix();
        if let EventBody::Settlement {
            counterparty,
            amount,
            ..
        } = &e.body
            && at > from
            && at <= to
            && amount.currency() == ccy
        {
            *out.entry(counterparty.as_str().to_lowercase()).or_insert(0) += amount.to_minor();
        }
    }
    out
}

/// Fold the record into the account's momentum. `events` is the anchored
/// record (any order); `bills` what the account owes ahead; `window_days`
/// the server's window (at least 1).
pub fn fold(
    events: &[Event],
    bills: &[Bill],
    balance_now: Money,
    floor: Money,
    now: i64,
    window_days: u32,
) -> Momentum {
    let ccy = balance_now.currency();
    let w = i64::from(window_days.max(1));
    let from = now - w * DAY;
    let spent: i128 = spend_by_payee(events, from, now, ccy).values().sum();
    let rate = spent / i128::from(w);
    let available = balance_now.to_minor() - floor.to_minor();
    Momentum {
        rate_per_day: Money::minor(rate, ccy),
        window_days: w as u32,
        runway_by_rate_days: (rate > 0)
            .then(|| i64::try_from(available.max(0) / rate).unwrap_or(i64::MAX)),
        first_shortfall_days: first_shortfall_days(bills, available, now),
        forces: forces(events, from, now, w, ccy),
    }
}

/// The events in `(from, now]` that moved the rate. Escalation reasons are
/// written by code with a fixed prefix (`mode: …`, the verifier's check names
/// `per-action-cap: …` / `daily-cap: …`), never by the model.
fn forces(events: &[Event], from: i64, now: i64, w: i64, ccy: Currency) -> Vec<Force> {
    let per_day = |minor: i128| Money::minor(minor / i128::from(w), ccy);
    let mut latest: BTreeMap<(ForceKind, String), Force> = BTreeMap::new();
    let mut put = |f: Force| {
        let key = (f.kind, f.counterparty.clone());
        if latest.get(&key).is_none_or(|old| old.at <= f.at) {
            latest.insert(key, f);
        }
    };
    let now_spend = spend_by_payee(events, from, now, ccy);
    let before_spend = spend_by_payee(events, from - w * DAY, from, ccy);
    let mut first_paid: BTreeMap<String, i64> = BTreeMap::new();
    for e in events {
        if let EventBody::Settlement { counterparty, .. } = &e.body {
            let k = counterparty.as_str().to_lowercase();
            let at = e.at.as_unix();
            first_paid
                .entry(k)
                .and_modify(|t| *t = (*t).min(at))
                .or_insert(at);
        }
    }
    for (who, &minor) in &now_spend {
        let first = first_paid[who];
        if first > from {
            put(Force {
                at: first,
                kind: ForceKind::BillAdded,
                counterparty: who.clone(),
                delta_rate_per_day: per_day(minor),
            });
        }
    }
    for (who, &minor) in &before_spend {
        if !now_spend.contains_key(who) {
            put(Force {
                at: from,
                kind: ForceKind::BillRemoved,
                counterparty: who.clone(),
                delta_rate_per_day: per_day(-minor),
            });
        }
    }
    for e in events {
        let at = e.at.as_unix();
        if at <= from || at > now {
            continue;
        }
        let (kind, who, amount) = match &e.body {
            EventBody::Decision {
                counterparty,
                amount,
                decision: Decision::Block,
                ..
            } => (ForceKind::Refused, counterparty, amount),
            EventBody::Escalation {
                counterparty,
                amount,
                reason,
                ..
            } if reason.starts_with("mode:") => (ForceKind::ModeChanged, counterparty, amount),
            EventBody::Escalation {
                counterparty,
                amount,
                reason,
                ..
            } if reason.contains("per-action-cap") || reason.contains("daily-cap") => {
                (ForceKind::CapChanged, counterparty, amount)
            }
            _ => continue,
        };
        if amount.currency() != ccy {
            continue;
        }
        put(Force {
            at,
            kind,
            counterparty: who.as_str().to_lowercase(),
            delta_rate_per_day: per_day(-amount.to_minor()),
        });
    }
    let mut out: Vec<Force> = latest.into_values().collect();
    out.sort_by(|a, b| (a.at, a.kind, &a.counterparty).cmp(&(b.at, b.kind, &b.counterparty)));
    out
}

/// How much to move in from the reserve when, at this burn, the floor is
/// nearer than `line_days`: what restores `line_days` of runway at the rate.
/// `None` while the runway by rate holds the line, or nothing is spent.
pub fn reserve_move(m: &Momentum, balance: Money, floor: Money, line_days: i64) -> Option<Money> {
    let days = m.runway_by_rate_days?;
    let above = (balance.to_minor() - floor.to_minor()).max(0);
    let need = m.rate_per_day.to_minor() * i128::from(line_days) - above;
    (days < line_days && need > 0).then(|| Money::minor(need, m.rate_per_day.currency()))
}

/// The flow check for `[period_start, period_end)`: the book's balance at the
/// start (the last check's chain balance, or zero for an account's first
/// check, which starts when the account was made), plus what came in (the
/// chain's transfers into the account, passed in: the record holds no
/// deposits), minus what the book says went out (its settlements), against
/// what the chain holds at the end. A settlement at `period_end` belongs to
/// the next period: a pass checks before it pays, and its payments carry the
/// pass's own time.
pub fn flow_check(
    events: &[Event],
    period_start: i64,
    period_end: i64,
    inflows: Money,
    balance_end_chain: Money,
) -> EventBody {
    let ccy = balance_end_chain.currency();
    let balance_start = events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::FlowCheck {
                period_end: end,
                balance_end_chain: b,
                ..
            } if *end <= period_start && b.currency() == ccy => Some((*end, *b)),
            _ => None,
        })
        .max_by_key(|(end, _)| *end)
        .map_or(Money::zero(ccy), |(_, b)| b);
    let outflows: i128 = events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Settlement { amount, .. }
                if e.at.as_unix() >= period_start
                    && e.at.as_unix() < period_end
                    && amount.currency() == ccy =>
            {
                Some(amount.to_minor())
            }
            _ => None,
        })
        .sum();
    let book = balance_start.to_minor() + inflows.to_minor() - outflows;
    let diff = balance_end_chain.to_minor() - book;
    let m = |minor: i128| Money::minor(minor, ccy);
    let note = format!(
        "Book: {balance_start} at the start + {} in - {} out = {}; the chain holds {balance_end_chain}; difference {}.{}",
        m(inflows.to_minor()),
        m(outflows),
        m(book),
        m(diff),
        match diff.signum() {
            0 => "",
            -1 => {
                " The chain moved more out than the book: a payment or withdrawal the owner signed, or one the agent did not record."
            }
            _ =>
                " The chain holds more than the book: money that arrived after the period's last block, or a recorded payment that never settled.",
        }
    );
    EventBody::FlowCheck {
        period_start,
        period_end,
        balance_start,
        inflows: m(inflows.to_minor()),
        outflows: m(outflows),
        balance_end_book: m(book),
        balance_end_chain,
        diff: m(diff),
        note,
    }
}

/// The events of several passes' records as one book, from their canonical
/// bytes (the form an anchored record is kept in, read back by
/// [`Event::decode`]): each event once (a purchase whose first record failed
/// to anchor is recorded again by the next pass), oldest first. Bytes that do
/// not decode are left out: the fold counts nothing it cannot vouch for.
pub fn book(records: impl IntoIterator<Item = Vec<u8>>) -> Vec<Event> {
    let mut seen = BTreeSet::new();
    let mut out: Vec<Event> = records
        .into_iter()
        .filter_map(|c| Event::decode(&c))
        .filter(|e| {
            seen.insert(
                Event {
                    seq: 0,
                    ..e.clone()
                }
                .canonical(),
            )
        })
        .collect();
    out.sort_by_key(|e| e.at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::ActionHash;
    use crate::obligations::{Confidence, Counterparty};
    use crate::time::Timestamp;

    fn usdc(minor: i128) -> Money {
        Money::minor(minor, Currency::Usdc)
    }

    fn paid(at: i64, who: &str, minor: i128) -> Event {
        Event {
            seq: 0,
            at: Timestamp::from_unix(at),
            body: EventBody::Settlement {
                counterparty: Counterparty::new(who),
                cycle_key: format!("{who}@{at}"),
                amount: usdc(minor),
                tx_hash: format!("0x{at:064x}"),
                action_hash: ActionHash::from_bytes([1; 32]),
                settlement_key: ActionHash::from_bytes([2; 32]),
                note: "paid".into(),
            },
        }
    }

    /// Moved from `decide::runway_days` with its test: walking each bill once
    /// is the walk over bills that fall once in the horizon.
    #[test]
    fn runway_is_the_first_bill_the_balance_cannot_cover() {
        let now = 1_000_000;
        let bill = |due_in_days: i64, minor: i128| Bill {
            next_due: now + due_in_days * DAY,
            every_secs: 0,
            amount_minor: minor,
        };
        let bills = [bill(12, 300), bill(2, 500)];
        assert_eq!(first_shortfall_days(&bills, 800, now), None);
        assert_eq!(first_shortfall_days(&bills, 799, now), Some(12));
        assert_eq!(first_shortfall_days(&bills, 100, now), Some(2));
        // At its cadence: 100 every 10 days from day 5 is 9 dates in 90 days.
        let weekly = [Bill {
            next_due: now + 5 * DAY,
            every_secs: 10 * DAY,
            amount_minor: 100,
        }];
        assert_eq!(due_dates(&weekly, now, HORIZON_DAYS * DAY).len(), 9);
        assert_eq!(first_shortfall_days(&weekly, 250, now), Some(25));
        // A full interval overdue: may have ended, counted once.
        let ended = [Bill {
            next_due: now - 11 * DAY,
            every_secs: 10 * DAY,
            amount_minor: 100,
        }];
        assert_eq!(due_dates(&ended, now, HORIZON_DAYS * DAY).len(), 1);
    }

    #[test]
    fn rate_runway_and_forces_from_the_record() {
        let now = 100 * DAY;
        let events = vec![
            paid(now - 20 * DAY, "0xold", 9_000_000),
            paid(now - 3 * DAY, "0xaa", 7_000_000),
            paid(now - DAY, "0xbb", 14_000_000),
            Event {
                seq: 0,
                at: Timestamp::from_unix(now - 3600),
                body: EventBody::Escalation {
                    counterparty: Counterparty::new("0xCC"),
                    amount: usdc(7_000_000),
                    reason: "mode: policy is in Propose, not Autopilot".into(),
                    note: String::new(),
                },
            },
        ];
        let m = fold(&events, &[], usdc(100_000_000), usdc(16_000_000), now, 7);
        // 21 USDC in the last 7 days: 3 a day; (100 - 16) / 3 = 28 days.
        assert_eq!(m.rate_per_day, usdc(3_000_000));
        assert_eq!(m.runway_by_rate_days, Some(28));
        assert_eq!(m.first_shortfall_days, None);
        let kinds: Vec<_> = m
            .forces
            .iter()
            .map(|f| (f.kind, f.counterparty.as_str()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (ForceKind::BillAdded, "0xaa"),
                (ForceKind::BillAdded, "0xbb"),
                (ForceKind::ModeChanged, "0xcc"),
            ]
        );
        assert_eq!(m.forces[2].delta_rate_per_day, usdc(-1_000_000));
        // Nothing spent in the window: no rate, no runway by rate.
        let quiet = fold(&events[..1], &[], usdc(5), usdc(0), now, 7);
        assert_eq!(
            (quiet.rate_per_day, quiet.runway_by_rate_days),
            (usdc(0), None)
        );
        // The payee paid before the window and not in it left the rate.
        assert_eq!(quiet.forces.len(), 0);
        let later = fold(&events[..1], &[], usdc(5), usdc(0), now - 15 * DAY, 7);
        assert_eq!(later.forces[0].kind, ForceKind::BillAdded);
        let gone = fold(&events[..1], &[], usdc(5), usdc(0), now - 7 * DAY, 7);
        assert_eq!(
            (gone.forces[0].kind, gone.forces[0].delta_rate_per_day),
            (ForceKind::BillRemoved, usdc(-9_000_000 / 7))
        );
    }

    #[test]
    fn the_reserve_move_restores_the_line() {
        let now = 30 * DAY;
        let events = [paid(now - DAY, "0xaa", 7_000_000)];
        // 1 USDC a day; 5 above the floor: 5 days, under a 14-day line by 9.
        let m = fold(&events, &[], usdc(6_000_000), usdc(1_000_000), now, 7);
        assert_eq!(m.runway_by_rate_days, Some(5));
        assert_eq!(
            reserve_move(&m, usdc(6_000_000), usdc(1_000_000), 14),
            Some(usdc(9_000_000))
        );
        assert_eq!(reserve_move(&m, usdc(6_000_000), usdc(1_000_000), 5), None);
        let quiet = fold(&[], &[], usdc(1), usdc(0), now, 7);
        assert_eq!(reserve_move(&quiet, usdc(1), usdc(0), 14), None);
    }

    #[test]
    fn a_flow_check_reads_back_and_starts_where_the_last_one_ended() {
        let first = flow_check(&[paid(50, "0xaa", 3)], 0, 100, usdc(10), usdc(7));
        assert!(
            matches!(&first, EventBody::FlowCheck { diff, balance_end_book, .. } if *diff == usdc(0) && *balance_end_book == usdc(7))
        );
        let mut events = vec![Event {
            seq: 4,
            at: Timestamp::from_unix(100),
            body: first,
        }];
        // A payment at the period's end belongs to the next period.
        events.push(paid(100, "0xaa", 2));
        events.push(paid(160, "0xbb", 1));
        let second = flow_check(&events, 100, 200, usdc(0), usdc(3));
        let EventBody::FlowCheck {
            balance_start,
            outflows,
            diff,
            ..
        } = &second
        else {
            panic!()
        };
        assert_eq!(
            (*balance_start, *outflows, *diff),
            (usdc(7), usdc(3), usdc(-1))
        );
        // Every kind the fold reads comes back from its canonical bytes.
        events.push(Event {
            seq: 7,
            at: Timestamp::from_unix(200),
            body: second,
        });
        events.push(Event {
            seq: 8,
            at: Timestamp::from_unix(201),
            body: EventBody::Decision {
                counterparty: Counterparty::new("0xdd"),
                amount: usdc(5),
                decision: Decision::Block,
                action_hash: ActionHash::from_bytes([3; 32]),
                confidence: Confidence::clamped(0.75),
                balance_seen: usdc(9),
                weakest_evidence: Some(crate::action::Provenance::Onchain),
                evidence: vec!["0xabc".into(), "invoice:1".into()],
                note: "refused".into(),
            },
        });
        for e in &events {
            assert_eq!(Event::decode(&e.canonical()).as_ref(), Some(e));
        }
        // Bytes cut short are left out of the book.
        let c = events[1].canonical();
        assert!(book([c[..c.len() - 1].to_vec(), events[2].canonical()]).len() == 1);
        // The same purchase recorded by two passes is one event of the book.
        let twice = [paid(5, "0xaa", 1), paid(5, "0xaa", 1), paid(4, "0xbb", 1)];
        let b = book(twice.iter().map(Event::canonical));
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].at.as_unix(), 4);
    }
}
