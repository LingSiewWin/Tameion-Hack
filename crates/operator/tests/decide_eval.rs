//! The decision grader (docs/113): golden scenarios, labelled and hashed before any decider
//! ran, and the score of a decider against them, in two columns:
//!
//! - **as the system would act** (the default): `allowed` recomputed from each case's facts with
//!   S10's own functions (`allowed_for_bill`, `allowed_for_invoice`, the verifier's rules and the
//!   confidence floor), ignoring the fixture's list, so the score is the running system's;
//! - **raw judgment** (`--raw` alone): the fixture's own list, so the score is the decider's
//!   judgment with whatever room the fixture gives it (docs/60 P-198).
//!
//! This test runs the rule baseline only (deterministic, no network) in both modes and asserts
//! what must hold for any decider: every action is one code allowed, and the production path
//! (`decide_or_rule`, which holds every answer to the case with `guard`) leaves the rule's own
//! decision unchanged. It prints the score and never asserts that the rule matches the labels.
//! `examples/decide_eval.rs` runs this same file against the model decider.
//!
//!     cargo test -p operator --test decide_eval -- --nocapture
//!     cargo run -p operator --example decide_eval -- --decider rule|model [--reps N] [--raw]

use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::Instant;

use ledger::ActorKind;
use operator::decide::{
    Action, CONFIDENCE_FLOOR, Case, CaseKind, Decider, Decision, HORIZON_DAYS, PAY_LEAD_DAYS,
    RuleBased, allowed_for_bill, allowed_for_invoice, decide_or_rule,
};
use operator::reasoner::ReasonerError;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Compiled in, so the gate grades exactly the bytes whose hash it prints.
pub const FIXTURE: &str = include_str!("fixtures/decide-scenarios.json");

/// Which `allowed` a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Recomputed from the case's facts, as the running system computes it.
    System,
    /// The fixture's own list.
    Raw,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::System => "as the system would act",
            Mode::Raw => "raw judgment",
        }
    }

    fn short(self) -> &'static str {
        match self {
            Mode::System => "system",
            Mode::Raw => "raw",
        }
    }
}

#[derive(Deserialize)]
pub struct Scenario {
    pub name: String,
    pub case: Case,
    pub golden: Golden,
    /// The fixture's own `allowed`, whichever list the run uses.
    #[serde(skip)]
    pub listed: Vec<Action>,
}

#[derive(Deserialize)]
pub struct Golden {
    pub action: Action,
    pub basis: operator::decide::Basis,
}

/// The scenarios of a fixture file (`FIXTURE`, or any file of the same shape), with `allowed`
/// as `mode` says.
pub fn scenarios(json: &str, mode: Mode) -> Vec<Scenario> {
    #[derive(Deserialize)]
    struct File {
        scenarios: Vec<Scenario>,
    }
    let mut all = serde_json::from_str::<File>(json)
        .expect("the fixture parses")
        .scenarios;
    for s in &mut all {
        s.listed = s.case.allowed.clone();
        if mode == Mode::System {
            s.case.allowed = system_allowed(&s.case);
        }
    }
    all
}

/// What the case is: its `kind` when it says, else its id's prefix (`bill:`, `invoice:`, `x402:`).
fn kind(c: &Case) -> CaseKind {
    c.kind
        .or(match c.id.split(':').next() {
            Some("bill") => Some(CaseKind::Bill),
            Some("invoice") => Some(CaseKind::Invoice),
            Some("x402" | "purchase") => Some(CaseKind::Purchase),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{}: no kind; add \"kind\": bill|invoice|purchase", c.id))
}

/// `allowed` as the running system computes it from these facts, on Autopilot.
///
/// Invoices: `allowed_for_invoice` (ask or refuse; a look-alike: refuse). Bills:
/// `allowed_for_bill` with the verifier's verdict — a look-alike blocks; an unapproved payee,
/// an amount over the cap, a confidence under the floor or an amount outside a quarter of the
/// usual goes to review — and `early_payments`' window (terms, inside the lead, a lead shorter
/// than the cadence, a runway over the horizon). x402 purchases in the same shape by
/// `x402::buy`'s rules: over the cap the chain refuses, an unapproved seller needs the owner,
/// and there is no amount band. A case without `cap_minor` is taken to be inside the cap.
pub fn system_allowed(c: &Case) -> Vec<Action> {
    use ledger::Decision as Verdict;
    let k = kind(c);
    if k == CaseKind::Invoice {
        return allowed_for_invoice(false, c.lookalike_of.is_some());
    }
    let over_cap = c.cap_minor.is_some_and(|cap| c.amount_minor > cap);
    let off_band = k == CaseKind::Bill
        && c.usual_minor
            .is_some_and(|u| c.amount_minor < u - u / 4 || c.amount_minor > u + u / 4);
    let verdict = if c.lookalike_of.is_some() || (over_cap && k == CaseKind::Purchase) {
        Verdict::Block
    } else if over_cap || c.payee_age_days.is_none() || c.confidence < CONFIDENCE_FLOOR || off_band
    {
        Verdict::Review
    } else {
        Verdict::Allow
    };
    let early = k == CaseKind::Bill
        && c.discount_bps > 0
        && c.due_in_days > PAY_LEAD_DAYS
        && c.due_in_days <= i64::from(c.early_days)
        && c.cadence_days.is_none_or(|d| c.early_days < d)
        && c.runway_days.is_none_or(|r| r >= HORIZON_DAYS);
    allowed_for_bill(verdict, true, early, c.due_in_days)
}

/// Why the decider's own answer is not what the account acts on.
pub enum Overruled {
    /// The decider failed; the rule decided (`decide_or_rule`'s note).
    Error(String),
    /// The decider answered with another kind's decision (`by` is not its own kind).
    Fallback,
    /// `guard`: code does not permit the action here; the rule decided.
    IllegalAction(Action),
    /// `guard`: the code wrote the sentence (an invented number, or none); the note and what was said.
    Rewritten(String, String),
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
            // `guard` swapped the action: code does not permit it here.
            Ok(d) if acted.action != d.action => Some(Overruled::IllegalAction(d.action)),
            Ok(d) => note.map(|n| Overruled::Rewritten(n, d.why)),
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

/// One mode's runs: its scenarios (with that mode's `allowed`) and every rep.
pub struct ModeRun {
    pub mode: Mode,
    pub all: Vec<Scenario>,
    pub reps: Vec<Vec<Row>>,
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
    pub rewritten: usize,
    pub errors: usize,
    pub fallbacks: usize,
    /// Scenarios whose acted action was the same in every rep.
    pub consistent: usize,
    /// Scenarios whose golden action code does not offer.
    pub unreachable: usize,
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
        rewritten: 0,
        errors: 0,
        fallbacks: 0,
        consistent: 0,
        unreachable: all
            .iter()
            .filter(|s| !s.case.allowed.contains(&s.golden.action))
            .count(),
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
                Some(Overruled::Rewritten(..)) => g.rewritten += 1,
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

fn actions(list: &[Action]) -> String {
    list.iter()
        .map(|a| a.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The table: one column per mode (the system first), then every scenario with the golden
/// answer, `allowed` in each mode, the rule's answer (beside another decider) and the decider's
/// in each rep, its why from the first mode's rep 1, and every overruled answer.
pub fn render(name: &str, kind: ActorKind, fixture: &str, runs: &[ModeRun]) -> String {
    let all = &runs[0].all;
    let n = all.len();
    let k = runs[0].reps.len();
    let total = n * k;
    let grades: Vec<Grade> = runs.iter().map(|r| grade(&r.all, &r.reps)).collect();
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
    let model = runs
        .iter()
        .flat_map(|r| r.reps.iter().flatten())
        .find_map(|r| r.acted.model.clone())
        .unwrap_or_else(|| "-".into());

    let mut o = String::new();
    let _ = writeln!(
        o,
        "fixture  sha256 {:x}",
        Sha256::digest(fixture.as_bytes())
    );
    let _ = writeln!(
        o,
        "decider  {name} ({kind}), model {model}; {n} scenarios x {k} rep(s) per column"
    );
    let _ = writeln!(
        o,
        "allowed  system = recomputed from each case's facts (S10's allowed_for_bill / allowed_for_invoice, the verifier's rules, \
         confidence floor {CONFIDENCE_FLOOR}, Autopilot; a case without cap_minor is taken as inside the cap); raw = the fixture's list"
    );
    let row = |o: &mut String, label: &str, cell: &dyn Fn(&ModeRun, &Grade) -> String| {
        let cells: Vec<String> = runs.iter().zip(&grades).map(|(r, g)| cell(r, g)).collect();
        let _ = writeln!(o, "| {label} | {} |", cells.join(" | "));
    };
    let heads: Vec<&str> = runs.iter().map(|r| r.mode.label()).collect();
    let _ = writeln!(o, "\n| | {} |", heads.join(" | "));
    let _ = writeln!(o, "|---|{}", "---|".repeat(runs.len()));
    row(&mut o, "action = label", &|r, g| {
        let per_rep: Vec<String> = r
            .reps
            .iter()
            .map(|rows| {
                r.all
                    .iter()
                    .zip(rows)
                    .filter(|(s, x)| x.acted.action == s.golden.action)
                    .count()
                    .to_string()
            })
            .collect();
        format!("{} (per rep {})", rate(g.action_ok), per_rep.join(", "))
    });
    row(&mut o, "basis = label", &|_, g| rate(g.basis_ok));
    row(
        &mut o,
        &format!("ask (golden {golden_asks}/{n})"),
        &|_, g| rate(g.asks),
    );
    row(&mut o, "**paid where the label holds**", &|_, g| {
        format!("**{}**", g.paid_against_label)
    });
    row(&mut o, "held where the label pays", &|_, g| {
        g.held_against_label.to_string()
    });
    row(
        &mut o,
        "guard: illegal action / sentence rewritten",
        &|_, g| format!("{} / {}", g.illegal, g.rewritten),
    );
    row(
        &mut o,
        "own answers (errors, the rule decided / other kind)",
        &|_, g| {
            format!(
                "{}/{total} ({} / {})",
                total - g.errors - g.fallbacks,
                g.errors,
                g.fallbacks
            )
        },
    );
    if k > 1 {
        row(&mut o, "same action in every rep", &|_, g| {
            format!("{}/{n}", g.consistent)
        });
    }
    row(&mut o, "latency p50 / p90", &|r, _| {
        let ms: Vec<f64> = r.reps.iter().flatten().map(|x| x.ms).collect();
        format!("{:.0} / {:.0} ms", pct(&ms, 50.0), pct(&ms, 90.0))
    });
    row(&mut o, "golden action not offered by code", &|_, g| {
        format!("{}/{n}", g.unreachable)
    });
    let _ = writeln!(
        o,
        "\n(always-{} would score {}/{n}.)",
        majority.1.as_str(),
        majority.0
    );

    // The rule's own answers only when another decider is graded beside it.
    let beside = kind != ActorKind::Rule;
    let _ = writeln!(
        o,
        "\nper scenario: golden{} | {name} (each rep), per column",
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
        let _ = writeln!(o, "{:02}. {}  [{}]", i + 1, s.name, s.case.id);
        let lists: Vec<String> = runs
            .iter()
            .map(|r| {
                let c = &r.all[i].case;
                let note = if !c.allowed.contains(&s.golden.action) {
                    " (golden not offered)"
                } else if c.allowed.len() == 1 {
                    " (decided by code)"
                } else {
                    ""
                };
                format!("{} [{}]{note}", r.mode.short(), actions(&c.allowed))
            })
            .collect();
        let _ = writeln!(
            o,
            "    allowed: {}; fixture listed [{}]",
            lists.join("; "),
            actions(&s.listed)
        );
        let answers: Vec<String> = runs
            .iter()
            .map(|r| {
                let first = &r.reps[0][i];
                let rule = if beside {
                    format!("rule {} | ", mark(&first.rule, &r.all[i]))
                } else {
                    String::new()
                };
                let mine: Vec<String> = r
                    .reps
                    .iter()
                    .map(|rows| mark(&rows[i].acted, &r.all[i]))
                    .collect();
                format!("{}: {rule}{}", r.mode.short(), mine.join(", "))
            })
            .collect();
        let _ = writeln!(
            o,
            "    golden {}/{} || {}",
            s.golden.action.as_str(),
            s.golden.basis.as_str(),
            answers.join(" || ")
        );
        let _ = writeln!(
            o,
            "    why ({}, rep 1): {}",
            runs[0].mode.short(),
            runs[0].reps[0][i].acted.why
        );
        for r in runs {
            for (rep, rows) in r.reps.iter().enumerate() {
                let note = match &rows[i].overruled {
                    None => continue,
                    Some(Overruled::Error(e)) => e.clone(),
                    Some(Overruled::Fallback) => "answered as another kind (fallback)".into(),
                    Some(Overruled::IllegalAction(a)) => format!(
                        "guard: {} is not permitted here; the rule decided",
                        a.as_str()
                    ),
                    Some(Overruled::Rewritten(note, said)) => {
                        format!("guard: {note}; it said: {said}")
                    }
                };
                let _ = writeln!(o, "    {} rep {}: {note}", r.mode.short(), rep + 1);
            }
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
        let all = scenarios(FIXTURE, Mode::Raw);
        assert!((12..=16).contains(&all.len()), "{} scenarios", all.len());
        let ids: BTreeSet<&str> = all.iter().map(|s| s.case.id.as_str()).collect();
        assert_eq!(ids.len(), all.len(), "case ids are unique");
        for s in &all {
            assert!(
                s.case.allowed.contains(&s.golden.action),
                "{}: the golden action is not in the fixture's allowed",
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
        // And the running system can reach every label: none asks for what code never offers.
        for s in scenarios(FIXTURE, Mode::System) {
            assert!(
                s.case.allowed.contains(&s.golden.action),
                "{}: code offers {:?}, not the golden {}",
                s.case.id,
                s.case.allowed,
                s.golden.action.as_str()
            );
        }
    }

    #[tokio::test]
    async fn the_rule_baseline_keeps_the_invariants() {
        let mut runs = Vec::new();
        for mode in [Mode::System, Mode::Raw] {
            let all = scenarios(FIXTURE, mode);
            let rows = run(&RuleBased, &all).await;
            for (s, r) in all.iter().zip(&rows) {
                assert!(
                    s.case.allowed.contains(&r.acted.action),
                    "{} ({mode:?}): the rule chose {} outside allowed",
                    s.case.id,
                    r.acted.action.as_str()
                );
                assert!(r.overruled.is_none(), "{} ({mode:?}): overruled", s.case.id);
                assert_eq!(
                    r.acted, r.rule,
                    "{} ({mode:?}): the production path changed the rule's own decision",
                    s.case.id
                );
                assert_eq!(
                    (r.acted.by, r.acted.model.as_deref()),
                    (ActorKind::Rule, None)
                );
            }
            runs.push(ModeRun {
                mode,
                all,
                reps: vec![rows],
            });
        }
        // Printed, not asserted: the rule is the control, not the answer key.
        println!(
            "{}",
            render(RuleBased.name(), RuleBased.kind(), FIXTURE, &runs)
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
        let all = scenarios(FIXTURE, Mode::System);
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
        assert_eq!(g.illegal + g.rewritten + g.errors + g.fallbacks, 0);

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
        assert_eq!((g.rewritten, g.illegal, g.errors), (1, 1, 1));
        assert_eq!(rows[banned].acted, rows[banned].rule, "the rule decides");
        assert_eq!(rows[all.len() - 1].acted, rows[all.len() - 1].rule);
    }
}
