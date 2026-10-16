//! Run both reasoners over the same obligations and print the difference.
//!
//! This is the measurement docs/archive/PLAN.md §1.2 asks for. The claim "the AI decides"
//! is cheap; what makes it checkable is showing the deterministic baseline
//! and the model side by side and counting the positions where they disagree.
//! A divergence of zero is a real and reportable result -- it means the model
//! reproduced a sort and earned nothing -- so this program prints whatever
//! comes out rather than looking for a number it likes.
//!
//! The scenario below is deliberately one where urgency alone is a poor
//! guide: the most overdue item is small and well-tolerated, and a larger
//! one falls due in two days against a balance that cannot cover both. It is
//! not rigged to force disagreement; the model may well agree.
//!
//!     ANTHROPIC_API_KEY=sk-ant-... cargo run -p operator --example compare_reasoners

use ledger::{
    Confidence, Counterparty, Currency, Money, Obligation, ObligationKind, Prioritised, Seconds,
    Timestamp, ordering_divergence,
};
use operator::claude::ClaudeReasoner;
use operator::reasoner::{Reasoner, RuleBased};

fn at(now: Timestamp, days: i64) -> Timestamp {
    now.checked_add(Seconds::days(days).unwrap()).unwrap()
}

fn obligation(
    now: Timestamp,
    who: &str,
    due_in_days: i64,
    amount: i128,
    confidence: f64,
    samples: usize,
) -> Obligation {
    let interval = Seconds::days(30).unwrap();
    let next_due = at(now, due_in_days);
    Obligation {
        counterparty: Counterparty::new(who),
        kind: ObligationKind::Subscription,
        typical_amount: Money::minor(amount, Currency::Usdc),
        interval,
        last_paid: next_due.checked_sub(interval).unwrap(),
        next_due,
        confidence: Confidence::new(confidence).unwrap(),
        sample_count: samples,
        interval_cv: 0.02,
        amount_cv: 0.0,
        evidence: (0..samples).map(|i| format!("0x{i:064x}")).collect(),
    }
}

fn show(title: &str, ranked: &[Prioritised]) {
    println!("\n{title}");
    println!("{}", "-".repeat(title.len()));
    for p in ranked {
        println!(
            "  {}. {:<10} {:>12}   {}",
            p.rank + 1,
            p.obligation.counterparty,
            p.obligation.typical_amount.to_string(),
            p.rationale
        );
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let now = Timestamp::from_unix(1_758_326_400);
    let balance = Money::minor(2_500_000, Currency::Usdc); // 2.50 USDC

    let obligations = vec![
        // Overdue, but small and long-established.
        obligation(now, "0xa11ce", -4, 400_000, 0.95, 6),
        // Not yet due, large, and would consume most of the balance.
        obligation(now, "0xb0b", 2, 1_800_000, 0.88, 5),
        // Due soonest of the not-yet-due, weakly evidenced.
        obligation(now, "0xca101", 1, 700_000, 0.62, 3),
    ];

    let total: i128 = obligations
        .iter()
        .map(|o| o.typical_amount.to_minor())
        .sum();
    println!(
        "balance {balance}, {} obligations totalling {} USDC minor units",
        obligations.len(),
        total
    );
    println!(
        "the balance covers {:.0}% of what is owed",
        100.0 * balance.to_minor() as f64 / total as f64
    );
    for o in &obligations {
        println!("  - {}", o.explain());
    }

    let baseline = RuleBased.prioritise(&obligations, balance, now).await?;
    show("rule-based (deterministic control)", &baseline);

    let Some(claude) = ClaudeReasoner::from_env() else {
        eprintln!(
            "\nNo credential found. Set ANTHROPIC_API_KEY (or ANTHROPIC_AUTH_TOKEN) \
             and re-run to get the comparison."
        );
        std::process::exit(2);
    };

    let model = match claude.prioritise(&obligations, balance, now).await {
        Ok(m) => m,
        Err(e) => {
            // A reasoner failure must never look like a reasoner opinion.
            eprintln!("\nclaude reasoner failed: {e}");
            eprintln!("the operator falls back to the rule-based ordering in this case.");
            std::process::exit(1);
        }
    };
    show(&format!("{} (model)", claude.name()), &model);

    match ordering_divergence(&baseline, &model) {
        Some(0) => println!(
            "\ndivergence 0 of {} -- the model reproduced the baseline exactly and \
             added nothing here.",
            obligations.len()
        ),
        Some(n) => {
            println!("\ndivergence {n} of {} positions", obligations.len());
            for (b, m) in baseline.iter().zip(model.iter()) {
                if b.obligation.cycle_key() != m.obligation.cycle_key() {
                    println!(
                        "  rank {}: rule chose {}, model chose {} -- {}",
                        b.rank + 1,
                        b.obligation.counterparty,
                        m.obligation.counterparty,
                        m.rationale
                    );
                }
            }
        }
        None => println!("\nnot comparable: the two orderings are over different sets"),
    }

    println!(
        "\nnote: this is ordering only. Every item above is still checked by the \
         verifier and by BudgetPolicy before any USDC moves."
    );
    Ok(())
}
