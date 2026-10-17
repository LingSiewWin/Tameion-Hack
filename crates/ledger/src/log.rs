//! The append-only record of what the agent did and why.
//!
//! *Agents and Ledgers* makes the case for this directly: an Athenian who
//! handled public money faced the euthyna, the "setting straight", at the end
//! of his term. The audit was annual because reading the accounts was slow.
//! An agent with spending authority can do better -- write a signed,
//! append-only record of every decision, including the balance it saw and the
//! rule it applied, so a reviewer replays the reasoning instead of
//! reconstructing it from bank statements.
//!
//! A root of the log is committed on chain. That commitment proves the log was
//! not edited after the fact. It does **not** prove the log was right: a
//! payment to the wrong vendor anchors perfectly. The controls that catch
//! that compare an entry to something outside the ledger, and they live in
//! [`crate::verify`].
//!
//! # No I/O
//!
//! This module holds events in memory and hashes them. Reading and writing a
//! file is the operator's job, because `ledger` performs no I/O at all.
//!
//! # The Merkle construction, and the bug it avoids
//!
//! The obvious binary tree duplicates the last node when a level has an odd
//! count. That is the Bitcoin construction, and it carries CVE-2012-2459:
//! `[A, B, C]` and `[A, B, C, C]` produce the **same root**, because the
//! first duplicates `C` into exactly the shape the second already has. Two
//! different logs then commit to one root, and for a log whose last entry is
//! a settlement that means a record of one payment and a record of two are
//! indistinguishable to anyone checking the anchor. The Python implementation
//! this replaces had precisely that shape.
//!
//! This uses the RFC 6962 (Certificate Transparency) construction instead:
//!
//! ```text
//! MTH({d0})  = H(0x00 || d0)
//! MTH(D[n])  = H(0x01 || MTH(D[0:k]) || MTH(D[k:n]))    k = largest power of 2 < n
//! ```
//!
//! Two properties matter. Splitting at a power of two means no node is ever
//! duplicated, so distinct leaf sets give distinct roots. And the `0x00` /
//! `0x01` prefixes separate the leaf and node domains, so an internal node
//! can never be presented as a leaf -- the second-preimage attack the
//! prefixes exist to stop.
//!
//! Keccak-256 rather than RFC 6962's SHA-256, so the root can be handed
//! straight to an EVM contract, and so the whole system uses one hash
//! function rather than two.

use crate::action::Provenance;
use crate::canon::ActionHash;
use crate::codec::Encoder;
use crate::money::Money;
use crate::obligations::{Confidence, Counterparty};
use crate::time::Timestamp;
use crate::verify::Decision;
use core::fmt;
use sha3::{Digest, Keccak256};
use std::collections::BTreeSet;

/// A 32-byte Keccak-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Hash32([u8; 32]);

impl Hash32 {
    pub const ZERO: Hash32 = Hash32([0u8; 32]);

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        let mut s = String::with_capacity(66);
        s.push_str("0x");
        for b in self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Hash32(bytes)
    }

    /// Parse `0x`-prefixed or bare 64 hex characters. `None` on anything else.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        if s.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16)?;
            let lo = (chunk[1] as char).to_digit(16)?;
            out[i] = (hi * 16 + lo) as u8;
        }
        Some(Hash32(out))
    }
}

/// The leaf of an event given only its canonical bytes: what an audit file
/// stores, so a reader with no `Event` decoder can still recompute the root.
pub fn leaf_from_canonical(canonical: &[u8]) -> Hash32 {
    hash_leaf(canonical)
}

/// The root over leaves in order; identical to [`DecisionLog::root`] over the
/// events those leaves came from.
pub fn root_from_leaves(leaves: &[Hash32]) -> Hash32 {
    merkle_root(leaves)
}

/// Inclusion path for `index` among `leaves`; `None` when out of range.
pub fn path_from_leaves(leaves: &[Hash32], index: usize) -> Option<Vec<ProofStep>> {
    (index < leaves.len()).then(|| merkle_path(leaves, index))
}

impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Domain prefix for a leaf, per RFC 6962.
const LEAF_PREFIX: u8 = 0x00;
/// Domain prefix for an internal node, per RFC 6962.
const NODE_PREFIX: u8 = 0x01;

fn hash_leaf(data: &[u8]) -> Hash32 {
    let mut h = Keccak256::new();
    h.update([LEAF_PREFIX]);
    h.update(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    Hash32(out)
}

fn hash_node(left: Hash32, right: Hash32) -> Hash32 {
    let mut h = Keccak256::new();
    h.update([NODE_PREFIX]);
    h.update(left.0);
    h.update(right.0);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    Hash32(out)
}

/// Largest power of two strictly less than `n`. Requires `n > 1`.
fn split_point(n: usize) -> usize {
    debug_assert!(n > 1);
    1usize << (usize::BITS - 1 - (n - 1).leading_zeros())
}

fn merkle_root(leaves: &[Hash32]) -> Hash32 {
    match leaves.len() {
        0 => Hash32::ZERO,
        1 => leaves[0],
        n => {
            let k = split_point(n);
            hash_node(merkle_root(&leaves[..k]), merkle_root(&leaves[k..]))
        }
    }
}

/// One step of an inclusion proof: a sibling, and which side it sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProofStep {
    pub sibling: Hash32,
    /// True when the sibling is to the left and hashes first.
    pub sibling_is_left: bool,
}

fn merkle_path(leaves: &[Hash32], index: usize) -> Vec<ProofStep> {
    if leaves.len() <= 1 {
        return Vec::new();
    }
    let k = split_point(leaves.len());
    if index < k {
        let mut path = merkle_path(&leaves[..k], index);
        path.push(ProofStep {
            sibling: merkle_root(&leaves[k..]),
            sibling_is_left: false,
        });
        path
    } else {
        let mut path = merkle_path(&leaves[k..], index - k);
        path.push(ProofStep {
            sibling: merkle_root(&leaves[..k]),
            sibling_is_left: true,
        });
        path
    }
}

/// Recompute a root from a leaf and its path.
///
/// The leaf passed in is the **leaf hash**, already domain-prefixed.
pub fn verify_inclusion(leaf: Hash32, path: &[ProofStep], root: Hash32) -> bool {
    let mut acc = leaf;
    for step in path {
        acc = if step.sibling_is_left {
            hash_node(step.sibling, acc)
        } else {
            hash_node(acc, step.sibling)
        };
    }
    acc == root
}

/// The consistency proof that the tree over the first `m` leaves is a prefix
/// of the tree over all `leaves` (RFC 9162 §2.1.4.1, `PROOF(m, D_n)`): the
/// book only grew between the two roots. `None` unless `0 < m <= n`; empty
/// when `m == n`.
pub fn consistency_from_leaves(leaves: &[Hash32], m: usize) -> Option<Vec<Hash32>> {
    (m > 0 && m <= leaves.len()).then(|| subproof(m, leaves, true))
}

fn subproof(m: usize, d: &[Hash32], complete: bool) -> Vec<Hash32> {
    let n = d.len();
    if m == n {
        return if complete {
            Vec::new()
        } else {
            vec![merkle_root(d)]
        };
    }
    let k = split_point(n);
    if m <= k {
        let mut p = subproof(m, &d[..k], complete);
        p.push(merkle_root(&d[k..]));
        p
    } else {
        let mut p = subproof(m - k, &d[k..], false);
        p.push(merkle_root(&d[..k]));
        p
    }
}

/// Verify that `second` (over `n` leaves) extends `first` (over `m`), from a
/// consistency path (RFC 9162 §2.1.4.2). Equal sizes need equal roots and an
/// empty path.
pub fn verify_consistency(
    first: Hash32,
    m: usize,
    second: Hash32,
    n: usize,
    path: &[Hash32],
) -> bool {
    if m == n {
        return path.is_empty() && first == second;
    }
    if m == 0 || m > n || path.is_empty() {
        return false;
    }
    let mut path = path.to_vec();
    if m.is_power_of_two() {
        path.insert(0, first);
    }
    let (mut f, mut s) = (m - 1, n - 1);
    while f & 1 == 1 {
        f >>= 1;
        s >>= 1;
    }
    let (mut fr, mut sr) = (path[0], path[0]);
    for &c in &path[1..] {
        if s == 0 {
            return false;
        }
        if f & 1 == 1 || f == s {
            fr = hash_node(c, fr);
            sr = hash_node(c, sr);
            while f & 1 == 0 && f != 0 {
                f >>= 1;
                s >>= 1;
            }
        } else {
            sr = hash_node(sr, c);
        }
        f >>= 1;
        s >>= 1;
    }
    fr == first && sr == second && s == 0
}

/// What an event records.
///
/// A closed set of typed variants rather than a free-form map. An untyped
/// payload lets a caller record a settlement with no cycle key, or a decision
/// with no amount, and nothing notices until an auditor needs the field that
/// was never written.
#[derive(Debug, Clone, PartialEq)]
pub enum EventBody {
    /// What the agent saw before deciding anything.
    Observation {
        balance: Money,
        transfers_seen: usize,
        obligations_found: usize,
        total_due: Money,
        note: String,
    },
    /// A verdict on one proposed action.
    Decision {
        counterparty: Counterparty,
        amount: Money,
        decision: Decision,
        action_hash: ActionHash,
        confidence: Confidence,
        balance_seen: Money,
        weakest_evidence: Option<Provenance>,
        evidence: Vec<String>,
        note: String,
    },
    /// A payment that settled, and the transaction that settled it.
    ///
    /// Recorded as a separate event rather than as an edit to the decision.
    /// An append-only log that rewrites earlier entries is not append-only,
    /// and the on-chain receipt is the external witness the decision record
    /// cannot be on its own.
    Settlement {
        counterparty: Counterparty,
        cycle_key: String,
        amount: Money,
        tx_hash: String,
        /// Provenance: the hash of the whole action, prose included. Moves
        /// when the reasoning moves, which is what makes it useful for
        /// showing exactly what was proposed and why.
        action_hash: ActionHash,
        /// What the chain was actually told. Derived from the idempotency
        /// pair alone, so it does not move when a model rewords itself.
        ///
        /// Recorded separately because these are two different questions and
        /// conflating them has already cost money: the on-chain replay guard
        /// keys on this, and a reviewer reconciling the record against the
        /// chain needs this one, not `action_hash`. The cross-check in
        /// `operator/examples/run_loop.rs` compared the wrong one of the two
        /// and therefore reported `false` for every settlement it had just
        /// made -- a check that could never pass, printing rather than
        /// asserting, which is how it survived four days unnoticed.
        settlement_key: ActionHash,
        note: String,
    },
    /// A decision handed to a human.
    Escalation {
        counterparty: Counterparty,
        amount: Money,
        reason: String,
        note: String,
    },
    /// A cycle passed over because it was already settled.
    Skipped {
        counterparty: Counterparty,
        cycle_key: String,
        note: String,
    },
    /// The hedge layer's verdict and action on one coin in one pass.
    ///
    /// Sizes and prices are canonical decimal strings: a hedge's base units
    /// are not money and a venue price is not a ledger amount, so neither
    /// belongs in [`Money`]. `notional_after` is USDC and does. Recorded
    /// whether or not an order went out, because a refusal is a decision too.
    Hedge {
        coin: String,
        /// `HOLD`, `DRY-RUN`, `TRADED`, `REFUSE` or `HALTED`.
        outcome: String,
        size_before: String,
        size_delta: String,
        size_after: String,
        mark_px: String,
        notional_after: Money,
        /// Venue order id when an order was sent.
        oid: Option<String>,
        note: String,
    },
    /// A model's proposal about the hedge policy, recorded so a human can
    /// act on it. It changes nothing by itself: the band and the caps are
    /// code, and only a human edits them. `by` names the model, so a reader
    /// never mistakes a suggestion for a decision.
    HedgeProposal {
        coin: String,
        /// `keep`, `tighten-band`, `widen-band` or `pause`.
        advice: String,
        /// Proposed band in basis points, for tighten/widen only.
        band_bps: Option<u64>,
        reason: String,
        by: String,
        note: String,
    },
    /// One period's flow check (docs/117 §1): the opening balance plus what
    /// came in minus what went out, against the balance the chain reports at
    /// the period's end. `diff` is `balance_end_chain - balance_end_book`, so
    /// zero means the book and the chain agree. Times are unix seconds.
    FlowCheck {
        period_start: i64,
        period_end: i64,
        balance_start: Money,
        inflows: Money,
        outflows: Money,
        balance_end_book: Money,
        balance_end_chain: Money,
        diff: Money,
        note: String,
    },
    /// One step in the life of a transferable reservation record (docs/117
    /// §4): who issued it, how much of what, who controls it now and who did
    /// before this step.
    Reservation {
        reservation_id: String,
        issuer: String,
        unit: String,
        qty: u64,
        controller: String,
        prev_controller: Option<String>,
        action: ReservationAction,
        note: String,
    },
    /// Our meter against the provider's usage report for one window and one
    /// model (docs/117 §6), tokens in and out on each side. Times are unix
    /// seconds.
    MeterCheck {
        from: i64,
        to: i64,
        model: String,
        ours_in: u64,
        ours_out: u64,
        theirs_in: u64,
        theirs_out: u64,
        note: String,
    },
    /// A price table the agent priced against (docs/117 §7): where it came
    /// from, when (unix seconds), the hash of its rows and how many there were.
    PriceTable {
        source: String,
        fetched_at: i64,
        table_hash: String,
        rows: u32,
        note: String,
    },
}

/// What one [`EventBody::Reservation`] event did to the record. The
/// discriminant is the canonical byte, so a variant's number never changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationAction {
    Created = 0,
    TransferProposed = 1,
    TransferApplied = 2,
    TransferCancelled = 3,
    Consumed = 4,
    Refused = 5,
}

impl EventBody {
    pub fn kind(&self) -> &'static str {
        match self {
            EventBody::Observation { .. } => "OBSERVATION",
            EventBody::Decision { .. } => "DECISION",
            EventBody::Settlement { .. } => "SETTLEMENT",
            EventBody::Escalation { .. } => "ESCALATION",
            EventBody::Skipped { .. } => "SKIPPED",
            EventBody::Hedge { .. } => "HEDGE",
            EventBody::HedgeProposal { .. } => "PROPOSAL",
            EventBody::FlowCheck { .. } => "FLOWCHECK",
            EventBody::Reservation { .. } => "RESERVATION",
            EventBody::MeterCheck { .. } => "METERCHECK",
            EventBody::PriceTable { .. } => "PRICETABLE",
        }
    }

    /// The sentence a reviewer reads.
    pub fn note(&self) -> &str {
        match self {
            EventBody::Observation { note, .. }
            | EventBody::Decision { note, .. }
            | EventBody::Settlement { note, .. }
            | EventBody::Escalation { note, .. }
            | EventBody::Skipped { note, .. }
            | EventBody::Hedge { note, .. }
            | EventBody::HedgeProposal { note, .. }
            | EventBody::FlowCheck { note, .. }
            | EventBody::Reservation { note, .. }
            | EventBody::MeterCheck { note, .. }
            | EventBody::PriceTable { note, .. } => note,
        }
    }
}

/// One immutable entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub seq: u64,
    pub at: Timestamp,
    pub body: EventBody,
}

impl Event {
    /// The id a model or a reader cites this event by: `evt:<seq>`.
    pub fn id(&self) -> String {
        format!("evt:{}", self.seq)
    }

    /// Byte-exact serialisation, stable across processes and machines.
    pub fn canonical(&self) -> Vec<u8> {
        let mut e = Encoder::new();
        e.str("air-log/v0.1");
        e.u64(self.seq);
        e.u64(self.at.as_unix() as u64);
        match &self.body {
            EventBody::Observation {
                balance,
                transfers_seen,
                obligations_found,
                total_due,
                note,
            } => {
                e.tag(1)
                    .money(*balance)
                    .u64(*transfers_seen as u64)
                    .u64(*obligations_found as u64)
                    .money(*total_due)
                    .str(note);
            }
            EventBody::Decision {
                counterparty,
                amount,
                decision,
                action_hash,
                confidence,
                balance_seen,
                weakest_evidence,
                evidence,
                note,
            } => {
                e.tag(2)
                    .str(counterparty.as_str())
                    .money(*amount)
                    .tag(match decision {
                        Decision::Allow => 1,
                        Decision::Review => 2,
                        Decision::Block => 3,
                    })
                    .bytes(action_hash.as_bytes())
                    // Confidence is written as its four-decimal fixed-point
                    // form. A raw f64 has platform-dependent bit patterns for
                    // the same decimal value, and a canonical encoding cannot
                    // depend on which machine produced it.
                    .u64((confidence.get() * 10_000.0).round() as u64)
                    .money(*balance_seen);
                match weakest_evidence {
                    None => {
                        e.tag(0);
                    }
                    Some(p) => {
                        e.tag(1).provenance(*p);
                    }
                }
                e.seq(evidence, |enc, r| {
                    enc.str(r);
                })
                .str(note);
            }
            EventBody::Settlement {
                counterparty,
                cycle_key,
                amount,
                tx_hash,
                action_hash,
                settlement_key,
                note,
            } => {
                e.tag(3)
                    .str(counterparty.as_str())
                    .str(cycle_key)
                    .money(*amount)
                    .str(tx_hash)
                    .bytes(action_hash.as_bytes())
                    .bytes(settlement_key.as_bytes())
                    .str(note);
            }
            EventBody::Escalation {
                counterparty,
                amount,
                reason,
                note,
            } => {
                e.tag(4)
                    .str(counterparty.as_str())
                    .money(*amount)
                    .str(reason)
                    .str(note);
            }
            EventBody::Skipped {
                counterparty,
                cycle_key,
                note,
            } => {
                e.tag(5).str(counterparty.as_str()).str(cycle_key).str(note);
            }
            EventBody::Hedge {
                coin,
                outcome,
                size_before,
                size_delta,
                size_after,
                mark_px,
                notional_after,
                oid,
                note,
            } => {
                e.tag(6)
                    .str(coin)
                    .str(outcome)
                    .str(size_before)
                    .str(size_delta)
                    .str(size_after)
                    .str(mark_px)
                    .money(*notional_after)
                    .opt_str(oid.as_deref())
                    .str(note);
            }
            EventBody::HedgeProposal {
                coin,
                advice,
                band_bps,
                reason,
                by,
                note,
            } => {
                e.tag(7).str(coin).str(advice);
                match band_bps {
                    None => {
                        e.tag(0);
                    }
                    Some(b) => {
                        e.tag(1).u64(*b);
                    }
                }
                e.str(reason).str(by).str(note);
            }
            // Unix seconds are written as their two's-complement u64, the
            // way `at` is above.
            EventBody::FlowCheck {
                period_start,
                period_end,
                balance_start,
                inflows,
                outflows,
                balance_end_book,
                balance_end_chain,
                diff,
                note,
            } => {
                e.tag(8)
                    .u64(*period_start as u64)
                    .u64(*period_end as u64)
                    .money(*balance_start)
                    .money(*inflows)
                    .money(*outflows)
                    .money(*balance_end_book)
                    .money(*balance_end_chain)
                    .money(*diff)
                    .str(note);
            }
            EventBody::Reservation {
                reservation_id,
                issuer,
                unit,
                qty,
                controller,
                prev_controller,
                action,
                note,
            } => {
                e.tag(9)
                    .str(reservation_id)
                    .str(issuer)
                    .str(unit)
                    .u64(*qty)
                    .str(controller)
                    .opt_str(prev_controller.as_deref())
                    .tag(*action as u8)
                    .str(note);
            }
            EventBody::MeterCheck {
                from,
                to,
                model,
                ours_in,
                ours_out,
                theirs_in,
                theirs_out,
                note,
            } => {
                e.tag(10)
                    .u64(*from as u64)
                    .u64(*to as u64)
                    .str(model)
                    .u64(*ours_in)
                    .u64(*ours_out)
                    .u64(*theirs_in)
                    .u64(*theirs_out)
                    .str(note);
            }
            EventBody::PriceTable {
                source,
                fetched_at,
                table_hash,
                rows,
                note,
            } => {
                e.tag(11)
                    .str(source)
                    .u64(*fetched_at as u64)
                    .str(table_hash)
                    .u64(u64::from(*rows))
                    .str(note);
            }
        }
        e.buf
    }

    /// This event's domain-prefixed Merkle leaf.
    pub fn leaf(&self) -> Hash32 {
        hash_leaf(&self.canonical())
    }
}

/// An append-only sequence of events, in memory.
#[derive(Debug, Clone, Default)]
pub struct DecisionLog {
    events: Vec<Event>,
}

impl DecisionLog {
    pub fn new() -> Self {
        DecisionLog { events: Vec::new() }
    }

    /// Rebuild from events read off disk. Sequence numbers are re-derived
    /// from position, so a file with gaps or duplicates cannot smuggle in a
    /// sequence that was never issued.
    pub fn from_events(events: impl IntoIterator<Item = Event>) -> Self {
        let events = events
            .into_iter()
            .enumerate()
            .map(|(i, mut e)| {
                e.seq = i as u64;
                e
            })
            .collect();
        DecisionLog { events }
    }

    /// Append. There is no corresponding removal, and no way to edit an
    /// entry already written.
    pub fn append(&mut self, at: Timestamp, body: EventBody) -> &Event {
        let seq = self.events.len() as u64;
        self.events.push(Event { seq, at, body });
        self.events.last().expect("just pushed")
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    fn leaves(&self) -> Vec<Hash32> {
        self.events.iter().map(|e| e.leaf()).collect()
    }

    /// 32-byte commitment to the entire log, ready for `MemoryAnchor`.
    ///
    /// An empty log commits to 32 zero bytes, which the anchor contract
    /// rejects, so an agent cannot accidentally anchor "I have done nothing".
    pub fn root(&self) -> Hash32 {
        merkle_root(&self.leaves())
    }

    /// Proof that event `seq` is in the current root.
    pub fn proof_for(&self, seq: usize) -> Option<Vec<ProofStep>> {
        if seq >= self.events.len() {
            return None;
        }
        Some(merkle_path(&self.leaves(), seq))
    }

    /// Cycle keys this log has already recorded a settlement for.
    ///
    /// *Agents and Ledgers* puts duplicate payment first among the failures an
    /// agent hits that a balanced ledger misses: a retry after a timeout pays
    /// the same bill twice and the books still reconcile. The defence is to
    /// refuse beforehand, not to notice afterwards, and this log is the only
    /// record that survives a process dying mid-transfer.
    pub fn settled_cycles(&self) -> BTreeSet<String> {
        self.events
            .iter()
            .filter_map(|e| match &e.body {
                EventBody::Settlement { cycle_key, .. } => Some(cycle_key.clone()),
                _ => None,
            })
            .collect()
    }

    /// Human-readable pass over the log, for the audit it exists for.
    pub fn replay(&self) -> impl Iterator<Item = String> + '_ {
        self.events
            .iter()
            .map(|e| format!("[{:04}] {:<11} {}", e.seq, e.body.kind(), e.body.note()))
    }
}

#[cfg(test)]
mod consistency_tests {
    use super::*;

    #[test]
    fn every_prefix_is_consistent_and_an_edited_one_is_not() {
        let leaves: Vec<Hash32> = (0u8..40).map(|i| hash_leaf(&[i])).collect();
        for n in 1..=leaves.len() {
            let second = merkle_root(&leaves[..n]);
            for m in 1..=n {
                let first = merkle_root(&leaves[..m]);
                let p = consistency_from_leaves(&leaves[..n], m).unwrap();
                assert!(verify_consistency(first, m, second, n, &p), "m={m} n={n}");
                // A first tree with a step edited or deleted does not extend.
                let mut edited = leaves[..m].to_vec();
                edited[m - 1] = hash_leaf(b"edited");
                assert!(
                    !verify_consistency(merkle_root(&edited), m, second, n, &p),
                    "m={m} n={n}"
                );
            }
        }
        assert!(consistency_from_leaves(&leaves, 0).is_none());
    }
}

#[cfg(test)]
mod audit_helper_tests {
    use super::*;
    use crate::money::Currency;

    #[test]
    fn a_root_recomputed_from_canonical_bytes_matches_the_log() {
        let mut log = DecisionLog::new();
        for i in 0..5u64 {
            log.append(
                Timestamp::from_unix(1_700_000_000 + i as i64),
                EventBody::Observation {
                    balance: Money::parse(&format!("10.{i:02}"), Currency::Usdc).unwrap(),
                    transfers_seen: i as usize,
                    obligations_found: 0,
                    total_due: Money::parse("0", Currency::Usdc).unwrap(),
                    note: format!("obs {i}"),
                },
            );
        }
        let leaves: Vec<Hash32> = log
            .events()
            .iter()
            .map(|e| leaf_from_canonical(&e.canonical()))
            .collect();
        assert_eq!(root_from_leaves(&leaves), log.root());
        let path = path_from_leaves(&leaves, 3).unwrap();
        assert!(verify_inclusion(leaves[3], &path, log.root()));
        assert!(path_from_leaves(&leaves, 5).is_none());
        let hex = log.root().to_hex();
        assert_eq!(Hash32::from_hex(&hex), Some(log.root()));
        assert_eq!(Hash32::from_hex("0x12"), None);
        assert_eq!(Hash32::from_hex(&hex[2..]), Some(log.root()));
    }
}
