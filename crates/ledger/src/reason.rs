//! Ordering obligations when the money will not stretch to all of them.
//!
//! The division of labour is the point of this module. Hard limits are
//! deterministic code in [`crate::verify`] and the on-chain policy, and
//! nothing here gets a vote on them: a reasoner that talks itself into a bad
//! payment still cannot make one.
//!
//! Inside those limits there is real judgment. When the balance cannot cover
//! everything due this week, something is paid first and something last, and
//! that ordering has consequences a rule cannot fully anticipate -- a vendor
//! who cuts service off at day one versus one who never chases, a
//! relationship worth more than the invoice. That is what a language model is
//! for, and RFB 04 is asking about exactly this when it weighs "how much does
//! the AI actually decide".
//!
//! [`rank_by_urgency`] is the deterministic baseline. It does real
//! prioritisation work, not placeholder work, so a deployment that never
//! plugs in a model still functions -- and, more usefully, it is the
//! **control** a model is measured against. A model that produces the same
//! ordering as this function has added nothing, and showing the two side by
//! side is how "the AI decided" stops being a claim and becomes evidence.

use crate::money::Money;
use crate::obligations::Obligation;
use crate::time::Timestamp;

/// An obligation with its place in the queue and why it landed there.
#[derive(Debug, Clone, PartialEq)]
pub struct Prioritised {
    pub obligation: Obligation,
    /// Zero-based position in the ordering.
    pub rank: usize,
    /// One sentence, written into the log whichever reasoner produced it.
    pub rationale: String,
}

/// Deterministic ordering: urgency first, then how well evidenced.
///
/// Anything already overdue outranks anything merely approaching, because a
/// late payment has a counterparty waiting on it. Within the same urgency
/// band the better-evidenced obligation goes first: spending cash on the
/// claim we understand best is the cheaper mistake if one of them turns out
/// to be wrong.
///
/// The running balance is carried down the list so each rationale can say
/// whether the cash would still be there by the time that obligation is
/// reached. That is information the caller would otherwise have to
/// recompute, and it is what makes the ordering legible rather than
/// arbitrary.
pub fn rank_by_urgency(
    obligations: &[Obligation],
    balance: Money,
    now: Timestamp,
) -> Vec<Prioritised> {
    let mut ordered: Vec<&Obligation> = obligations.iter().collect();
    ordered.sort_by(|a, b| {
        let overdue = |o: &Obligation| u8::from(o.next_due > now);
        overdue(a)
            .cmp(&overdue(b))
            .then(a.next_due.cmp(&b.next_due))
            // Higher confidence first; partial_cmp is safe because
            // Confidence rejects NaN at construction.
            .then(
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(core::cmp::Ordering::Equal),
            )
            .then(a.counterparty.cmp(&b.counterparty))
    });

    let total = ordered.len();
    let mut running = balance;
    let mut out = Vec::with_capacity(total);

    for (rank, o) in ordered.into_iter().enumerate() {
        let urgency = if o.next_due <= now {
            format!("overdue by {:.1} days", o.overdue_by(now).as_days())
        } else {
            format!("due in {:.1} days", (o.next_due - now).as_days())
        };

        let covered = running.currency() == o.typical_amount.currency()
            && running.to_minor() >= o.typical_amount.to_minor();
        let affordable = if covered {
            "covered"
        } else {
            "not covered by remaining cash"
        };

        out.push(Prioritised {
            obligation: o.clone(),
            rank,
            rationale: format!(
                "ranked {} of {}: {}, confidence {}, {} {}",
                rank + 1,
                total,
                urgency,
                o.confidence,
                o.typical_amount,
                affordable
            ),
        });

        if covered {
            running = running.checked_sub(o.typical_amount).unwrap_or(running);
        }
    }

    out
}

/// How far two orderings of the same obligations disagree.
///
/// The number of positions that differ. Zero means a model reproduced the
/// deterministic baseline exactly and contributed nothing an `if` statement
/// could not have; a non-zero count is the concrete difference to point at,
/// and docs/archive/PLAN.md §1.2 accepts the feature only when a run "shows the model
/// deferring a different obligation than the rule-based default would have".
///
/// Returns `None` if the two orderings are not over the same set, since a
/// comparison between different populations means nothing.
pub fn ordering_divergence(a: &[Prioritised], b: &[Prioritised]) -> Option<usize> {
    if a.len() != b.len() {
        return None;
    }
    let key = |p: &Prioritised| p.obligation.cycle_key();
    let mut a_keys: Vec<String> = a.iter().map(key).collect();
    let mut b_keys: Vec<String> = b.iter().map(key).collect();
    a_keys.sort();
    b_keys.sort();
    if a_keys != b_keys {
        return None;
    }
    Some(
        a.iter()
            .zip(b.iter())
            .filter(|(x, y)| key(x) != key(y))
            .count(),
    )
}
