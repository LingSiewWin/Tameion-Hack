//! The loop: read what is held, work out what is owed, decide, verify, pay
//! what clears, escalate what does not, and commit a root of the record.
//!
//! # Where each responsibility lives
//!
//! ```text
//!   arc        reads the chain, sends the transaction
//!    │
//!   ledger     detects obligations, builds the Action, verifies it, logs it
//!    │
//!   operator   sequences the above and nothing else
//! ```
//!
//! This crate contains no arithmetic on money, no policy thresholds, and no
//! decision. It arranges calls. That is deliberate: the interesting failures
//! in an agent that spends money are decisions, and decisions are testable
//! only when they are pure functions of stated inputs.
//!
//! # The duplicate guard lives on chain
//!
//! A retry after a timeout is how one payment becomes two, and it is the
//! failure an autonomous payer is most likely to cause. The defence is not a
//! file that a crashed process may not have flushed: `BudgetPolicy` records
//! every spent action hash, so the guard survives a lost disk, a restarted
//! container and a fresh machine. The log is the audit record; the chain is
//! the authority on what has already been paid.

// Every line this crate and the service print passes `guard::scrub` first (docs/114 row 9, docs/60 P-250):
// these shadow std's `println!` / `eprintln!` in every module below, and in a binary that imports them.
#[macro_export]
macro_rules! println {
    () => { ::std::println!() };
    ($($t:tt)*) => { ::std::println!("{}", $crate::guard::scrub(&::std::format!($($t)*))) };
}
#[macro_export]
macro_rules! eprintln {
    () => { ::std::eprintln!() };
    ($($t:tt)*) => { ::std::eprintln!("{}", $crate::guard::scrub(&::std::format!($($t)*))) };
}

pub mod agent_api;
pub mod audit;
pub mod claude;
pub mod decide;
pub mod demo;
pub mod desk;
pub mod desk_invite;
pub mod desk_live;
pub mod guard;
pub mod hedge_advice;
pub mod invoice;
pub mod level1;
pub mod local;
pub mod mcp;
pub mod meter;
pub mod parties;
pub mod pay;
pub mod price;
pub mod push;
pub mod reasoner;
pub mod recall;
pub mod refusal;
pub mod sig;
pub mod statement;
pub mod watch;
pub mod x402;

use alloy::primitives::{Address, B256};
use arc::{
    ArcClient, ArcError, PolicyMode, PreparedCall, prepare_payee_attestation,
    prepare_principal_settlement,
};
use core::fmt;
use ledger::log::{DecisionLog, EventBody, Hash32};
use ledger::{
    AccountId, Action, ActionHash, Actor, ActorKind, Authority, Decision, Delta, DetectionConfig,
    DocumentRecord, Evidence, EvidenceKind, ExpectedEffect, Idempotency, Intent, Money, Obligation,
    Precondition, Principal, Prioritised, Provenance, Seconds, Target, Timestamp, Transfer,
    Validity, Verb, Verdict, World, action_hash, detect, rank_by_urgency, settlement_key, upcoming,
    verify,
};
use reasoner::{Reasoner, ReasonerError};

/// Most `eth_getLogs` round trips one pass will make before refusing.
///
/// At Arc's 1,000-block window this is 2,000 blocks short of a day's worth
/// of history on a fast chain, which is plenty for a treasury whose funding
/// block is known and far too little to hide a runaway scan.
const MAX_HISTORY_CALLS: u64 = 2_000;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, thiserror::Error)]
pub enum OperatorError {
    #[error(transparent)]
    Chain(#[from] ArcError),
    #[error(transparent)]
    Reasoner(#[from] ReasonerError),
    #[error("no signer configured, so nothing can be executed")]
    ReadOnly,
    #[error("could not write the audit record before anchoring: {0}")]
    AuditWrite(String),
    #[error("obligation for {0} carries no amount that can be paid")]
    Unpayable(String),
    #[error("timestamp {0} is before the Unix epoch; the chain clock is wrong")]
    TimeBeforeEpoch(i64),
    #[error(
        "history_from_block is {from}, head is {head}: that is {blocks} blocks and \
         {calls} round trips. Set Config::history_from_block to the block this \
         treasury was funded at."
    )]
    HistoryTooWide {
        from: u64,
        head: u64,
        blocks: u64,
        calls: u64,
    },
    #[error(
        "the chain records {cycle_key} as settled, which is further ahead than any pass \
         could have paid it: the settlement record and this obligation's schedule disagree"
    )]
    SettledAhead { cycle_key: String },
    #[error(
        "a payment's read-back did not match and HALT could not be written ({0}); \
         this pass stopped paying and its record is saved, but the next pass will \
         not see the halt: write the file by hand"
    )]
    HaltWrite(String),
}

/// Everything the operator needs that is not read from a chain.
#[derive(Debug, Clone)]
pub struct Config {
    /// The address that holds funds and makes payments: the policy contract.
    pub treasury: Address,
    /// `BudgetPolicy`, which enforces the limits.
    pub policy: Address,
    /// `MemoryAnchor`, which takes the log root.
    pub memory_anchor: Address,
    /// Where this operator's audit record goes (`audit::persist`). `None`
    /// means `TAMEION_LOG_DIR` / `runtime/log`; the hosted service keeps one
    /// directory per tenant.
    pub audit_dir: Option<std::path::PathBuf>,
    /// Identifier for the organisation on whose behalf the agent acts.
    pub principal_id: String,
    /// The ledger account the treasury maps to.
    pub treasury_account: AccountId,
    /// Other accounts whose outgoing payments are the principal's history:
    /// typically the wallet the owner paid bills from before there was an
    /// agent. A payee found only there is a real obligation the policy has
    /// never been told about, which is exactly what the principal should be
    /// asked to approve. Transfers among these accounts, the treasury and the
    /// policy are the owner moving their own money and are dropped.
    pub also_history: Vec<Address>,
    /// First block of history to read.
    pub history_from_block: u64,
    /// First block to scan for the policy's own transfers and the agent's
    /// payments; `None` means `history_from_block`. A policy deployed at a
    /// known block has nothing before it, and on a public RPC every 10,000
    /// blocks is a round trip: a tenant's first pass went from three scans
    /// of the whole window to one.
    pub policy_from_block: Option<u64>,
    /// How obligations are recovered from history, including the dust line
    /// below which a transfer is not evidence of a payee.
    pub detection: DetectionConfig,
    /// Bills the owner told us about (docs/55 step 6, PLAN 5.8): used when
    /// history has not shown a payee yet, so a new treasury does not wait
    /// three cycles before the agent knows what it owes. History wins once
    /// it exists for the same payee.
    pub seeded: Vec<Obligation>,
    /// Early-payment terms the owner's invoices offer (PLAN 5.56): pay a bill
    /// at least `lead` before it is due and pay `discount_bps` less. Taken
    /// only while every bill in the horizon stays covered ([`early_payments`]).
    pub early: Vec<EarlyTerms>,
    /// How far ahead to look for obligations: the liquidity view.
    pub horizon: Seconds,
    /// How early to pay. An obligation is acted on once it falls due within
    /// this, or within a tenth of its own interval if that is shorter -- so
    /// a compressed demonstration cadence of minutes is not paid days ahead,
    /// and no bill is ever paid while the cycle before it is still to come.
    pub pay_lead: Seconds,
    /// How long a verified action stays good for.
    pub validity_window: Seconds,
    /// How long an escalation may sit unanswered before the agent reports it
    /// as overdue.
    ///
    /// An escalation that nobody answers is a failure, not a safe resting
    /// state: the bill stays unpaid and, without this, nothing in the system
    /// ever says so. Staleness is measured from the obligation's own due
    /// date, which is derived from chain history, so it survives a restart
    /// and needs no stored log.
    ///
    /// Note honestly what this is not: a config value the operator can edit,
    /// not an immutable on-chain guarantee. It makes the gap *visible*; it
    /// does not make the payment happen.
    pub escalation_deadline: Seconds,
    /// Checks the principal requires on every action, whatever the agent
    /// proposes. See [`ledger::World::required_preconditions`].
    pub required_preconditions: Vec<Precondition>,
    /// Purchase orders the principal's systems hold, keyed by reference.
    ///
    /// Empty by default, and an empty store is not the same as a disabled
    /// check: with `Precondition::ThreeWayMatch` required and nothing here,
    /// every action citing an order is refused for citing one that does not
    /// exist. That is the correct behaviour -- a document store nobody
    /// populated should not silently approve -- but it means the principal
    /// turns the match on and fills these in together, or not at all.
    ///
    /// The obligations the agent recovers from transfer history have no
    /// documents behind them by construction, which is why the match is
    /// opt-in rather than always on. See `docs/18-action-ir.md`.
    pub purchase_orders: BTreeMap<String, DocumentRecord>,
    /// Goods-received notes, keyed by reference. See `purchase_orders`.
    pub receipts: BTreeMap<String, DocumentRecord>,
    /// Counterparties whose payment details changed and whom the principal
    /// has since vouched for, keyed by address.
    ///
    /// A changed payee is refused rather than escalated, so this is the only
    /// way through for a counterparty that legitimately moved. It is
    /// deliberately a value the *principal* sets out of band rather than
    /// anything the agent can reach: an agent that could attest its own
    /// payees would have no payee control at all, the same way an agent that
    /// could raise its own cap would have no cap.
    pub attested_payees: BTreeMap<String, String>,
    /// Every reference the service offered the owner for a bill-cycle, with
    /// that cycle's key (`payee@due`): the proposal hashes and settlement keys
    /// of the passes' Needs-you rows. An owner payment under one of them pays
    /// exactly that cycle (docs/60 P-210); nothing else ties a hand payment to
    /// a bill, so payee and amount alone never do (P-213).
    pub offered: BTreeMap<B256, String>,
}

/// A configuration that would run but could not work.
///
/// Carries every problem rather than the first, because a caller who fixes
/// one and re-runs to find the next learns the list one round trip at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub problems: Vec<String>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "configuration refused ({} problems):",
            self.problems.len()
        )?;
        for p in &self.problems {
            writeln!(f, "  - {p}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Sensible defaults for everything except the addresses.
    pub fn new(treasury: Address, policy: Address, memory_anchor: Address) -> Self {
        Config {
            treasury,
            policy,
            memory_anchor,
            audit_dir: None,
            principal_id: "org_tameion".into(),
            treasury_account: AccountId::new("asset:usdc"),
            also_history: Vec::new(),
            // Deliberately not zero. A real chain refuses a log query that
            // wide -- Arc testnet answers 1,000 blocks and refuses 2,000 --
            // and while `arc` now chunks the range, starting at block zero
            // on a 63-million-block chain means 63,000 round trips. The
            // caller must say where this treasury's history starts.
            history_from_block: 0,
            policy_from_block: None,
            detection: DetectionConfig::default(),
            seeded: Vec::new(),
            early: Vec::new(),
            horizon: Seconds::days(30).expect("30 days fits"),
            // A person pays a few days before the due date, not a month.
            // Capped per obligation at a tenth of its interval.
            pay_lead: Seconds::days(3).expect("3 days fits"),
            validity_window: Seconds::HOUR,
            escalation_deadline: Seconds::days(7).expect("7 days fits"),
            required_preconditions: vec![
                Precondition::NotAlreadySettled,
                Precondition::PayeeUnchanged,
                Precondition::EvidenceAtLeast(1),
                Precondition::ProvenanceAtLeast(Provenance::CounterpartyAsserted),
                // 0.75. Not tuned -- chosen so that the three obligations the
                // end-to-end run recovers split the way a person would split
                // them: the 0.85 subscription and the 0.78 recurring payment
                // are the agent's to make, and the 0.62 one, which rests on
                // three samples rather than six, is not.
                //
                // Set as a default rather than left to the caller because a
                // safety floor nobody turns on is the same as no floor, and
                // this one had been computed-but-unread since the project
                // started. If it is wrong it should be wrong loudly and get
                // moved, not quietly absent.
                Precondition::ConfidenceAtLeast(ledger::action::CONFIDENCE_FLOOR_AUTOPILOT_BP),
            ],
            // Empty by default: nothing has changed yet, so there is nothing
            // to have vouched for.
            attested_payees: BTreeMap::new(),
            offered: BTreeMap::new(),
            // Empty by default, and deliberately not accompanied by a
            // `ThreeWayMatch` in `required_preconditions` above: requiring a
            // match against stores nobody has filled refuses everything.
            purchase_orders: BTreeMap::new(),
            receipts: BTreeMap::new(),
        }
    }

    /// Reject a configuration that cannot do what it says.
    ///
    /// Not a lint. Each case here is one where the agent would run, refuse or
    /// escalate everything, and look like it was working -- the worst way for
    /// a policy to be wrong, because a treasury that pays nothing looks
    /// exactly like a treasury with nothing due.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut problems = Vec::new();

        let wants_match = self
            .required_preconditions
            .contains(&Precondition::ThreeWayMatch);

        if wants_match && self.purchase_orders.is_empty() && self.receipts.is_empty() {
            problems.push(
                "ThreeWayMatch is required but no purchase orders or receipts are configured:                  every action would be refused for citing documents that do not exist"
                    .to_string(),
            );
        }

        // The loop builds actions from obligations recovered out of transfer
        // history, and `build_action` cites `OnchainTransfer` evidence and
        // nothing else -- an inferred subscription has no invoice behind it
        // by construction. So a principal who requires the match on *this*
        // path escalates every payment forever, having configured what looks
        // like a stricter policy and actually got a stopped one.
        if wants_match {
            problems.push(
                "ThreeWayMatch is required, but actions built from inferred obligations cite                  no invoice, order or receipt, so every one of them will escalate. The match                  belongs on an invoice-driven path, which this loop does not yet have"
                    .to_string(),
            );
        }

        if self.pay_lead.as_i64() <= 0 {
            problems.push(
                "pay_lead is not positive: nothing is ever paid before it is late".to_string(),
            );
        }
        if self.horizon.as_i64() <= 0 {
            problems.push("horizon is not positive: nothing is ever due".to_string());
        }

        if self.validity_window.as_i64() <= 0 {
            problems.push(
                "validity_window is not positive: every action expires before it can execute"
                    .to_string(),
            );
        }

        for (counterparty, fingerprint) in &self.attested_payees {
            if fingerprint.is_empty() {
                problems.push(format!(
                    "attested_payees[{counterparty}] is empty: an empty attestation matches                      nothing and silently leaves the payee blocked"
                ));
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(ConfigError { problems })
        }
    }
}

/// Step `o` past every cycle `settled` says is paid.
///
/// The bound comes from time, not from a constant. The agent pays a cycle at
/// most a pay lead before it falls due, so a settled cycle cannot lie more
/// than one interval past `now`; the steps needed are the intervals elapsed
/// since `o.next_due`, plus one. A fixed bound of 36 -- the first version --
/// stopped a weekly bill after nine months of the agent paying it: the pass
/// then sat on a settled cycle, skipped it as already paid, and never paid
/// that bill again, silently. If the bound is reached and the cycle is still
/// settled, the record claims payments in the future; that is an error, not
/// something to step around.
async fn step_past_settled<F, Fut>(
    o: &mut Obligation,
    now: Timestamp,
    settled: F,
) -> Result<(), OperatorError>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<bool, ArcError>>,
{
    let step = o.interval.as_i64().max(1);
    let elapsed = (now - o.next_due).as_i64().max(0);
    let bound = elapsed / step + 2;
    for _ in 0..bound {
        if !settled(o.cycle_key()).await? {
            return Ok(());
        }
        o.next_due = o.next_due + o.interval;
    }
    Err(OperatorError::SettledAhead {
        cycle_key: o.cycle_key(),
    })
}

/// Whether the owner paid this bill-cycle by hand under a reference the
/// service offered for exactly this cycle (docs/60 P-175, P-210). `owner` is the
/// owner's `SettledByPrincipal` action hashes, `offered` every reference a pass
/// handed the owner, with its cycle. Matched by the bill's identity, never by
/// payee and amount: an invoice of the same amount to the same payee is not
/// this bill, and one cycle paid twice is still one cycle (P-213).
fn owner_paid_cycle(
    owner: &std::collections::BTreeSet<B256>,
    offered: &BTreeMap<B256, String>,
    cycle: &str,
) -> bool {
    owner.iter().any(|h| {
        offered
            .get(h)
            .is_some_and(|c| c.eq_ignore_ascii_case(cycle))
    })
}

/// Each stepped bill, kept, or set aside with why when its record runs ahead
/// of its schedule: one bill's disagreement stops that bill, never the pass and
/// the other bills (docs/60 P-210: one account's every check failed on it). Any
/// other error still fails the pass.
#[allow(clippy::type_complexity)]
fn keep_stepped(
    stepped: Vec<(Obligation, Result<(), OperatorError>)>,
) -> Result<(Vec<Obligation>, Vec<(Obligation, String)>), OperatorError> {
    let (mut kept, mut aside) = (Vec::new(), Vec::new());
    for (o, r) in stepped {
        match r {
            Ok(()) => kept.push(o),
            Err(e @ OperatorError::SettledAhead { .. }) => {
                let why = format!(
                    "not asked or paid this pass: {e}. The other bills are checked as usual; this one waits until its record and its schedule agree"
                );
                aside.push((o, why));
            }
            Err(e) => return Err(e),
        }
    }
    Ok((kept, aside))
}

/// A model refuses a bill only as a look-alike; any other refusal becomes the
/// owner's question, with the model's own why (docs/60 P-218) — the rule
/// `decide_or_rule` applies to invoices (P-216). A model's refusal left a real
/// bill unpaid with no one asked. A look-alike is the case's fact, not the
/// model's word: the basis counts only when code found one (P-223). The rule
/// decider's refusals come from code and stand, and so does a refusal where
/// code does not allow `ask`.
fn model_refusal_asks(
    case: &decide::Case,
    d: decide::Decision,
    note: Option<String>,
    name: &str,
) -> (decide::Decision, Option<String>) {
    if d.by != ActorKind::Model
        || d.action != decide::Action::Refuse
        || (d.basis == decide::Basis::LookAlike && case.lookalike_of.is_some())
        || !case.allowed.contains(&decide::Action::Ask)
    {
        return (d, note);
    }
    let words = format!(
        "{name} refused a bill; only a look-alike is refused without the owner, so the owner is asked"
    );
    let asked = decide::Decision {
        action: decide::Action::Ask,
        ..d
    };
    (
        asked,
        Some(match note {
            Some(n) => format!("{n}; {words}"),
            None => words,
        }),
    )
}

/// Whether the pass may trigger this due date of a standing order, and why not
/// (docs/60 P-211): only on Autopilot, and never for a bill-cycle some path
/// already paid. `payDue` pays the order's oldest date under its own key, which
/// no bill payment carries, so the contract refuses nothing here.
fn order_due_step(propose_only: bool, paid: bool, id: u64, due_at: u64) -> Result<(), String> {
    if propose_only {
        return Err(format!(
            "Standing order #{id} is due (unix {due_at}). The account asks before paying, so the agent does not trigger it: \
             the owner pays it with one tap (Pay now), or the agent does on Autopilot."
        ));
    }
    if paid {
        return Err(format!(
            "Standing order #{id}'s due date {due_at} is a bill-cycle already paid; payDue would pay it again, so it is not \
             triggered. The order cannot skip a date on chain: end it, then make it standing again from the next unpaid cycle."
        ));
    }
    Ok(())
}

/// The transfers a bill's rhythm is learned from: history without the agent's
/// own payments (`own`) and without the owner's payments through the policy
/// (`owner_txs`, `SettledByPrincipal`). Both answer the account's own asks, at
/// the time the account asked, so their timing is the checks' and the owner's,
/// not the vendor's (docs/60 P-230); the cycles they paid are marked paid by
/// key and by the references offered for them.
fn rhythm_evidence(
    history: &[Transfer],
    own: &std::collections::BTreeSet<String>,
    owner_txs: &std::collections::BTreeSet<String>,
) -> Vec<Transfer> {
    history
        .iter()
        .filter(|t| !own.contains(&t.tx_hash) && !owner_txs.contains(&t.tx_hash))
        .cloned()
        .collect()
}

/// What [`Operator::obligations_due`] found.
#[derive(Debug, Clone)]
pub struct DuePlan {
    /// The agent's own payments, kept out of the evidence.
    pub own: std::collections::BTreeSet<String>,
    /// Every obligation inferred, stepped past settled cycles.
    pub obligations: Vec<Obligation>,
    /// Those falling due within the horizon: the liquidity view.
    pub upcoming: Vec<Obligation>,
    /// Those within the pay lead: what a pass acts on.
    pub due: Vec<Obligation>,
    /// Set aside this pass, with why ([`keep_stepped`]).
    pub aside: Vec<(Obligation, String)>,
    /// The owner's own payments' action hashes ([`owner_paid_cycle`]).
    pub owner_paid: std::collections::BTreeSet<B256>,
}

/// The reasoner orders; it does not decide what exists.
///
/// `ClaudeReasoner` already refuses a model answer that adds, drops or
/// repeats an obligation. This holds the same line for every reasoner,
/// in the one place every ordering passes through: the ranking must name each
/// obligation in `due` exactly once, by cycle key, and the obligation acted on
/// is always the pass's own copy, never the reasoner's. A reasoner that hands
/// back an obligation the pass did not infer -- or one with a different due
/// date, which is a different bill -- gets `None`, and the pass falls back to
/// the deterministic order.
fn conform(ranked: Vec<Prioritised>, due: &[Obligation]) -> Option<Vec<Prioritised>> {
    if ranked.len() != due.len() {
        return None;
    }
    let mut remaining: Vec<&Obligation> = due.iter().collect();
    let mut out = Vec::with_capacity(ranked.len());
    for (rank, item) in ranked.into_iter().enumerate() {
        let key = item.obligation.cycle_key();
        let pos = remaining.iter().position(|o| o.cycle_key() == key)?;
        out.push(Prioritised {
            obligation: remaining.remove(pos).clone(),
            rank,
            rationale: item.rationale,
        });
    }
    Some(out)
}

/// Move `o` to its latest cycle due by `now`, past the cycles that fell due
/// before it (docs/60 P-267): for a bill the owner typed, never a found one.
fn skip_missed_cycles(o: &mut Obligation, now: Timestamp) {
    let behind = i64::from(cycles_behind(o, now));
    o.next_due = o.next_due + Seconds::new(o.interval.as_i64().saturating_mul(behind));
}

/// Later cycles of `o` that have also fallen due by `now`: zero for a bill
/// that is current or overdue by less than one interval.
fn cycles_behind(o: &Obligation, now: Timestamp) -> u32 {
    let overdue = (now - o.next_due).as_i64();
    let step = o.interval.as_i64();
    if overdue <= 0 || step <= 0 {
        return 0;
    }
    u32::try_from(overdue / step).unwrap_or(u32::MAX)
}

/// Early-payment terms on one payee's invoices (PLAN 5.56; RFB 04 "which
/// obligations are worth paying early for the discount"): "2% off if paid
/// ten days early" is `lead` = 10 days, `discount_bps` = 200.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlyTerms {
    pub counterparty: ledger::Counterparty,
    pub lead: Seconds,
    pub discount_bps: u16,
}

/// The most a discount may take off a bill (10 %). Above it the terms are
/// refused at import: the verifier's band is a quarter, and a "discount"
/// that large is a different invoice, not terms on this one.
pub const MAX_DISCOUNT_BPS: u16 = 1_000;

/// The bills worth paying early now, at their discounted amount, with what
/// each saves. A bill qualifies when its payee offers terms, it is not
/// already due by the normal lead, and today is inside the terms' window.
/// All of them only while the balance above the floor covers **every** bill
/// in the horizon at full price: early payment spends money a later bill
/// might need, so it is taken only from money no bill in the horizon needs.
/// Pure, so the rule is tested without a chain.
pub fn early_payments(
    upcoming: &[Obligation],
    due: &[Obligation],
    terms: &[EarlyTerms],
    now: Timestamp,
    balance: Money,
    floor: Money,
) -> Vec<(Obligation, Money)> {
    let ccy = balance.currency();
    let Some(total) = ledger::total_due(upcoming, ccy) else {
        return Vec::new();
    };
    if balance.to_minor() - floor.to_minor() < total.to_minor() {
        return Vec::new();
    }
    upcoming
        .iter()
        .filter(|o| !due.iter().any(|d| d.cycle_key() == o.cycle_key()))
        .filter_map(|o| {
            let t = terms.iter().find(|t| t.counterparty == o.counterparty)?;
            if t.discount_bps == 0 || t.discount_bps > MAX_DISCOUNT_BPS {
                return None;
            }
            // A window of a whole interval or more pays the cycle after this
            // one early too: two cycles settled ahead of now, which
            // `step_past_settled` refuses as SettledAhead, failing every
            // later pass (audit 2026-10-15).
            if t.lead >= o.interval {
                return None;
            }
            if o.next_due <= now || o.next_due - now > t.lead {
                return None;
            }
            let full = o.typical_amount.to_minor();
            let off = full * i128::from(t.discount_bps) / 10_000;
            if off <= 0 {
                return None;
            }
            let mut early = o.clone();
            early.typical_amount = Money::minor(full - off, o.typical_amount.currency());
            Some((early, Money::minor(off, o.typical_amount.currency())))
        })
        .collect()
}

/// Whether `o` is close enough to due to pay now: within `pay_lead`, or
/// within a tenth of its own interval if that is shorter -- the proportion
/// three days is of a month, so a compressed cadence of minutes is led by
/// seconds, not by half a cycle. Overdue always counts.
///
/// Half the interval was the first cap, and the fork rehearsal on 10-02 showed
/// why it was wrong: a cycle paid late left the next one under half an
/// interval away, so the very next pass paid it early -- by the same
/// proportion, fifteen days early on a monthly bill.
fn within_pay_lead(o: &Obligation, now: Timestamp, pay_lead: Seconds) -> bool {
    let tenth = Seconds::new(o.interval.as_i64() / 10);
    let lead = if tenth < pay_lead { tenth } else { pay_lead };
    o.next_due - now <= lead
}

/// Whether the hedge may trade live on `policy`: only in Autopilot (docs/46
/// #4). A policy from before modes (the read reverts) is Autopilot by
/// construction; a read that failed is not, so the hedge runs dry (audit
/// 2026-10-15, docs/60 P-74).
pub async fn hedge_may_trade_live(client: &ArcClient, policy: Address) -> bool {
    match client.governance(policy).await {
        Ok(g) => g.mode == PolicyMode::Autopilot,
        Err(e) => arc::is_revert(&e),
    }
}

/// Write the payment HALT file; `Some(reason)` when it could not be written.
fn write_halt(path: &std::path::Path, text: &str) -> Option<String> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, text)
        .err()
        .map(|e| format!("{}: {e}", path.display()))
}

/// Merge transfers read from several of the owner's accounts into one
/// history: drop the owner moving money between their own accounts (those
/// are not obligations, and a funding transfer to the policy would otherwise
/// read as a payee), drop the same transaction seen from both ends, and
/// order by time.
fn merge_history(
    mut all: Vec<Transfer>,
    own: &std::collections::BTreeSet<String>,
) -> Vec<Transfer> {
    all.retain(|t| !own.contains(t.counterparty.as_str()));
    all.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.tx_hash.cmp(&b.tx_hash)));
    all.dedup_by(|a, b| {
        a.tx_hash == b.tx_hash && a.counterparty == b.counterparty && a.amount == b.amount
    });
    all
}

#[cfg(test)]
mod stepping_tests {
    use super::*;
    use ledger::{Confidence, Counterparty, Currency, ObligationKind};
    use std::collections::BTreeSet;

    const WEEK: i64 = 7 * 86_400;

    fn weekly(due: i64) -> Obligation {
        Obligation {
            counterparty: Counterparty::new("0x1111111111111111111111111111111111111111"),
            kind: ObligationKind::Subscription,
            typical_amount: Money::minor(1_000_000, Currency::Usdc),
            interval: Seconds::new(WEEK),
            last_paid: Timestamp::from_unix(due - WEEK),
            next_due: Timestamp::from_unix(due),
            confidence: Confidence::new(0.85).unwrap(),
            sample_count: 6,
            interval_cv: 0.0,
            amount_cv: 0.0,
            evidence: vec![],
        }
    }

    /// The first `n` weekly cycles from `start` are settled.
    fn settled(start: i64, n: i64) -> BTreeSet<String> {
        (0..n)
            .map(|k| weekly(start + k * WEEK).cycle_key())
            .collect()
    }

    /// docs/60 P-267, S17b's account on :7917: a typed 120 s bill eight cycles behind
    /// is paid at its current cycle, not caught up from the oldest, and the cycles-behind
    /// question no longer applies; a bill current or less than a cycle late stays put.
    #[test]
    fn a_typed_bill_behind_moves_to_its_current_cycle() {
        let mut o = weekly(1_000_000);
        o.interval = Seconds::new(120);
        let now = Timestamp::from_unix(1_000_000 + 8 * 120 + 30);
        skip_missed_cycles(&mut o, now);
        assert_eq!(o.next_due.as_unix(), 1_000_000 + 8 * 120);
        assert_eq!(cycles_behind(&o, now), 0);
        let mut late = weekly(1_000_000);
        skip_missed_cycles(&mut late, Timestamp::from_unix(1_000_000 + 5));
        assert_eq!(late.next_due.as_unix(), 1_000_000, "less than a cycle late");
    }

    async fn step(
        o: &mut Obligation,
        now: i64,
        paid: &BTreeSet<String>,
    ) -> Result<(), OperatorError> {
        step_past_settled(o, Timestamp::from_unix(now), |k| {
            let hit = paid.contains(&k);
            async move { Ok(hit) }
        })
        .await
    }

    #[tokio::test]
    async fn a_weekly_bill_paid_by_the_agent_for_two_years_keeps_being_paid() {
        // 104 settled cycles. The first version stopped at 36, after nine
        // months, and never paid this bill again.
        let start = 1_700_000_000;
        let paid = settled(start, 104);
        let mut o = weekly(start);
        let now = start + 104 * WEEK - 86_400; // a day before cycle 104 falls due
        step(&mut o, now, &paid).await.unwrap();
        assert_eq!(
            o.next_due.as_unix(),
            start + 104 * WEEK,
            "the first unsettled cycle"
        );
    }

    #[tokio::test]
    async fn the_first_unpaid_cycle_is_where_stepping_stops() {
        let start = 1_700_000_000;
        let mut paid = settled(start, 10);
        paid.remove(&weekly(start + 4 * WEEK).cycle_key()); // cycle 4 never paid
        let mut o = weekly(start);
        step(&mut o, start + 11 * WEEK, &paid).await.unwrap();
        assert_eq!(o.next_due.as_unix(), start + 4 * WEEK);
    }

    /// uat-u2-fin (docs/60 P-175, P-210): the owner paid one 30-day bill-cycle
    /// twice by hand, each tap under the hash a pass offered for that cycle.
    /// Matched by the bill's identity (the hashes offered for the cycle), the
    /// two payments settle cycle 1 only: the pass steps to cycle 2, a month
    /// away, asks nothing now and never runs ahead of the schedule. Counted by
    /// payee and amount (the first fix) they settled cycles 1 and 2, the step
    /// bound ended on a settled cycle and every pass of the account failed.
    #[tokio::test]
    async fn two_hand_payments_for_one_cycle_settle_that_cycle_only() {
        const MONTH: i64 = 30 * 86_400;
        let payee = "0x179dca89f69faef5765d2e7445aa5452b8159520";
        let d0 = 1_792_156_029;
        let cycle1 = format!("{payee}@{d0}");
        let (h1, h2) = (B256::repeat_byte(0x85), B256::repeat_byte(0xd1));
        let owner = BTreeSet::from([h1, h2]);
        // Every pass offered the waiting cycle under a new proposal hash.
        let offered = BTreeMap::from([
            (h1, cycle1.clone()),
            (h2, cycle1.clone()),
            (B256::repeat_byte(0x8e), cycle1.clone()),
        ]);
        assert!(owner_paid_cycle(&owner, &offered, &cycle1));
        assert!(!owner_paid_cycle(
            &owner,
            &offered,
            &format!("{payee}@{}", d0 + MONTH)
        ));
        let mut o = weekly(d0);
        o.counterparty = Counterparty::new(payee);
        o.interval = Seconds::new(MONTH);
        // The reviewer's pass on 10-17 12:50 (block time 1792213608).
        let r = step_past_settled(&mut o, Timestamp::from_unix(1_792_213_608), |k| {
            let hit = owner_paid_cycle(&owner, &offered, &k);
            async move { Ok(hit) }
        })
        .await;
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(o.next_due.as_unix(), d0 + MONTH, "cycle 2, a month away");
    }

    /// docs/60 P-230, U1's re-check (uat2-u1-rc): a vendor paid 0.002 USDC five
    /// times, 243 s apart; the check at the due time came late, the owner paid
    /// that cycle 318 s after the last payment ("Approve & pay"), and the next
    /// check came a cycle late. Counted as the vendor's rhythm, the owner's late
    /// payment dropped the bill below the Autopilot floor (0.75); it answers an
    /// ask, so it is not evidence: the clean rhythm keeps its confidence.
    #[test]
    fn a_late_owner_payment_is_not_a_broken_rhythm() {
        let vendor = Counterparty::new("0xf4d08abf297f57acd381f000a25442186d860e43");
        let t0 = 1_792_219_135i64; // 14:38:55
        let pay = |i: usize, at: i64| Transfer {
            tx_hash: format!("0x{i:064x}"),
            counterparty: vendor.clone(),
            amount: Money::minor(2_000, Currency::Usdc),
            at: Timestamp::from_unix(at),
            direction: ledger::Direction::Out,
        };
        let mut history: Vec<Transfer> = (0..5).map(|i| pay(i, t0 + 243 * i as i64)).collect();
        history.push(pay(9, t0 + 972 + 318)); // the owner's settle, through the policy
        let config = ledger::DetectionConfig {
            dust_line: Some(Money::minor(1_000, Currency::Usdc)),
            ..Default::default()
        };
        let score = |evidence: &[Transfer]| ledger::detect(evidence, config)[0].confidence.get();
        let owner = BTreeSet::from([format!("0x{:064x}", 9)]);
        let counted = score(&rhythm_evidence(
            &history,
            &BTreeSet::new(),
            &BTreeSet::new(),
        ));
        assert!(counted < 0.75, "the cause, reproduced: {counted}");
        let clean = rhythm_evidence(&history, &BTreeSet::new(), &owner);
        assert_eq!(clean.len(), 5);
        assert!(score(&clean) >= 0.75, "{}", score(&clean));
        // The bill's next due stays on the vendor's rhythm (the paid cycle is
        // stepped past by key), not moved by the owner's late tap.
        assert_eq!(
            ledger::detect(&clean, config)[0].next_due.as_unix(),
            t0 + 972 + 243
        );
    }

    /// docs/60 P-213: an owner payment of the same amount to the same payee
    /// under a reference no pass offered for the bill (a one-off invoice, a
    /// payment for something else) is not a payment of the bill.
    #[test]
    fn an_invoice_of_the_bill_amount_to_the_bill_payee_is_not_the_bill() {
        let cycle = "0x179dca89f69faef5765d2e7445aa5452b8159520@1792156029";
        let invoice = B256::repeat_byte(0x1b);
        let offered = BTreeMap::from([(B256::repeat_byte(0x85), cycle.to_string())]);
        assert!(!owner_paid_cycle(
            &BTreeSet::from([invoice]),
            &offered,
            cycle
        ));
        // Another cycle's reference does not answer this one either.
        let other = BTreeMap::from([(invoice, format!("{cycle}0"))]);
        assert!(!owner_paid_cycle(&BTreeSet::from([invoice]), &other, cycle));
        // The key's case is not the bill's identity.
        let upper = BTreeMap::from([(invoice, cycle.to_uppercase().replace("0X", "0x"))]);
        assert!(owner_paid_cycle(&BTreeSet::from([invoice]), &upper, cycle));
    }

    /// docs/60 P-211: a standing order's due date is triggered on Autopilot
    /// only, and never for a bill-cycle already paid by any path.
    #[test]
    fn a_standing_order_pays_only_on_autopilot_and_only_an_unpaid_cycle() {
        assert!(order_due_step(false, false, 3, 1_792_156_029).is_ok());
        let propose = order_due_step(true, false, 3, 1_792_156_029).unwrap_err();
        assert!(
            propose.contains("asks before paying") && propose.contains("Pay now"),
            "{propose}"
        );
        let paid = order_due_step(false, true, 3, 1_792_156_029).unwrap_err();
        assert!(
            paid.contains("already paid") && paid.contains("next unpaid cycle"),
            "{paid}"
        );
    }

    /// docs/60 P-210: one bill whose record runs ahead of its schedule is set
    /// aside with words; the other bills go on, and the pass does not fail.
    /// Any other error still fails the pass.
    #[test]
    fn a_bill_settled_ahead_is_set_aside_and_the_pass_goes_on() {
        let ahead = weekly(1_700_000_000);
        let fine = weekly(1_700_000_000 + WEEK);
        let (kept, aside) = keep_stepped(vec![
            (
                ahead.clone(),
                Err(OperatorError::SettledAhead {
                    cycle_key: ahead.cycle_key(),
                }),
            ),
            (fine.clone(), Ok(())),
        ])
        .unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].cycle_key(), fine.cycle_key());
        assert_eq!(aside.len(), 1);
        assert!(
            aside[0].1.contains("not asked or paid this pass"),
            "{}",
            aside[0].1
        );
        assert!(aside[0].1.contains(&ahead.cycle_key()), "{}", aside[0].1);
        let failed = keep_stepped(vec![(fine, Err(OperatorError::ReadOnly))]);
        assert!(failed.is_err());
    }

    #[tokio::test]
    async fn a_cycle_settled_beyond_any_pass_is_an_error_not_a_skip() {
        let start = 1_700_000_000;
        let paid = settled(start, 50);
        let mut o = weekly(start);
        // Now is week 3, but the record says 50 weeks are paid.
        let r = step(&mut o, start + 3 * WEEK, &paid).await;
        assert!(
            matches!(r, Err(OperatorError::SettledAhead { .. })),
            "{r:?}"
        );
    }
}

#[cfg(test)]
mod conform_tests {
    use super::*;
    use ledger::{Confidence, Counterparty, Currency, ObligationKind};

    fn ob(who: &str, due: i64, minor: i128) -> Obligation {
        Obligation {
            counterparty: Counterparty::new(who),
            kind: ObligationKind::Subscription,
            typical_amount: Money::minor(minor, Currency::Usdc),
            interval: Seconds::days(30).unwrap(),
            last_paid: Timestamp::from_unix(due - 2_592_000),
            next_due: Timestamp::from_unix(due),
            confidence: Confidence::new(0.85).unwrap(),
            sample_count: 6,
            interval_cv: 0.0,
            amount_cv: 0.0,
            evidence: vec![],
        }
    }
    fn ranked(list: &[Obligation]) -> Vec<Prioritised> {
        list.iter()
            .enumerate()
            .map(|(rank, o)| Prioritised {
                obligation: o.clone(),
                rank,
                rationale: "r".into(),
            })
            .collect()
    }
    const A: &str = "0x1111111111111111111111111111111111111111";
    const B: &str = "0x3333333333333333333333333333333333333333";

    #[test]
    fn a_reordering_is_kept_and_acts_on_the_passs_own_obligations() {
        let due = vec![ob(A, 1_000, 1_500_000), ob(B, 2_000, 1_200_000)];
        // Same bills, reversed -- and the reasoner's copy of A claims a bigger amount.
        let mut theirs = vec![due[1].clone(), due[0].clone()];
        theirs[1].typical_amount = Money::minor(99_000_000, Currency::Usdc);
        let out = conform(ranked(&theirs), &due).expect("a permutation");
        assert_eq!(out[0].obligation.counterparty.as_str(), B);
        assert_eq!(
            out[1].obligation.typical_amount,
            Money::minor(1_500_000, Currency::Usdc)
        );
    }

    #[test]
    fn an_obligation_the_pass_did_not_infer_is_refused() {
        let due = vec![ob(A, 1_000, 1_500_000)];
        let extra = vec![ob(A, 1_000, 1_500_000), ob(B, 2_000, 1_200_000)];
        assert!(conform(ranked(&extra), &due).is_none(), "added");
        let swapped = vec![ob(B, 1_000, 1_500_000)];
        assert!(conform(ranked(&swapped), &due).is_none(), "different payee");
    }

    #[test]
    fn the_same_payee_on_another_due_date_is_another_bill() {
        // What the demo server's old precomputed ordering looked like: the
        // agent's own payment had moved the due date.
        let due = vec![ob(A, 1_000, 1_500_000)];
        assert!(conform(ranked(&[ob(A, 1_500, 1_500_000)]), &due).is_none());
    }

    #[test]
    fn dropped_or_repeated_obligations_are_refused() {
        let due = vec![ob(A, 1_000, 1_500_000), ob(B, 2_000, 1_200_000)];
        assert!(conform(ranked(&due[..1]), &due).is_none(), "dropped");
        assert!(
            conform(ranked(&[due[0].clone(), due[0].clone()]), &due).is_none(),
            "repeated"
        );
    }
}

#[cfg(test)]
mod pay_lead_tests {
    use super::*;
    use ledger::{Confidence, Counterparty, Currency, ObligationKind};

    const DAY: i64 = 86_400;

    fn pays(interval: i64, due_in: i64) -> bool {
        let now = Timestamp::from_unix(1_800_000_000);
        let o = Obligation {
            counterparty: Counterparty::new("0x1111111111111111111111111111111111111111"),
            kind: ObligationKind::Subscription,
            typical_amount: Money::minor(1_500_000, Currency::Usdc),
            interval: Seconds::new(interval),
            last_paid: Timestamp::from_unix(now.as_unix() + due_in - interval),
            next_due: Timestamp::from_unix(now.as_unix() + due_in),
            confidence: Confidence::new(0.85).unwrap(),
            sample_count: 6,
            interval_cv: 0.0,
            amount_cv: 0.0,
            evidence: vec![],
        };
        within_pay_lead(&o, now, Seconds::days(3).unwrap())
    }

    #[test]
    fn a_monthly_bill_is_paid_in_the_last_few_days() {
        assert!(pays(30 * DAY, DAY), "due tomorrow");
        assert!(pays(30 * DAY, 3 * DAY), "due in exactly the lead");
        assert!(!pays(30 * DAY, 4 * DAY), "not yet");
    }

    #[test]
    fn paid_late_the_next_cycle_is_not_paid_early_to_make_up() {
        // Paid a day after the due date: the next cycle is 29 days away.
        assert!(!pays(30 * DAY, 29 * DAY));
    }

    #[test]
    fn overdue_is_always_within_the_lead() {
        assert!(pays(30 * DAY, -5 * DAY));
    }

    fn behind(interval: i64, due_in: i64) -> u32 {
        let now = Timestamp::from_unix(1_800_000_000);
        let o = Obligation {
            counterparty: Counterparty::new("0x1111111111111111111111111111111111111111"),
            kind: ObligationKind::Subscription,
            typical_amount: Money::minor(1_500_000, Currency::Usdc),
            interval: Seconds::new(interval),
            last_paid: Timestamp::from_unix(now.as_unix() + due_in - interval),
            next_due: Timestamp::from_unix(now.as_unix() + due_in),
            confidence: Confidence::new(0.85).unwrap(),
            sample_count: 6,
            interval_cv: 0.0,
            amount_cv: 0.0,
            evidence: vec![],
        };
        cycles_behind(&o, now)
    }

    #[test]
    fn a_bill_counts_as_behind_only_once_a_whole_further_cycle_is_due() {
        assert_eq!(behind(30 * DAY, DAY), 0, "not yet due");
        assert_eq!(
            behind(30 * DAY, -29 * DAY),
            0,
            "late, but the next is not due"
        );
        assert_eq!(behind(30 * DAY, -30 * DAY), 1, "the next one is due too");
        assert_eq!(
            behind(30 * DAY, -470 * DAY),
            15,
            "the acceptance fixture's old subscription"
        );
    }

    #[test]
    fn a_compressed_cadence_is_led_by_a_tenth_of_its_interval_not_by_days() {
        // Three-minute cycles standing in for months: 18 s is "three days".
        assert!(pays(180, 18));
        assert!(!pays(180, 19));
    }

    #[test]
    fn paid_late_on_a_compressed_cadence_the_next_cycle_still_waits() {
        // The fork rehearsal case: 60 s cycles, one paid 40 s late, the next
        // due 20 s later. Under a half-interval cap that was paid at once.
        assert!(!pays(60, 20));
        assert!(pays(60, 6));
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use ledger::{Counterparty, Currency, Direction, Money, Timestamp};

    fn t(hash: &str, to: &str, minor: i128, at: i64) -> Transfer {
        Transfer {
            tx_hash: hash.into(),
            counterparty: Counterparty::new(to),
            amount: Money::minor(minor, Currency::Usdc),
            at: Timestamp::from_unix(at),
            direction: Direction::Out,
        }
    }

    const WALLET: &str = "0x00000000000000000000000000000000000000aa";
    const POLICY: &str = "0x00000000000000000000000000000000000000bb";
    const VENDOR: &str = "0x0000000000000000000000000000000000001111";

    fn own() -> std::collections::BTreeSet<String> {
        [WALLET, POLICY].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_owner_funding_their_own_policy_is_not_a_payee() {
        let merged = merge_history(
            vec![
                t("0x01", POLICY, 50_000_000, 10),
                t("0x02", VENDOR, 9_000_000, 20),
            ],
            &own(),
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].counterparty.as_str(), VENDOR);
    }

    #[test]
    fn two_wallets_merge_into_one_timeline() {
        // The wallet paid the vendor in month one; the policy paid in month two.
        let merged = merge_history(
            vec![
                t("0x0b", VENDOR, 9_000_000, 2_592_000),
                t("0x0a", VENDOR, 9_000_000, 0),
            ],
            &own(),
        );
        let hashes: Vec<_> = merged.iter().map(|x| x.tx_hash.as_str()).collect();
        assert_eq!(hashes, ["0x0a", "0x0b"]);
    }

    #[test]
    fn one_payment_read_from_two_accounts_counts_once() {
        let merged = merge_history(
            vec![
                t("0x0a", VENDOR, 9_000_000, 0),
                t("0x0a", VENDOR, 9_000_000, 0),
            ],
            &own(),
        );
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn the_own_set_matches_how_addresses_are_formatted() {
        // Counterparty lowercases; the own set is built with `{:#x}`. If the
        // two ever disagree, every own transfer silently becomes a payee.
        let a: Address = "0x00000000000000000000000000000000000000AA"
            .parse()
            .unwrap();
        assert_eq!(format!("{a:#x}"), WALLET);
        assert_eq!(
            Counterparty::new("0x00000000000000000000000000000000000000AA").as_str(),
            WALLET
        );
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    fn cfg() -> Config {
        Config::new(Address::ZERO, Address::ZERO, Address::ZERO)
    }

    #[test]
    fn the_default_configuration_is_valid() {
        assert!(cfg().validate().is_ok());
    }

    /// The trap this exists for. Requiring the match reads like tightening
    /// policy and, on the inferred-obligation path, stops the agent dead.
    #[test]
    fn requiring_the_three_way_match_on_the_inferred_path_is_refused() {
        let mut c = cfg();
        c.required_preconditions.push(Precondition::ThreeWayMatch);
        let problems = c.validate().expect_err("should not validate").problems;
        assert!(
            problems.iter().any(|p| p.contains("escalate")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("do not exist")),
            "{problems:?}"
        );
    }

    /// Filling the stores removes one complaint and not the other: the
    /// documents existing does not make the agent cite them.
    #[test]
    fn filling_the_stores_does_not_make_the_inferred_path_work() {
        let mut c = cfg();
        c.required_preconditions.push(Precondition::ThreeWayMatch);
        c.purchase_orders.insert(
            "PO-1".into(),
            DocumentRecord {
                amount: Money::minor(20_000_000, ledger::Currency::Usdc),
                counterparty: "0x2222".into(),
            },
        );
        let problems = c
            .validate()
            .expect_err("should still not validate")
            .problems;
        assert!(
            problems.iter().any(|p| p.contains("escalate")),
            "{problems:?}"
        );
        assert!(
            !problems.iter().any(|p| p.contains("do not exist")),
            "{problems:?}"
        );
    }

    #[test]
    fn an_empty_attestation_is_refused() {
        let mut c = cfg();
        c.attested_payees.insert("0x1111".into(), String::new());
        let problems = c.validate().expect_err("should not validate").problems;
        assert!(
            problems.iter().any(|p| p.contains("empty attestation")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_non_positive_horizon_is_refused() {
        let mut c = cfg();
        c.horizon = Seconds::new(0);
        assert!(c.validate().is_err());
    }
}

/// What one pass did. Every entry is also in the log.
#[derive(Debug)]
pub struct RunOutcome {
    pub balance: Money,
    pub transfers_seen: usize,
    pub obligations_found: usize,
    pub reasoner: String,
    /// Paid, with the transaction that settled it.
    pub settled: Vec<(Obligation, B256)>,
    /// Sent to a human, with the verdict explaining why.
    pub escalated: Vec<(Obligation, Verdict)>,
    /// Escalated payments, composed and ready for the principal to sign.
    ///
    /// Deliberately *not* written into the anchored log: every input to it
    /// (payee, amount, action hash) is already a logged field, so storing the
    /// encoded blob as well would bloat each Merkle leaf with something a
    /// reader can re-derive. The log records the decision; this is a
    /// convenience for the human acting on it.
    pub pending_signature: Vec<PreparedPayment>,
    /// How many of those have now been waiting longer than
    /// [`Config::escalation_deadline`]. Non-zero means a human owes the agent
    /// an answer -- the one failure mode escalation itself cannot fix.
    pub stale_escalations: usize,
    /// Refused outright.
    pub blocked: Vec<(Obligation, Verdict)>,
    /// Passed over: this cycle is already paid on chain.
    pub skipped: Vec<Obligation>,
    /// Every recurring obligation this pass recovered (history and the
    /// owner's imports, stepped past settled cycles), due or not: what the
    /// account page projects runway from (PLAN 5.17).
    pub obligations: Vec<Obligation>,
    pub root: Hash32,
    pub anchor_tx: Option<B256>,
    /// The policy's autonomy level as the contract held it this pass
    /// (docs/46 #4). `None` for a policy deployed before modes existed,
    /// which behaves as Autopilot.
    pub mode: Option<PolicyMode>,
    /// When the agent's authority to pay lapses (PLAN 5.24); `None` on a
    /// policy from before expiries.
    pub agent_expiry: Option<u64>,
    /// Transactions in which the agent paid, as read at the start of the
    /// pass: kept out of the evidence, and handed back so a caller asking
    /// [`Operator::payees_to_approve`] does not walk the chain for them again.
    pub own_payments: std::collections::BTreeSet<String>,
    /// Bills paid early for a discount this pass (PLAN 5.56): cycle key and
    /// what the discount saved.
    pub early: Vec<(String, Money)>,
    /// Why this pass's record (or an earlier one) is not anchored yet.
    pub anchor_error: Option<String>,
    /// Earlier records anchored by this pass, with their transactions.
    pub caught_up: Vec<(Hash32, B256)>,
    /// Who chose at each decision point this pass (docs/112).
    pub decider: String,
    /// Every decision this pass made, in order: the case code built, the
    /// decision after [`decide::guard`], and -- when the decider's answer was
    /// not used as given (an error, an action code does not allow, an
    /// invented number) -- why.
    pub decisions: Vec<(decide::Case, decide::Decision, Option<String>)>,
}

/// One escalated payment, encoded for a human signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPayment {
    pub cycle_key: String,
    pub counterparty: String,
    pub amount: Money,
    /// Provenance: what was proposed, and why. Moves when the wording moves.
    pub action_hash: String,
    /// What the calldata actually carries, and what the chain records as
    /// executed. Stable across runs for the same billing cycle -- that is the
    /// whole point of it being a separate value.
    pub settlement_key: String,
    /// Whether this has already outrun [`Config::escalation_deadline`].
    pub stale: bool,
    /// Settle this cycle by hand: `settleAsPrincipal`.
    pub call: PreparedCall,
    /// Present when the payee is not attested on chain: `setPayee(payee,
    /// true)`, which lets the agent pay this cycle and the next ones itself.
    /// Settling by hand fixes one cycle; attesting fixes the cause.
    pub attest: Option<PreparedCall>,
}

impl RunOutcome {
    pub fn summary(&self) -> String {
        format!(
            "balance {} · {} obligations · {} settled · {} escalated{} · {} blocked · {} already paid",
            self.balance,
            self.obligations_found,
            self.settled.len(),
            self.escalated.len(),
            // Only shown when non-zero: a stale escalation is an exception,
            // and burying it behind a constant "0 stale" trains the reader to
            // skip it.
            if self.stale_escalations > 0 {
                format!(" ({} UNANSWERED past deadline)", self.stale_escalations)
            } else {
                String::new()
            },
            self.blocked.len(),
            self.skipped.len()
        )
    }
}

/// Runs the decide-verify-pay-record loop for one treasury.
pub struct Operator<R: Reasoner> {
    client: ArcClient,
    config: Config,
    reasoner: R,
    log: DecisionLog,
}

impl<R: Reasoner> Operator<R> {
    /// Build an operator, refusing a configuration that cannot do what it
    /// says. See [`Config::validate`].
    ///
    /// The check lives here rather than in each caller because a caller that
    /// has to remember is a caller that will not. A policy that silently
    /// escalates every payment is indistinguishable, from the outside, from a
    /// quiet week.
    pub fn new(client: ArcClient, config: Config, reasoner: R) -> Result<Self, ConfigError> {
        config.validate()?;
        Ok(Operator {
            client,
            config,
            reasoner,
            log: DecisionLog::new(),
        })
    }

    /// Build without validating. For tests that want to observe what a bad
    /// configuration actually does, rather than being stopped from making one.
    pub fn new_unchecked(client: ArcClient, config: Config, reasoner: R) -> Self {
        Operator {
            client,
            config,
            reasoner,
            log: DecisionLog::new(),
        }
    }

    pub fn log(&self) -> &DecisionLog {
        &self.log
    }

    /// The log, writable, for a caller that records a step of its own in
    /// the same anchored record -- the hedge layer runs before the payment
    /// pass and appends here, so one root covers both. Append-only still:
    /// [`DecisionLog`] has no edit or remove.
    pub fn log_mut(&mut self) -> &mut DecisionLog {
        &mut self.log
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The chain client, for cross-checks a caller wants to run itself.
    pub fn client_ref(&self) -> &ArcClient {
        &self.client
    }

    /// Read the treasury's outgoing history off the chain.
    pub async fn read_history(&self) -> Result<Vec<Transfer>, OperatorError> {
        let head = self.client.block_number().await?;
        let from = self.config.history_from_block;
        let blocks = head.saturating_sub(from);

        // Refuse a scan that would take thousands of round trips. A node
        // caps how wide a single log query may be, so a treasury's history
        // has to start somewhere the caller knows about. Failing here with
        // the number is far better than a run that appears to hang.
        let accounts = 1 + self.config.also_history.len() as u64;
        let calls = (blocks / arc::LOG_CHUNK_BLOCKS + 1) * accounts;
        if calls > MAX_HISTORY_CALLS {
            return Err(OperatorError::HistoryTooWide {
                from,
                head,
                blocks,
                calls,
            });
        }

        let policy_from = self.config.policy_from_block.unwrap_or(from);
        let mut all = self
            .client
            .transfers(self.config.treasury, policy_from.max(from), head)
            .await?;
        if self.config.also_history.is_empty() {
            return Ok(all);
        }
        for account in &self.config.also_history {
            all.extend(self.client.transfers(*account, from, head).await?);
        }
        let own: std::collections::BTreeSet<String> = [self.config.treasury, self.config.policy]
            .iter()
            .chain(&self.config.also_history)
            .map(|a| format!("{a:#x}"))
            .collect();
        Ok(merge_history(all, &own))
    }

    /// What a pass at `now` would act on, from `history`.
    ///
    /// The agent's own payments are not evidence. They are its outputs, and
    /// learning a schedule from them lets one early payment move the next due
    /// date earlier, every pass. The schedule comes from what the owner paid;
    /// what the agent paid only marks cycles as settled, and the contract's
    /// `executed` record already says which.
    ///
    /// Liquidity looks a full horizon ahead. Paying does not: an obligation is
    /// acted on only once it falls due within the pay lead, capped at a tenth
    /// of its own interval. Paid late, a monthly bill's next cycle is still
    /// weeks away, so it is not paid early to compensate; and no cycle can be
    /// paid while the one before it is still to come.
    ///
    /// Public so that anything showing a pass -- the demo server -- shows the
    /// same obligations the pass acts on, from the same computation.
    /// Every `SettledByPrincipal` on the policy: the owner's own payments. A
    /// failed read fails the pass, as the agent's own payments' read does:
    /// without it a cycle the owner paid under an offered reference would be
    /// asked again, bound to a key no payment used (docs/60 P-210).
    async fn owner_settlements(
        &self,
        head: u64,
    ) -> Result<Vec<arc::PrincipalSettlement>, OperatorError> {
        let from = self
            .config
            .policy_from_block
            .unwrap_or(self.config.history_from_block);
        Ok(self
            .client
            .principal_settlements(self.config.policy, from, head)
            .await?)
    }

    pub async fn obligations_due(
        &self,
        history: &[Transfer],
        now: Timestamp,
    ) -> Result<DuePlan, OperatorError> {
        let head = self.client.block_number().await?;
        let own = self
            .client
            .agent_payments(
                self.config.policy,
                self.config
                    .policy_from_block
                    .unwrap_or(self.config.history_from_block),
                head,
            )
            .await?;
        // The owner's own payments through the policy, read before detection:
        // like the agent's, they answer the account's asks, so they mark cycles
        // paid and are not the vendor's rhythm (docs/60 P-230: an owner paying
        // late because the check came late dropped a clean bill below the floor).
        let settlements = self.owner_settlements(head).await?;
        let owner_txs: std::collections::BTreeSet<String> =
            settlements.iter().map(|s| format!("{:#x}", s.tx)).collect();
        let evidence = rhythm_evidence(history, &own, &owner_txs);
        let mut obligations = detect(&evidence, self.config.detection);
        for seed in &self.config.seeded {
            if !obligations
                .iter()
                .any(|o| o.counterparty.as_str() == seed.counterparty.as_str())
            {
                obligations.push(seed.clone());
            }
        }
        // What the owner paid by hand (`SettledByPrincipal`): before docs/60
        // P-175 the service bound the owner's tap to the pass's proposal hash,
        // which changes every pass, so the settlement keys below never saw
        // those payments and the next pass asked again. Each counts for the
        // cycle it was offered for, and only that one (P-210).
        let owner_hashes: std::collections::BTreeSet<B256> =
            settlements.iter().map(|s| s.action_hash).collect();
        let owner_paid = std::sync::Arc::new(owner_hashes.clone());
        let offered = std::sync::Arc::new(self.config.offered.clone());
        // A standing order's payment answers the same bill-cycle (docs/60
        // P-219): ended or not, an order to this payee whose key for this due
        // date is spent (`PaidDue`) paid it. A failed read fails the pass, as
        // the pass's own read of the orders does.
        let orders = std::sync::Arc::new(self.client.schedules(self.config.policy).await?);
        // Bills' settled cycles eight bills at a time, not one bill after
        // another (docs/68 F4); within a bill the cycles stay in order.
        // docs/60 P-267: a bill the owner typed is the owner's own schedule. Cycles that
        // went by unpaid (before Autopilot, or while the owner was asked) are not caught
        // up by inference: the agent pays the current cycle, and a found bill keeps the
        // verifier's cycles-behind question.
        let typed: std::sync::Arc<std::collections::BTreeSet<String>> = std::sync::Arc::new(
            self.config
                .seeded
                .iter()
                .map(|s| s.counterparty.as_str().to_string())
                .collect(),
        );
        let stepped = arc::eight_at_a_time(std::mem::take(&mut obligations), |mut o| {
            let (client, policy) = (self.client.clone(), self.config.policy);
            let scope = self.config.principal_id.clone();
            let (owner_paid, offered, orders) =
                (owner_paid.clone(), offered.clone(), orders.clone());
            if o.sample_count == 0 && typed.contains(o.counterparty.as_str()) {
                skip_missed_cycles(&mut o, now);
            }
            async move {
                let r = step_past_settled(&mut o, now, |cycle| {
                    let key = B256::from(
                        *ledger::settlement_key_for(&Idempotency {
                            key: cycle.clone(),
                            scope: scope.clone(),
                        })
                        .as_bytes(),
                    );
                    let (client, owner_paid, offered, orders) = (
                        client.clone(),
                        owner_paid.clone(),
                        offered.clone(),
                        orders.clone(),
                    );
                    async move {
                        if client.already_executed(policy, key).await?
                            || owner_paid_cycle(&owner_paid, &offered, &cycle)
                        {
                            return Ok(true);
                        }
                        let Some((payee, due)) = cycle
                            .rsplit_once('@')
                            .and_then(|(p, d)| Some((p.to_string(), d.parse::<u64>().ok()?)))
                        else {
                            return Ok(false);
                        };
                        for sc in orders
                            .iter()
                            .filter(|sc| format!("{:#x}", sc.payee) == payee)
                        {
                            let paid_due = client.schedule_action_hash(policy, sc.id, due).await?;
                            if client.already_executed(policy, paid_due).await? {
                                return Ok(true);
                            }
                        }
                        Ok(false)
                    }
                })
                .await;
                (o, r)
            }
        })
        .await?;
        let (kept, aside) = keep_stepped(stepped)?;
        obligations.extend(kept);
        let upcoming: Vec<Obligation> = upcoming(&obligations, now, self.config.horizon)
            .into_iter()
            .cloned()
            .collect();
        let due: Vec<Obligation> = upcoming
            .iter()
            .filter(|o| within_pay_lead(o, now, self.config.pay_lead))
            .cloned()
            .collect();
        Ok(DuePlan {
            own,
            obligations,
            upcoming,
            due,
            aside,
            owner_paid: owner_hashes,
        })
    }

    /// Everyone the owner pays on a schedule whom the policy will not yet
    /// let the agent pay: the list to approve in one tap at onboarding.
    ///
    /// Learned from the same evidence the agent acts on (its own payments
    /// excluded). A payee that looks like another payee in the list -- first
    /// and last four hex characters shared -- is left off: one approval for
    /// many addresses is exactly where a poisoned address would slip through,
    /// and it is the verifier's own look-alike rule that keeps it out.
    ///
    /// `own` is the agent's own payments, as [`RunOutcome::own_payments`]
    /// returns them for the pass that read `history`.
    pub async fn payees_to_approve(
        &self,
        history: &[Transfer],
        own: &std::collections::BTreeSet<String>,
    ) -> Result<Vec<Address>, OperatorError> {
        let evidence: Vec<Transfer> = history
            .iter()
            .filter(|t| !own.contains(&t.tx_hash))
            .cloned()
            .collect();
        let found: Vec<String> = detect(&evidence, self.config.detection)
            .into_iter()
            .map(|o| o.counterparty.as_str().to_string())
            .collect();
        let candidates: Vec<Address> = found
            .iter()
            .filter(|p| !found.iter().any(|q| ledger::looks_alike(p, q)))
            .filter_map(|p| parse_address(p))
            .collect();
        let attested = self
            .client
            .attested_among(self.config.policy, &candidates)
            .await?
            .unwrap_or_default();
        Ok(candidates
            .into_iter()
            .filter(|a| !attested.contains(&format!("{a:#x}")))
            .collect())
    }

    /// Move `o` forward past every cycle already settled on chain.
    ///
    /// With the agent's payments kept out of the evidence, `detect` projects
    /// the cycle after the owner's last payment. If the agent (or the
    /// principal, by `settleAsPrincipal`) has settled that one, the bill now
    /// owed is the next. Stepping by the interval keeps every later due date
    /// on the schedule the owner established, rather than on whatever day the
    /// agent happened to pay.
    /// One full pass.
    ///
    /// `execute = false` runs everything except the transfers, which is how
    /// you inspect what the agent would do before handing it the keys.
    pub async fn run_once(
        &mut self,
        now: Timestamp,
        history: Option<Vec<Transfer>>,
        execute: bool,
    ) -> Result<RunOutcome, OperatorError> {
        self.run_once_deciding(now, history, execute, &decide::RuleBased)
            .await
    }

    /// [`Operator::run_once`] with `decider` choosing at every decision
    /// point (docs/112): after the verifier, code lists what it allows for
    /// the bill, the decider picks one -- pay, pay early, wait, ask the owner,
    /// refuse -- and [`decide::guard`] holds the answer to that list and to
    /// the case's own numbers. A decider that fails gives the rule's decision,
    /// recorded as a fallback. `run_once` is this with the rule.
    pub async fn run_once_deciding<D: decide::Decider + Sync>(
        &mut self,
        now: Timestamp,
        history: Option<Vec<Transfer>>,
        execute: bool,
        decider: &D,
    ) -> Result<RunOutcome, OperatorError> {
        // The Action IR carries unsigned seconds, so a clock before the
        // epoch has no representation there. Refuse rather than clamp: a
        // silently-clamped timestamp produces an action whose validity
        // window is nonsense, and the verifier would then reason about a
        // time that never existed.
        let now_unsigned = u64::try_from(now.as_unix())
            .map_err(|_| OperatorError::TimeBeforeEpoch(now.as_unix()))?;

        let history = match history {
            Some(h) => h,
            None => self.read_history().await?,
        };

        // The opening reads do not depend on each other (docs/68 F4).
        let (balance, authority, governance, agent_expiry) = tokio::join!(
            self.client.policy_balance(self.config.policy),
            self.client.authority(self.config.policy),
            self.client.governance(self.config.policy),
            self.client.agent_expiry(self.config.policy),
        );
        let (balance, authority) = (balance?, authority?);
        // docs/46 #4: the autonomy level, from the contract. Below Autopilot
        // the contract refuses `pay`, so an Allow verdict becomes a prepared
        // payment for the owner's passkey rather than a transaction. A policy
        // from before modes existed has no `mode()`; it is Autopilot by
        // construction and is reported as such. Only a revert says that: a
        // read that failed fails the pass, never reads as Autopilot (audit
        // 2026-10-15, docs/60 P-74).
        let governance = match governance {
            Ok(g) => Some(g),
            Err(e) if arc::is_revert(&e) => None,
            Err(e) => return Err(e.into()),
        };
        let mode = governance.as_ref().map(|g| g.mode);
        let propose_only = mode.is_some_and(|m| m != PolicyMode::Autopilot);
        // PLAN 5.21 (docs/59 §3.15): a read-back mismatch on an earlier pass
        // wrote HALT beside the audit log; until a human removes it, this
        // operator proposes and never pays.
        let halt_path = self
            .config
            .audit_dir
            .clone()
            .unwrap_or_else(audit::dir)
            .join("HALT");
        let mut halted = halt_path.exists();
        // PLAN 5.24: the agent's authority lapses; a lapsed agent proposes and
        // does not try to pay (the contract would refuse with AgentLapsed).
        let lapsed = agent_expiry.is_some_and(|e| now_unsigned >= e);
        let propose_only = propose_only || halted || lapsed;
        let mode_note = match mode {
            Some(m) if m != PolicyMode::Autopilot => format!(
                " Policy mode is {}: the contract refuses my payments, so each one below is \
                 prepared for the owner's passkey instead of sent.",
                m.label()
            ),
            _ => String::new(),
        };

        let DuePlan {
            own,
            obligations,
            upcoming: upcoming_all,
            due,
            aside,
            owner_paid,
        } = self.obligations_due(&history, now).await?;
        // PLAN 5.56: a bill whose invoice takes money off for paying early is
        // paid early at the discounted amount -- only from money no bill in
        // the horizon needs. Same cycle key, so it is paid once either way.
        // With no floor read (a policy from before governance), nothing is
        // paid early: a floor of zero would spend the reserve.
        let early = match &governance {
            Some(g) => early_payments(
                &upcoming_all,
                &due,
                &self.config.early,
                now,
                balance,
                g.reserve_floor,
            ),
            None => Vec::new(),
        };
        let mut due = due;
        due.extend(early.iter().map(|(o, _)| o.clone()));

        // How sure the detector is about each of these, keyed the way the
        // verifier will look them up. Built here, from `detect`'s own output,
        // before the reasoner is given anything -- so a model that reorders
        // the queue, or is talked into reordering it, cannot touch the number
        // its own proposals are judged against.
        let inferred_confidence: BTreeMap<String, ledger::Confidence> = due
            .iter()
            .map(|o| {
                (
                    format!("{}:{}", self.config.principal_id, o.cycle_key()),
                    o.confidence,
                )
            })
            .collect();
        // How far behind each due bill is: later cycles that have also fallen
        // due on top of the one this pass would pay. Same keying as the
        // confidence map, and computed here for the same reason -- from the
        // evidence and the chain, out of the reasoner's reach.
        let cycles_behind: BTreeMap<String, u32> = due
            .iter()
            .filter_map(|o| {
                let behind = cycles_behind(o, now);
                (behind > 0).then(|| {
                    (
                        format!("{}:{}", self.config.principal_id, o.cycle_key()),
                        behind,
                    )
                })
            })
            .collect();
        // The range each due bill was ever paid in, from the evidence
        // transfers the obligation cites. Same keying as the confidence map;
        // computed here, from the chain, out of the reasoner's reach.
        let by_hash: BTreeMap<&str, Money> = history
            .iter()
            .map(|t| (t.tx_hash.as_str(), t.amount))
            .collect();
        let observed_amounts: BTreeMap<String, (Money, Money)> = due
            .iter()
            .filter_map(|o| {
                let mut amounts: Vec<Money> = o
                    .evidence
                    .iter()
                    .filter_map(|h| by_hash.get(h.as_str()).copied())
                    .collect();
                if amounts.is_empty() {
                    return None;
                }
                amounts.sort_by_key(|m| m.to_minor());
                Some((
                    format!("{}:{}", self.config.principal_id, o.cycle_key()),
                    (amounts[0], amounts[amounts.len() - 1]),
                ))
            })
            .collect();
        let total_due = ledger::total_due(&upcoming_all, balance.currency())
            .unwrap_or_else(|| Money::zero(balance.currency()));

        self.log.append(
            now,
            EventBody::Observation {
                balance,
                transfers_seen: history.len(),
                obligations_found: obligations.len(),
                total_due,
                note: format!(
                    "Holding {balance}. Read {} transfers ({} of them my own payments, \
                     which are not evidence) and recovered {} recurring obligations, of \
                     which {} fall due within {} totalling {total_due}; {} within {} (or a \
                     tenth of their interval), which is when I pay.{mode_note}",
                    history.len(),
                    history.iter().filter(|t| own.contains(&t.tx_hash)).count(),
                    obligations.len(),
                    upcoming_all.len(),
                    self.config.horizon,
                    due.len(),
                    self.config.pay_lead
                ),
            },
        );

        let mut outcome = RunOutcome {
            anchor_error: None,
            caught_up: Vec::new(),
            // Filled when the decider takes the discount (docs/112).
            early: Vec::new(),
            decider: decider.name().to_string(),
            decisions: Vec::new(),
            balance,
            transfers_seen: history.len(),
            obligations_found: obligations.len(),
            obligations: obligations.clone(),
            reasoner: self.reasoner.name().to_string(),
            settled: Vec::new(),
            escalated: Vec::new(),
            pending_signature: Vec::new(),
            stale_escalations: 0,
            blocked: Vec::new(),
            skipped: Vec::new(),
            root: Hash32::ZERO,
            anchor_tx: None,
            mode,
            own_payments: own.clone(),
            agent_expiry,
        };
        // A bill set aside is said in the record, in words, not dropped silently (docs/60 P-210).
        for (o, why) in aside {
            self.log.append(
                now,
                EventBody::Skipped {
                    counterparty: o.counterparty.clone(),
                    cycle_key: o.cycle_key(),
                    note: why,
                },
            );
            outcome.skipped.push(o);
        }

        // Who we have paid before, and to which details. Built from the log
        // rather than configured, so an unfamiliar counterparty is genuinely
        // one this agent has not dealt with.
        let known_payees = self.known_payees(&history);

        // Who the contract will actually pay. Read once per run, for the
        // payees this run might propose, so a payee the principal has not
        // attested reaches the principal as a question instead of reaching
        // the chain as a revert.
        let candidates: Vec<Address> = due
            .iter()
            .filter_map(|o| parse_address(o.counterparty.as_str()))
            .collect();
        // PLAN 5.47: the issuer's blocklist, for every payee this run might
        // pay and for the account the money leaves from. Read from the token,
        // like the attestations from the policy; a frozen address is refused.
        // Both lists are read at once (docs/68 F4).
        let mut screened = candidates.clone();
        screened.push(self.config.policy);
        // docs/112: when the owner approved each payee (`attestedSince`), read
        // with them; a policy from before it answers nothing (see `payee_age`).
        let (onchain_attested, issuer_blocked, approved_since) = tokio::join!(
            self.client.attested_among(self.config.policy, &candidates),
            self.client.issuer_blocked_among(&screened),
            self.client
                .attested_since_among(self.config.policy, &candidates),
        );
        let onchain_attested = onchain_attested?;
        let approved_since: BTreeMap<String, u64> = approved_since
            .map(|v| v.into_iter().map(|(a, t)| (format!("{a:#x}"), t)).collect())
            .unwrap_or_default();
        // The decider's facts that are the same for every bill this pass: the
        // runway against the balance above the floor, and whom a payee could
        // imitate (the verifier's references for its look-alike rule).
        let available = balance.to_minor()
            - governance
                .as_ref()
                .map(|g| g.reserve_floor.to_minor())
                .unwrap_or(0);
        let runway = decide::runway_days(&upcoming_all, available, now.as_unix());
        let references: Vec<String> = known_payees
            .keys()
            .cloned()
            .chain(onchain_attested.iter().flatten().cloned())
            .collect();

        // Judgment is the only part of this loop that depends on a service
        // outside it. A model that is rate-limited, timing out, or simply
        // down must not stop the agent from paying bills it can already
        // justify, so the deterministic control takes over.
        //
        // The fallback is recorded rather than silent, and the actions it
        // produces are attributed to `rule-based`. Attributing them to the
        // model would be a false provenance claim in a log that gets hashed
        // and anchored on-chain -- the one place a lie is expensive to undo.
        // Safety is unaffected either way: both orderings face the same
        // verifier and the same on-chain caps.
        // One more try on a transient failure (a dropped connection, a 429,
        // a 5xx) before the rule takes over; a permanent one (a bad key)
        // falls back at once. docs/54 §3: classified recovery, cheaply.
        let mut attempt = self
            .reasoner
            .prioritise(&due, balance, now)
            .await
            .and_then(|r| conform(r, &due).ok_or(ReasonerError::WrongPopulation));
        if matches!(&attempt, Err(e) if e.retryable()) {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            attempt = self
                .reasoner
                .prioritise(&due, balance, now)
                .await
                .and_then(|r| conform(r, &due).ok_or(ReasonerError::WrongPopulation));
        }
        let (ranked, actor_name, actor_kind) = match attempt {
            Ok(r) => (r, self.reasoner.name().to_string(), self.reasoner.kind()),
            Err(e) => {
                // The code and the retryable flag go in the record, not
                // just the sentence: an operator reading the log needs to
                // know whether this was a blip worth re-running or a
                // misconfiguration that will repeat until someone acts.
                let note = format!(
                    "Reasoner {} failed [{}], retryable={}: {e}. Ordered by the \
                         deterministic baseline instead. Limits are unchanged.",
                    self.reasoner.name(),
                    e.code(),
                    e.retryable()
                );
                self.log.append(
                    now,
                    EventBody::Observation {
                        balance,
                        transfers_seen: history.len(),
                        obligations_found: obligations.len(),
                        total_due,
                        note,
                    },
                );
                outcome.reasoner = format!("rule-based (fallback from {})", self.reasoner.name());
                (
                    rank_by_urgency(&due, balance, now),
                    "rule-based".to_string(),
                    ActorKind::Rule,
                )
            }
        };

        // PLAN 5.25 (docs/59 §3.3): standing orders the owner wrote into the
        // contract. Due ones are paid first, on Autopilot only (docs/60 P-211:
        // in Propose the owner taps Pay now), never for a bill-cycle already
        // paid by any path, and not while HALT is present. Anyone could call
        // `payDue`; the agent simply does it on time, and the caps bound it like any pay.
        // A failed read fails the pass: with no list, a bill an order pays
        // would also be paid below under its own cycle key, and the contract
        // cannot catch it (different replay keys; audit 2026-10-15).
        let schedules = self.client.schedules(self.config.policy).await?;
        // A HALT that could not be written: the pass still saves and anchors
        // its record, then fails (audit 2026-10-15).
        let mut halt_write_error: Option<String> = None;
        let mut covered: Vec<(String, i64, i64)> = Vec::new();
        'orders: for sc in schedules.iter().filter(|s| s.active) {
            let cp = format!("{:#x}", sc.payee);
            covered.push((
                cp.clone(),
                sc.next_due.saturating_sub(sc.every_secs) as i64,
                sc.next_due.saturating_add(sc.every_secs) as i64,
            ));
            if !execute || halted || mode == Some(PolicyMode::Watch) {
                continue;
            }
            let mut due_at = sc.next_due;
            let mut paid_cycles = 0u8;
            // Arrears are paid one cycle per call, at most three per pass.
            while due_at <= now_unsigned && paid_cycles < 3 {
                let counterparty = ledger::Counterparty::new(cp.clone());
                // The bill-cycle this date is, paid by any path: its own
                // settlement key, or a reference offered for it (docs/60 P-211).
                let cycle = format!("{cp}@{due_at}");
                let bill_key = B256::from(
                    *ledger::settlement_key_for(&Idempotency {
                        key: cycle.clone(),
                        scope: self.config.principal_id.clone(),
                    })
                    .as_bytes(),
                );
                let paid = self
                    .client
                    .already_executed(self.config.policy, bill_key)
                    .await?
                    || owner_paid_cycle(&owner_paid, &self.config.offered, &cycle);
                if let Err(why) = order_due_step(propose_only, paid, sc.id, due_at) {
                    self.log.append(
                        now,
                        EventBody::Skipped {
                            counterparty,
                            cycle_key: format!("schedule:{}:{due_at}", sc.id),
                            note: why,
                        },
                    );
                    break;
                }
                let key = self
                    .client
                    .schedule_action_hash(self.config.policy, sc.id, due_at)
                    .await?;
                match self.client.pay_due(self.config.policy, sc.id).await {
                    Ok(tx) => {
                        let readback = self
                            .client
                            .confirm_paid(tx, self.config.policy, sc.payee, sc.amount, key)
                            .await;
                        let confirm_note = match &readback {
                            Ok(()) => {
                                "Read back from the receipt: the policy's Paid event matches."
                                    .to_string()
                            }
                            Err(e) => format!("READ-BACK MISMATCH: {e}"),
                        };
                        self.log.append(
                            now,
                            EventBody::Settlement {
                                counterparty: counterparty.clone(),
                                cycle_key: format!("schedule:{}:{due_at}", sc.id),
                                amount: sc.amount,
                                tx_hash: format!("{tx:#x}"),
                                action_hash: ledger::ActionHash::from_bytes(key.into()),
                                settlement_key: ledger::ActionHash::from_bytes(key.into()),
                                note: format!(
                                    "Standing order #{} paid: {} to {} for its due date {due_at}, settled in {tx:#x}. The owner set this order; anyone could have triggered it, the agent did it on time. {confirm_note}",
                                    sc.id, sc.amount, cp
                                ),
                            },
                        );
                        outcome.settled.push((
                            Obligation {
                                counterparty: counterparty.clone(),
                                kind: ledger::obligations::ObligationKind::Subscription,
                                typical_amount: sc.amount,
                                interval: ledger::Seconds::new(sc.every_secs as i64),
                                last_paid: Timestamp::from_unix(due_at as i64),
                                next_due: Timestamp::from_unix((due_at + sc.every_secs) as i64),
                                confidence: ledger::obligations::Confidence::clamped(1.0),
                                sample_count: 0,
                                interval_cv: 0.0,
                                amount_cv: 0.0,
                                evidence: Vec::new(),
                            },
                            tx,
                        ));
                        if let Err(e) = &readback {
                            halt_write_error = write_halt(
                                &halt_path,
                                &format!(
                                    "{} read-back mismatch after {tx:#x} (standing order #{}): {e}\nRemove this file once a human has reconciled the policy's balance and events against the record.\n",
                                    now.as_unix(),
                                    sc.id
                                ),
                            );
                            self.log.append(
                                now,
                                EventBody::Escalation {
                                    counterparty,
                                    amount: sc.amount,
                                    reason: "read-back mismatch".into(),
                                    note: format!("The receipt of {tx:#x} does not say what standing order #{} should have paid. HALT written at {}; nothing more is paid until a human removes it.", sc.id, halt_path.display()),
                                },
                            );
                            halted = true;
                            break 'orders;
                        }
                        due_at += sc.every_secs;
                        paid_cycles += 1;
                    }
                    Err(e) => {
                        self.log.append(
                            now,
                            EventBody::Escalation {
                                counterparty,
                                amount: sc.amount,
                                reason: "standing order not paid".into(),
                                note: format!("payDue({}) for the due date {due_at} failed: {e}. The contract's caps, the payee list or the balance refused it; the order stays due.", sc.id),
                            },
                        );
                        break;
                    }
                }
            }
        }
        let propose_only = propose_only || halted;

        'ranked: for item in ranked {
            let obligation = item.obligation.clone();
            let cycle_key = obligation.cycle_key();
            // A bill a standing order covers is not proposed again: the order
            // pays it on its due date, whoever triggers it.
            if let Some((_, a, b)) = covered.iter().find(|(cp, a, b)| {
                *cp == obligation.counterparty.to_string().to_lowercase() && {
                    let d = obligation.next_due.as_unix();
                    d >= *a && d <= *b
                }
            }) {
                self.log.append(
                    now,
                    EventBody::Skipped {
                        counterparty: obligation.counterparty.clone(),
                        cycle_key: cycle_key.clone(),
                        note: format!(
                            "covered by a standing order the owner set (due window {a}–{b}); it is paid on its due date by payDue, not proposed again"
                        ),
                    },
                );
                outcome.skipped.push(obligation);
                continue 'ranked;
            }

            let action = self.build_action(
                &obligation,
                &item,
                &authority,
                now_unsigned,
                &actor_name,
                actor_kind,
            );
            let hash = action_hash(&action);
            // Two different hashes, deliberately. `hash` records what was
            // proposed and why; `key` identifies which bill this is. Only the
            // second is stable across runs, so only the second may be used
            // where the chain is asked "have we done this already?".
            let key = B256::from(*settlement_key(&action).as_bytes());

            // The chain is the authority on what is already paid. A log file
            // may not have survived the crash that caused the retry.
            if self
                .client
                .already_executed(self.config.policy, key)
                .await?
            {
                self.log.append(
                    now,
                    EventBody::Skipped {
                        counterparty: obligation.counterparty.clone(),
                        cycle_key: cycle_key.clone(),
                        note: format!(
                            "Skipping {}: this billing cycle is already settled on chain. \
                             Paying it again would balance the books and still be a double \
                             payment.",
                            obligation.counterparty
                        ),
                    },
                );
                outcome.skipped.push(obligation);
                continue;
            }

            // `proposed_payee` is deliberately the same map as
            // `known_payees`, so `payee_changed` cannot fire. That is honest
            // rather than lazy: on chain the payee's "bank details" ARE the
            // address, and an obligation is keyed by address, so the details
            // of a given obligation cannot change by construction. The check
            // becomes meaningful only once a counterparty carries a
            // human-confirmed label that can outlive an address change --
            // `docs/18-action-ir.md` lists that under what is not built.
            let world = World {
                now: now_unsigned,
                treasury: self.config.treasury_account.clone(),
                balance,
                authority: authority.clone(),
                settled: self
                    .log
                    .settled_cycles()
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                known_payees: known_payees.clone(),
                proposed_payee: known_payees.clone(),
                // docs/76 §7 row 7: the confidence floor is per mode. Where the
                // agent cannot pay alone this pass (Propose, a halt, a lapsed
                // agent) the default 0.75 becomes 0.60 -- a wrong guess there
                // costs the owner a glance. A floor a caller set to anything
                // other than the Autopilot default is theirs and is kept.
                required_preconditions: self
                    .config
                    .required_preconditions
                    .iter()
                    .map(|p| match p {
                        Precondition::ConfidenceAtLeast(bp)
                            if *bp == ledger::action::CONFIDENCE_FLOOR_AUTOPILOT_BP =>
                        {
                            Precondition::ConfidenceAtLeast(ledger::action::confidence_floor_bp(
                                !propose_only,
                            ))
                        }
                        other => other.clone(),
                    })
                    .collect(),
                attested_payees: self.config.attested_payees.clone(),
                inferred_confidence: inferred_confidence.clone(),
                purchase_orders: self.config.purchase_orders.clone(),
                receipts: self.config.receipts.clone(),
                onchain_attested: onchain_attested.clone(),
                cycles_behind: cycles_behind.clone(),
                observed_amounts: observed_amounts.clone(),
                issuer_blocked: issuer_blocked.clone(),
            };

            let verdict = verify(&action, &world);

            // The decision point (docs/112). Every number in the case is the
            // pass's own; code lists what it allows from the verdict and the
            // contract's state; the decider picks inside that list.
            let early_saved = early
                .iter()
                .find(|(e, _)| e.cycle_key() == cycle_key)
                .map(|(_, saved)| *saved);
            // An early candidate is cased at its full amount: `pay_early` pays
            // the discounted one, `wait` the full one later.
            let full =
                obligation.typical_amount.to_minor() + early_saved.map_or(0, |s| s.to_minor());
            let terms = self
                .config
                .early
                .iter()
                .find(|t| t.counterparty == obligation.counterparty);
            let payee = obligation.counterparty.as_str().to_lowercase();
            let on_list = onchain_attested
                .as_ref()
                .is_some_and(|l| l.contains(&payee));
            // ponytail: a policy from before `attestedSince` (10-16) gives no
            // approval date; an approved payee there counts from the owner's
            // first payment to it.
            let payee_age = (on_list || onchain_attested.is_none())
                .then(|| {
                    approved_since
                        .get(&payee)
                        .copied()
                        .filter(|t| *t > 0)
                        .map(|t| t as i64)
                        .or_else(|| {
                            history
                                .iter()
                                .filter(|t| obligation.evidence.contains(&t.tx_hash))
                                .map(|t| t.at.as_unix())
                                .min()
                        })
                        .map(|t| u32::try_from((now.as_unix() - t).max(0) / 86_400).unwrap_or(0))
                })
                .flatten();
            let due_in_days = decide::days_until(obligation.next_due.as_unix(), now.as_unix());
            let case = decide::Case {
                id: cycle_key.clone(),
                payee: decide::label(obligation.counterparty.as_str()),
                amount_minor: u128::try_from(full).unwrap_or(0),
                usual_minor: (obligation.sample_count > 0)
                    .then(|| u128::try_from(full).unwrap_or(0)),
                samples: u32::try_from(obligation.sample_count).unwrap_or(u32::MAX),
                cadence_days: u32::try_from(obligation.interval.as_i64().max(0) / 86_400).ok(),
                due_in_days,
                runway_days: runway,
                discount_bps: terms.map_or(0, |t| t.discount_bps),
                early_days: terms.map_or(0, |t| {
                    u32::try_from(t.lead.as_i64().max(0) / 86_400).unwrap_or(0)
                }),
                payee_age_days: payee_age,
                // As the verifier: a payee the owner attested is never a look-alike.
                lookalike_of: (!on_list)
                    .then(|| references.iter().find(|r| ledger::looks_alike(&payee, r)))
                    .flatten()
                    .map(|r| decide::label(r)),
                confidence: obligation.confidence.get(),
                allowed: decide::allowed_for_bill(
                    verdict.decision,
                    !propose_only,
                    early_saved.is_some(),
                    due_in_days,
                ),
                kind: Some(decide::CaseKind::Bill),
                cap_minor: u128::try_from(authority.per_action_cap.to_minor()).ok(),
                typed: obligation.sample_count == 0 && self.is_seeded(&obligation),
                evidence: Vec::new(),
                evidence_root: None,
            };
            // docs/117 §8: what the decider may cite about this payee, from the
            // latest anchored record (this pass's own is not anchored yet).
            let (evidence_root, evidence) = recall::evidence(
                &self.config.audit_dir.clone().unwrap_or_else(audit::dir),
                &payee,
                now.as_unix(),
            );
            let case = decide::Case {
                evidence,
                evidence_root,
                ..case
            };
            let (decided, decided_note) = decide::decide_or_rule(decider, &case).await;
            let (decided, decided_note) =
                model_refusal_asks(&case, decided, decided_note, decider.name());
            let decided_line = decide::record_line(&decided, decided_note.as_deref())
                + &decide::cited_record(&case, &decided).unwrap_or_default();

            self.log.append(
                now,
                EventBody::Decision {
                    counterparty: obligation.counterparty.clone(),
                    amount: obligation.typical_amount,
                    decision: verdict.decision,
                    action_hash: hash,
                    confidence: obligation.confidence,
                    balance_seen: balance,
                    weakest_evidence: action.weakest_provenance(),
                    evidence: obligation.evidence.clone(),
                    note: format!(
                        "{} for {}. {} Basis: {} {decided_line}",
                        verdict.decision,
                        obligation.counterparty,
                        item.rationale,
                        obligation.explain()
                    ),
                },
            );
            outcome
                .decisions
                .push((case, decided.clone(), decided_note.clone()));

            match verdict.decision {
                // The decider refused what the checks allowed or escalated:
                // its own record, so the statement lists it as refused.
                _ if decided.action == decide::Action::Refuse => {
                    if verdict.decision != Decision::Block {
                        self.log.append(
                            now,
                            EventBody::Decision {
                                counterparty: obligation.counterparty.clone(),
                                amount: obligation.typical_amount,
                                decision: Decision::Block,
                                action_hash: hash,
                                confidence: obligation.confidence,
                                balance_seen: balance,
                                weakest_evidence: action.weakest_provenance(),
                                evidence: obligation.evidence.clone(),
                                note: format!(
                                    "Refused for {}, which the checks did not refuse. {decided_line}",
                                    obligation.counterparty
                                ),
                            },
                        );
                    }
                    outcome.blocked.push((obligation, verdict));
                }
                _ if decided.action == decide::Action::Wait => {
                    self.log.append(
                        now,
                        EventBody::Skipped {
                            counterparty: obligation.counterparty.clone(),
                            cycle_key: cycle_key.clone(),
                            note: format!(
                                "Waiting this pass; the next pass decides again. {decided_line}"
                            ),
                        },
                    );
                    outcome.skipped.push(obligation);
                }
                // Only `refuse` is allowed on a block; kept so the match is total.
                Decision::Block => {
                    outcome.blocked.push((obligation, verdict));
                }
                Decision::Review => {
                    let reason = verdict
                        .problems()
                        .map(|c| format!("{}: {}", c.name, c.detail))
                        .collect::<Vec<_>>()
                        .join("; ");

                    // How long this has been waiting. Derived from the due
                    // date rather than from a stored escalation record, so
                    // the check is correct on a fresh process and cannot be
                    // reset by losing the log.
                    let waited = obligation.overdue_by(now);
                    let stale = waited > self.config.escalation_deadline;
                    if stale {
                        outcome.stale_escalations += 1;
                    }

                    let note = if stale {
                        format!(
                            "UNANSWERED for {:.1} days, past the {:.1}-day deadline: {reason}. \
                             I have escalated this and nobody has acted. The bill is still \
                             unpaid and I cannot pay it myself.",
                            waited.as_days(),
                            self.config.escalation_deadline.as_days()
                        )
                    } else {
                        format!(
                            "Stopping and asking a human: {reason}. This is outside the \
                             authority delegated to me, so I will not proceed on my own. \
                             If unanswered for {:.1} days I will report it as overdue.",
                            self.config.escalation_deadline.as_days()
                        )
                    };

                    self.log.append(
                        now,
                        EventBody::Escalation {
                            counterparty: obligation.counterparty.clone(),
                            amount: obligation.typical_amount,
                            reason: reason.clone(),
                            note,
                        },
                    );
                    self.hand_over(
                        &mut outcome,
                        now,
                        &obligation,
                        &cycle_key,
                        key,
                        &hash,
                        &verdict,
                        stale,
                    );

                    outcome.escalated.push((obligation, verdict));
                }
                Decision::Allow => {
                    if !execute {
                        outcome.escalated.push((obligation, verdict));
                        continue;
                    }
                    // docs/112: the decider chose to ask although the agent
                    // could pay; the owner gets the same calldata and its why.
                    if decided.action == decide::Action::Ask && !propose_only {
                        let reason = format!("the agent asks: {}", decided.why);
                        self.log.append(
                            now,
                            EventBody::Escalation {
                                counterparty: obligation.counterparty.clone(),
                                amount: obligation.typical_amount,
                                reason: reason.clone(),
                                note: format!(
                                    "Prepared for the owner, not paid: everything the verifier \
                                     checked passed, and I chose to ask. {decided_line}"
                                ),
                            },
                        );
                        self.hand_over(
                            &mut outcome,
                            now,
                            &obligation,
                            &cycle_key,
                            key,
                            &hash,
                            &verdict,
                            false,
                        );
                        outcome.escalated.push((obligation, verdict));
                        continue;
                    }
                    if propose_only {
                        // The verifier would let this through; the contract
                        // would not. Hand it to the owner with the same
                        // calldata Autopilot would have sent (docs/46 #4).
                        // Say the real cause: a lapsed term or a HALT reads as a
                        // contradiction under the mode's label (docs/60 P-106).
                        let label = mode.map(|m| m.label()).unwrap_or("Propose");
                        let reason = if lapsed {
                            "term: the agent's authority has lapsed; the contract refuses its \
                             payments until the owner renews it, and the owner's passkey settles this one"
                                .to_string()
                        } else if halted {
                            "halt: a safety check stopped the agent (HALT); nothing is paid until a \
                             human clears it, and the owner's passkey can settle this one"
                                .to_string()
                        } else {
                            format!(
                                "mode: policy is in {label}, not Autopilot; the contract refuses the \
                                 agent's payment and the owner's passkey settles it"
                            )
                        };
                        self.log.append(
                            now,
                            EventBody::Escalation {
                                counterparty: obligation.counterparty.clone(),
                                amount: obligation.typical_amount,
                                reason: reason.clone(),
                                note: format!(
                                    "Prepared for the owner, not paid: {reason}. Everything the \
                                     verifier checked passed; only the autonomy level stops me."
                                ),
                            },
                        );
                        self.hand_over(
                            &mut outcome,
                            now,
                            &obligation,
                            &cycle_key,
                            key,
                            &hash,
                            &verdict,
                            false,
                        );
                        outcome.escalated.push((obligation, verdict));
                        continue;
                    }
                    // PLAN 5.56: the decider took the discount (`pay_early` is
                    // allowed only for the pass's own early candidates, so the
                    // amount here is already the discounted one).
                    if decided.action == decide::Action::PayEarly
                        && let Some(saved) = early_saved
                    {
                        let full_amount = obligation
                            .typical_amount
                            .checked_add(saved)
                            .unwrap_or(obligation.typical_amount);
                        let lead = terms.map(|t| t.lead).unwrap_or(self.config.pay_lead);
                        self.log.append(
                            now,
                            EventBody::Observation {
                                balance,
                                transfers_seen: history.len(),
                                obligations_found: obligations.len(),
                                total_due,
                                note: format!(
                                    "Paying {} early: its invoice takes {saved} off when paid at least {lead} \
                                     before the due date ({}). Every bill due within {} is covered with the \
                                     floor kept, so I pay {} now instead of {full_amount} later.",
                                    obligation.counterparty,
                                    obligation.next_due.as_unix(),
                                    self.config.horizon,
                                    obligation.typical_amount
                                ),
                            },
                        );
                        outcome.early.push((cycle_key.clone(), saved));
                    }
                    let to = parse_address(obligation.counterparty.as_str())
                        .ok_or_else(|| OperatorError::Unpayable(cycle_key.clone()))?;

                    // Ask before spending. The verifier has already approved
                    // this against the limits it read from chain, so a revert
                    // here means those limits moved underneath us, or the
                    // cycle was settled by someone else between the replay
                    // check and now. Either way it is a fact worth recording
                    // rather than a receipt worth paying gas for.
                    if let Err(e) = self
                        .client
                        .simulate_payment(self.config.policy, to, obligation.typical_amount, key)
                        .await
                    {
                        self.log.append(
                            now,
                            EventBody::Skipped {
                                counterparty: obligation.counterparty.clone(),
                                cycle_key: cycle_key.clone(),
                                note: format!(
                                    "simulation-reverted: the chain refuses this payment, so \
                                     it was not sent. {e}"
                                ),
                            },
                        );
                        outcome.skipped.push(obligation);
                        continue;
                    }

                    let tx = self
                        .client
                        .pay_via_policy(self.config.policy, to, obligation.typical_amount, key)
                        .await?;
                    // PLAN 5.21 (docs/59 §3.15): no payment is done until the
                    // receipt is read back and the policy's own Paid event
                    // names what was sent. A mismatch halts this operator —
                    // the hedge's discipline, applied to payments.
                    let readback = self
                        .client
                        .confirm_paid(tx, self.config.policy, to, obligation.typical_amount, key)
                        .await;
                    let confirm_note = match &readback {
                        Ok(()) => "Read back from the receipt: the policy's Paid event matches."
                            .to_string(),
                        Err(e) => format!("READ-BACK MISMATCH: {e}"),
                    };

                    // A separate record, never an edit to the decision. An
                    // append-only log that rewrites earlier entries is not
                    // append-only, and the receipt is the external witness
                    // the decision record cannot be on its own.
                    self.log.append(
                        now,
                        EventBody::Settlement {
                            counterparty: obligation.counterparty.clone(),
                            cycle_key: cycle_key.clone(),
                            amount: obligation.typical_amount,
                            tx_hash: format!("{tx:#x}"),
                            action_hash: hash,
                            // `key` is what `pay_via_policy` just sent, so
                            // the record and the chain agree by construction
                            // rather than by coincidence.
                            settlement_key: ledger::ActionHash::from_bytes(key.into()),
                            note: format!(
                                "Paid {} to {}, settled in {tx:#x}. {confirm_note}",
                                obligation.typical_amount, obligation.counterparty
                            ),
                        },
                    );
                    if let Err(e) = &readback {
                        halt_write_error = write_halt(
                            &halt_path,
                            &format!(
                                "{} read-back mismatch after {tx:#x}: {e}\nRemove this file once a human has reconciled the policy's balance and events against the record.\n",
                                now.as_unix()
                            ),
                        );
                        self.log.append(
                            now,
                            EventBody::Escalation {
                                counterparty: obligation.counterparty.clone(),
                                amount: obligation.typical_amount,
                                reason: "read-back mismatch".into(),
                                note: format!(
                                    "The receipt of {tx:#x} does not say what the agent sent. HALT written at {}; nothing more is paid this pass or later until a human removes it.",
                                    halt_path.display()
                                ),
                            },
                        );
                        outcome.settled.push((obligation, tx));
                        break 'ranked;
                    }
                    outcome.settled.push((obligation, tx));
                }
            }
        }

        outcome.root = self.log.root();
        if execute && !self.log.is_empty() {
            // The events behind the root go to disk *before* the root goes
            // on chain: an anchored root with nothing to hash is a claim no
            // auditor can check (docs/46 #1). A failed write refuses to anchor.
            let audit_dir = self.config.audit_dir.clone().unwrap_or_else(audit::dir);
            // A record an earlier pass wrote but could not anchor (it paid,
            // then ran out of gas or RPC) is anchored first: a payment whose
            // reasons sit under no root is a claim no auditor can check.
            let earlier = audit::unanchored(&audit_dir)
                .map_err(|e| OperatorError::AuditWrite(e.to_string()))?;
            audit::persist(&audit_dir, &self.log)
                .map_err(|e| OperatorError::AuditWrite(e.to_string()))?;
            for (root, events) in earlier.into_iter().filter(|(r, _)| *r != outcome.root) {
                match self
                    .client
                    .anchor_root(self.config.memory_anchor, B256::from(*root.as_bytes()))
                    .await
                {
                    Ok(tx) => {
                        let _ = audit::append_anchor(
                            &audit_dir,
                            root,
                            Some(format!("{tx:#x}")),
                            self.client.address().map(|a| format!("{a:#x}")),
                            format!("{:#x}", self.config.memory_anchor),
                            now.as_unix(),
                            events,
                        );
                        outcome.caught_up.push((root, tx));
                    }
                    Err(e) => {
                        outcome.anchor_error =
                            Some(format!("an earlier record is still not anchored: {e}"));
                        break;
                    }
                }
            }
            // A failed anchor no longer throws the pass away: what was paid is
            // reported (the payments are on chain), and the next pass anchors
            // this record before its own.
            match self
                .client
                .anchor_root(
                    self.config.memory_anchor,
                    B256::from(*outcome.root.as_bytes()),
                )
                .await
            {
                Ok(tx) => {
                    outcome.anchor_tx = Some(tx);
                    if let Err(e) = audit::append_anchor(
                        &audit_dir,
                        outcome.root,
                        outcome.anchor_tx.map(|t| format!("{t:#x}")),
                        self.client.address().map(|a| format!("{a:#x}")),
                        format!("{:#x}", self.config.memory_anchor),
                        now.as_unix(),
                        self.log.len(),
                    ) {
                        // The anchor is already on chain and the events file
                        // exists; losing the index line is recoverable, so
                        // report, don't fail.
                        eprintln!("audit: anchor is on chain but not in anchors.jsonl: {e}");
                    }
                }
                Err(e) => {
                    outcome.anchor_error = Some(format!(
                        "this record is on disk but not anchored yet ({e}); the next pass anchors it first"
                    ));
                }
            }
        }

        match halt_write_error {
            Some(e) => Err(OperatorError::HaltWrite(e)),
            None => Ok(outcome),
        }
    }

    /// Compose the transaction the principal would need to sign and attach
    /// it to the outcome. Building it requires no authority -- that is the
    /// whole point of the split -- so an action the agent is forbidden to
    /// send is still one it can hand over. Used for escalations and, in
    /// Watch/Propose mode, for every payment (docs/46 #4).
    #[allow(clippy::too_many_arguments)]
    fn hand_over(
        &mut self,
        outcome: &mut RunOutcome,
        now: Timestamp,
        obligation: &Obligation,
        cycle_key: &str,
        key: B256,
        hash: &ActionHash,
        verdict: &Verdict,
        stale: bool,
    ) {
        // Compose the transaction the principal would need to
        // sign. Building it requires no authority -- that is the
        // whole point of the split -- so an action the agent is
        // forbidden to send is still one it can hand over.
        let prepared = parse_address(obligation.counterparty.as_str())
            .ok_or_else(|| ArcError::Contract("counterparty is not an address".into()))
            .and_then(|payee| {
                prepare_principal_settlement(
                    self.config.policy,
                    payee,
                    obligation.typical_amount,
                    key,
                )
            });
        match prepared {
            Ok(call) => {
                let attest = verdict
                    .problems()
                    .any(|c| c.name == "payee-attested")
                    .then(|| {
                        parse_address(obligation.counterparty.as_str())
                            .map(|payee| prepare_payee_attestation(self.config.policy, payee))
                    })
                    .flatten();
                outcome.pending_signature.push(PreparedPayment {
                    cycle_key: cycle_key.to_string(),
                    counterparty: obligation.counterparty.to_string(),
                    amount: obligation.typical_amount,
                    action_hash: hash.to_string(),
                    settlement_key: format!("{key:#x}"),
                    stale,
                    call,
                    attest,
                });
            }
            // Never fatal: a handover we could not encode must
            // not cost the run its other decisions.
            Err(e) => {
                self.log.append(
                    now,
                    EventBody::Skipped {
                        counterparty: obligation.counterparty.clone(),
                        cycle_key: cycle_key.to_string(),
                        note: format!("Could not prepare calldata for signature: {e}"),
                    },
                );
            }
        }
    }

    /// Turn an obligation into a proposed action.
    ///
    /// Everything the verifier will check is stated here, including the
    /// limits the agent believes it is under -- which the verifier then
    /// compares against the limits the contract actually holds.
    fn build_action(
        &self,
        o: &Obligation,
        item: &Prioritised,
        authority: &Authority,
        not_before: u64,
        actor_name: &str,
        actor_kind: ActorKind,
    ) -> Action {
        Action {
            id: format!("act_{}", o.cycle_key()),
            actor: Actor {
                id: actor_name.to_string(),
                kind: actor_kind,
            },
            principal: Principal {
                id: self.config.principal_id.clone(),
                treasury: self.config.treasury_account.clone(),
            },
            intent: Intent {
                verb: Verb::Settle,
                rationale: item.rationale.clone(),
            },
            target: Target {
                id: o.counterparty.to_string(),
                label: None,
            },
            amount: o.typical_amount,
            // An obligation the owner imported (PLAN 5.8) has no transfers
            // behind it yet; its evidence is the owner's word, recorded as
            // one human-confirmed piece, and the bar is one piece rather
            // than three samples. History takes over for the same payee as
            // soon as it exists (`obligations_due` prefers the detected one).
            evidence: if o.evidence.is_empty() && self.is_seeded(o) {
                vec![Evidence {
                    kind: EvidenceKind::Invoice,
                    reference: format!("owner-import:{}", o.counterparty.as_str()),
                    provenance: Provenance::HumanConfirmed,
                }]
            } else {
                o.evidence
                    .iter()
                    .map(|tx| Evidence {
                        kind: EvidenceKind::OnchainTransfer,
                        reference: tx.clone(),
                        provenance: Provenance::Onchain,
                    })
                    .collect()
            },
            preconditions: vec![
                Precondition::NotAlreadySettled,
                Precondition::CounterpartyKnown,
                Precondition::EvidenceAtLeast(if o.evidence.is_empty() && self.is_seeded(o) {
                    1
                } else {
                    ledger::obligations::MIN_SAMPLES
                }),
            ],
            authority: authority.clone(),
            expected_effects: vec![ExpectedEffect {
                account: self.config.treasury_account.clone(),
                delta: Delta::Decrease(o.typical_amount),
            }],
            idempotency: Idempotency {
                key: o.cycle_key(),
                scope: self.config.principal_id.clone(),
            },
            validity: Validity {
                not_before,
                expires_at: not_before
                    .saturating_add(self.config.validity_window.as_i64().max(0) as u64),
            },
        }
    }

    /// Counterparties this treasury has paid before, from observed history.
    ///
    /// The fingerprint is the address itself. A real vendor-master would
    /// fingerprint the payment *details* and notice when they change, which
    /// is the control that catches invoice fraud against a genuine invoice.
    /// Nothing populates that yet, so `payee_changed` is currently always
    /// false and the check is a placeholder that passes. Named here rather
    /// than left to be discovered.
    fn known_payees(&self, history: &[Transfer]) -> BTreeMap<String, String> {
        let mut known: BTreeMap<String, String> = history
            .iter()
            // Dust is how a poisoner writes a look-alike into the history.
            // Counting it as "paid before" would make the look-alike a known
            // payee, and the real payee its look-alike.
            .filter(|t| {
                t.direction == ledger::Direction::Out && !self.config.detection.is_dust(t.amount)
            })
            .map(|t| (t.counterparty.to_string(), t.counterparty.to_string()))
            .collect();
        // A payee the owner imported is known by the owner's word; the
        // service only keeps imports whose payee the owner attested on chain.
        for s in &self.config.seeded {
            known
                .entry(s.counterparty.to_string())
                .or_insert_with(|| s.counterparty.to_string());
        }
        known
    }

    /// Whether this obligation came from the owner's import rather than
    /// from history.
    fn is_seeded(&self, o: &Obligation) -> bool {
        self.config
            .seeded
            .iter()
            .any(|s| s.counterparty.as_str() == o.counterparty.as_str())
    }
}

/// Parse a `0x`-prefixed hex address.
fn parse_address(s: &str) -> Option<Address> {
    s.parse().ok()
}

/// Render the log as lines a person can read.
pub fn replay(log: &DecisionLog) -> Vec<String> {
    log.replay().collect()
}

/// The settlement key carried by each settlement, for cross-checking the
/// record against the chain.
///
/// This returns the **settlement key**, not the action hash. The on-chain
/// `executed[]` guard is keyed on the settlement key, so querying it with an
/// action hash answers `false` for a payment that certainly happened. This
/// function returned the action hash until 2026-09-25 and the cross-check in
/// `examples/run_loop.rs` therefore reported `false` for every settlement it
/// had just made. It printed the result rather than asserting on it, which is
/// why a check that could never pass went unnoticed.
pub fn settlement_hashes(log: &DecisionLog) -> Vec<(String, ActionHash)> {
    log.events()
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Settlement {
                cycle_key,
                settlement_key,
                ..
            } => Some((cycle_key.clone(), *settlement_key)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod early_payment_tests {
    //! PLAN 5.56: pay early for a discount only from money no bill needs.
    use super::*;
    use ledger::{Confidence, Counterparty, Currency, ObligationKind};

    const DAY: i64 = 86_400;
    const NOW: i64 = 1_800_000_000;

    fn bill(who: &str, minor: i128, due_in_days: i64) -> Obligation {
        Obligation {
            counterparty: Counterparty::new(who),
            kind: ObligationKind::Subscription,
            typical_amount: Money::minor(minor, Currency::Usdc),
            interval: Seconds::new(30 * DAY),
            last_paid: Timestamp::from_unix(NOW + due_in_days * DAY - 30 * DAY),
            next_due: Timestamp::from_unix(NOW + due_in_days * DAY),
            confidence: Confidence::new(1.0).unwrap(),
            sample_count: 1,
            interval_cv: 0.0,
            amount_cv: 0.0,
            evidence: vec![],
        }
    }

    fn terms(who: &str, lead_days: i64, bps: u16) -> EarlyTerms {
        EarlyTerms {
            counterparty: Counterparty::new(who),
            lead: Seconds::days(lead_days).unwrap(),
            discount_bps: bps,
        }
    }

    fn usdc(minor: i128) -> Money {
        Money::minor(minor, Currency::Usdc)
    }

    const A: &str = "0xaaaa000000000000000000000000000000000001";
    const B: &str = "0xbbbb000000000000000000000000000000000002";

    #[test]
    fn inside_the_window_and_covered_it_pays_the_discounted_amount_now() {
        let up = vec![bill(A, 100_000_000, 8), bill(B, 50_000_000, 20)];
        let got = early_payments(
            &up,
            &[],
            &[terms(A, 10, 200)],
            Timestamp::from_unix(NOW),
            usdc(1_000_000_000),
            usdc(0),
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.typical_amount, usdc(98_000_000), "2% off 100");
        assert_eq!(got[0].1, usdc(2_000_000), "saves 2");
        assert_eq!(
            got[0].0.cycle_key(),
            up[0].cycle_key(),
            "same cycle: paid once either way"
        );
    }

    #[test]
    fn outside_the_window_or_without_terms_nothing_is_paid_early() {
        let up = vec![bill(A, 100_000_000, 15), bill(B, 50_000_000, 5)];
        let got = early_payments(
            &up,
            &[],
            &[terms(A, 10, 200)],
            Timestamp::from_unix(NOW),
            usdc(1_000_000_000),
            usdc(0),
        );
        assert!(
            got.is_empty(),
            "A is 15 days out with a 10-day window; B offers no terms"
        );
    }

    #[test]
    fn early_payment_never_spends_money_a_later_bill_needs() {
        // 150 due in the horizon, 160 held, floor 20: 140 above the floor < 150.
        let up = vec![bill(A, 100_000_000, 8), bill(B, 50_000_000, 20)];
        let got = early_payments(
            &up,
            &[],
            &[terms(A, 10, 200)],
            Timestamp::from_unix(NOW),
            usdc(160_000_000),
            usdc(20_000_000),
        );
        assert!(got.is_empty());
    }

    #[test]
    fn a_bill_already_due_is_paid_in_full_not_twice() {
        let up = vec![bill(A, 100_000_000, 2)];
        let got = early_payments(
            &up,
            &up,
            &[terms(A, 10, 200)],
            Timestamp::from_unix(NOW),
            usdc(1_000_000_000),
            usdc(0),
        );
        assert!(
            got.is_empty(),
            "inside the normal lead it is already on the due list"
        );
    }

    #[test]
    fn terms_as_long_as_the_interval_pay_nothing_early() {
        // 30-day bill, 30-day window: paying this cycle early would make the
        // next one eligible at once, two cycles settled ahead of now, and
        // every later pass would fail with SettledAhead (audit 2026-10-15).
        let up = vec![bill(A, 100_000_000, 8)];
        let got = early_payments(
            &up,
            &[],
            &[terms(A, 30, 200)],
            Timestamp::from_unix(NOW),
            usdc(1_000_000_000),
            usdc(0),
        );
        assert!(got.is_empty());
    }

    #[test]
    fn absurd_terms_are_ignored() {
        let up = vec![bill(A, 100_000_000, 8)];
        let now = Timestamp::from_unix(NOW);
        assert!(
            early_payments(
                &up,
                &[],
                &[terms(A, 10, 0)],
                now,
                usdc(1_000_000_000),
                usdc(0)
            )
            .is_empty()
        );
        assert!(
            early_payments(
                &up,
                &[],
                &[terms(A, 10, MAX_DISCOUNT_BPS + 1)],
                now,
                usdc(1_000_000_000),
                usdc(0)
            )
            .is_empty()
        );
    }
}

/// Audit 2026-10-15 (docs/60 P-74): a failed read of the standing orders was
/// taken for "none", so a bill an order covers was paid by `pay` as well.
/// A local JSON-RPC stand-in plays a policy in Autopilot with one due bill
/// whose `scheduleCount()` read fails.
#[cfg(test)]
mod schedules_read_tests {
    use super::*;
    use ledger::{Counterparty, Currency, Direction};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn selector(sig: &str) -> String {
        alloy::hex::encode(&alloy::primitives::keccak256(sig)[..4])
    }

    fn word(v: u128) -> String {
        format!("0x{v:064x}")
    }

    /// Answers like the policy would, records every call's selector and
    /// every method asked.
    async fn node(seen: Arc<Mutex<Vec<String>>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let body = loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len: usize = text[..end]
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length")
                                        .then(|| v.trim().parse().ok())?
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break text[end + 4..end + 4 + len].to_string();
                            }
                        }
                    };
                    let req: Value = serde_json::from_str(&body).unwrap();
                    let method = req["method"].as_str().unwrap_or("").to_string();
                    let call = &req["params"][0];
                    let data = call["input"]
                        .as_str()
                        .or(call["data"].as_str())
                        .unwrap_or("");
                    let sel = data
                        .trim_start_matches("0x")
                        .get(..8)
                        .unwrap_or("")
                        .to_string();
                    seen.lock().unwrap().push(format!("{method} {sel}"));
                    let big = 1_000_000_000_000u128;
                    let result: Result<Value, &str> = match method.as_str() {
                        "eth_blockNumber" => Ok(json!("0x10")),
                        "eth_getLogs" => Ok(json!([])),
                        "eth_call" => match sel.as_str() {
                            s if s == selector("scheduleCount()") => Err("internal error"),
                            s if s == selector("mode()") => Ok(json!(word(2))),
                            s if s == selector("agentExpiry()") => Ok(json!(word(u64::MAX.into()))),
                            s if s == selector("balanceUsdc()")
                                || s == selector("perActionCap()")
                                || s == selector("dailyCap()") =>
                            {
                                Ok(json!(word(big)))
                            }
                            s if s == selector("attestedPayee(address)") => Ok(json!(word(1))),
                            // Everything else (spent, floor, delay, pending,
                            // executed, isBlacklisted): zero.
                            _ => Ok(json!(format!("0x{}", "0".repeat(64 * 8)))),
                        },
                        _ => Err("unsupported"),
                    };
                    let reply = match result {
                        Ok(r) => json!({"jsonrpc": "2.0", "id": req["id"], "result": r}),
                        Err(m) => json!({"jsonrpc": "2.0", "id": req["id"],
                            "error": {"code": -32000, "message": m}}),
                    };
                    let out = reply.to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                        out.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        url
    }

    #[tokio::test]
    async fn a_failed_standing_order_read_fails_the_pass_before_any_payment() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = node(seen.clone()).await;
        let client = ArcClient::with_signer(&url, &format!("0x{}", "11".repeat(32))).unwrap();
        let policy = Address::repeat_byte(0xcc);
        let mut config = Config::new(policy, policy, Address::repeat_byte(0xdd));
        let dir = std::env::temp_dir().join(format!("op-schedules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        config.audit_dir = Some(dir.clone());
        let mut op = Operator::new_unchecked(client, config, reasoner::RuleBased);

        // Six monthly payments of 20 USDC, the last 31 days ago: one is due.
        const DAY: i64 = 86_400;
        let now = Timestamp::from_unix(1_800_000_000);
        let payee = "0x1111111111111111111111111111111111111111";
        let history: Vec<Transfer> = (0..6)
            .map(|i| Transfer {
                tx_hash: format!("0xseed{i:02}"),
                counterparty: Counterparty::new(payee),
                amount: Money::minor(20_000_000, Currency::Usdc),
                at: Timestamp::from_unix(now.as_unix() - (31 + (5 - i) * 30) * DAY),
                direction: Direction::Out,
            })
            .collect();

        let r = op.run_once(now, Some(history), true).await;
        assert!(
            matches!(r, Err(OperatorError::Chain(_))),
            "the pass must fail on the read: {r:?}"
        );
        let seen = seen.lock().unwrap();
        let pay = selector("pay(address,uint256,bytes32)");
        assert!(
            !seen
                .iter()
                .any(|s| s.ends_with(&pay) || s.starts_with("eth_sendRawTransaction")),
            "nothing paid: {seen:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod decide_pass_tests {
    //! docs/112: the pass acts on the decider's choice, inside what code allows.
    use super::*;
    use decide::{Action, Basis, Decider};
    use ledger::{Counterparty, Currency, Direction};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn selector(sig: &str) -> String {
        alloy::hex::encode(&alloy::primitives::keccak256(sig)[..4])
    }

    fn word(v: u128) -> String {
        format!("0x{v:064x}")
    }

    /// A policy node: `mode` as given, no standing orders, every payee
    /// approved, big caps; every call's method and selector recorded. Sending
    /// a transaction fails (so a pass that tried to pay would fail loudly).
    /// `order_paid`: one ended standing order to the bill's payee whose payment
    /// for that due date is spent (`PaidDue`); `None`, no orders.
    async fn node(mode: u128, seen: Arc<Mutex<Vec<String>>>, order_paid: Option<u64>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let body = loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len: usize = text[..end]
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length")
                                        .then(|| v.trim().parse().ok())?
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break text[end + 4..end + 4 + len].to_string();
                            }
                        }
                    };
                    let req: Value = serde_json::from_str(&body).unwrap();
                    let method = req["method"].as_str().unwrap_or("").to_string();
                    let call = &req["params"][0];
                    let data = call["input"]
                        .as_str()
                        .or(call["data"].as_str())
                        .unwrap_or("");
                    let sel = data
                        .trim_start_matches("0x")
                        .get(..8)
                        .unwrap_or("")
                        .to_string();
                    seen.lock().unwrap().push(format!("{method} {sel}"));
                    let big = 1_000_000_000_000u128;
                    let result: Result<Value, &str> = match method.as_str() {
                        "eth_blockNumber" => Ok(json!("0x10")),
                        "eth_getLogs" => Ok(json!([])),
                        "eth_call" => match sel.as_str() {
                            s if s == selector("mode()") => Ok(json!(word(mode))),
                            s if s == selector("agentExpiry()") => Ok(json!(word(u64::MAX.into()))),
                            s if s == selector("balanceUsdc()")
                                || s == selector("perActionCap()")
                                || s == selector("dailyCap()") =>
                            {
                                Ok(json!(word(big)))
                            }
                            s if s == selector("attestedPayee(address)") => Ok(json!(word(1))),
                            s if order_paid.is_some() && s == selector("scheduleCount()") => {
                                Ok(json!(word(1)))
                            }
                            s if order_paid.is_some() && s == selector("schedules(uint256)") => {
                                let due = order_paid.unwrap_or(0);
                                Ok(json!(format!(
                                    "0x{:0>64}{:064x}{:064x}{:064x}{:064x}",
                                    "1111111111111111111111111111111111111111",
                                    20_000_000u128,
                                    30 * 86_400u64,
                                    due + 30 * 86_400,
                                    0u8
                                )))
                            }
                            // The order's key for a due date: that date, as a word.
                            s if s == selector("scheduleActionHash(uint256,uint64)") => {
                                let due = data.trim_start_matches("0x").get(72..136).unwrap_or("0");
                                Ok(json!(format!("0x{due}")))
                            }
                            s if s == selector("executed(bytes32)") => {
                                let h = data.trim_start_matches("0x").get(8..72).unwrap_or("");
                                let spent = order_paid.is_some_and(|d| h == format!("{d:064x}"));
                                Ok(json!(word(u128::from(spent))))
                            }
                            _ => Ok(json!(format!("0x{}", "0".repeat(64 * 8)))),
                        },
                        _ => Err("unsupported"),
                    };
                    let reply = match result {
                        Ok(r) => json!({"jsonrpc": "2.0", "id": req["id"], "result": r}),
                        Err(m) => {
                            json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32000, "message": m}})
                        }
                    };
                    let out = reply.to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                        out.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        url
    }

    /// A decider that answers `Some(action)` as a model would, or fails.
    struct Fake(Option<Action>);

    impl Decider for Fake {
        async fn decide(&self, _case: &decide::Case) -> Result<decide::Decision, ReasonerError> {
            match self.0 {
                Some(action) => Ok(decide::Decision {
                    action,
                    basis: Basis::AmountDeviates,
                    why: format!("The fake model chose {}.", action.as_str()),
                    by: ActorKind::Model,
                    model: Some("fake-model".into()),
                }),
                None => Err(ReasonerError::Transient("the fake model is down".into())),
            }
        }

        fn name(&self) -> &str {
            "fake-model"
        }

        fn kind(&self) -> ActorKind {
            ActorKind::Model
        }
    }

    /// One pass over six monthly payments of 20 USDC, the last 31 days ago
    /// (one cycle due, a day overdue), with `decider`, against a policy in `mode`.
    async fn pass<D: Decider + Sync>(
        mode: u128,
        decider: &D,
        tag: &str,
    ) -> (RunOutcome, Vec<String>, Vec<String>) {
        pass_with(mode, decider, tag, None).await
    }

    async fn pass_with<D: Decider + Sync>(
        mode: u128,
        decider: &D,
        tag: &str,
        order_paid: Option<u64>,
    ) -> (RunOutcome, Vec<String>, Vec<String>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = node(mode, seen.clone(), order_paid).await;
        let client = ArcClient::with_signer(&url, &format!("0x{}", "11".repeat(32))).unwrap();
        let policy = Address::repeat_byte(0xcc);
        let mut config = Config::new(policy, policy, Address::repeat_byte(0xdd));
        let dir = std::env::temp_dir().join(format!("op-decide-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        config.audit_dir = Some(dir.clone());
        let mut op = Operator::new_unchecked(client, config, reasoner::RuleBased);
        const DAY: i64 = 86_400;
        let now = Timestamp::from_unix(1_800_000_000);
        let history: Vec<Transfer> = (0..6)
            .map(|i| Transfer {
                tx_hash: format!("0xseed{i:02}"),
                counterparty: Counterparty::new("0x1111111111111111111111111111111111111111"),
                amount: Money::minor(20_000_000, Currency::Usdc),
                at: Timestamp::from_unix(now.as_unix() - (31 + (5 - i) * 30) * DAY),
                direction: Direction::Out,
            })
            .collect();
        let outcome = op
            .run_once_deciding(now, Some(history), true, decider)
            .await
            .unwrap();
        let log = replay(op.log());
        let _ = std::fs::remove_dir_all(&dir);
        let seen = seen.lock().unwrap().clone();
        (outcome, log, seen)
    }

    fn paid(seen: &[String]) -> bool {
        let pay = selector("pay(address,uint256,bytes32)");
        seen.iter().any(|s| s.ends_with(&pay))
    }

    /// docs/117 §2, hash first: a pass whose agent key is known, with an
    /// invoice its payee signed with a bound ML-DSA-65 key (received, decided
    /// and recorded the way `level1::run` does it), anchors leaves in which
    /// neither public key nor either signature appears, raw or in hex — nor
    /// does anything written to the audit directory.
    #[tokio::test]
    async fn no_public_key_in_any_anchored_leaf() {
        use crate::invoice::{self, Checks, Invoice, KeyBinding};
        use crate::sig::{self, Alg};
        use alloy::signers::SignerSync;
        use alloy::signers::local::PrivateKeySigner;
        use ml_dsa::{KeyExport, Keypair, SignatureEncoding, Signer};
        let seen = Arc::new(Mutex::new(Vec::new()));
        let url = node(2, seen, None).await;
        let agent_secret = B256::repeat_byte(0x4b);
        let agent = PrivateKeySigner::from_bytes(&agent_secret).unwrap();
        let client = ArcClient::with_signer(&url, &format!("{agent_secret:#x}")).unwrap();
        let policy = Address::repeat_byte(0xcc);
        let dir = std::env::temp_dir().join(format!("op-no-key-leaf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(level1::tenants_dir(&dir)).unwrap();
        let t: level1::Tenant = serde_json::from_value(json!({
            "chain": "arbitrum-sepolia", "owner": "0x0000000000000000000000000000000000000001",
            "policy": format!("{policy:#x}"), "agent": format!("{:#x}", agent.address()),
            "operator": "0x0000000000000000000000000000000000000004", "watched": "0x0000000000000000000000000000000000000005",
            "recovery": "0x0000000000000000000000000000000000000000", "per_action": "1", "daily": "1", "floor": "0",
            "delay_secs": 60, "mode": 2, "created_block": 0, "history_from_block": 0, "created": 0
        }))
        .unwrap();
        // The payee: an address key that binds, and the ML-DSA-65 key that bills.
        const CHAIN: u64 = 421_614;
        let now = Timestamp::from_unix(1_800_000_000);
        let at = now.as_unix() as u64;
        let payee = PrivateKeySigner::random();
        let mlkey = ml_dsa::SigningKey::<ml_dsa::MlDsa65>::from_seed(&[42u8; 32].into());
        let ml_pub = mlkey.verifying_key().to_bytes();
        let b = KeyBinding {
            payee: payee.address(),
            policy,
            alg: Alg::MlDsa65,
            pk_sha256: sig::key_hash(&ml_pub),
            bound_at: at,
        };
        let bsig = payee
            .sign_hash_sync(&invoice::binding_digest(CHAIN, &b))
            .unwrap()
            .as_bytes();
        invoice::bind(&dir, &t, CHAIN, &invoice::binding_json(&b, &bsig), at).unwrap();
        let inv = Invoice {
            payee: payee.address(),
            policy,
            amount: 20_000_000,
            due_at: at + 5 * 86_400,
            every_days: 30,
            discount_bps: 0,
            early_days: 0,
            reference: alloy::primitives::keccak256("INV-PQ-1"),
            issued_at: at - 60,
            issuer: None,
            alg: Some(Alg::MlDsa65),
            key_hash: None,
        };
        let ml_sig = mlkey
            .sign(invoice::digest(CHAIN, &inv).as_slice())
            .to_bytes();
        let mut body = invoice::to_json(&inv, &ml_sig);
        body["publicKey"] = json!(alloy::hex::encode_prefixed(ml_pub));
        let checked = invoice::verify(CHAIN, policy, &body, at).unwrap();
        invoice::floor(
            &checked,
            invoice::binding_for(&dir, &t.policy, payee.address()).as_ref(),
        )
        .unwrap();
        let line =
            invoice::record_received(&dir, &t.policy, &checked, &body, &Checks::default(), at)
                .unwrap();
        assert_eq!(
            line["publicKey"], body["publicKey"],
            "the invoice store keeps the key"
        );
        // The pass, as `level1::run` drives it: the invoice decided first, its
        // event in the same log, then the pass over the history.
        let history: Vec<Transfer> = (0..6)
            .map(|i| Transfer {
                tx_hash: format!("0xseed{i:02}"),
                counterparty: Counterparty::new("0x1111111111111111111111111111111111111111"),
                amount: Money::minor(20_000_000, Currency::Usdc),
                at: Timestamp::from_unix(now.as_unix() - (31 + (5 - i) * 30) * 86_400),
                direction: Direction::Out,
            })
            .collect();
        let decider = Fake(Some(Action::Ask));
        let (events, records) = level1::decide_invoices(
            &dir,
            &t,
            &client,
            ledger::DetectionConfig::default(),
            &history,
            &decider,
            now,
        )
        .await;
        assert_eq!((events.len(), records.len()), (1, 1), "{records:?}");
        let mut config = Config::new(policy, policy, Address::repeat_byte(0xdd));
        config.audit_dir = Some(dir.join("log"));
        let mut op = Operator::new_unchecked(client, config, reasoner::RuleBased);
        for (when, body) in events {
            op.log_mut().append(Timestamp::from_unix(when as i64), body);
        }
        op.run_once_deciding(now, Some(history), true, &decider)
            .await
            .unwrap();
        // Every key and signature, raw and hex, whole and its first 32 bytes.
        let agent_pub = agent.public_key();
        let point = agent.credential().verifying_key().to_encoded_point(true);
        let raws: Vec<Vec<u8>> = vec![
            agent_pub.to_vec(),
            point.as_bytes().to_vec(),
            agent_secret.to_vec(),
            ml_pub.to_vec(),
            ml_sig.to_vec(),
            [42u8; 32].to_vec(),
        ];
        let mut needles: Vec<Vec<u8>> = Vec::new();
        for r in &raws {
            for part in [&r[..], &r[..32]] {
                needles.push(part.to_vec());
                let h = alloy::hex::encode(part);
                needles.push(h.clone().into_bytes());
                needles.push(h.to_uppercase().into_bytes());
            }
        }
        let has = |hay: &[u8], n: &[u8]| hay.windows(n.len()).any(|w| w == n);
        let events = op.log().events();
        assert!(
            events.iter().any(|e| e.body.note().contains(&format!(
                "Invoice {:#x}",
                invoice::id(inv.payee, inv.reference)
            ))),
            "the ML-DSA invoice is in the record"
        );
        assert!(events.len() >= 3, "a whole pass: {}", events.len());
        for e in events {
            let leaf = e.canonical();
            for n in &needles {
                assert!(
                    !has(&leaf, n),
                    "event {} ({}) carries key material",
                    e.seq,
                    e.body.kind()
                );
            }
        }
        // What the audit directory holds for anyone to download: the same.
        let mut files = 0;
        for f in std::fs::read_dir(dir.join("log")).unwrap().flatten() {
            let text = std::fs::read(f.path()).unwrap();
            files += 1;
            for n in &needles {
                assert!(
                    !has(&text, n),
                    "{} carries key material",
                    f.path().display()
                );
            }
        }
        assert!(files >= 1, "the pass wrote its record");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_model_ask_is_one_needs_you_row_with_its_why_and_nothing_is_paid() {
        let (o, log, seen) = pass(2, &Fake(Some(Action::Ask)), "ask").await;
        assert_eq!(
            (
                o.pending_signature.len(),
                o.escalated.len(),
                o.settled.len()
            ),
            (1, 1, 0)
        );
        assert!(!paid(&seen), "nothing paid: {seen:?}");
        let (case, d, note) = &o.decisions[0];
        assert_eq!(case.allowed, vec![Action::Pay, Action::Ask, Action::Refuse]);
        assert_eq!(
            (d.action, d.by, d.model.as_deref(), note),
            (Action::Ask, ActorKind::Model, Some("fake-model"), &None)
        );
        assert_eq!(o.decider, "fake-model");
        let line = log
            .iter()
            .find(|l| l.contains("Decided by model fake-model: ask"))
            .expect("in the record");
        assert!(line.contains("The fake model chose ask."), "{line}");
        assert!(
            log.iter()
                .any(|l| l.contains("ESCALATION") && l.contains("I chose to ask")),
            "{log:?}"
        );
    }

    /// docs/60 P-218: a model's refusal of a bill is the owner's question, with
    /// the model's own why — until P-218 it landed in refused and the bill went
    /// unpaid with no one asked. Named as a look-alike, the refusal stands.
    #[tokio::test]
    async fn a_model_refusal_of_a_bill_asks_the_owner_unless_a_look_alike() {
        let (o, log, seen) = pass(2, &Fake(Some(Action::Refuse)), "refuse").await;
        assert_eq!(
            (o.blocked.len(), o.pending_signature.len(), o.settled.len()),
            (0, 1, 0)
        );
        assert!(!paid(&seen));
        let (_, d, note) = &o.decisions[0];
        assert_eq!((d.action, d.by), (Action::Ask, ActorKind::Model));
        assert_eq!(d.why, "The fake model chose refuse.", "the model's own why");
        assert!(
            note.as_deref()
                .is_some_and(|n| n.contains("only a look-alike")),
            "{note:?}"
        );
        assert!(
            log.iter()
                .any(|l| l.contains("Decided by model fake-model: ask")),
            "{log:?}"
        );

        // P-223: "look-alike" is the case's fact, not the model's word. This
        // payee is approved, so code found no look-alike: the owner is asked.
        let (o, _, seen) = pass(2, &LookAlike, "refuse-lookalike").await;
        assert_eq!(
            (o.blocked.len(), o.pending_signature.len(), o.settled.len()),
            (0, 1, 0)
        );
        assert!(!paid(&seen));
        // Where code found one, the model's look-alike refusal stands.
        let (mut case, d, _) = o.decisions[0].clone();
        let refused = decide::Decision {
            action: Action::Refuse,
            basis: Basis::LookAlike,
            ..d
        };
        case.lookalike_of = Some("0x111111…111111".into());
        let (kept, note) = model_refusal_asks(&case, refused, None, "fake-model");
        assert_eq!((kept.action, note), (Action::Refuse, None));
    }

    /// docs/60 P-219: a cycle an ended standing order paid (`PaidDue` under the
    /// order's key for that due date) is paid: the pass asks nothing for it and
    /// steps to the next cycle. Without the order's payment the cycle is asked.
    #[tokio::test]
    async fn a_cycle_a_standing_order_paid_is_not_asked_again() {
        const DAY: i64 = 86_400;
        // The bill's cycle: six monthly payments, the last 31 days ago.
        let due = (1_800_000_000 - DAY) as u64;
        let (o, _, seen) = pass_with(1, &Fake(Some(Action::Ask)), "order-paid", Some(due)).await;
        assert_eq!(
            (o.pending_signature.len(), o.settled.len()),
            (0, 0),
            "{:?}",
            o.pending_signature
        );
        assert!(!paid(&seen));
        let next = o
            .obligations
            .iter()
            .find(|b| b.counterparty.as_str().starts_with("0x1111"))
            .unwrap();
        assert_eq!(
            next.next_due.as_unix(),
            due as i64 + 30 * DAY,
            "stepped past the paid cycle"
        );
        let (o, _, _) = pass_with(1, &Fake(Some(Action::Ask)), "order-unpaid", Some(due + 1)).await;
        assert_eq!(
            o.pending_signature.len(),
            1,
            "another date's payment does not answer this cycle"
        );
    }

    /// A model that refuses, naming a look-alike.
    struct LookAlike;

    impl Decider for LookAlike {
        async fn decide(&self, _case: &decide::Case) -> Result<decide::Decision, ReasonerError> {
            Ok(decide::Decision {
                action: Action::Refuse,
                basis: Basis::LookAlike,
                why: "The payee imitates one the owner pays.".into(),
                by: ActorKind::Model,
                model: Some("fake-model".into()),
            })
        }

        fn name(&self) -> &str {
            "fake-model"
        }

        fn kind(&self) -> ActorKind {
            ActorKind::Model
        }
    }

    #[tokio::test]
    async fn a_failing_model_gives_the_rules_decision_recorded_as_a_fallback() {
        // Propose: the rule hands the bill to the owner, as before docs/112.
        let (o, log, _) = pass(1, &Fake(None), "fallback").await;
        assert_eq!(o.pending_signature.len(), 1);
        let (_, d, note) = &o.decisions[0];
        assert_eq!(
            (d.action, d.by, d.model.as_deref()),
            (Action::Ask, ActorKind::Rule, None)
        );
        assert!(
            note.as_deref()
                .unwrap()
                .contains("fake-model failed [reasoner-transient]"),
            "{note:?}"
        );
        assert!(
            log.iter()
                .any(|l| l.contains("Decided by rule: ask") && l.contains("fake-model failed")),
            "{log:?}"
        );
        // An action code does not allow is the rule's decision too: in Propose
        // the contract refuses the agent's payment, so `pay` is not offered.
        let (o, _, seen) = pass(1, &Fake(Some(Action::Pay)), "illegal").await;
        assert!(!paid(&seen));
        let (case, d, note) = &o.decisions[0];
        assert!(!case.allowed.contains(&Action::Pay));
        assert_eq!((d.action, d.by), (Action::Ask, ActorKind::Rule));
        assert!(
            note.as_deref().unwrap().contains("does not allow"),
            "{note:?}"
        );
    }

    #[tokio::test]
    async fn the_rule_decider_keeps_todays_pass() {
        // Propose, rule: one Needs-you row; the record names the rule.
        let (o, log, _) = pass(1, &decide::RuleBased, "rule").await;
        assert_eq!(
            (o.pending_signature.len(), o.decider.as_str()),
            (1, "rule-based")
        );
        assert_eq!(
            (o.decisions[0].1.action, o.decisions[0].1.by),
            (Action::Ask, ActorKind::Rule)
        );
        assert!(
            log.iter()
                .any(|l| l.contains("Decided by rule: ask (usual)")),
            "{log:?}"
        );
    }
}
