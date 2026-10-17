//! The agent against the rule baseline on the golden scenarios of docs/113.
//!
//!     cargo run -p operator --example decide_eval -- --decider rule
//!     ANTHROPIC_API_KEY="$(cat ~/.tameion/anthropic.key)" \
//!       cargo run -p operator --example decide_eval -- --decider model --reps 3
//!
//! Two columns by default: "as the system would act" (`allowed` recomputed from each case's
//! facts with S10's own functions, the fixture's list ignored) and "raw judgment" (the
//! fixture's list). `--raw` runs the raw column alone, as before docs/60 P-198.
//!
//! `--fixture <file>` grades your own cases instead (same JSON shape as
//! `tests/fixtures/decide-scenarios.json`): the deciders were written after those labels
//! were public, so cases they never saw are the fair test.
//!
//! The grading code is `tests/decide_eval.rs`, compiled into both, so the gate and this run
//! cannot drift apart. Without a key the model run stops; it is never faked.

#[path = "../tests/decide_eval.rs"]
mod grader;

use grader::{Mode, ModeRun};
use operator::decide::{Decider, ModelDecider, RuleBased};

async fn report<D: Decider + Sync>(decider: &D, fixture: &str, modes: &[Mode], reps: usize) {
    let mut runs = Vec::with_capacity(modes.len());
    for &mode in modes {
        let all = grader::scenarios(fixture, mode);
        let mut rows = Vec::with_capacity(reps);
        for _ in 0..reps {
            rows.push(grader::run(decider, &all).await);
        }
        runs.push(ModeRun {
            mode,
            all,
            reps: rows,
        });
    }
    println!(
        "{}",
        grader::render(decider.name(), decider.kind(), fixture, &runs)
    );
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    let reps = flag("--reps").map_or(1, |r| r.parse().expect("--reps takes a whole number"));
    let fixture = match flag("--fixture") {
        Some(path) => std::fs::read_to_string(path).expect("--fixture names a readable file"),
        None => grader::FIXTURE.to_string(),
    };
    let modes: &[Mode] = if args.iter().any(|a| a == "--raw") {
        &[Mode::Raw]
    } else {
        &[Mode::System, Mode::Raw]
    };
    match flag("--decider") {
        Some("rule") => report(&RuleBased, &fixture, modes, 1).await,
        Some("model") => {
            let Some(model) = ModelDecider::from_env() else {
                eprintln!(
                    "no ANTHROPIC_API_KEY (or ANTHROPIC_AUTH_TOKEN): the model run needs one"
                );
                std::process::exit(2);
            };
            report(&model, &fixture, modes, reps).await
        }
        _ => {
            eprintln!(
                "usage: decide_eval --decider rule|model [--reps N] [--fixture FILE] [--raw]"
            );
            std::process::exit(2);
        }
    }
}
