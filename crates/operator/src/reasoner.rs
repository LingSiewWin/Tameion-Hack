//! The seam a language model plugs into.
//!
//! The trait is async because the intended production implementation makes a
//! network call. The deterministic baseline does not, and implements the
//! trait trivially.
//!
//! A reasoner returns an *ordering with reasons*. It does not return an
//! authorisation, cannot see the policy limits, and cannot alter them. That
//! is the whole design: the worst a compromised or confused reasoner can do
//! is propose a bad order, which the verifier and the on-chain cap then
//! refuse independently.

use ledger::{ActorKind, Money, Obligation, Prioritised, Timestamp, rank_by_urgency};

/// Why a reasoner failed to produce an ordering.
///
/// The split is by **what the caller should do next**, not by where the
/// failure happened. Merging these was costing us: a 400 from a misconfigured
/// key and a 529 from an overloaded service are the same sentence to a human
/// and opposite instructions to a retry loop. Both real failures during
/// bring-up were permanent 400s, where retrying is pure waste.
#[derive(Debug, thiserror::Error)]
pub enum ReasonerError {
    /// Worth retrying as-is: network trouble, a timeout, a 5xx, a rate limit.
    #[error("reasoner temporarily unavailable: {0}")]
    Transient(String),

    /// Will fail identically until a human changes something: a bad key, a
    /// missing workspace, an exhausted balance, a malformed request. Retrying
    /// only burns time.
    #[error("reasoner permanently unavailable: {0}")]
    Permanent(String),

    /// The model returned successfully and declined the task. Not an error in
    /// the transport sense, and not retryable by repetition.
    #[error("reasoner declined: {0}")]
    Declined(String),

    /// The reply did not parse, or parsed into something that was not an
    /// ordering of what we asked about.
    #[error("reasoner returned an unusable reply: {0}")]
    Unusable(String),

    /// The reply added, dropped or duplicated an obligation. Kept as its own
    /// variant because it is the security-relevant one.
    #[error("reasoner returned an ordering over a different set of obligations")]
    WrongPopulation,
}

impl ReasonerError {
    /// Whether repeating the identical request could succeed.
    pub fn retryable(&self) -> bool {
        matches!(self, ReasonerError::Transient(_))
    }

    /// Stable machine-readable code, for logs and wire formats that should
    /// not be parsed as prose.
    pub fn code(&self) -> &'static str {
        match self {
            ReasonerError::Transient(_) => "reasoner-transient",
            ReasonerError::Permanent(_) => "reasoner-permanent",
            ReasonerError::Declined(_) => "reasoner-declined",
            ReasonerError::Unusable(_) => "reasoner-unusable",
            ReasonerError::WrongPopulation => "reasoner-wrong-population",
        }
    }
}

/// Judgment inside the policy boundary. Never authority over it.
pub trait Reasoner {
    /// Order obligations by which should be settled first.
    fn prioritise(
        &self,
        obligations: &[Obligation],
        balance: Money,
        now: Timestamp,
    ) -> impl std::future::Future<Output = Result<Vec<Prioritised>, ReasonerError>> + Send;

    /// Name recorded in the log, so a reader knows what did the reasoning.
    fn name(&self) -> &str;

    /// Whether a model or deterministic code produced the ordering.
    ///
    /// This is part of the action that gets hashed and anchored, so it is a
    /// provenance claim rather than a label: an implementation that reports
    /// `Rule` while a model did the ranking makes the on-chain record false.
    /// Required rather than defaulted for that reason -- a default would let
    /// a new reasoner inherit the wrong answer by saying nothing.
    fn kind(&self) -> ActorKind;
}

/// The deterministic baseline, and the control a model is measured against.
#[derive(Debug, Clone, Copy, Default)]
pub struct RuleBased;

impl Reasoner for RuleBased {
    async fn prioritise(
        &self,
        obligations: &[Obligation],
        balance: Money,
        now: Timestamp,
    ) -> Result<Vec<Prioritised>, ReasonerError> {
        Ok(rank_by_urgency(obligations, balance, now))
    }

    fn name(&self) -> &str {
        "rule-based"
    }

    fn kind(&self) -> ActorKind {
        ActorKind::Rule
    }
}
