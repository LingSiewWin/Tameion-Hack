//! The verifier: given a proposed action and the world as observed, decide
//! whether it may execute.
//!
//! Three outcomes, not two. A single allow-or-block gate is the design the
//! compliance world already knows is wrong -- the risk-based approach asks
//! for limits scaled to risk, and collapsing that into a boolean is what
//! produces both rubber-stamping and needless refusals. So:
//!
//! - [`Decision::Allow`]  every check passed; execute without a human
//! - [`Decision::Review`] nothing is *wrong*, but something wants judgment
//! - [`Decision::Block`]  a check failed; no human can wave this through
//!
//! The split between Review and Block is deliberate and is the safety
//! argument of the whole system. Block is for things a human should never be
//! asked to approve, because asking is how rubber-stamping starts: a
//! duplicate payment, an expired action, an agent misreporting its own
//! limits. Review is for genuine judgment: an unfamiliar counterparty, an
//! amount above the agent's cap, evidence that rests on a guess.
//!
//! The model's rationale is never consulted. It is carried into the record
//! and ignored by every check. Circle's own escrow sample releases funds
//! when a model returns `confidence === "HIGH"`; that is the mistake this
//! module exists to not make.

use crate::account::AccountId;
use crate::action::{Action, Authority, Delta, EvidenceKind, Precondition, Provenance};
use crate::canon::{ActionHash, action_hash};
use crate::money::Money;
use crate::obligations::Confidence;
use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

/// What the verifier decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Decision {
    Allow,
    Review,
    Block,
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Decision::Allow => "ALLOW",
            Decision::Review => "REVIEW",
            Decision::Block => "BLOCK",
        })
    }
}

/// The result of one named check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    /// Needs a human, but is not an error.
    Review,
    /// Wrong. Not a judgment call.
    Fail,
}

impl Outcome {
    /// Stable machine-readable form. `Display` on [`Check`] is for humans and
    /// its layout is free to change; this is what a wire format or a log
    /// consumer should branch on.
    pub const fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Review => "review",
            Outcome::Fail => "fail",
        }
    }
}

/// One check, its outcome, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub outcome: Outcome,
    pub detail: String,
}

impl Check {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Check {
            name,
            outcome: Outcome::Pass,
            detail: detail.into(),
        }
    }
    fn review(name: &'static str, detail: impl Into<String>) -> Self {
        Check {
            name,
            outcome: Outcome::Review,
            detail: detail.into(),
        }
    }
    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Check {
            name,
            outcome: Outcome::Fail,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mark = match self.outcome {
            Outcome::Pass => "pass",
            Outcome::Review => "review",
            Outcome::Fail => "FAIL",
        };
        write!(f, "[{mark:>6}] {:<24} {}", self.name, self.detail)
    }
}

/// The verifier's output. Carries the hash so that whatever executes can
/// confirm it is executing the thing that was verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub decision: Decision,
    pub action_hash: ActionHash,
    pub checks: Vec<Check>,
}

impl Verdict {
    /// Checks that did not pass, which is what a reviewer should read first.
    pub fn problems(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|c| c.outcome != Outcome::Pass)
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}  {}", self.decision, self.action_hash)?;
        for c in &self.checks {
            writeln!(f, "  {c}")?;
        }
        Ok(())
    }
}

/// A document the principal's own systems hold, read independently of the
/// agent.
///
/// The agent cites a reference; this is what the reference actually resolves
/// to. The separation is the whole point. An `Evidence` entry carries a
/// reference and a provenance and nothing else, deliberately: if the amount
/// travelled with the citation, the agent would be supplying both the claim
/// and the thing the claim is checked against, which is the authority hole
/// this design exists to close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentRecord {
    /// What the document says the amount is.
    pub amount: Money,
    /// Who the document names as the counterparty.
    pub counterparty: String,
}

/// Whether a counterparty's payment details have moved, and if so whether
/// the principal has since vouched for the new ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayeeStatus {
    /// Never paid before, so there is nothing to have changed.
    Unseen,
    /// Same details as last time.
    Unchanged,
    /// Details moved, and the principal has explicitly attested the new
    /// ones out of band.
    ChangedAndAttested,
    /// Details moved and nobody has vouched for the new ones.
    ChangedUnattested,
}

/// The world as observed, independently of what the action claims about it.
///
/// Every field here is read from outside the agent: the balance from chain
/// state, the settled keys from the journal, the payee fingerprints from the
/// vendor master. The agent gets no say in any of it, which is the point --
/// an action that misdescribes the world is detected by comparison, not by
/// trusting the description.
#[derive(Debug, Clone)]
pub struct World {
    pub now: u64,
    pub treasury: AccountId,
    pub balance: Money,
    /// The real limits, from the same source the on-chain contract uses.
    pub authority: Authority,
    /// Idempotency keys that have already settled.
    pub settled: BTreeSet<String>,
    /// Counterparty id to a fingerprint of its current payment details.
    ///
    /// Odoo logs every change to a vendor's account number in the chatter,
    /// because substituting the payee is how an invoice-fraud payment gets
    /// made against a real invoice. Presence here means "paid before";
    /// a changed fingerprint means the details moved since.
    pub known_payees: BTreeMap<String, String>,
    /// Fingerprint the action is about to pay to.
    pub proposed_payee: BTreeMap<String, String>,
    /// Preconditions the principal requires on every action, whatever the
    /// action itself proposes.
    ///
    /// This field exists because `Action::preconditions` is written by the
    /// agent, and anything the agent writes it can also decline to write.
    /// An action with no preconditions and no evidence used to verify clean
    /// -- not because it passed the checks, but because it supplied none.
    /// These are applied on top and cannot be subtracted from, so the
    /// agent's own preconditions are additive only.
    pub required_preconditions: Vec<Precondition>,
    /// Payee fingerprints the principal has explicitly vouched for after a
    /// change, keyed by counterparty.
    ///
    /// A changed fingerprint is refused outright rather than escalated, so
    /// there has to be a way through for the legitimate case where a vendor
    /// really did move banks. That way through is here, and it is out of
    /// band: the principal attests the new details by some channel the
    /// agent has no reach into, and only then does a payment to them
    /// verify. An attacker who can rewrite this map has already won by
    /// other means.
    pub attested_payees: BTreeMap<String, String>,
    /// How strong the inference behind each proposed action is, keyed by the
    /// action's idempotency key (`scope:key`).
    ///
    /// Lives here rather than on the [`Action`] for the usual reason: a floor
    /// the agent can satisfy by asserting a number is not a floor. The values
    /// come from [`crate::obligations::detect`], which is deterministic code
    /// the reasoner cannot reach -- a model can reorder the queue and cannot
    /// make a 0.62 guess look like a 0.95 one.
    ///
    /// An action whose key is absent has no inference behind it at all, which
    /// is not the same as a weak one: a hand-entered invoice is not a guess.
    /// [`Precondition::ConfidenceAtLeast`] therefore passes on absence and
    /// judges only what is present.
    pub inferred_confidence: BTreeMap<String, Confidence>,
    /// Purchase orders, keyed by reference, as the principal's systems hold
    /// them.
    pub purchase_orders: BTreeMap<String, DocumentRecord>,
    /// Goods-received notes, keyed by reference.
    pub receipts: BTreeMap<String, DocumentRecord>,
    /// Addresses the principal has attested on chain, which are the only
    /// ones `BudgetPolicy.pay` will send to. `None` when there is no on-chain
    /// list to consult, in which case the check does not apply.
    ///
    /// Read from the contract, not from configuration: an agent proposing a
    /// payee the contract will refuse is told so here, as a question for the
    /// principal, rather than finding out from a reverted transaction.
    pub onchain_attested: Option<BTreeSet<String>>,
    /// How many later cycles of an inferred obligation have also fallen due,
    /// keyed by the action's idempotency key (`scope:key`). Absent means none.
    ///
    /// Computed by the operator from the owner's evidence and the chain's
    /// settlement record, not by the reasoner: a bill whose owner stopped
    /// paying it several cycles ago may have ended, and that is the owner's
    /// call, not a backlog for the agent to clear.
    pub cycles_behind: BTreeMap<String, u32>,
    /// The smallest and largest amount actually paid to this counterparty in
    /// the evidence, keyed by the action's idempotency key (`scope:key`).
    /// Absent means no observed range, which is not the same as "any amount
    /// is fine": the amount cap still applies; only the outlier check is
    /// silent. Modern Treasury's expected payments carry amount bounds the
    /// operator declares (`docs/50`); here the bounds are what the chain
    /// shows was paid, out of the reasoner's reach.
    pub observed_amounts: BTreeMap<String, (Money, Money)>,
    /// Addresses the stablecoin's issuer refuses to move money to or from
    /// (Circle's USDC blocklist, `isBlacklisted`), read from the token
    /// contract for the payees this run might pay and for the treasury
    /// itself. Empty when the token has no such list to ask.
    ///
    /// A payment to one of these would revert at the token, and a payment
    /// the issuer has frozen is not one the owner can approve either -- so
    /// this is a refusal, not a question (PLAN 5.47, docs/62).
    pub issuer_blocked: BTreeSet<String>,
}

impl World {
    /// The preconditions actually evaluated: what the principal requires,
    /// plus anything the agent volunteered, with duplicates collapsed.
    pub fn effective_preconditions(&self, action: &Action) -> Vec<Precondition> {
        let mut all = self.required_preconditions.clone();
        for p in &action.preconditions {
            if !all.contains(p) {
                all.push(p.clone());
            }
        }
        all
    }
}

impl World {
    /// Where a counterparty's payment details stand relative to the last
    /// time they were paid.
    pub fn payee_status(&self, counterparty: &str) -> PayeeStatus {
        let (known, proposed) = match (
            self.known_payees.get(counterparty),
            self.proposed_payee.get(counterparty),
        ) {
            (Some(k), Some(p)) => (k, p),
            _ => return PayeeStatus::Unseen,
        };
        if known == proposed {
            return PayeeStatus::Unchanged;
        }
        match self.attested_payees.get(counterparty) {
            Some(attested) if attested == proposed => PayeeStatus::ChangedAndAttested,
            _ => PayeeStatus::ChangedUnattested,
        }
    }

    /// Retained for callers that only need the boolean.
    pub fn payee_changed(&self, counterparty: &str) -> bool {
        matches!(
            self.payee_status(counterparty),
            PayeeStatus::ChangedAndAttested | PayeeStatus::ChangedUnattested
        )
    }
}

/// Verify a proposed action against the observed world.
///
/// Never panics, never executes anything, and never reads
/// `action.intent.rationale`.
pub fn verify(action: &Action, world: &World) -> Verdict {
    let mut checks = Vec::new();
    let hash = action_hash(action);

    checks.push(check_validity(action, world));
    checks.push(check_shape(action));
    checks.push(check_currency(action, world));
    checks.push(check_authority_sane(world));
    checks.push(check_solvency(action, world));
    checks.push(check_authority_claim(action, world));
    checks.push(check_per_action_cap(action, world));
    checks.push(check_daily_cap(action, world));
    checks.push(check_idempotency(action, world));
    checks.push(check_effects(action, world));
    checks.push(check_payee(action, world));
    checks.push(check_payee_lookalike(action, world));
    checks.push(check_payee_attested(action, world));
    checks.push(check_issuer_blocklist(action, world));
    checks.push(check_cycles_behind(action, world));
    checks.push(check_amount_range(action, world));
    for pre in world.effective_preconditions(action) {
        checks.push(check_precondition(&pre, action, world));
    }

    let decision = if checks.iter().any(|c| c.outcome == Outcome::Fail) {
        Decision::Block
    } else if checks.iter().any(|c| c.outcome == Outcome::Review) {
        Decision::Review
    } else {
        Decision::Allow
    };

    Verdict {
        decision,
        action_hash: hash,
        checks,
    }
}

fn check_validity(a: &Action, w: &World) -> Check {
    if w.now < a.validity.not_before {
        return Check::fail(
            "validity",
            format!("not valid until {}", a.validity.not_before),
        );
    }
    if w.now >= a.validity.expires_at {
        // Verified at noon, executed at six: the checks were of a world that
        // has since moved. Not a judgment call, so not a Review.
        return Check::fail(
            "validity",
            format!("expired at {} (now {})", a.validity.expires_at, w.now),
        );
    }
    Check::pass("validity", format!("valid until {}", a.validity.expires_at))
}

fn check_shape(a: &Action) -> Check {
    if a.amount.is_negative() {
        return Check::fail("shape", format!("negative amount {}", a.amount));
    }
    if a.amount.is_zero() {
        return Check::fail("shape", "zero amount");
    }
    if a.expected_effects.is_empty() {
        return Check::fail("shape", "no expected effects stated");
    }
    if a.idempotency.key.is_empty() {
        return Check::fail("shape", "empty idempotency key");
    }
    Check::pass("shape", format!("{} {}", a.intent.verb, a.amount))
}

fn check_currency(a: &Action, w: &World) -> Check {
    if a.amount.currency() != w.balance.currency() {
        return Check::fail(
            "currency",
            format!(
                "action in {}, treasury in {}",
                a.amount.currency(),
                w.balance.currency()
            ),
        );
    }
    if a.principal.treasury != w.treasury {
        return Check::fail(
            "currency",
            format!(
                "action draws on {}, world is {}",
                a.principal.treasury, w.treasury
            ),
        );
    }
    Check::pass("currency", a.amount.currency().to_string())
}

/// The agent states the limits it believes it is under. If that statement
/// disagrees with the observed limits, the agent is either stale or lying,
/// and neither is something a human should be asked to approve.
/// The treasury must be able to cover the action.
///
/// Intrinsic, not a precondition. `BalanceAtLeast` is written by the agent
/// and states an absolute floor unrelated to the amount, so an action could
/// carry `BalanceAtLeast(0.000001)`, spend the entire treasury and more, and
/// verify clean -- which it did, until this check existed.
///
/// Blocked rather than escalated: money that is not there cannot be sent,
/// and no human approval changes that. On chain the transfer would simply
/// revert, so allowing it only moves the failure somewhere with less
/// context.
fn check_solvency(a: &Action, w: &World) -> Check {
    if w.balance.currency() != a.amount.currency() {
        // check_currency reports this; avoid a second confusing message.
        return Check::pass("solvency", "deferred to currency check");
    }
    if w.balance.to_minor() < a.amount.to_minor() {
        return Check::fail(
            "solvency",
            format!("balance {} cannot cover {}", w.balance, a.amount),
        );
    }
    match w.balance.checked_sub(a.amount) {
        Ok(after) => Check::pass("solvency", format!("{} leaves {after}", w.balance)),
        Err(e) => Check::fail("solvency", format!("arithmetic: {e}")),
    }
}

/// Limits must be non-negative and ordered. A negative `spent_today` would
/// let the daily cap absorb arbitrary spending; a per-action cap above the
/// daily cap is a policy that cannot mean what it says.
fn check_authority_sane(w: &World) -> Check {
    let a = &w.authority;
    if a.per_action_cap.is_negative() || a.daily_cap.is_negative() || a.spent_today.is_negative() {
        return Check::fail(
            "authority-sane",
            format!(
                "negative limit: cap {} daily {} spent {}",
                a.per_action_cap, a.daily_cap, a.spent_today
            ),
        );
    }
    if a.per_action_cap.to_minor() > a.daily_cap.to_minor() {
        return Check::fail(
            "authority-sane",
            format!(
                "per-action cap {} exceeds daily cap {}",
                a.per_action_cap, a.daily_cap
            ),
        );
    }
    Check::pass(
        "authority-sane",
        format!("cap {} daily {}", a.per_action_cap, a.daily_cap),
    )
}

fn check_authority_claim(a: &Action, w: &World) -> Check {
    if a.authority != w.authority {
        return Check::fail(
            "authority-claim",
            format!(
                "agent claims cap {} / daily {} / spent {}, observed {} / {} / {}",
                a.authority.per_action_cap,
                a.authority.daily_cap,
                a.authority.spent_today,
                w.authority.per_action_cap,
                w.authority.daily_cap,
                w.authority.spent_today
            ),
        );
    }
    Check::pass("authority-claim", "matches observed limits")
}

fn check_per_action_cap(a: &Action, w: &World) -> Check {
    if a.amount.to_minor() > w.authority.per_action_cap.to_minor() {
        // A human decision, not an error: this is the escalation path.
        return Check::review(
            "per-action-cap",
            format!("{} over cap {}", a.amount, w.authority.per_action_cap),
        );
    }
    Check::pass(
        "per-action-cap",
        format!("{} within {}", a.amount, w.authority.per_action_cap),
    )
}

fn check_daily_cap(a: &Action, w: &World) -> Check {
    match w.authority.spent_today.checked_add(a.amount) {
        Err(e) => Check::fail("daily-cap", format!("arithmetic: {e}")),
        Ok(total) => {
            if total.to_minor() > w.authority.daily_cap.to_minor() {
                Check::review(
                    "daily-cap",
                    format!("would reach {total}, daily cap {}", w.authority.daily_cap),
                )
            } else {
                Check::pass("daily-cap", format!("{total} of {}", w.authority.daily_cap))
            }
        }
    }
}

fn check_idempotency(a: &Action, w: &World) -> Check {
    let scoped = format!("{}:{}", a.idempotency.scope, a.idempotency.key);
    if w.settled.contains(&scoped) {
        // A retry after a timeout is how one payment becomes two. Never a
        // question for a human.
        return Check::fail("idempotency", format!("{scoped} already settled"));
    }
    Check::pass("idempotency", scoped)
}

/// The stated effects must account for exactly the amount leaving the
/// treasury. Without this an action could claim to move 20 and describe
/// effects worth 200, and the receipt would be checked against the lie.
fn check_effects(a: &Action, w: &World) -> Check {
    // More than one effect on the same account makes the action ambiguous:
    // a receipt could be reconciled against either reading, and whoever
    // reconciles picks. Refuse rather than choose. An earlier version of
    // this function kept only the last match, and an action stating
    // [-500, -20] on the treasury verified against the -20.
    let mut seen: Vec<&AccountId> = Vec::with_capacity(a.expected_effects.len());
    for e in &a.expected_effects {
        if seen.contains(&&e.account) {
            return Check::fail(
                "effects",
                format!("more than one stated effect on {}", e.account),
            );
        }
        seen.push(&e.account);
    }

    let mut treasury_delta: Option<&Delta> = None;
    for e in &a.expected_effects {
        if e.account == w.treasury {
            treasury_delta = Some(&e.delta);
        }
    }
    match treasury_delta {
        None => Check::fail(
            "effects",
            format!("no stated effect on treasury {}", w.treasury),
        ),
        Some(Delta::Increase(m)) => Check::fail(
            "effects",
            format!(
                "{} moves value out, but states +{m} on the treasury",
                a.intent.verb
            ),
        ),
        Some(Delta::Decrease(m)) => {
            if *m != a.amount {
                Check::fail(
                    "effects",
                    format!("treasury decrease {m} does not equal amount {}", a.amount),
                )
            } else {
                Check::pass("effects", format!("treasury -{m}"))
            }
        }
    }
}

/// Payment details that moved since the last payment stop the action dead.
///
/// Invoice fraud against a *real* invoice: the bill is genuine, the work was
/// done, the amount is right, and the account it points at belongs to
/// somebody else. No comparison of two ledgers catches it. Reconciling
/// against a bank confirms you paid exactly whom you instructed; reconciling
/// against a chain is weaker still, because the counterparty's ledger is
/// your ledger and the chain is not an independent witness. Odoo's answer is
/// to log every change to a vendor's `acc_number` in the chatter, which
/// records the change for someone who later goes looking.
///
/// Refused rather than escalated, and this is a deliberate change from the
/// earlier behaviour. A changed payee arriving as one more approval on a
/// reviewer's screen is the precise shape of the rubber-stamping problem:
/// the reader shown enough of these stops reading them. Accuracy among
/// radiologists with fifteen years' experience fell from 82% to 45.5% when
/// the machine's suggestion was wrong, and they were looking at images, not
/// at a hex string that differs from last month's somewhere in the middle.
///
/// The legitimate case -- a vendor that really did move banks -- goes
/// through [`World::attested_payees`], which the principal writes out of
/// band. That is a different act from clicking approve on a queue.
fn check_payee(a: &Action, w: &World) -> Check {
    match w.payee_status(&a.target.id) {
        PayeeStatus::Unseen => Check::pass("payee-unchanged", "no prior payment to compare"),
        PayeeStatus::Unchanged => Check::pass("payee-unchanged", "no change since last payment"),
        PayeeStatus::ChangedAndAttested => Check::pass(
            "payee-unchanged",
            format!(
                "details for {} changed, attested by the principal",
                a.target.id
            ),
        ),
        PayeeStatus::ChangedUnattested => Check::fail(
            "payee-unchanged",
            format!(
                "payment details for {} changed since last payment and are not attested",
                a.target.id
            ),
        ),
    }
}

/// Characters an address poisoner copies from the payee it imitates: the
/// first four and the last four of the hex, which is how wallets and
/// explorers abbreviate an address. The same definition docs/21 §7 measured.
const LOOKALIKE_PREFIX: usize = 4;
const LOOKALIKE_SUFFIX: usize = 4;

/// The 40 hex characters of an address, lowercased, or `None` if `id` is
/// not a full address. Short identifiers are not compared: a prefix and a
/// suffix overlap and the comparison would mean nothing.
fn address_body(id: &str) -> Option<String> {
    let s = id.trim().to_ascii_lowercase();
    let body = s.strip_prefix("0x")?;
    (body.len() == 40 && body.bytes().all(|b| b.is_ascii_hexdigit())).then(|| body.to_string())
}

fn imitates(candidate: &str, real: &str) -> bool {
    candidate != real
        && candidate[..LOOKALIKE_PREFIX] == real[..LOOKALIKE_PREFIX]
        && candidate[40 - LOOKALIKE_SUFFIX..] == real[40 - LOOKALIKE_SUFFIX..]
}

/// Whether two full addresses look alike the way a poisoner makes them: same
/// first four and last four hex characters, different in between. The
/// definition `payee-lookalike` refuses on, exposed so that anything else
/// that hands a list of payees to a human -- onboarding -- can keep a
/// look-alike off it by the same rule.
pub fn looks_alike(a: &str, b: &str) -> bool {
    match (address_body(a), address_body(b)) {
        (Some(x), Some(y)) => imitates(&x, &y),
        _ => false,
    }
}

/// A payee that imitates a known payee is refused, never escalated.
///
/// Address poisoning plants an address with the same first and last
/// characters as someone the treasury really pays, so that whoever copies
/// "the payee" from history copies the attacker. docs/21 §7 found one in 64
/// of 130 real treasuries; public losses run to $83.8M on Ethereum alone.
///
/// Refused rather than escalated for the reason [`check_payee`] gives: a
/// reviewer shown this is shown a hex string that matches at both ends, which
/// is the exact thing the attack is built to get approved. Asking is the
/// attack working.
///
/// The legitimate collision -- two real payees that happen to share both
/// ends -- goes through the principal attesting the address on chain, the
/// same out-of-band act that lets a changed payee through.
fn check_payee_lookalike(a: &Action, w: &World) -> Check {
    let Some(target) = address_body(&a.target.id) else {
        return Check::pass("payee-lookalike", "not a full address; nothing to compare");
    };
    let attested = w
        .onchain_attested
        .iter()
        .flatten()
        .any(|x| address_body(x).as_deref() == Some(target.as_str()));
    if attested {
        return Check::pass(
            "payee-lookalike",
            format!("{} is attested by the principal", a.target.id),
        );
    }
    let references = w
        .known_payees
        .keys()
        .chain(w.attested_payees.keys())
        .chain(w.onchain_attested.iter().flatten());
    let imitated = references
        .into_iter()
        .find(|r| address_body(r).is_some_and(|real| imitates(&target, &real)));
    match imitated {
        Some(r) => Check::fail(
            "payee-lookalike",
            format!(
                "{} shares its first {LOOKALIKE_PREFIX} and last {LOOKALIKE_SUFFIX} \
                 characters with known payee {r} but is a different address",
                a.target.id
            ),
        ),
        None => Check::pass("payee-lookalike", "imitates no known payee"),
    }
}

/// A bill more than one cycle behind is a question for the principal.
///
/// The agent pays the cycle after the owner's last payment, and the one after
/// each it settles. If another cycle has fallen due on top of the one being
/// proposed, nobody has paid this bill for at least a full interval: the
/// subscription may have been cancelled, the contractor may have left. Paying
/// back-cycles on inference alone is exactly the kind of confident mistake the
/// project exists to prevent, so it escalates. The principal confirms by
/// settling one cycle by hand, which is fresh evidence, and the agent carries
/// on from there.
///
/// Found 2026-10-02: the acceptance fixture's bills were all 70 to 470 days
/// overdue, and the agent, once it stopped re-paying its own cycles, began
/// catching up a year of a subscription one cycle per pass.
fn check_cycles_behind(a: &Action, w: &World) -> Check {
    let scoped = format!("{}:{}", a.idempotency.scope, a.idempotency.key);
    match w.cycles_behind.get(&scoped).copied().unwrap_or(0) {
        0 => Check::pass("cycles-behind", "current"),
        n => Check::review(
            "cycles-behind",
            format!(
                "{n} more cycle{} of this bill fell due after this one and nobody paid                  {}: it may have ended. Settle one by hand to confirm, and I will                  continue from there",
                if n == 1 { "" } else { "s" },
                if n == 1 { "it" } else { "them" }
            ),
        ),
    }
}

/// An amount outside what this counterparty was ever paid is a question,
/// not a payment. The band is a quarter either side of the observed range:
/// wide enough for a usage-based bill, narrow enough that a doubled invoice
/// or a "pay 10x this once" instruction stops here. Kyriba sells this as
/// fraud detection against payment history (`docs/50`); it is one comparison.
fn check_amount_range(a: &Action, w: &World) -> Check {
    let scoped = format!("{}:{}", a.idempotency.scope, a.idempotency.key);
    let Some((lo, hi)) = w.observed_amounts.get(&scoped) else {
        return Check::pass("amount-range", "no observed range for this bill");
    };
    let amt = a.amount.to_minor();
    let floor = lo.to_minor() - lo.to_minor() / 4;
    let ceiling = hi.to_minor() + hi.to_minor() / 4;
    if amt < floor || amt > ceiling {
        return Check::review(
            "amount-range",
            format!(
                "{} is outside what this payee was ever paid ({lo} to {hi}, with a quarter either side); confirm before paying",
                a.amount
            ),
        );
    }
    Check::pass(
        "amount-range",
        format!("{} is inside the observed {lo} to {hi}", a.amount),
    )
}

/// A payee the contract will refuse is a question for the principal.
///
/// `BudgetPolicy.pay` sends only to addresses the principal attested. A new
/// vendor is normal, not an error, so this escalates rather than refuses; the
/// principal can attest the address or settle the payment by hand. Without an
/// on-chain list (`None`) the check does not apply.
fn check_payee_attested(a: &Action, w: &World) -> Check {
    let Some(list) = &w.onchain_attested else {
        return Check::pass("payee-attested", "no on-chain payee list to consult");
    };
    let target = a.target.id.trim().to_ascii_lowercase();
    if list.iter().any(|x| x.trim().to_ascii_lowercase() == target) {
        Check::pass(
            "payee-attested",
            format!("{} is attested on chain", a.target.id),
        )
    } else {
        Check::review(
            "payee-attested",
            format!(
                "{} is not attested on chain, so the contract would refuse it. The principal \
                 can attest it or settle by hand",
                a.target.id
            ),
        )
    }
}

/// The token's issuer has frozen the payee or the treasury (PLAN 5.47).
///
/// Refused outright: the token itself would revert the transfer, and no
/// approval -- the agent's or the owner's -- makes a frozen address payable.
fn check_issuer_blocklist(a: &Action, w: &World) -> Check {
    let target = a.target.id.trim().to_ascii_lowercase();
    // The treasury id may carry a prefix ("arc:0x…"); an address inside it counts.
    let treasury = w.treasury.as_str().trim().to_ascii_lowercase();
    let blocked = |b: &String| b.trim().to_ascii_lowercase();
    let listed = |x: &str| w.issuer_blocked.iter().any(|b| blocked(b) == x);
    let treasury_listed = w
        .issuer_blocked
        .iter()
        .any(|b| !blocked(b).is_empty() && treasury.contains(&blocked(b)));
    if listed(&target) {
        Check::fail(
            "issuer-blocklist",
            format!(
                "{} is on the stablecoin issuer's blocklist; the token refuses transfers to it",
                a.target.id
            ),
        )
    } else if treasury_listed {
        Check::fail(
            "issuer-blocklist",
            "the paying account is on the stablecoin issuer's blocklist; nothing can move",
        )
    } else {
        Check::pass(
            "issuer-blocklist",
            "neither side is on the issuer's blocklist",
        )
    }
}

/// The invoice, the order, and the receipt have to agree with each other and
/// with the action.
///
/// This is the control that catches what reconciliation structurally cannot.
/// Double entry proves two columns agree. An error of commission -- the right
/// amount to the wrong party -- and an error of original entry that is
/// internally consistent both leave debits equal to credits, and both survive
/// a clean bank reconciliation. What catches them is asking whether the entry
/// corresponds to something outside the ledger: an order raised before the
/// fact, and a confirmation that the goods arrived.
///
/// ERPNext carries this in its schema already. Every Purchase Invoice line
/// has `purchase_order`, `po_detail`, `purchase_receipt`, `pr_detail`,
/// `received_qty` and `rejected_qty`, and two Buying Settings switches decide
/// whether any of it is enforced -- `po_required` and `pr_required`, both
/// defaulting to No. They default off because the control was built for a
/// person who closes the month and can chase an exception. An agent closes
/// nothing and chases nothing, so here the principal turns it on through
/// [`Precondition::ThreeWayMatch`] and the agent has no way to turn it off:
/// a required precondition is evaluated whatever the action proposes.
///
/// # Why there is no tolerance
///
/// ERPNext absorbs a mismatch of up to half a unit of currency into a Round
/// Off account (`process_debit_credit_difference`, general_ledger.py:397).
/// That account is an ordinary expense account with an ordinary balance, so
/// the repair is silent. GnuCash does the same thing loudly, into an
/// `Imbalance-CUR` account a person sees on the next screen, and beancount
/// makes the acceptable disagreement an explicit tolerance on the assertion.
/// Loud or refusing are the two defensible options; silent is the one that
/// turns an error of original entry into a permanent compensating one.
///
/// Here the amounts must agree exactly, which costs nothing because [`Money`]
/// is an integer count of minor units. The remedy for a genuine variance is
/// an amended purchase order, not a rounding.
fn check_three_way_match(a: &Action, w: &World) -> Check {
    const NAME: &str = "pre:three-way";

    let refs = |kind: EvidenceKind| -> Vec<&str> {
        a.evidence
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.reference.as_str())
            .collect()
    };

    let invoices = refs(EvidenceKind::Invoice);
    let orders = refs(EvidenceKind::PurchaseOrder);
    let receipts = refs(EvidenceKind::Receipt);

    // More than one of either side makes the match ambiguous: a total could
    // be reconciled against more than one reading, and whoever reconciles
    // picks. The same reasoning refuses two stated effects on one account.
    if orders.len() > 1 {
        return Check::fail(
            NAME,
            format!("{} purchase orders cited, expected one", orders.len()),
        );
    }
    if receipts.len() > 1 {
        return Check::fail(
            NAME,
            format!("{} receipts cited, expected one", receipts.len()),
        );
    }

    // Nothing cited at all. A human can legitimately approve a purchase that
    // never had an order -- an emergency, a vendor who does not work that
    // way -- so this is judgment rather than error.
    let mut missing = Vec::new();
    if invoices.is_empty() {
        missing.push("invoice");
    }
    if orders.is_empty() {
        missing.push("purchase order");
    }
    if receipts.is_empty() {
        missing.push("receipt");
    }
    if !missing.is_empty() {
        return Check::review(NAME, format!("no {} cited", missing.join(", no ")));
    }

    let order_ref = orders[0];
    let receipt_ref = receipts[0];

    // A reference that resolves to nothing is a document the agent named and
    // the principal's systems have never seen. Not a judgment call.
    let Some(order) = w.purchase_orders.get(order_ref) else {
        return Check::fail(NAME, format!("purchase order {order_ref} does not exist"));
    };
    let Some(receipt) = w.receipts.get(receipt_ref) else {
        return Check::fail(NAME, format!("receipt {receipt_ref} does not exist"));
    };

    // Commission at the party level: an order to one vendor settled to
    // another. The books balance either way.
    if order.counterparty != a.target.id {
        return Check::fail(
            NAME,
            format!(
                "purchase order {order_ref} names {}, action pays {}",
                order.counterparty, a.target.id
            ),
        );
    }
    if receipt.counterparty != a.target.id {
        return Check::fail(
            NAME,
            format!(
                "receipt {receipt_ref} names {}, action pays {}",
                receipt.counterparty, a.target.id
            ),
        );
    }

    if order.amount.currency() != a.amount.currency()
        || receipt.amount.currency() != a.amount.currency()
    {
        return Check::fail(
            NAME,
            format!(
                "currency mismatch: action {}, order {}, receipt {}",
                a.amount.currency(),
                order.amount.currency(),
                receipt.amount.currency()
            ),
        );
    }

    // Billed for more than was ordered. The classic altered invoice, and the
    // one that survives every arithmetic check in the ledger.
    if a.amount.to_minor() > order.amount.to_minor() {
        return Check::fail(
            NAME,
            format!(
                "action pays {}, purchase order {order_ref} authorises {}; amend the order rather than the payment",
                a.amount, order.amount
            ),
        );
    }

    // Billed for something other than what arrived. Exact, because a
    // tolerance here is where a silent compensating entry comes from.
    if a.amount.to_minor() != receipt.amount.to_minor() {
        return Check::fail(
            NAME,
            format!(
                "action pays {}, receipt {receipt_ref} records {} received",
                a.amount, receipt.amount
            ),
        );
    }

    Check::pass(
        NAME,
        format!(
            "{} matches order {order_ref} and receipt {receipt_ref}",
            a.amount
        ),
    )
}

fn check_precondition(pre: &Precondition, a: &Action, w: &World) -> Check {
    match pre {
        Precondition::BalanceAtLeast(min) => {
            if w.balance.to_minor() < min.to_minor() {
                Check::fail("pre:balance", format!("balance {} below {min}", w.balance))
            } else {
                Check::pass("pre:balance", format!("balance {}", w.balance))
            }
        }
        Precondition::ReserveFloor(floor) => match w.balance.checked_sub(a.amount) {
            Err(e) => Check::fail("pre:reserve", format!("arithmetic: {e}")),
            Ok(after) => {
                if after.to_minor() < floor.to_minor() {
                    Check::review("pre:reserve", format!("would leave {after}, floor {floor}"))
                } else {
                    Check::pass("pre:reserve", format!("leaves {after}"))
                }
            }
        },
        Precondition::NotAlreadySettled => {
            let scoped = format!("{}:{}", a.idempotency.scope, a.idempotency.key);
            if w.settled.contains(&scoped) {
                Check::fail("pre:not-settled", format!("{scoped} already settled"))
            } else {
                Check::pass("pre:not-settled", "no prior settlement")
            }
        }
        Precondition::CounterpartyKnown => {
            if w.known_payees.contains_key(&a.target.id) {
                Check::pass("pre:known", format!("{} paid before", a.target.id))
            } else {
                Check::review("pre:known", format!("{} never paid before", a.target.id))
            }
        }
        Precondition::PayeeUnchanged => match w.payee_status(&a.target.id) {
            PayeeStatus::ChangedUnattested => {
                Check::fail("pre:payee", "payment details changed and are not attested")
            }
            PayeeStatus::ChangedAndAttested => Check::pass(
                "pre:payee",
                "payment details changed, attested by the principal",
            ),
            PayeeStatus::Unseen | PayeeStatus::Unchanged => {
                Check::pass("pre:payee", "payment details unchanged")
            }
        },
        Precondition::ConfidenceAtLeast(bp) => {
            let scoped = format!("{}:{}", a.idempotency.scope, a.idempotency.key);
            let floor = Confidence::new(f64::from(*bp) / 10_000.0).unwrap_or(Confidence::ZERO);
            match w.inferred_confidence.get(&scoped) {
                // Nothing inferred this action, so there is no guess to
                // doubt. Silence is not a low score.
                None => Check::pass("pre:confidence", "not an inferred obligation"),
                Some(got) if got.at_least(floor) => {
                    Check::pass("pre:confidence", format!("confidence {got}"))
                }
                Some(got) => Check::review(
                    "pre:confidence",
                    format!("confidence {got} below floor {floor}"),
                ),
            }
        }
        Precondition::ThreeWayMatch => check_three_way_match(a, w),
        Precondition::EvidenceAtLeast(n) => {
            if a.evidence.len() < *n {
                Check::review(
                    "pre:evidence",
                    format!("{} pieces of evidence, wanted {n}", a.evidence.len()),
                )
            } else {
                Check::pass("pre:evidence", format!("{} pieces", a.evidence.len()))
            }
        }
        Precondition::ProvenanceAtLeast(want) => match a.weakest_provenance() {
            None => Check::review("pre:provenance", "no evidence at all"),
            Some(got) => {
                if got < *want {
                    Check::review(
                        "pre:provenance",
                        format!("weakest evidence is {got}, wanted {want}"),
                    )
                } else {
                    Check::pass("pre:provenance", format!("weakest is {got}"))
                }
            }
        },
    }
}

/// Re-exported so callers can build a `World` without importing `action`.
pub use crate::action::Authority as ObservedAuthority;

/// Provenance ordering is load-bearing; assert it here so a reordering of
/// the enum cannot silently weaken every `ProvenanceAtLeast` check.
const _: () = {
    assert!((Provenance::Inferred as u8) < (Provenance::CounterpartyAsserted as u8));
    assert!((Provenance::CounterpartyAsserted as u8) < (Provenance::HumanConfirmed as u8));
    assert!((Provenance::HumanConfirmed as u8) < (Provenance::Onchain as u8));
};
