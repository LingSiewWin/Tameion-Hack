//! The symbolon as `ledger::symbolon` defines it (docs/117 §13): one record,
//! three signature slots, and nothing else. A copy until that module lands on
//! main; then this file goes and `lib.rs` re-exports the ledger's types.
//! Only what the fold reads is copied.

/// One signature in one slot. `alg` is a registry id (`p256-webauthn`,
/// `secp256k1-eip712`, `ml-dsa-65`, `chain-receipt`, `provider-report`,
/// `balance-read`); `key_id` names the key or the witness (an address,
/// `chain:421614`, `anthropic:usage_report`, `gateway:<wallet>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotSig {
    pub alg: String,
    pub key_id: String,
    pub sig: Vec<u8>,
    pub at: i64,
}

/// What the record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolonKind {
    Settlement,
    Invoice,
    Decision,
    FlowCheck,
    MeterCheck,
    PriceTable,
}

/// A record and its three slots. A slot holds a signature only when it was
/// verified (a signature the registry accepts, or a witness read back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbolon {
    pub record_hash: [u8; 32],
    pub kind: SymbolonKind,
    pub agent: Option<SlotSig>,
    pub counterparty: Option<SlotSig>,
    pub witness: Option<SlotSig>,
}
