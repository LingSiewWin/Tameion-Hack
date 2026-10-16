//! The decision grader (docs/113): golden scenarios, labelled and hashed before any decider
//! ran, and the score of a decider against them.
//!
//! This test runs the rule baseline only (deterministic, no network) and asserts what must
//! hold for any decider: every action is one code allowed, and the production path
//! (`decide_or_rule`, which holds every answer to the case with `guard`) leaves the rule's own
//! decision unchanged. It prints the rule's score and never asserts that the rule matches the
//! labels. `examples/decide_eval.rs` runs this same file against the model decider.
//!
//!     cargo test -p operator --test decide_eval -- --nocapture
//!     cargo run -p operator --example decide_eval -- --decider rule|model [--reps N]

use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::Instant;

use ledger::ActorKind;
use operator::decide::{Action, Case, Decider, Decision, RuleBased, decide_or_rule};
use operator::reasoner::ReasonerError;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Compiled in, so the gate grades exactly the bytes whose hash it prints.
pub const FIXTURE: &str = include_str!("fixtures/decide-scenarios.json");

#[derive(Deserialize)]
pub struct Scenario {
    pub name: String,
    pub case: Case,
    pub golden: Golden,
}

#[derive(Deserialize)]
pub struct Golden {
    pub action: Action,
    pub basis: operator::decide::Basis,
}

/// The scenarios of a fixture file (`FIXTURE`, or any file of the same shape).
pub fn scenarios(json: &str) -> Vec<Scenario> {
    #[derive(Deserialize)]
    struct File {
        scenarios: Vec<Scenario>,
    }
    serde_json::from_str::<File>(json)
        .expect("the fixture parses")
        .scenarios
}

/// Why the decider's own answer is not what the account acts on.
pub enum Overruled {
    /// The decider failed; the rule decided (`decide_or_rule`'s note).
    Error(String),
    /// The decider answered with another kind's decision (`by` is not its own kind).
    Fallback,
    /// `guard`: the action was not in `allowed`.
    IllegalAction(Action),
    /// `guard`: the why cited a number the case does not hold; the code wrote the sentence.
    InventedNumber(String),
}

/// One decision point, graded.
pub struct Row {
    /// What the account acts on: `decide_or_rule`'s decision.
    pub acted: Decision,
    pub rule: Decision,
    pub overruled: Option<Overruled>,
    /// The decider's own call only.
    pub ms: f64,
}

/// Times the decider's own call and keeps its raw answer, so the grade can say why
/// `decide_or_rule` did not use it.
struct Watched<'a, D> {
    inner: &'a D,
    raw: Mutex<Option<(Result<Decision, String>, f64)>>,
}

impl<D: Decider + Sync> Decider for Watched<'_, D> {
    async fn decide(&self, case: &Case) -> Result<Decision, ReasonerError> {
        let start = Instant::now();
        let out = self.inner.decide(case).await;
        let ms = start.elapsed().as_secs_f64() * 1e3;
        let kept = out.as_ref().map(Clone::clone).map_err(|e| e.to_string());
        *self.raw.lock().expect("not poisoned") = Some((kept, ms));
        out
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn kind(&self) -> ActorKind {
        self.inner.kind()
    }
}

/// Every scenario once, through the production path: `decide_or_rule`.
pub async fn run<D: Decider + Sync>(decider: &D, all: &[Scenario]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(all.len());
    for s in all {
        let watched = Watched {
            inner: decider,
            raw: Mutex::new(None),
        };
        let (acted, note) = decide_or_rule(&watched, &s.case).await;
        let (raw, ms) = watched
            .raw
            .into_inner()
            .expect("not poisoned")
            .expect("decide_or_rule asks the decider");
        let overruled = match raw {
            Err(e) => Some(Overruled::Error(note.unwrap_or(e))),
            Ok(d) if d.by != decider.kind() => Some(Overruled::Fallback),
            Ok(d) if !s.case.allowed.contains(&d.action) => {
                Some(Overruled::IllegalAction(d.action))
            }
            // A legal answer with a note: the note is the invented number.
            Ok(d) => note.map(|_| Overruled::InventedNumber(d.why)),
        };
        rows.push(Row {
            acted,
            rule: RuleBased::decision(&s.case),
            overruled,
            ms,
        });
    }
    rows
}

/// The counts over every rep (`n` scenarios × `reps.len()` runs).
pub struct Grade {
    pub action_ok: usize,
    pub basis_ok: usize,
    pub asks: usize,
    /// Paid (or paid early) where the label holds the money: the expensive mistake.
    pub paid_against_label: usize,
    /// Held (wait, ask, refuse) where the label pays: the nagging or late mistake.
    pub held_against_label: usize,
    pub illegal: usize,
    pub invented: usize,
    pub errors: usize,
    pub fallbacks: usize,
    /// Scenarios whose acted action was the same in every rep.
    pub consistent: usize,
}

fn pays(a: Action) -> bool {
    matches!(a, Action::Pay | Action::PayEarly)
}

pub fn grade(all: &[Scenario], reps: &[Vec<Row>]) -> Grade {
    let mut g = Grade {
        action_ok: 0,
        basis_ok: 0,
        asks: 0,
        paid_against_label: 0,
        held_against_label: 0,
        illegal: 0,
        invented: 0,
        errors: 0,
        fallbacks: 0,
        consistent: 0,
    };
    for rows in reps {
        for (s, r) in all.iter().zip(rows) {
            let a = r.acted.action;
            g.action_ok += usize::from(a == s.golden.action);
            g.basis_ok += usize::from(r.acted.basis == s.golden.basis);
            g.asks += usize::from(a == Action::Ask);
            g.paid_against_label += usize::from(pays(a) && !pays(s.golden.action));
            g.held_against_label += usize::from(!pays(a) && pays(s.golden.action));
            match &r.overruled {
                Some(Overruled::Error(_)) => g.errors += 1,
                Some(Overruled::Fallback) => g.fallbacks += 1,
                Some(Overruled::IllegalAction(_)) => g.illegal += 1,
                Some(Overruled::InventedNumber(_)) => g.invented += 1,
                None => {}
            }
        }
    }
    g.consistent = (0..all.len())
        .filter(|&i| {
            reps.iter()
                .all(|rows| rows[i].acted.action == reps[0][i].acted.action)
        })
        .count();
    g
}

/// Nearest-rank percentile (as `examples/measure_reasoners.rs`).
fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * s.len() as f64).ceil() as usize;
    s[rank.clamp(1, s.len()) - 1]
}

/// The table: the headline counts, then every scenario with the golden answer, the rule's
/// (beside another decider) and the decider's in each rep, and the decider's why from rep 1.
pub fn render(
    name: &str,
    kind: ActorKind,
    fixture: &str,
    all: &[Scenario],
    reps: &[Vec<Row>],
) -> String {
    let g = grade(all, reps);
    let (n, k) = (all.len(), reps.len());
    let total = n * k;
    let rate = |x: usize| format!("{x}/{total} = {:.0}%", 100.0 * x as f64 / total as f64);
    let golden_asks = all
        .iter()
        .filter(|s| s.golden.action == Action::Ask)
        .count();
    let majority = [
        Action::Pay,
        Action::PayEarly,
        Action::Wait,
        Action::Ask,
        Action::Refuse,
    ]
    .into_iter()
    .map(|a| (all.iter().filter(|s| s.golden.action == a).count(), a))
    .max_by_key(|(c, _)| *c)
    .expect("five actions");
    let per_rep: Vec<String> = reps
        .iter()
        .map(|rows| {
            all.iter()
                .zip(rows)
                .filter(|(s, r)| r.acted.action == s.golden.action)
                .count()
                .to_string()
        })
        .collect();
    let model = reps
        .iter()
        .flatten()
        .find_map(|r| r.acted.model.clone())
        .unwrap_or_else(|| "-".into());
    let ms: Vec<f64> = reps.iter().flatten().map(|r| r.ms).collect();

    let mut o = String::new();
    let _ = writeln!(
        o,
        "fixture  sha256 {:x}",
        Sha256::digest(fixture.as_bytes())
    );
    let _ = writeln!(
        o,
        "decider  {name} ({kind}), model {model}; {n} scenarios x {k} rep(s)"
    );
    let _ = writeln!(
        o,
        "action   {}  (correct per rep: {}; always-{} would score {}/{n})",
        rate(g.action_ok),
        per_rep.join(" "),
        majority.1.as_str(),
        majority.0
    );
    let _ = writeln!(o, "basis    {}", rate(g.basis_ok));
    let _ = writeln!(o, "ask      {}  (golden {golden_asks}/{n})", rate(g.asks));
    let _ = writeln!(
        o,
        "against the label: paid where it holds {}, held where it pays {}",
        g.paid_against_label, g.held_against_label
    );
    let _ = writeln!(
        o,
        "guard    illegal action {}, invented number {}",
        g.illegal, g.invented
    );
    let _ = writeln!(
        o,
        "own answers {}/{total}  (errors, the rule decided: {}; answered as another kind: {})",
        total - g.errors - g.fallbacks,
        g.errors,
        g.fallbacks
    );
    if k > 1 {
        let _ = writeln!(
            o,
            "same action in every rep: {}/{n} scenarios",
            g.consistent
        );
    }
    let _ = writeln!(
        o,
        "latency  p50 {:.0} ms, p90 {:.0} ms (n={})",
        pct(&ms, 50.0),
        pct(&ms, 90.0),
        ms.len()
    );
    // The rule's own column only when another decider is graded beside it.
    let beside = kind != ActorKind::Rule;
    let _ = writeln!(
        o,
        "\nper scenario: golden{} | {name} (each rep)",
        if beside { " | rule" } else { "" }
    );
    let mark = |d: &Decision, s: &Scenario| {
        format!(
            "{}/{} {}",
            d.action.as_str(),
            d.basis.as_str(),
            if d.action != s.golden.action {
                "MISS"
            } else if d.basis != s.golden.basis {
                "ok(basis differs)"
            } else {
                "ok"
            }
        )
    };
    for (i, s) in all.iter().enumerate() {
        let first = &reps[0][i];
        let forced = if s.case.allowed.len() == 1 {
            "  (decided by code: one action allowed)"
        } else {
            ""
        };
        let _ = writeln!(o, "s{:02} {}  [{}]{forced}", i + 1, s.name, s.case.id);
        let rule = if beside {
            format!(" | {}", mark(&first.rule, s))
        } else {
            String::new()
        };
        let mine: Vec<String> = reps.iter().map(|rows| mark(&rows[i].acted, s)).collect();
        let _ = writeln!(
            o,
            "    {}/{}{rule} | {}",
            s.golden.action.as_str(),
            s.golden.basis.as_str(),
            mine.join(", ")
        );
        let _ = writeln!(o, "    why: {}", first.acted.why);
        for (rep, rows) in reps.iter().enumerate() {
            let note = match &rows[i].overruled {
                None => continue,
                Some(Overruled::Error(e)) => e.clone(),
                Some(Overruled::Fallback) => "answered as another kind (fallback)".into(),
                Some(Overruled::IllegalAction(a)) => format!(
                    "guard: {} is not allowed here; the rule decided",
                    a.as_str()
                ),
                Some(Overruled::InventedNumber(w)) => {
                    format!("guard: invented number, the code wrote the sentence; it said: {w}")
                }
            };
            let _ = writeln!(o, "    rep {}: {note}", rep + 1);
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use operator::decide::Basis;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn the_fixture_is_well_formed() {
        let all = scenarios(FIXTURE);
        assert!((12..=16).contains(&all.len()), "{} scenarios", all.len());
        let ids: BTreeSet<&str> = all.iter().map(|s| s.case.id.as_str()).collect();
        assert_eq!(ids.len(), all.len(), "case ids are unique");
        for s in &all {
            assert!(
                s.case.allowed.contains(&s.golden.action),
                "{}: the golden action is not one code allows",
                s.case.id
            );
        }
        let actions: BTreeSet<&str> = all.iter().map(|s| s.golden.action.as_str()).collect();
        let bases: BTreeSet<&str> = all.iter().map(|s| s.golden.basis.as_str()).collect();
        assert_eq!(
            actions.len(),
            5,
            "every action is golden somewhere: {actions:?}"
        );
        assert_eq!(
            bases.len(),
            Basis::ALL.len(),
            "every basis is golden somewhere: {bases:?}"
        );
    }

    #[tokio::test]
    async fn the_rule_baseline_keeps_the_invariants() {
        let all = scenarios(FIXTURE);
        let rows = run(&RuleBased, &all).await;
        for (s, r) in all.iter().zip(&rows) {
            assert!(
                s.case.allowed.contains(&r.acted.action),
                "{}: the rule chose {} outside allowed",
                s.case.id,
                r.acted.action.as_str()
            );
            assert!(r.overruled.is_none(), "{}: overruled", s.case.id);
            assert_eq!(
                r.acted, r.rule,
                "{}: the production path changed the rule's own decision",
                s.case.id
            );
            assert_eq!(
                (r.acted.by, r.acted.model.as_deref()),
                (ActorKind::Rule, None)
            );
        }
        // Printed, not asserted: the rule is the control, not the answer key.
        println!(
            "{}",
            render(RuleBased.name(), RuleBased.kind(), FIXTURE, &all, &[rows])
        );
    }

    /// Plays back fixed answers through the grading path.
    struct Planted(BTreeMap<String, Decision>);

    impl Decider for Planted {
        async fn decide(&self, case: &Case) -> Result<Decision, ReasonerError> {
            self.0
                .get(&case.id)
                .cloned()
                .ok_or_else(|| ReasonerError::Unusable(format!("nothing planted for {}", case.id)))
        }

        fn name(&self) -> &str {
            "planted"
        }

        fn kind(&self) -> ActorKind {
            ActorKind::Model
        }
    }

    /// The grader sees what it claims to: the golden answers score full marks, and a planted
    /// invented number, a planted illegal action and a failure are each counted once, with
    /// the rule deciding where the answer is not used.
    #[tokio::test]
    async fn the_grader_counts_what_it_claims() {
        let all = scenarios(FIXTURE);
        let answer = |s: &Scenario, action: Action, why: &str| Decision {
            action,
            basis: s.golden.basis,
            why: why.into(),
            by: ActorKind::Model,
            model: Some("planted".into()),
        };
        let golden: BTreeMap<String, Decision> = all
            .iter()
            .map(|s| (s.case.id.clone(), answer(s, s.golden.action, "as labelled")))
            .collect();
        let g = grade(&all, &[run(&Planted(golden.clone()), &all).await]);
        assert_eq!((g.action_ok, g.basis_ok), (all.len(), all.len()));
        assert_eq!(g.illegal + g.invented + g.errors + g.fallbacks, 0);

        let mut bad = golden;
        let first = &all[0];
        bad.insert(
            first.case.id.clone(),
            answer(first, first.golden.action, "pay 987654321 now"),
        );
        let banned = all
            .iter()
            .position(|s| !s.case.allowed.contains(&Action::Pay))
            .expect("a case where pay is not allowed");
        let b = &all[banned];
        bad.insert(b.case.id.clone(), answer(b, Action::Pay, "pay it"));
        let gone = all.last().expect("scenarios");
        bad.remove(&gone.case.id);
        let rows = run(&Planted(bad), &all).await;
        let g = grade(&all, std::slice::from_ref(&rows));
        assert_eq!((g.invented, g.illegal, g.errors), (1, 1, 1));
        assert_eq!(rows[banned].acted, rows[banned].rule, "the rule decides");
        assert_eq!(rows[all.len() - 1].acted, rows[all.len() - 1].rule);
    }
}
