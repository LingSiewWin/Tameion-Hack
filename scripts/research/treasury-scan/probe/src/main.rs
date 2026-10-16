//! Research probe: run the repo's own obligation detector, unchanged, over
//! real outgoing USDC transfers. Input TSV on stdin: safe, tx, to, minor, unix.
use ledger::obligations::{detect, DetectionConfig, Direction, Counterparty, Transfer};
use ledger::{Currency, Money, Timestamp};
use std::collections::BTreeMap;
use std::io::{self, BufRead};

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
    for (safe, ts) in &by_safe {
        let obs = detect(ts, DetectionConfig::default());
        let mut payees: Vec<&str> = ts.iter().map(|t| t.counterparty.as_str()).collect();
        payees.sort(); payees.dedup();
        println!("SAFE\t{}\t{}\t{}\t{}", safe, ts.len(), payees.len(), obs.len());
        for o in &obs {
            println!("OB\t{}\t{}\t{}\t{}\t{:.2}\t{}\t{}\t{}\t{}\t{}",
                safe, o.counterparty, o.kind, o.typical_amount.to_minor(),
                o.interval.as_days(), o.sample_count, o.confidence,
                o.interval_cv, o.amount_cv, o.next_due.as_unix());
        }
    }
}
