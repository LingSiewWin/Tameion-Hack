//! Replay real treasury histories through the verifier, as if every payment
//! had been an agent's proposal, with address-poisoning look-alikes injected.
//!
//! The test agreed in docs/36 (Round 2, test 3): at most 10 questions to a
//! human per 100 legitimate payments, and no look-alike ever reaches ALLOW or
//! REVIEW. Run from the data directory:
//!
//!   cargo run --release --bin replay -- transfers.tsv incoming.tsv
//!
//! The model of the principal is deliberately simple and stated: the first
//! payment to a new payee is a question (the contract pays only attested
//! addresses); the principal answers yes, and from then on that payee is
//! attested. Everything else is the verifier as shipped, unchanged.
//!
//! With `ONBOARD_DAYS=N` the principal is modelled as onboarding instead: for
//! the first N days of each treasury's history the owner pays by hand and the
//! agent only watches (those payments are history, not proposals); then every
//! payee paid more than dust at least twice in that window -- minus any that
//! looks like another on the list -- is approved in ONE batch (`setPayees`, one
//! tap of a passkey), which counts as one question. After that, a first
//! payment to a payee not on the list is a question, exactly as before. Unset,
//! the replay is the 10-01 model, unchanged.
//!
//! With `AGENT_SCOPE=1` a payment is a proposal only if the agent could have
//! made it: the payee already has `ledger::MIN_SAMPLES` (3) earlier non-dust
//! payments, the least `detect` needs to infer an obligation. Earlier payments
//! are the owner paying by hand -- history, not proposals. (Generous to the
//! agent in one way and stated: it ignores the regularity bound, which would
//! leave even fewer proposals.) Look-alike injection is unchanged, so part 1
//! is no easier.
//!
//! With `BATCH_DAILY=1`, new-payee questions arising on the same UTC day in
//! the same treasury are one question: what `run_arc` actually does, since a
//! pass hands every unapproved payee to the principal as a single
//! `setPayees` link, and an agent run daily makes at most one such request
//! a day.
//!
//! A look-alike is an address that appears in a treasury's history only with
//! zero-value or dust transfers, in either direction, and shares its first 4
//! and last 4 hex characters with an address the treasury really paid --
//! the same definition docs/21 §7 measured with `poison.py`. It is injected as
//! a proposal at every later real payment to the payee it imitates, which is
//! the moment a person or an agent copying "the payee" from history would pick
//! it up.
use ledger::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

const DUST_MINOR: i128 = 1_000_000;

#[derive(Clone)]
struct Row {
    safe: String,
    other: String,
    minor: i128,
    at: u64,
    out: bool,
}

fn read(path: &str, out: bool) -> Vec<Row> {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 5 {
                return None;
            }
            Some(Row {
                safe: f[0].to_ascii_lowercase(),
                other: f[2].to_ascii_lowercase(),
                minor: f[3].parse().ok()?,
                at: f[4].parse().ok()?,
                out,
            })
        })
        .collect()
}

fn usdc(minor: i128) -> Money {
    Money::minor(minor, Currency::Usdc)
}

fn authority() -> Authority {
    // Caps far above any payment in the data, so only the payee rules decide.
    Authority {
        per_action_cap: usdc(1_000_000_000_000_000),
        daily_cap: usdc(1_000_000_000_000_000),
        spent_today: usdc(0),
    }
}

fn treasury() -> AccountId {
    AccountId::new("asset:usdc")
}

fn proposal(to: &str, minor: i128, at: u64, n: usize) -> Action {
    Action {
        id: format!("replay_{n}"),
        actor: Actor { id: "replay".into(), kind: ActorKind::Rule },
        principal: Principal { id: "replay".into(), treasury: treasury() },
        intent: Intent { verb: Verb::Settle, rationale: String::new() },
        target: Target { id: to.into(), label: None },
        amount: usdc(minor),
        evidence: vec![Evidence {
            kind: EvidenceKind::OnchainTransfer,
            reference: format!("replay_{n}"),
            provenance: Provenance::Onchain,
        }],
        preconditions: vec![],
        authority: authority(),
        expected_effects: vec![ExpectedEffect {
            account: treasury(),
            delta: Delta::Decrease(usdc(minor)),
        }],
        idempotency: Idempotency { key: format!("{to}@{at}#{n}"), scope: "replay".into() },
        validity: Validity { not_before: at.saturating_sub(60), expires_at: at + 3600 },
    }
}

fn world(at: u64, known: &BTreeSet<String>, attested: &BTreeSet<String>) -> World {
    let payees: BTreeMap<String, String> = known.iter().map(|k| (k.clone(), k.clone())).collect();
    World {
        now: at,
        treasury: treasury(),
        balance: usdc(1_000_000_000_000_000),
        authority: authority(),
        settled: BTreeSet::new(),
        known_payees: payees.clone(),
        proposed_payee: payees,
        required_preconditions: vec![
            Precondition::NotAlreadySettled,
            Precondition::PayeeUnchanged,
            Precondition::EvidenceAtLeast(1),
        ],
        attested_payees: BTreeMap::new(),
        inferred_confidence: BTreeMap::new(),
        purchase_orders: BTreeMap::new(),
        receipts: BTreeMap::new(),
        onchain_attested: Some(attested.clone()),
        cycles_behind: BTreeMap::new(),
    }
}

fn key(a: &str) -> Option<(String, String)> {
    let b = a.strip_prefix("0x")?;
    (b.len() == 40).then(|| (b[..4].to_string(), b[36..].to_string()))
}

#[derive(Default)]
struct Tally {
    legit: usize,
    allow: usize,
    ask: usize,
    block: usize,
    ask_new_payee: usize,
    ask_other: usize,
    block_reasons: BTreeMap<String, usize>,
    injected: usize,
    lookalike_blocked: usize,
    lookalike_asked: usize,
    lookalike_allowed: usize,
    recurring_legit: usize,
    recurring_ask: usize,
    oneoff_legit: usize,
    oneoff_ask: usize,
    safes: usize,
    safes_with_lookalikes: usize,
    onboard_batches: usize,
    merged: usize,
    onboard_payees: usize,
    watched: usize,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (out_path, in_path) = (
        args.get(1).map(String::as_str).unwrap_or("transfers.tsv"),
        args.get(2).map(String::as_str).unwrap_or("incoming.tsv"),
    );
    let onboard: Option<u64> = std::env::var("ONBOARD_DAYS").ok().map(|d| d.parse().expect("ONBOARD_DAYS"));
    let agent_scope = std::env::var("AGENT_SCOPE").is_ok_and(|v| v == "1");
    let batch_daily = std::env::var("BATCH_DAILY").is_ok_and(|v| v == "1");
    let mut rows = read(out_path, true);
    rows.extend(read(in_path, false));

    let mut by_safe: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for r in rows {
        by_safe.entry(r.safe.clone()).or_default().push(r);
    }

    let mut t = Tally::default();
    let mut n = 0usize;
    let mut examples: Vec<String> = Vec::new();

    for (safe, mut hist) in by_safe {
        t.safes += 1;
        hist.sort_by_key(|r| (r.at, !r.out));

        // Genuine payees: paid more than dust at least once.
        let genuine: BTreeSet<String> =
            hist.iter().filter(|r| r.out && r.minor > DUST_MINOR).map(|r| r.other.clone()).collect();
        // Look-alikes: seen only with dust, imitating a genuine payee.
        let mut dust_only: BTreeMap<String, u64> = BTreeMap::new();
        for r in &hist {
            if r.minor <= DUST_MINOR && !genuine.contains(&r.other) {
                dust_only.entry(r.other.clone()).or_insert(r.at);
            }
        }
        let mut imitators: BTreeMap<String, Vec<(String, u64)>> = BTreeMap::new();
        for (fake, first_seen) in &dust_only {
            let Some(k) = key(fake) else { continue };
            for real in &genuine {
                if key(real).as_ref() == Some(&k) {
                    imitators.entry(real.clone()).or_default().push((fake.clone(), *first_seen));
                }
            }
        }
        if !imitators.is_empty() {
            t.safes_with_lookalikes += 1;
        }

        let mut known: BTreeSet<String> = BTreeSet::new();
        let mut attested: BTreeSet<String> = BTreeSet::new();
        // A recurring payee is paid more than dust at least three times in
        // the history: the kind of obligation the agent exists to pay.
        let mut times_paid: BTreeMap<String, usize> = BTreeMap::new();
        for r in hist.iter().filter(|r| r.out && r.minor > DUST_MINOR) {
            *times_paid.entry(r.other.clone()).or_default() += 1;
        }

        let paid: Vec<&Row> = hist.iter().filter(|r| r.out && r.minor > DUST_MINOR).collect();
        let cutover = onboard.and_then(|d| paid.first().map(|f| f.at + d * 86_400));
        let mut onboarded = cutover.is_none();
        let mut prior: BTreeMap<String, usize> = BTreeMap::new();
        let mut asked_on: BTreeSet<u64> = BTreeSet::new();

        for r in paid {
            if let Some(t0) = cutover {
                if r.at < t0 {
                    // Learning period: the owner pays, the agent watches.
                    t.watched += 1;
                    known.insert(r.other.clone());
                    continue;
                }
                if !onboarded {
                    onboarded = true;
                    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
                    for w in hist.iter().filter(|w| w.out && w.minor > DUST_MINOR && w.at < t0) {
                        *seen.entry(w.other.as_str()).or_default() += 1;
                    }
                    let list: Vec<&str> = seen.iter().filter(|(_, n)| **n >= 2).map(|(p, _)| *p).collect();
                    let batch: Vec<&str> = list
                        .iter()
                        .copied()
                        .filter(|p| !list.iter().any(|q| looks_alike(p, q)))
                        .collect();
                    if !batch.is_empty() {
                        t.ask += 1;
                        t.onboard_batches += 1;
                        t.onboard_payees += batch.len();
                        attested.extend(batch.iter().map(|p| p.to_string()));
                    }
                }
            }
            // The poisoned proposal first, at the moment the real payee is
            // about to be paid again.
            if known.contains(&r.other) {
                if let Some(fakes) = imitators.get(&r.other) {
                    for (fake, first_seen) in fakes {
                        if *first_seen >= r.at {
                            continue;
                        }
                        n += 1;
                        t.injected += 1;
                        let v = verify(&proposal(fake, r.minor, r.at, n), &world(r.at, &known, &attested));
                        match v.decision {
                            Decision::Block => t.lookalike_blocked += 1,
                            Decision::Review => {
                                t.lookalike_asked += 1;
                                if examples.len() < 5 {
                                    examples.push(format!("ASKED  {safe} {fake} imitating {}: {v}", r.other));
                                }
                            }
                            Decision::Allow => {
                                t.lookalike_allowed += 1;
                                if examples.len() < 5 {
                                    examples.push(format!("ALLOWED {safe} {fake} imitating {}", r.other));
                                }
                            }
                        }
                    }
                }
            }

            // Outside the agent's reach: the owner pays by hand.
            let before = prior.get(&r.other).copied().unwrap_or(0);
            *prior.entry(r.other.clone()).or_default() += 1;
            if agent_scope && before < ledger::obligations::MIN_SAMPLES {
                t.watched += 1;
                known.insert(r.other.clone());
                continue;
            }

            // The real payment, as a proposal.
            n += 1;
            t.legit += 1;
            let recurring = times_paid.get(&r.other).copied().unwrap_or(0) >= 3;
            if recurring { t.recurring_legit += 1 } else { t.oneoff_legit += 1 }
            let v = verify(&proposal(&r.other, r.minor, r.at, n), &world(r.at, &known, &attested));
            match v.decision {
                Decision::Allow => t.allow += 1,
                Decision::Review => {
                    let only_new_payee = v
                        .problems()
                        .all(|c| c.name == "payee-attested");
                    // Same day, same treasury: already in today's one link.
                    let merged = batch_daily && only_new_payee && !asked_on.insert(r.at / 86_400);
                    if merged {
                        t.merged += 1;
                    } else {
                        t.ask += 1;
                        if recurring { t.recurring_ask += 1 } else { t.oneoff_ask += 1 }
                        if only_new_payee {
                            t.ask_new_payee += 1;
                        } else {
                            t.ask_other += 1;
                        }
                    }
                    // The principal answers yes to a genuine payee.
                    attested.insert(r.other.clone());
                }
                Decision::Block => {
                    t.block += 1;
                    for c in v.problems() {
                        *t.block_reasons.entry(c.name.to_string()).or_default() += 1;
                    }
                    if examples.len() < 5 {
                        examples.push(format!("BLOCKED legit {safe} {}: {v}", r.other));
                    }
                }
            }
            known.insert(r.other.clone());
        }
    }

    let per100 = |x: usize| 100.0 * x as f64 / t.legit.max(1) as f64;
    if let Some(d) = onboard {
        println!("model: onboarding after {d} days (owner pays by hand meanwhile; one batch approval)");
        println!("payments in the learning periods         {}  (history, not proposals)", t.watched);
        println!("onboarding batches approved             {}  covering {} payees  (one question each)",
            t.onboard_batches, t.onboard_payees);
    } else {
        println!("model: first payment to every payee is a question (10-01)");
    }
    if batch_daily {
        println!("batching: one approval request per treasury per day ({} new payees rode along in an earlier request)", t.merged);
    }
    if agent_scope {
        println!("scope: only payments the agent could propose (payee has >= {} earlier payments)", ledger::obligations::MIN_SAMPLES);
        println!("payments the owner made by hand          {}  (incl. any learning period)", t.watched);
    }
    println!("treasuries replayed                     {}", t.safes);
    println!("treasuries with a planted look-alike    {}", t.safes_with_lookalikes);
    println!("legitimate payments proposed            {}", t.legit);
    println!("  allowed without asking                {}", t.allow);
    println!("  asked a human                         {}  ({:.2} per 100)", t.ask, per100(t.ask));
    println!("    of which: onboarding batches        {}", t.onboard_batches);
    println!("    of which: first payment to a payee  {}", t.ask_new_payee);
    println!("    of which: anything else             {}", t.ask_other);
    println!("  blocked                               {}  ({:.2} per 100)", t.block, per100(t.block));
    for (k, v) in &t.block_reasons {
        println!("    {k:<34}  {v}");
    }
    let rate = |a: usize, b: usize| 100.0 * a as f64 / b.max(1) as f64;
    println!("  payments to recurring payees (>=3)    {}  asked {}  ({:.2} per 100)",
        t.recurring_legit, t.recurring_ask, rate(t.recurring_ask, t.recurring_legit));
    println!("  payments to one-off payees (<3)       {}  asked {}  ({:.2} per 100)",
        t.oneoff_legit, t.oneoff_ask, rate(t.oneoff_ask, t.oneoff_legit));
    println!("look-alike proposals injected           {}", t.injected);
    println!("  refused (BLOCK)                       {}", t.lookalike_blocked);
    println!("  asked a human (REVIEW)                {}", t.lookalike_asked);
    println!("  paid (ALLOW)                          {}", t.lookalike_allowed);
    let pass = per100(t.ask) <= 10.0 && t.lookalike_asked == 0 && t.lookalike_allowed == 0;
    println!(
        "TEST 3 (docs/36): asks <= 10 per 100 and no look-alike asked or allowed -> {}",
        if pass { "PASS" } else { "FAIL" }
    );
    let poison_pass = t.lookalike_asked == 0 && t.lookalike_allowed == 0 && t.block == 0;
    println!(
        "  part 1, look-alikes: every one refused, no legitimate payment refused -> {}",
        if poison_pass { "PASS" } else { "FAIL" }
    );
    println!(
        "  part 2, asks per 100 legitimate payments <= 10 -> {} ({:.2})",
        if per100(t.ask) <= 10.0 { "PASS" } else { "FAIL" },
        per100(t.ask)
    );
    for e in examples {
        println!("  e.g. {e}");
    }
}
