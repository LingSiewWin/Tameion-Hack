//! A symbolon: one record and the three signatures that can stand behind it.
//!
//! In Athens a symbolon was a token broken in two; each party kept a half,
//! and the halves fitting together proved the agreement. Here a record (a
//! settlement, an invoice, a decision, a flow, meter or price check) has
//! three slots: the agent's signature, the counterparty's, and a witness's.
//! A slot is filled only by an artefact someone else can check again: a
//! signature the operator's verifier accepted, or for the witness a chain
//! receipt, a `rootOf` match, a provider's bucket or a balance read that was
//! read back. This module performs no I/O and verifies nothing itself: the
//! operator checks, and passes only what held in [`Extras`].
//!
//! The id is hash-first, `keccak256(record_hash || kind)`. No slot ever
//! enters it, so adding a signature never changes which record it is.

use crate::log::{Event, EventBody};
use sha3::{Digest, Keccak256};
use std::collections::BTreeMap;

/// Who a slot is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Agent,
    Counterparty,
    Witness,
}

/// One filled slot. `alg` is a registry id: `p256-webauthn`,
/// `secp256k1-eip712` or `ml-dsa-65` for a signature; `chain-receipt`,
/// `provider-report` or `balance-read` for a witness read back. `key_id`
/// names the key or the witness: an address, `sha256(pk)`, `chain:421614`,
/// `anthropic:usage_report`, `gateway:<wallet>`. `sig` is the signature, or
/// the reference that was read back (a transaction hash). `at` is unix
/// seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotSig {
    pub alg: String,
    pub key_id: String,
    pub sig: Vec<u8>,
    pub at: i64,
}

/// What a record is. The discriminant is the byte the id hashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolonKind {
    Settlement = 1,
    /// A payee-signed invoice. No event carries one: the operator builds
    /// this kind from its invoice store; [`project`] never emits it.
    Invoice = 2,
    Decision = 3,
    FlowCheck = 4,
    MeterCheck = 5,
    PriceTable = 6,
}

/// One record and its three slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbolon {
    pub record_hash: [u8; 32],
    pub kind: SymbolonKind,
    pub agent: Option<SlotSig>,
    pub counterparty: Option<SlotSig>,
    pub witness: Option<SlotSig>,
}

/// How many slots hold a checkable artefact. `Unsigned` (none) is counted,
/// never hidden: a record nobody can check yet is part of the honest number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Signedness {
    Unsigned,
    Single,
    Double,
    Triple,
}

impl Symbolon {
    /// `keccak256(record_hash || kind)`: never a slot.
    pub fn id(&self) -> [u8; 32] {
        let mut h = Keccak256::new();
        h.update(self.record_hash);
        h.update([self.kind as u8]);
        h.finalize().into()
    }

    pub fn slot(&self, which: Slot) -> Option<&SlotSig> {
        match which {
            Slot::Agent => self.agent.as_ref(),
            Slot::Counterparty => self.counterparty.as_ref(),
            Slot::Witness => self.witness.as_ref(),
        }
    }

    /// The count of filled slots.
    pub fn signedness(&self) -> Signedness {
        let filled = [Slot::Agent, Slot::Counterparty, Slot::Witness]
            .into_iter()
            .filter(|s| self.slot(*s).is_some())
            .count();
        match filled {
            0 => Signedness::Unsigned,
            1 => Signedness::Single,
            2 => Signedness::Double,
            _ => Signedness::Triple,
        }
    }
}

/// What the operator verified or read back, keyed by the [`record_hash`] of
/// the record it stands behind. Only artefacts that held go in.
#[derive(Debug, Clone, Default)]
pub struct Extras {
    /// The agent's own signature over a record.
    pub agent: BTreeMap<[u8; 32], SlotSig>,
    /// A counterparty's signature: the payee-signed invoice a settlement paid.
    pub counterparty: BTreeMap<[u8; 32], SlotSig>,
    /// A record's own witness: its payment's receipt, the provider's bucket
    /// behind a meter check, the balance read behind a flow check.
    pub witness: BTreeMap<[u8; 32], SlotSig>,
    /// The anchoring of the root over these events, read back on chain: a
    /// witness for each of them that has no witness of its own.
    pub anchored: Option<SlotSig>,
}

/// An event's record hash: its Merkle leaf, the value an inclusion proof
/// against an anchored root starts from.
pub fn record_hash(e: &Event) -> [u8; 32] {
    *e.leaf().as_bytes()
}

/// One symbolon per settlement, decision, flow check, meter check and price
/// table in `events`, in order, with the slots `extras` fills. Other kinds
/// (observations, escalations, skips, hedges, reservations) are not records
/// anyone signs and are left out.
pub fn project(events: &[Event], extras: &Extras) -> Vec<Symbolon> {
    events
        .iter()
        .filter_map(|e| {
            let kind = match &e.body {
                EventBody::Settlement { .. } => SymbolonKind::Settlement,
                EventBody::Decision { .. } => SymbolonKind::Decision,
                EventBody::FlowCheck { .. } => SymbolonKind::FlowCheck,
                EventBody::MeterCheck { .. } => SymbolonKind::MeterCheck,
                EventBody::PriceTable { .. } => SymbolonKind::PriceTable,
                _ => return None,
            };
            let h = record_hash(e);
            Some(Symbolon {
                record_hash: h,
                kind,
                agent: extras.agent.get(&h).cloned(),
                counterparty: extras.counterparty.get(&h).cloned(),
                witness: extras.witness.get(&h).or(extras.anchored.as_ref()).cloned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::ActionHash;
    use crate::log::DecisionLog;
    use crate::money::{Currency, Money};
    use crate::obligations::Counterparty;
    use crate::time::Timestamp;

    fn sig(alg: &str, key_id: &str) -> SlotSig {
        SlotSig {
            alg: alg.into(),
            key_id: key_id.into(),
            sig: vec![0xab; 4],
            at: 1_760_659_200,
        }
    }

    fn bare(kind: SymbolonKind) -> Symbolon {
        Symbolon {
            record_hash: [0x11; 32],
            kind,
            agent: None,
            counterparty: None,
            witness: None,
        }
    }

    fn hex(b: [u8; 32]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn the_id_ignores_every_slot_and_pins_the_kind_byte() {
        let plain = bare(SymbolonKind::Settlement);
        let mut signed = plain.clone();
        signed.agent = Some(sig("secp256k1-eip712", "0xa1"));
        signed.counterparty = Some(sig("ml-dsa-65", "sha256:00"));
        signed.witness = Some(sig("chain-receipt", "chain:421614"));
        assert_eq!(plain.id(), signed.id(), "a slot never enters the id");
        // keccak256(0x11 x 32 || 0x01), computed with `cast keccak`.
        assert_eq!(
            hex(plain.id()),
            "5e5fbb0202ce2fe377f6cbe704e21d3767fc8838a4b270e59b9093b355c852a9"
        );
        // ... and || 0x03: the same record hash as a decision is another record.
        assert_eq!(
            hex(bare(SymbolonKind::Decision).id()),
            "b8a22b8b7e6dc271b3fcd32f3b6876569644744f4b1d2a187ac319d13f06920a"
        );
    }

    #[test]
    fn signedness_counts_the_filled_slots() {
        let mut s = bare(SymbolonKind::MeterCheck);
        assert_eq!(s.signedness(), Signedness::Unsigned);
        s.witness = Some(sig("provider-report", "anthropic:usage_report"));
        assert_eq!(s.signedness(), Signedness::Single);
        s.agent = Some(sig("secp256k1-eip712", "0xa1"));
        assert_eq!(s.signedness(), Signedness::Double);
        s.counterparty = Some(sig("secp256k1-eip712", "0xb2"));
        assert_eq!(s.signedness(), Signedness::Triple);
        assert_eq!(s.slot(Slot::Counterparty).unwrap().key_id, "0xb2");
    }

    #[test]
    fn a_settlement_with_a_receipt_then_an_invoice_then_the_agent_is_single_double_triple() {
        let usdc = |s: &str| Money::parse(s, Currency::Usdc).unwrap();
        let mut log = DecisionLog::new();
        log.append(
            Timestamp::from_unix(1_760_659_200),
            EventBody::Observation {
                balance: usdc("10"),
                transfers_seen: 1,
                obligations_found: 1,
                total_due: usdc("1"),
                note: "saw".into(),
            },
        );
        log.append(
            Timestamp::from_unix(1_760_659_205),
            EventBody::Settlement {
                counterparty: Counterparty::new("0xb2"),
                cycle_key: "0xb2@1760659200".into(),
                amount: usdc("1"),
                tx_hash: "0xfeed".into(),
                action_hash: ActionHash::from_bytes([1; 32]),
                settlement_key: ActionHash::from_bytes([2; 32]),
                note: "paid".into(),
            },
        );
        let paid = &log.events()[1];
        let h = record_hash(paid);
        assert_eq!(h, *paid.leaf().as_bytes());

        let mut extras = Extras::default();
        assert_eq!(
            project(log.events(), &extras)
                .iter()
                .map(Symbolon::signedness)
                .collect::<Vec<_>>(),
            [Signedness::Unsigned],
            "an observation is not a record; an unwitnessed settlement is counted"
        );
        extras
            .witness
            .insert(h, sig("chain-receipt", "chain:421614"));
        let one = project(log.events(), &extras);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].kind, SymbolonKind::Settlement);
        assert_eq!(one[0].signedness(), Signedness::Single);
        assert_eq!(one[0].witness.as_ref().unwrap().alg, "chain-receipt");
        extras
            .counterparty
            .insert(h, sig("secp256k1-eip712", "0xb2"));
        assert_eq!(
            project(log.events(), &extras)[0].signedness(),
            Signedness::Double
        );
        extras.agent.insert(h, sig("secp256k1-eip712", "0xa1"));
        let three = project(log.events(), &extras);
        assert_eq!(three[0].signedness(), Signedness::Triple);
        assert_eq!(three[0].id(), one[0].id(), "same record, more signatures");

        // The anchoring witnesses every record without one of its own, and
        // never replaces a record's own witness.
        let anchored = Extras {
            anchored: Some(sig("chain-receipt", "chain:5042002")),
            ..Extras::default()
        };
        assert_eq!(
            project(log.events(), &anchored)[0]
                .witness
                .as_ref()
                .unwrap()
                .key_id,
            "chain:5042002"
        );
        let both = Extras {
            anchored: Some(sig("chain-receipt", "chain:5042002")),
            ..extras
        };
        assert_eq!(
            project(log.events(), &both)[0]
                .witness
                .as_ref()
                .unwrap()
                .key_id,
            "chain:421614"
        );
    }
}
