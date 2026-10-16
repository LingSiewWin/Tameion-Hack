//! Walk-forward backtest of the repo's detector on real history.
//! For each treasury, at cut points 30 days apart, run `detect` on what was
//! visible before the cut and print every obligation it says falls due in the
//! following 30 days. Scoring against what actually happened is done outside.
use ledger::obligations::{detect, Counterparty, DetectionConfig, Direction, Transfer};
use ledger::{Currency, Money, Timestamp};
use std::collections::BTreeMap;
use std::io::{self, BufRead};

const DAY: i64 = 86_400;

fn main() {
    let mut by_safe: BTreeMap<String, Vec<Transfer>> = BTreeMap::new();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 5 { continue; }
        by_safe.entry(f[0].to_string()).or_default().push(Transfer {
            tx_hash: f[1].to_string(),
            counterparty: Counterparty::new(f[2]),
            amount: Money::minor(f[3].parse().unwrap(), Currency::Usdc),
            at: Timestamp::from_unix(f[4].parse().unwrap()),
            direction: Direction::Out,
        });
    }
    let end: i64 = std::env::args().nth(1).unwrap().parse().unwrap();
    for (safe, ts) in &by_safe {
        let first = ts.iter().map(|t| t.at.as_unix()).min().unwrap();
        let mut cut = first + 60 * DAY;
        while cut + 45 * DAY <= end {
            let seen: Vec<Transfer> = ts.iter().filter(|t| t.at.as_unix() < cut).cloned().collect();
            for o in detect(&seen, DetectionConfig::default()) {
                let due = o.next_due.as_unix();
                if due >= cut && due < cut + 30 * DAY {
                    println!("PRED\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                        safe, cut, o.counterparty, o.typical_amount.to_minor(),
                        o.interval.as_i64(), due, o.confidence, o.sample_count, o.kind, o.amount_cv, o.interval_cv);
                }
            }
            cut += 30 * DAY;
        }
    }
}
