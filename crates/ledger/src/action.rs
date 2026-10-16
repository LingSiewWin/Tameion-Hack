//! Action IR v0.1 — what an agent proposes, before anything happens.
//!
//! Double-entry proves two columns agree with each other. It cannot prove
//! either column corresponds to the world: a payment to the wrong vendor, a
//! duplicate retry, a fabricated invoice and a misread amount all leave
//! debits equal to credits. The controls that catch those compare the entry
//! to something outside the ledger.
//!
//! An `Action` is that comparison made explicit and made *first*. It is a
//! proposal, not an effect. It answers ten questions and nothing else:
//!
//! 1. WHO is acting                  [`Action::actor`]
//! 2. WHO is the principal           [`Action::principal`]
//! 3. WHAT should happen             [`Action::intent`]
//! 4. WHY                            [`Intent::rationale`]
//! 5. TO WHOM                        [`Action::target`]
//! 6. WHAT evidence supports it      [`Action::evidence`]
//! 7. WHAT must be true first        [`Action::preconditions`]
//! 8. IS the agent authorised        [`Action::authority`]
//! 9. WHAT should change afterwards  [`Action::expected_effects`]
//! 10. HOW is it done exactly once   [`Action::idempotency`]
//!
//! Anything that does not answer one of those is not in v0.1.
//!
//! The proposal is separated from the effect deliberately. Circle's own
//! escrow sample releases funds on a model's `confidence === "HIGH"` string,
//! which makes the model's output the release condition. Here the model
//! fills in an `Action` and a deterministic verifier decides, so the model's
//! output is an input.

use crate::account::AccountId;
use crate::money::{Currency, Money};
use core::fmt;

/// Who performed the reasoning. Usually a model, occasionally a person.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Actor {
    /// Stable id for the agent, e.g. an ERC-8004 identity or a model name.
    pub id: String,
    /// What kind of thing did the reasoning.
    pub kind: ActorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActorKind {
    /// A language model proposed this.
    Model,
    /// Deterministic code proposed this.
    Rule,
    /// A person proposed this.
    Human,
}

impl fmt::Display for ActorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ActorKind::Model => "model",
            ActorKind::Rule => "rule",
            ActorKind::Human => "human",
        })
    }
}

/// On whose behalf the action is taken, and whose money is at stake.
///
/// Separated from [`Actor`] because they are different parties with
/// different liability. The agent reasons; the principal is accountable.
/// Collapsing them is what makes "the model decided" sound like a defence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Principal {
    /// The organisation or person the agent acts for.
    pub id: String,
    /// The account the value moves out of.
    pub treasury: AccountId,
}

/// What the agent wants to happen, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    pub verb: Verb,
    /// Free text from whoever reasoned. Recorded, never parsed, never
    /// treated as a condition.
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verb {
    /// Move value to a counterparty to discharge an obligation.
    Settle,
    /// Move value between the principal's own accounts.
    Transfer,
    /// Buy a service at a stated price.
    Purchase,
    /// Return value previously received.
    Refund,
}

impl fmt::Display for Verb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Verb::Settle => "settle",
            Verb::Transfer => "transfer",
            Verb::Purchase => "purchase",
            Verb::Refund => "refund",
        })
    }
}

/// Who or what the action is aimed at.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    /// The counterparty address or account being paid.
    pub id: String,
    /// A human-readable label, if one has ever been confirmed.
    ///
    /// `None` is meaningful: an unnamed counterparty is one nobody has
    /// vouched for, and the verifier treats that as a reason to review.
    pub label: Option<String>,
}

/// A fact the action rests on, and where that fact came from.
///
/// Provenance travels with the evidence because "the model said so" and
/// "the chain says so" are not the same claim and must not be weighed the
/// same way.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Evidence {
    pub kind: EvidenceKind,
    /// Identifier of the underlying artefact: a tx hash, an invoice number,
    /// a document id.
    pub reference: String,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvidenceKind {
    /// A settled transaction observed on chain.
    OnchainTransfer,
    /// A bill received from a counterparty.
    Invoice,
    /// Confirmation that goods or services arrived.
    Receipt,
    /// A purchase order raised before the fact.
    PurchaseOrder,
    /// A label a human confirmed for a counterparty.
    HumanConfirmation,
    /// A pattern the agent inferred from history.
    InferredPattern,
}

/// Where a piece of evidence came from, ordered by how much weight it
/// deserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provenance {
    /// A model inferred it. Weakest: it can be wrong and can be induced to
    /// be wrong.
    Inferred = 0,
    /// A counterparty asserted it. They have an interest.
    CounterpartyAsserted = 1,
    /// A person at the principal confirmed it.
    HumanConfirmed = 2,
    /// Read from chain state. Cannot be retracted, but only ever proves
    /// that something happened -- never that it should have.
    Onchain = 3,
}

impl fmt::Display for Provenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Provenance::Inferred => "inferred",
            Provenance::CounterpartyAsserted => "counterparty-asserted",
            Provenance::HumanConfirmed => "human-confirmed",
            Provenance::Onchain => "onchain",
        })
    }
}

/// The confidence floor, in basis points, where the agent may pay by itself
/// (Autopilot). Not tuned: chosen on 2025-09-25 so that the three obligations
/// the end-to-end run recovers split the way a person would split them.
pub const CONFIDENCE_FLOOR_AUTOPILOT_BP: u16 = 7_500;

/// The confidence floor where the agent cannot pay by itself (Propose, a halt,
/// a lapsed agent): it may bring a bill to the owner at 0.60. Measured, not
/// tuned: in the walk-forward backtest on 22 real treasuries the 0.60-0.75
/// bucket was paid on schedule 81.3 % of the time over 123 predictions; the
/// bucket at or above 0.75, 70.0 % over 10 (`docs/21` §3b, table row at line
/// 227). Below Autopilot a wrong guess costs the owner a glance, never a payment.
pub const CONFIDENCE_FLOOR_PROPOSE_BP: u16 = 6_000;

// The floor where nothing is paid alone is never the stricter one.
const _: () = assert!(CONFIDENCE_FLOOR_PROPOSE_BP < CONFIDENCE_FLOOR_AUTOPILOT_BP);

/// The floor for a pass: `pays_alone` is whether the contract would let the
/// agent's key move money on its own verdict this pass.
pub fn confidence_floor_bp(pays_alone: bool) -> u16 {
    if pays_alone {
        CONFIDENCE_FLOOR_AUTOPILOT_BP
    } else {
        CONFIDENCE_FLOOR_PROPOSE_BP
    }
}

/// A condition that must hold before execution.
///
/// Deliberately a small closed set rather than a general expression
/// language. A closed set can be exhaustively tested and cannot be extended
/// by a prompt; a DSL invites the agent to write its own conditions, which
/// is the authority hole the whole design exists to close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// The treasury holds at least this much.
    BalanceAtLeast(Money),
    /// After this action, the treasury still holds at least this much.
    ReserveFloor(Money),
    /// No prior action with the same idempotency key has succeeded.
    NotAlreadySettled,
    /// The counterparty has been paid before from this treasury.
    CounterpartyKnown,
    /// The counterparty's payment details have not changed since last time.
    PayeeUnchanged,
    /// At least this many independent pieces of evidence exist.
    EvidenceAtLeast(usize),
    /// The weakest piece of evidence is at least this strong.
    ProvenanceAtLeast(Provenance),
    /// The inference this action rests on is at least this strong.
    ///
    /// An obligation recovered from transfer history is a claim about the
    /// past made from incomplete evidence, and the detector says how sure it
    /// is. Paying a 0.62 out of 1.0 guess on the agent's own authority is the
    /// project's headline unmitigated risk (`docs/14-risks.md`); this is the
    /// dial that turns it into an escalation instead.
    ///
    /// The floor is carried as basis points rather than a float so that the
    /// canonical encoding stays integer -- `0.75` and `0.750000` must not
    /// hash differently, and a precondition that is part of the action hash
    /// cannot afford a float's ambiguity.
    ConfidenceAtLeast(u16),
    /// The invoice, the purchase order and the receipt agree with each
    /// other and with this action.
    ///
    /// The profession's defence against a fictitious or altered invoice,
    /// and the only control here that catches a payment which is wrong in
    /// a way the ledger cannot see. ERPNext carries the same check as two
    /// settings that default to No; requiring it is the principal's call,
    /// but once required the agent cannot decline to satisfy it.
    ThreeWayMatch,
}

impl fmt::Display for Precondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Precondition::BalanceAtLeast(m) => write!(f, "balance >= {m}"),
            Precondition::ReserveFloor(m) => write!(f, "reserve floor {m}"),
            Precondition::NotAlreadySettled => f.write_str("not already settled"),
            Precondition::CounterpartyKnown => f.write_str("counterparty known"),
            Precondition::PayeeUnchanged => f.write_str("payee details unchanged"),
            Precondition::EvidenceAtLeast(n) => write!(f, "evidence count >= {n}"),
            Precondition::ProvenanceAtLeast(p) => write!(f, "provenance >= {p}"),
            Precondition::ConfidenceAtLeast(bp) => {
                write!(
                    f,
                    "inference confidence >= {:.2}",
                    f64::from(*bp) / 10_000.0
                )
            }
            Precondition::ThreeWayMatch => f.write_str("invoice matches order and receipt"),
        }
    }
}

/// The limits the agent is operating under.
///
/// These are a *claim* about what the agent believes it may do. The verifier
/// checks the action against them, and the same numbers are enforced on
/// chain, so an agent that lies here still cannot spend past the contract.
/// Freysa released 47,316 USD on the 482nd attempt against the strongest
/// possible prompt-level prohibition; a limit that lives only where the
/// agent can reach it is not a limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authority {
    /// Largest single action this agent may take unaided.
    pub per_action_cap: Money,
    /// Largest total across any 24 hours (the contract's trailing window).
    pub daily_cap: Money,
    /// Already spent in the trailing 24-hour window, as observed on chain --
    /// not as the agent reports it. The name predates the window: the
    /// contract used to reset at UTC midnight, which let the cap be spent
    /// twice across it (docs/14). Kept because it is part of the canonical
    /// encoding that other implementations hash.
    pub spent_today: Money,
}

/// What the world should look like afterwards.
///
/// Stated before execution so that the receipt can be checked against it.
/// Without this, "did it work" means "did the call return 200".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedEffect {
    pub account: AccountId,
    pub delta: Delta,
}

/// A signed change to an account balance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    Decrease(Money),
    Increase(Money),
}

impl Delta {
    pub const fn amount(&self) -> Money {
        match self {
            Delta::Decrease(m) | Delta::Increase(m) => *m,
        }
    }
}

impl fmt::Display for Delta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Delta::Decrease(m) => write!(f, "-{m}"),
            Delta::Increase(m) => write!(f, "+{m}"),
        }
    }
}

/// How this action is made to happen at most once.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Idempotency {
    /// A *business* key, not a transport key. Two different API calls can be
    /// the same real-world action, and a retry after a timeout is the
    /// classic way an error of original entry gets made twice over.
    ///
    /// For a recurring obligation this is the counterparty plus the cycle:
    /// `0x1111...@1984916082`.
    pub key: String,
    /// The namespace the key is unique within.
    pub scope: String,
}

/// When the action is good for.
///
/// Verify at noon, execute at six, and the world has moved. An action that
/// cannot go stale is an action whose checks can be arbitrarily old.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Validity {
    pub not_before: u64,
    pub expires_at: u64,
}

/// A proposed action. Nothing here has happened yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub id: String,
    pub actor: Actor,
    pub principal: Principal,
    pub intent: Intent,
    pub target: Target,
    pub amount: Money,
    pub evidence: Vec<Evidence>,
    pub preconditions: Vec<Precondition>,
    pub authority: Authority,
    pub expected_effects: Vec<ExpectedEffect>,
    pub idempotency: Idempotency,
    pub validity: Validity,
}

impl Action {
    pub fn currency(&self) -> Currency {
        self.amount.currency()
    }

    /// The weakest provenance among the evidence, or `None` if there is no
    /// evidence at all.
    ///
    /// The weakest link is the right summary: six onchain transfers plus one
    /// inferred guess is only as good as the guess if the guess is what ties
    /// them to this counterparty.
    pub fn weakest_provenance(&self) -> Option<Provenance> {
        self.evidence.iter().map(|e| e.provenance).min()
    }
}

#[cfg(test)]
mod floor_tests {
    use super::*;

    /// `docs/21` §3b, line 227: the 0.60-0.75 bucket is 81.3 % on schedule
    /// over 123 predictions; the floor where nothing is paid alone sits at
    /// its bottom edge and nowhere lower.
    #[test]
    fn the_floor_follows_whether_the_agent_pays_alone() {
        assert_eq!(confidence_floor_bp(true), 7_500);
        assert_eq!(
            confidence_floor_bp(false),
            6_000,
            "0.60 is the measured bucket's lower edge"
        );
        assert!(confidence_floor_bp(false) < confidence_floor_bp(true));
    }
}
