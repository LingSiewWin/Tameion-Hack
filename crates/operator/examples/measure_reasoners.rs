//! Rule vs local model vs cloud model vs redacted cloud, over the same bills.
//!
//! `compare_reasoners` shows one scenario with reasons; this one counts
//! (docs/86): latency p50/p95, parse failures, agreement with the rule, and
//! what the deterministic verifier would do with each ordering when the cash
//! and the daily cap are carried down the list the way the contract carries
//! them. A model can only reorder; this measures what reordering costs.
//!
//!     ollama serve &
//!     ANTHROPIC_API_KEY=$(cat ~/.tameion/anthropic.key) \
//!     LOCAL_MODELS=llama3.1:8b,llama3.2:3b REPEATS=3 \
//!       cargo run -p operator --example measure_reasoners
//!
//! Without a key the cloud rows are skipped, not faked.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use ledger::{
    AccountId, Action, Actor, ActorKind, Authority, Confidence, Counterparty, Currency, Decision,
    Delta, Evidence, EvidenceKind, ExpectedEffect, Idempotency, Intent, Money, Obligation,
    ObligationKind, Precondition, Principal, Prioritised, Provenance, Seconds, Target, Timestamp,
    Validity, Verb, World, ordering_divergence, rank_by_urgency, verify,
};
use operator::claude::ClaudeReasoner;
use operator::local::{LocalReasoner, RedactedCloud, cloud_payload};
use operator::reasoner::{Reasoner, RuleBased};

const PER_ACTION_CAP: i128 = 2_500_000; // 2.5 USDC
const DAILY_CAP: i128 = 5_000_000; // 5 USDC

struct Scenario {
    name: String,
    balance: Money,
    bills: Vec<Obligation>,
}

/// Fixed-seed generator: the same scenarios on every run, no dependency.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }
}

#[allow(clippy::too_many_arguments)]
fn bill(
    now: Timestamp,
    who: &str,
    due_in_days: i64,
    amount: i128,
    conf: f64,
    samples: usize,
    interval_days: i64,
    amount_cv: f64,
) -> Obligation {
    let interval = Seconds::days(interval_days).unwrap();
    let next_due = now
        .checked_add(Seconds::days(due_in_days).unwrap())
        .unwrap();
    Obligation {
        counterparty: Counterparty::new(who),
        kind: ObligationKind::Subscription,
        typical_amount: Money::minor(amount, Currency::Usdc),
        interval,
        last_paid: next_due.checked_sub(interval).unwrap(),
        next_due,
        confidence: Confidence::new(conf).unwrap(),
        sample_count: samples,
        interval_cv: 0.02,
        amount_cv,
        evidence: (0..samples).map(|i| format!("0x{i:064x}")).collect(),
    }
}

fn scenarios(now: Timestamp) -> Vec<Scenario> {
    // The hand-written one from compare_reasoners.
    let mut out = vec![Scenario {
        name: "s00-compare".into(),
        balance: Money::minor(2_500_000, Currency::Usdc),
        bills: vec![
            bill(now, "0xa11ce", -4, 400_000, 0.95, 6, 30, 0.0),
            bill(now, "0xb0b", 2, 1_800_000, 0.88, 5, 30, 0.0),
            bill(now, "0xca101", 1, 700_000, 0.62, 3, 30, 0.0),
        ],
    }];
    let mut g = Lcg(0x7a3e_1015);
    for s in 1..=11 {
        let n = g.range(3, 6) as usize;
        let bills: Vec<Obligation> = (0..n)
            .map(|_| {
                let who = format!(
                    "0x{:016x}{:016x}{:08x}",
                    g.next(),
                    g.next(),
                    g.next() as u32
                );
                bill(
                    now,
                    &who,
                    g.range(-6, 7),
                    g.range(2, 30) as i128 * 100_000,
                    g.range(55, 98) as f64 / 100.0,
                    g.range(3, 8) as usize,
                    if g.range(0, 1) == 0 { 7 } else { 30 },
                    g.range(0, 30) as f64 / 100.0,
                )
            })
            .collect();
        let total: i128 = bills.iter().map(|b| b.typical_amount.to_minor()).sum();
        let balance = Money::minor(total * g.range(40, 110) as i128 / 100, Currency::Usdc);
        out.push(Scenario {
            name: format!("s{s:02}-{n}bills"),
            balance,
            bills,
        });
    }
    out
}

/// Walk an ordering through `ledger::verify` with the balance and the daily
/// spend carried down the list, as the contract carries them.
fn simulate(order: &[Prioritised], balance: Money, now: Timestamp) -> Vec<(String, Decision)> {
    let usdc = |m: i128| Money::minor(m, Currency::Usdc);
    let treasury = AccountId::new("treasury");
    let payees: BTreeSet<String> = order
        .iter()
        .map(|p| p.obligation.counterparty.to_string())
        .collect();
    let mut bal = balance;
    let mut spent = usdc(0);
    let at = now.as_unix() as u64;
    order
        .iter()
        .map(|p| {
            let o = &p.obligation;
            let key = o.cycle_key();
            let authority = Authority {
                per_action_cap: usdc(PER_ACTION_CAP),
                daily_cap: usdc(DAILY_CAP),
                spent_today: spent,
            };
            let action = Action {
                id: format!("act_{key}"),
                actor: Actor {
                    id: "measure".into(),
                    kind: ActorKind::Model,
                },
                principal: Principal {
                    id: "principal".into(),
                    treasury: treasury.clone(),
                },
                intent: Intent {
                    verb: Verb::Settle,
                    rationale: p.rationale.clone(),
                },
                target: Target {
                    id: o.counterparty.to_string(),
                    label: None,
                },
                amount: o.typical_amount,
                evidence: o
                    .evidence
                    .iter()
                    .map(|tx| Evidence {
                        kind: EvidenceKind::OnchainTransfer,
                        reference: tx.clone(),
                        provenance: Provenance::Onchain,
                    })
                    .collect(),
                preconditions: vec![
                    Precondition::NotAlreadySettled,
                    Precondition::CounterpartyKnown,
                    Precondition::EvidenceAtLeast(ledger::obligations::MIN_SAMPLES),
                ],
                authority: authority.clone(),
                expected_effects: vec![ExpectedEffect {
                    account: treasury.clone(),
                    delta: Delta::Decrease(o.typical_amount),
                }],
                idempotency: Idempotency {
                    key: key.clone(),
                    scope: "principal".into(),
                },
                validity: Validity {
                    not_before: at,
                    expires_at: at + 3600,
                },
            };
            let known: BTreeMap<String, String> =
                payees.iter().map(|p| (p.clone(), "fp".into())).collect();
            let world = World {
                now: at,
                treasury: treasury.clone(),
                balance: bal,
                authority,
                settled: BTreeSet::new(),
                known_payees: known.clone(),
                proposed_payee: known,
                // Autopilot's floor: the agent pays alone.
                required_preconditions: vec![Precondition::ConfidenceAtLeast(
                    ledger::action::CONFIDENCE_FLOOR_AUTOPILOT_BP,
                )],
                attested_payees: BTreeMap::new(),
                inferred_confidence: [(format!("principal:{key}"), o.confidence)].into(),
                purchase_orders: BTreeMap::new(),
                receipts: BTreeMap::new(),
                onchain_attested: Some(payees.clone()),
                cycles_behind: BTreeMap::new(),
                observed_amounts: BTreeMap::new(),
                issuer_blocked: BTreeSet::new(),
            };
            let d = verify(&action, &world).decision;
            if d == Decision::Allow {
                bal = bal.checked_sub(o.typical_amount).unwrap();
                spent = spent.checked_add(o.typical_amount).unwrap();
            }
            (key, d)
        })
        .collect()
}

#[derive(Default)]
struct Tally {
    latencies_ms: Vec<f64>,
    first_ms: Option<f64>,
    ok: usize,
    parse_fail: usize,
    other_fail: BTreeMap<&'static str, usize>,
    divergence: Vec<f64>,
    top1_same: usize,
    verdicts_same: usize,
    verdicts_total: usize,
    refused: usize,
    refused_rule_allowed: usize,
    top_pick_refused: usize,
    paid_minor: i128,
    rule_paid_minor: i128,
    repeat_same: usize,
    repeat_total: usize,
}

fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * s.len() as f64).ceil() as usize;
    s[rank.clamp(1, s.len()) - 1]
}

async fn measure<R: Reasoner>(
    r: &R,
    all: &[Scenario],
    repeats: usize,
    now: Timestamp,
    samples: &mut Vec<String>,
) -> Tally {
    let mut t = Tally::default();
    for s in all {
        let rule = rank_by_urgency(&s.bills, s.balance, now);
        let rule_v = simulate(&rule, s.balance, now);
        let mut first_order: Option<Vec<String>> = None;
        for rep in 0..repeats {
            let start = Instant::now();
            let got = r.prioritise(&s.bills, s.balance, now).await;
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            if t.first_ms.is_none() {
                t.first_ms = Some(ms);
            } else {
                t.latencies_ms.push(ms);
            }
            let ranked = match got {
                Ok(r) => r,
                Err(e) => {
                    println!(
                        "  {:<14} rep{rep} {ms:>8.0} ms  FAIL [{}] {e}",
                        s.name,
                        e.code()
                    );
                    match e.code() {
                        "reasoner-unusable" | "reasoner-wrong-population" => t.parse_fail += 1,
                        c => *t.other_fail.entry(c).or_default() += 1,
                    }
                    continue;
                }
            };
            t.ok += 1;
            let order: Vec<String> = ranked.iter().map(|p| p.obligation.cycle_key()).collect();
            match &first_order {
                None => first_order = Some(order.clone()),
                Some(f) => {
                    t.repeat_total += 1;
                    t.repeat_same += usize::from(*f == order);
                }
            }
            let div = ordering_divergence(&rule, &ranked).expect("same population");
            t.divergence.push(div as f64 / s.bills.len() as f64);
            t.top1_same += usize::from(order[0] == rule[0].obligation.cycle_key());

            let v = simulate(&ranked, s.balance, now);
            let rule_by: BTreeMap<&String, Decision> =
                rule_v.iter().map(|(k, d)| (k, *d)).collect();
            for (k, d) in &v {
                t.verdicts_total += 1;
                t.verdicts_same += usize::from(rule_by[k] == *d);
                if *d != Decision::Allow {
                    t.refused += 1;
                    t.refused_rule_allowed += usize::from(rule_by[k] == Decision::Allow);
                }
            }
            t.top_pick_refused += usize::from(v[0].1 != Decision::Allow);
            let paid = |vv: &[(String, Decision)]| -> i128 {
                vv.iter()
                    .filter(|(_, d)| *d == Decision::Allow)
                    .map(|(k, _)| {
                        s.bills
                            .iter()
                            .find(|b| b.cycle_key() == *k)
                            .unwrap()
                            .typical_amount
                            .to_minor()
                    })
                    .sum()
            };
            t.paid_minor += paid(&v);
            t.rule_paid_minor += paid(&rule_v);
            let verdicts: Vec<String> = v.iter().map(|(_, d)| d.to_string()).collect();
            println!(
                "  {:<14} rep{rep} {ms:>8.0} ms  ok  divergence {div}/{}  verdicts {}",
                s.name,
                s.bills.len(),
                verdicts.join(",")
            );
            if rep == 0 && s.name == "s00-compare" {
                for p in &ranked {
                    samples.push(format!(
                        "    {}. {:<8} {}",
                        p.rank + 1,
                        p.obligation.counterparty,
                        p.rationale
                    ));
                }
            }
        }
    }
    t
}

fn row(name: &str, t: &Tally) -> String {
    let calls = t.ok + t.parse_fail + t.other_fail.values().sum::<usize>();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    format!(
        "{name:<28} calls {calls:>3}  ok {:>3}  parse-fail {:>2} ({:.0}%)  other-fail {:?}\n\
         {:<28} latency first {:.0} ms, p50 {:.0} ms, p95 {:.0} ms (n={} after the first)\n\
         {:<28} vs rule: top-1 same {}/{}, mean positions moved {:.0}%, repeat-identical {}/{}\n\
         {:<28} verifier: verdict agreement {}/{} ({:.1}%), refused {} (of which rule-allowed {}), top pick refused {}\n\
         {:<28} paid under this ordering {:.2} USDC vs rule {:.2} USDC",
        t.ok,
        t.parse_fail,
        100.0 * t.parse_fail as f64 / calls.max(1) as f64,
        t.other_fail,
        "",
        t.first_ms.unwrap_or(f64::NAN),
        pct(&t.latencies_ms, 50.0),
        pct(&t.latencies_ms, 95.0),
        t.latencies_ms.len(),
        "",
        t.top1_same,
        t.ok,
        100.0 * mean(&t.divergence),
        t.repeat_same,
        t.repeat_total,
        "",
        t.verdicts_same,
        t.verdicts_total,
        100.0 * t.verdicts_same as f64 / t.verdicts_total.max(1) as f64,
        t.refused,
        t.refused_rule_allowed,
        t.top_pick_refused,
        "",
        t.paid_minor as f64 / 1e6,
        t.rule_paid_minor as f64 / 1e6,
    )
}

#[tokio::main]
async fn main() {
    let now = Timestamp::from_unix(1_758_326_400);
    let all = scenarios(now);
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let repeats: usize = env("REPEATS", "3").parse().unwrap();
    let models = env("LOCAL_MODELS", operator::local::DEFAULT_MODEL);
    let bills: usize = all.iter().map(|s| s.bills.len()).sum();
    println!(
        "{} scenarios, {bills} bills; caps per action {:.2} / daily {:.2} USDC; confidence floor 0.75 (Autopilot)",
        all.len(),
        PER_ACTION_CAP as f64 / 1e6,
        DAILY_CAP as f64 / 1e6
    );

    // What the cloud would see for s00, and the check that gates sending it.
    let s0 = &all[0];
    let (payload, red) = cloud_payload(&s0.bills, s0.balance, now);
    println!("\nredacted payload for s00 (exactly what RedactedCloud sends):\n  {payload}");
    println!("  leak check: {:?}", red.check(&payload));

    let mut summary = Vec::new();
    let mut samples = Vec::new();

    println!("\n== rule-based ==");
    let t = measure(&RuleBased, &all, 1, now, &mut samples).await;
    summary.push(row("rule-based", &t));
    samples.push("  ^ rule-based on s00".into());

    println!("\n== local model down (OLLAMA_HOST=127.0.0.1:1) ==");
    let down = LocalReasoner::new("127.0.0.1:1", "llama3.1:8b", Duration::from_secs(45));
    let start = Instant::now();
    let e = down
        .prioritise(&s0.bills, s0.balance, now)
        .await
        .unwrap_err();
    println!(
        "  error after {:.0} ms: [{}] retryable={} -- Operator::run_once orders by rank_by_urgency and logs 'rule-based (fallback from {})'",
        start.elapsed().as_secs_f64() * 1000.0,
        e.code(),
        e.retryable(),
        down.name()
    );

    for m in models.split(',').map(str::trim).filter(|m| !m.is_empty()) {
        println!("\n== ollama:{m} (local) ==");
        let r = LocalReasoner::new(operator::local::DEFAULT_URL, m, Duration::from_secs(120));
        let t = measure(&r, &all, repeats, now, &mut samples).await;
        summary.push(row(r.name(), &t));
        samples.push(format!("  ^ {} on s00", r.name()));
    }

    let cloud_repeats: usize = env("CLOUD_REPEATS", "1").parse().unwrap();
    if ClaudeReasoner::from_env().is_some() && cloud_repeats > 0 {
        let raw = ClaudeReasoner::from_env().unwrap();
        println!("\n== {} (cloud, sees everything) ==", raw.name());
        let t = measure(&raw, &all, cloud_repeats, now, &mut samples).await;
        summary.push(row(&format!("{} raw", raw.name()), &t));
        samples.push(format!("  ^ {} raw on s00", raw.name()));

        let red = RedactedCloud::new(ClaudeReasoner::from_env().unwrap());
        println!("\n== {} (cloud, placeholders only) ==", red.name());
        let t = measure(&red, &all, cloud_repeats, now, &mut samples).await;
        summary.push(row(red.name(), &t));
        samples.push(format!("  ^ {} on s00", red.name()));
    } else {
        println!("\n(no ANTHROPIC_API_KEY: cloud rows skipped)");
    }

    println!("\n== rationales on s00 (rank, payee, reason) ==");
    for l in &samples {
        println!("{l}");
    }
    println!("\n== summary ==");
    for r in &summary {
        println!("{r}\n");
    }
}
