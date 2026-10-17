//! What an account's own record is worth as evidence, as one number
//! (docs/117 §13, "the ledger has a price", step a). No external lender;
//! testnet; not a lending protocol.
//!
//! `score = survival × independence × recency`, each in [0, 1], over the
//! account's records as symbolons: one record, three slots (the agent, the
//! counterparty, a witness outside both), each slot filled only by a
//! signature or a read-back that verified.
//!
//! - **survival**: of the countersigned payments and claims (two or three
//!   slots), the share nothing later refused. A `Refused` record for the same
//!   counterparty and amount, dated after the record, is a dispute.
//! - **independence**: the share of settled volume paid to counterparties
//!   whose money does not come back. A counterparty that pays the account
//!   within `loop_window_days` of being paid by it closes a loop.
//! - **recency**: triple-signed volume, each record decayed by
//!   `2^(-age / half_life)`, over all triple-signed volume.
//!
//! No evidence is no score: a factor with nothing to count is 0, not 1.
//!
//! Evidence for the account counts only from counterparties outside a loop;
//! evidence against it (a refusal, a loop) counts from anyone, signed or not.
//! That asymmetry is what makes a limit safe to hang on the number: a closed
//! loop never raises the score, however well it is signed, and a wash
//! trader's counterparty signs its slot for free.
//!
//! Records are counted by their hash-first id (`Symbolon::id`): the same
//! record persisted in two files is one record, with its best slots.
//!
//! Pure: no I/O and no clock; `now` is an argument.

pub use ledger::symbolon::{SlotSig, Symbolon, SymbolonKind};

use std::collections::{BTreeMap, BTreeSet};

const DAY: i64 = 86_400;

/// What a record says passed between the account and its counterparty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// The account paid the counterparty (a settlement).
    Paid,
    /// The counterparty paid the account.
    Received,
    /// The counterparty claims money from the account (an invoice).
    Claimed,
    /// The account's controls blocked paying the counterparty this amount,
    /// or handed the payment to a human.
    Refused,
    /// Anything else: an allowed or reviewed decision, a check, a price table.
    Other,
}

/// One symbolon and the facts of its record that the fold reads.
#[derive(Debug, Clone)]
pub struct Record {
    pub sym: Symbolon,
    /// Unix seconds.
    pub at: i64,
    /// Compared byte for byte: the caller normalises it (lowercase hex for an
    /// address).
    pub counterparty: String,
    /// USDC minor units.
    pub amount: u128,
    pub flow: Flow,
}

impl Record {
    /// How many of the three slots hold a verified signature, 0 to 3.
    pub fn slots(&self) -> usize {
        self.sym.signedness() as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// Money that returns from a counterparty within this many days of
    /// reaching it closes a loop.
    pub loop_window_days: u32,
    /// Age at which a record counts half towards recency.
    pub half_life_days: u32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            loop_window_days: 30,
            half_life_days: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    /// `round(10000 × survival × independence × recency)`, 0 to 10000.
    pub bps: u16,
    pub survival: f64,
    pub independence: f64,
    pub recency: f64,
    /// Records, each counted once, with three, two and one slot filled.
    pub n_triple: usize,
    pub n_double: usize,
    pub n_single: usize,
}

/// `a / b` with `a <= b`, and 0 when there is nothing to divide by.
fn ratio(a: u128, b: u128) -> f64 {
    if b == 0 { 0.0 } else { a as f64 / b as f64 }
}

fn volume<'a>(rs: impl Iterator<Item = &'a Record>) -> u128 {
    rs.fold(0, |acc, r| acc.saturating_add(r.amount))
}

pub fn score(records: &[Record], now: i64, p: &Params) -> Score {
    let mut best: BTreeMap<[u8; 32], &Record> = BTreeMap::new();
    for r in records {
        let kept = best.entry(r.sym.id()).or_insert(r);
        if r.slots() > kept.slots() {
            *kept = r;
        }
    }
    let records: Vec<&Record> = best.into_values().collect();
    let window = i64::from(p.loop_window_days) * DAY;
    // ponytail: quadratic scans; index by counterparty when one account holds
    // tens of thousands of records.
    let looped: BTreeSet<&str> = records
        .iter()
        .filter(|o| {
            o.flow == Flow::Paid
                && records.iter().any(|i| {
                    i.flow == Flow::Received
                        && i.counterparty == o.counterparty
                        && i.at >= o.at
                        && i.at <= o.at.saturating_add(window)
                })
        })
        .map(|o| o.counterparty.as_str())
        .collect();
    let clean = |r: &Record| !looped.contains(r.counterparty.as_str());
    let disputed = |r: &Record| {
        records.iter().any(|q| {
            q.flow == Flow::Refused
                && q.counterparty == r.counterparty
                && q.amount == r.amount
                && q.at > r.at
        })
    };

    let countersigned: Vec<&Record> = records
        .iter()
        .copied()
        .filter(|r| matches!(r.flow, Flow::Paid | Flow::Claimed) && r.slots() >= 2)
        .collect();
    let n_clean = countersigned.iter().filter(|r| clean(r)).count() as u128;
    let n_disputed = countersigned.iter().filter(|r| disputed(r)).count() as u128;
    let survival = ratio(n_clean.saturating_sub(n_disputed), n_clean);

    let settled: Vec<&Record> = records
        .iter()
        .copied()
        .filter(|r| r.flow == Flow::Paid && r.slots() >= 1)
        .collect();
    let independence = ratio(
        volume(settled.iter().copied().filter(|r| clean(r))),
        volume(settled.iter().copied()),
    );

    let triple: Vec<&Record> = records
        .iter()
        .copied()
        .filter(|r| r.slots() == 3 && matches!(r.flow, Flow::Paid | Flow::Received | Flow::Claimed))
        .collect();
    let half_life = f64::from(p.half_life_days.max(1));
    let decayed: f64 = triple
        .iter()
        .filter(|r| clean(r))
        .map(|r| {
            let age_days = now.saturating_sub(r.at).max(0) as f64 / DAY as f64;
            r.amount as f64 * (-age_days / half_life).exp2()
        })
        .sum();
    let all_triple = volume(triple.iter().copied());
    let recency = if all_triple == 0 {
        0.0
    } else {
        (decayed / all_triple as f64).min(1.0)
    };

    let count = |n: usize| records.iter().filter(|r| r.slots() == n).count();
    Score {
        bps: (10_000.0 * survival * independence * recency).round() as u16,
        survival,
        independence,
        recency,
        n_triple: count(3),
        n_double: count(2),
        n_single: count(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const NOW: i64 = 1_800_000_000;
    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn sig() -> Option<SlotSig> {
        Some(SlotSig {
            alg: "chain-receipt".into(),
            key_id: "chain:421614".into(),
            sig: vec![1],
            at: 0,
        })
    }

    /// A record with its own hash, so no two calls are the same record.
    fn rec(flow: Flow, cp: &str, amount: u128, days_ago: i64, slots: usize) -> Record {
        let mut record_hash = [0; 32];
        record_hash[..8].copy_from_slice(&NEXT.fetch_add(1, Ordering::Relaxed).to_be_bytes());
        Record {
            sym: Symbolon {
                record_hash,
                kind: SymbolonKind::Settlement,
                agent: (slots >= 3).then(sig).flatten(),
                counterparty: (slots >= 2).then(sig).flatten(),
                witness: (slots >= 1).then(sig).flatten(),
            },
            at: NOW - days_ago * DAY,
            counterparty: cp.into(),
            amount,
            flow,
        }
    }

    #[test]
    fn no_records_is_no_score() {
        let s = score(&[], NOW, &Params::default());
        assert_eq!(
            (s.bps, s.survival, s.independence, s.recency),
            (0, 0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn witness_only_records_have_no_price() {
        // Settled and witnessed, but nobody countersigned: independence only.
        let s = score(&[rec(Flow::Paid, "a", 100, 1, 1)], NOW, &Params::default());
        assert_eq!(
            (s.bps, s.survival, s.independence, s.recency),
            (0, 0.0, 1.0, 0.0)
        );
        assert_eq!((s.n_single, s.n_double, s.n_triple), (1, 0, 0));
    }

    #[test]
    fn one_fresh_triple_settlement_scores_full_and_a_half_life_halves_it() {
        let fresh = score(&[rec(Flow::Paid, "a", 100, 0, 3)], NOW, &Params::default());
        assert_eq!(fresh.bps, 10_000);
        let aged = score(&[rec(Flow::Paid, "a", 100, 30, 3)], NOW, &Params::default());
        assert!((aged.recency - 0.5).abs() < 1e-12);
        assert_eq!(aged.bps, 5_000);
    }

    #[test]
    fn a_counterparty_whose_money_comes_back_contributes_nothing() {
        let records = [
            rec(Flow::Paid, "a", 300, 10, 3),
            rec(Flow::Paid, "b", 100, 10, 3),
            // b pays the account back five days later: a closed loop.
            rec(Flow::Received, "b", 100, 5, 1),
        ];
        let s = score(&records, NOW, &Params::default());
        assert_eq!(s.survival, 1.0); // a is countersigned and clean
        assert_eq!(s.independence, 0.75); // 300 of 400 settled
        // Triple-signed volume is 400; only a's 300 counts, ten days old.
        assert!((s.recency - 0.75 * (-10.0f64 / 30.0).exp2()).abs() < 1e-12);
        // The same return after the window is not a loop.
        let late = [
            records[0].clone(),
            records[1].clone(),
            rec(Flow::Received, "b", 100, 0, 1),
        ];
        let p = Params {
            loop_window_days: 7,
            ..Params::default()
        };
        assert_eq!(score(&late, NOW, &p).independence, 1.0);
    }

    #[test]
    fn a_later_refusal_of_the_same_amount_is_a_dispute_even_unsigned() {
        let records = [
            rec(Flow::Paid, "a", 100, 10, 2),
            rec(Flow::Paid, "b", 200, 10, 2),
            rec(Flow::Refused, "a", 100, 3, 0),
            // A refusal of another amount, or an earlier one, is not.
            rec(Flow::Refused, "b", 201, 3, 0),
            rec(Flow::Refused, "b", 200, 11, 0),
        ];
        assert_eq!(score(&records, NOW, &Params::default()).survival, 0.5);
    }

    #[test]
    fn closing_a_loop_takes_back_the_counterpartys_evidence() {
        // The one case where a triple-signed record lowers survival: it
        // closes a loop, and evidence from a loop does not count.
        let base = vec![
            rec(Flow::Paid, "c", 100, 60, 2),
            rec(Flow::Paid, "d", 100, 60, 2),
            rec(Flow::Refused, "d", 100, 50, 0),
            rec(Flow::Received, "c", 50, 10, 1),
        ];
        let before = score(&base, NOW, &Params::default());
        assert_eq!(before.survival, 0.5);
        let mut after = base.clone();
        after.push(rec(Flow::Paid, "c", 70, 11, 3)); // c's money returns a day later
        assert_eq!(score(&after, NOW, &Params::default()).survival, 0.0);
    }

    #[test]
    fn the_same_record_twice_counts_once_with_its_best_slots() {
        let single = rec(Flow::Paid, "a", 100, 0, 1);
        let mut triple = single.clone();
        triple.sym.agent = sig();
        triple.sym.counterparty = sig();
        let p = Params::default();
        let both = score(&[single.clone(), triple.clone(), single], NOW, &p);
        assert_eq!(both, score(&[triple], NOW, &p));
        assert_eq!((both.n_single, both.n_triple, both.bps), (0, 1, 10_000));
    }

    #[test]
    fn extreme_inputs_do_not_panic() {
        let records = [
            rec(Flow::Paid, "a", u128::MAX, 0, 3),
            rec(Flow::Paid, "b", u128::MAX, 0, 3),
            Record {
                at: i64::MIN,
                ..rec(Flow::Received, "a", 1, 0, 3)
            },
        ];
        let s = score(
            &records,
            i64::MAX,
            &Params {
                loop_window_days: u32::MAX,
                half_life_days: 0,
            },
        );
        assert!(s.bps <= 10_000);
    }
}
