//! The desk inside the service (`docs/83` W5): milestones signed by the party
//! who knows them, one book per desk on disk, the offset on the venue driven
//! from the book, and the lender's statement signed by the statement key.
//!
//! Trust follows the surface rule. Approving a counterparty (a name bound to
//! the address that signs for it) is the owner's decision. After that, a
//! supplier's purchase confirmation, a carrier's bill of lading or a buyer's
//! sale confirmation arrives signed and is booked without anyone tapping: the
//! signature is checked against the approved address, and a milestone the
//! owner types in without a signature is marked as such on every statement.

use ::desk::{Desk, Goods, Milestone, MilestoneKind, Statement, StatementInputs};
use alloy::primitives::{Address, B256, Signature, keccak256};
use alloy::signers::{SignerSync, local::PrivateKeySigner};
use alloy::sol_types::{SolStruct, eip712_domain};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

alloy::sol! {
    /// What a supplier, carrier or buyer signs for one milestone.
    struct ShipmentEvent {
        string deal;
        string kind;
        string goods;
        string quantity;
        string usdc;
        string counterparty;
        string document;
        uint64 at;
    }

    /// What the statement key signs for one deal: the statement's hash and
    /// the root of the desk it was computed from.
    struct DeskStatement {
        string deal;
        bytes32 root;
        bytes32 statementHash;
        uint64 at;
    }
}

pub const DOMAIN_NAME: &str = "Tameion Desk";
/// The statement key's domain (statements carry the desk's root).
pub const DOMAIN_VERSION: &str = "1";
/// A counterparty's step: version 2 binds it to one desk through
/// `verifyingContract` = the desk's account, so a step signed for one
/// trader cannot be replayed on another's desk (docs/89 H3).
pub const EVENT_DOMAIN_VERSION: &str = "2";

fn domain(chain_id: u64) -> alloy::sol_types::Eip712Domain {
    eip712_domain! { name: DOMAIN_NAME, version: DOMAIN_VERSION, chain_id: chain_id, }
}

fn event_domain(chain_id: u64, desk: Address) -> alloy::sol_types::Eip712Domain {
    eip712_domain! { name: DOMAIN_NAME, version: EVENT_DOMAIN_VERSION, chain_id: chain_id, verifying_contract: desk, }
}

/// The EIP-712 digest a counterparty signs for `m` on the desk `desk`.
pub fn event_digest(chain_id: u64, desk: Address, m: &Milestone) -> B256 {
    ShipmentEvent {
        deal: m.deal.clone(),
        kind: m.kind.name().into(),
        goods: m.goods.name().into(),
        quantity: m.quantity.clone(),
        usdc: m.usdc.clone().unwrap_or_default(),
        counterparty: m.counterparty.clone(),
        document: m.document.clone(),
        at: u64::try_from(m.at).unwrap_or(0),
    }
    .eip712_signing_hash(&event_domain(chain_id, desk))
}

/// Sign `m` as its counterparty for the desk `desk`. `0x` + 65 bytes.
pub fn sign_event(
    chain_id: u64,
    desk: Address,
    m: &Milestone,
    key: &PrivateKeySigner,
) -> Result<String, String> {
    let sig = key
        .sign_hash_sync(&event_digest(chain_id, desk, m))
        .map_err(|e| e.to_string())?;
    Ok(format!("0x{}", alloy::hex::encode(sig.as_bytes())))
}

/// Who signed `m` for the desk `desk`.
pub fn recover_event(
    chain_id: u64,
    desk: Address,
    m: &Milestone,
    sig: &str,
) -> Result<Address, String> {
    let raw =
        alloy::hex::decode(sig.trim_start_matches("0x")).map_err(|e| format!("signature: {e}"))?;
    let s = Signature::from_raw(&raw).map_err(|e| format!("signature: {e}"))?;
    s.recover_address_from_prehash(&event_digest(chain_id, desk, m))
        .map_err(|e| format!("signature: {e}"))
}

/// A counterparty the owner approved: a name and the address that signs for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Party {
    pub name: String,
    pub address: String,
    /// The owner's passkey assertion over [`party_challenge`] and when it was
    /// issued (docs/89 M5): adding a signer is a trust decision, like adding
    /// a payee, so it carries the owner's own signature, kept for audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Value>,
    /// The kinds of step it may sign (docs/91 R7): a vault vouches for what
    /// came in, not for a sale. `None` = any kind (approved before roles).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<MilestoneKind>>,
}

const KINDS: [MilestoneKind; 7] = [
    MilestoneKind::Bought,
    MilestoneKind::Shipped,
    MilestoneKind::Arrived,
    MilestoneKind::ToFabrication,
    MilestoneKind::Consigned,
    MilestoneKind::Returned,
    MilestoneKind::Sold,
];

/// A kind as the book writes it ("to_fabrication").
pub fn kind_name(k: MilestoneKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// One bit per kind, in the order of the journey: bought = 1 … sold = 64.
pub fn kinds_mask(kinds: &[MilestoneKind]) -> u8 {
    KINDS
        .iter()
        .enumerate()
        .filter(|(_, k)| kinds.contains(k))
        .fold(0, |m, (i, _)| m | (1 << i))
}

/// [`party_challenge`] for a counterparty limited to some kinds of step
/// (docs/91 R7): `keccak256("tameion.desk.party.v2" ‖ chainId u64 ‖ policy ‖
/// len(name) u32 ‖ name ‖ address ‖ kinds mask u8 ‖ issued u64)`.
pub fn party_challenge_v2(
    chain_id: u64,
    policy: Address,
    name: &str,
    address: Address,
    kinds: &[MilestoneKind],
    issued: u64,
) -> B256 {
    let mut m = tagged(b"tameion.desk.party.v2", chain_id, policy);
    with_str(&mut m, name);
    m.extend_from_slice(address.as_slice());
    m.push(kinds_mask(kinds));
    m.extend_from_slice(&issued.to_be_bytes());
    keccak256(m)
}

/// What the owner's passkey signs to approve `address` as `name` on the desk
/// `policy`: `keccak256("tameion.desk.party.v1" ‖ chainId u64 ‖ policy ‖
/// len(name) u32 ‖ name ‖ address ‖ issued u64)`; the console computes the
/// same bytes (`apps/console/src/lib/guard.ts`).
pub fn party_challenge(
    chain_id: u64,
    policy: Address,
    name: &str,
    address: Address,
    issued: u64,
) -> B256 {
    let mut m = b"tameion.desk.party.v1".to_vec();
    m.extend_from_slice(&chain_id.to_be_bytes());
    m.extend_from_slice(policy.as_slice());
    m.extend_from_slice(&(name.len() as u32).to_be_bytes());
    m.extend_from_slice(name.as_bytes());
    m.extend_from_slice(address.as_slice());
    m.extend_from_slice(&issued.to_be_bytes());
    keccak256(m)
}

/// One line of a book file, in the order it happened.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Line {
    Party(Party),
    /// The owner's unhedged base for a good (`Desk::set_base`).
    Policy {
        goods: Goods,
        #[serde(rename = "unhedgedBase")]
        unhedged_base: String,
        /// The owner's passkey over [`base_challenge`] (docs/89 M5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval: Option<Value>,
    },
    Milestone {
        milestone: Milestone,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        signature: String,
        /// For the owner's own (unsigned) step: the owner's passkey over
        /// [`step_challenge`] (docs/89 M5). A counterparty's step carries its
        /// signature instead.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<Value>,
        /// The venue's mark when the step was booked (USDC per venue unit), as
        /// the service read it. Nobody signs it: it only names a price far from
        /// the market on the deal card ([`price_note`]), never moves the book.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mark: Option<String>,
    },
    /// The owner stops a counterparty (docs/91 R4). Trust shrinks at once, so
    /// the device token is enough. Steps it signed before stay booked; a later
    /// one is refused. Approving the name again (passkey) restores it.
    Revoke {
        name: String,
        at: u64,
    },
    /// What the offset moved, in grams, and whether the owner's passkey
    /// allowed it (docs/91 R3): `auto` moves add up until the owner confirms
    /// one, and the desk moves by itself only while that sum stays inside the
    /// owner's limit.
    Moved {
        grams: String,
        auto: bool,
        at: u64,
        /// The owner's passkey over [`offset_challenge`] when the owner
        /// released the move: kept so it is never good twice (docs/94 U8).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval: Option<Value>,
    },
    /// Who holds the venue's master key, in the owner's words (`docs/87`
    /// option A: "master key held by <lender>"). Printed on every statement
    /// beside what the desk itself knows about its key.
    Control {
        text: String,
        /// The owner's passkey over [`control_challenge`] (docs/89 M5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval: Option<Value>,
    },
}

fn tagged(tag: &[u8], chain_id: u64, policy: Address) -> Vec<u8> {
    let mut m = tag.to_vec();
    m.extend_from_slice(&chain_id.to_be_bytes());
    m.extend_from_slice(policy.as_slice());
    m
}

fn with_str(m: &mut Vec<u8>, s: &str) {
    m.extend_from_slice(&(s.len() as u32).to_be_bytes());
    m.extend_from_slice(s.as_bytes());
}

/// What the owner's passkey signs to set the unhedged base:
/// `keccak256("tameion.desk.base.v1" ‖ chainId u64 ‖ policy ‖ len u32 ‖ goods ‖
/// len u32 ‖ grams ‖ issued u64)` (`apps/console/src/lib/guard.ts`).
pub fn base_challenge(
    chain_id: u64,
    policy: Address,
    goods: &str,
    grams: &str,
    issued: u64,
) -> B256 {
    let mut m = tagged(b"tameion.desk.base.v1", chain_id, policy);
    with_str(&mut m, goods);
    with_str(&mut m, grams);
    m.extend_from_slice(&issued.to_be_bytes());
    keccak256(m)
}

/// What the owner's passkey signs to state who holds the master key:
/// `keccak256("tameion.desk.control.v1" ‖ chainId u64 ‖ policy ‖ len u32 ‖ text ‖ issued u64)`.
pub fn control_challenge(chain_id: u64, policy: Address, text: &str, issued: u64) -> B256 {
    let mut m = tagged(b"tameion.desk.control.v1", chain_id, policy);
    with_str(&mut m, text);
    m.extend_from_slice(&issued.to_be_bytes());
    keccak256(m)
}

/// What the owner's passkey signs to enter a step of their own: the step's
/// EIP-712 digest (bound to this desk, [`event_digest`]) under a tag:
/// `keccak256("tameion.desk.step.v1" ‖ chainId u64 ‖ policy ‖ digest ‖ issued u64)`.
pub fn step_challenge(chain_id: u64, policy: Address, digest: B256, issued: u64) -> B256 {
    let mut m = tagged(b"tameion.desk.step.v1", chain_id, policy);
    m.extend_from_slice(digest.as_slice());
    m.extend_from_slice(&issued.to_be_bytes());
    keccak256(m)
}

/// What the owner's passkey signs to release an offset larger than the desk
/// may move by itself (docs/91 R6): the target short, as the page shows it.
/// `keccak256("tameion.desk.offset.v1" ‖ chainId u64 ‖ policy ‖ len u32 ‖ target ‖ issued u64)`.
pub fn offset_challenge(chain_id: u64, policy: Address, target: &str, issued: u64) -> B256 {
    let mut m = tagged(b"tameion.desk.offset.v1", chain_id, policy);
    with_str(&mut m, target);
    m.extend_from_slice(&issued.to_be_bytes());
    keccak256(m)
}

/// Grams the venue trades to bring a position of `pos` contracts (signed: a
/// short is negative) to a short of `target` contracts. A long counts in full:
/// the engine sells it and then opens the short (docs/91 R13).
pub fn trade_grams(
    goods: Goods,
    target: rust_decimal::Decimal,
    pos: rust_decimal::Decimal,
) -> rust_decimal::Decimal {
    (target + pos).abs() * goods.units_per_contract()
}

/// A desk's book on disk: append-only JSON lines, replayed and re-verified
/// on every open, so a hand-edited file is refused rather than believed.
pub struct Book {
    pub path: PathBuf,
    pub chain_id: u64,
    /// The desk's account: every counterparty signature is bound to it.
    pub desk_id: Address,
    pub parties: Vec<Party>,
    pub desk: Desk,
    /// The counterparty's signature for each milestone, in the desk's order
    /// (empty for the owner's own entries), so a lender can check them.
    pub signatures: Vec<String>,
    /// The venue's mark seen when each milestone was booked, in the same order
    /// (`Line::Milestone::mark`; `None` for steps booked before it was kept).
    pub marks: Vec<Option<rust_decimal::Decimal>>,
    /// The owner's statement of who holds the venue's master key (`Line::Control`).
    pub control: Option<String>,
    /// Counterparties the owner stopped (`Line::Revoke`).
    pub revoked: std::collections::HashSet<String>,
    /// Grams the offset moved without the owner's passkey since the owner
    /// last confirmed a move (`Line::Moved`).
    pub auto_moved: rust_decimal::Decimal,
    /// The latest `issued` of each kind of owner approval in the book
    /// ("party", "base", "control"): an older one is a replay (docs/91 R11).
    last_issued: std::collections::HashMap<&'static str, u64>,
    /// Every owner challenge already used in this book: an approval is good
    /// once, across restarts (docs/92 T7).
    spent: std::collections::HashSet<String>,
    /// Every step already booked, by its EIP-712 digest: the same signed
    /// step is never booked twice (docs/89 H3).
    booked: std::collections::HashSet<B256>,
}

impl Book {
    pub fn open(path: &Path, chain_id: u64, desk_id: Address) -> Result<Book, String> {
        let mut b = Book {
            path: path.to_path_buf(),
            chain_id,
            desk_id,
            booked: std::collections::HashSet::new(),
            parties: Vec::new(),
            desk: Desk::new(),
            signatures: Vec::new(),
            marks: Vec::new(),
            control: None,
            revoked: std::collections::HashSet::new(),
            auto_moved: rust_decimal::Decimal::ZERO,
            last_issued: std::collections::HashMap::new(),
            spent: std::collections::HashSet::new(),
        };
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(b),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        for (n, raw) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let line: Line = serde_json::from_str(raw)
                .map_err(|e| format!("{} line {}: {e}", path.display(), n + 1))?;
            match &line {
                Line::Party(p) => b.saw("party", &p.approval),
                Line::Policy { approval, .. } => b.saw("base", approval),
                Line::Control { approval, .. } => b.saw("control", approval),
                _ => {}
            }
            match line {
                Line::Party(p) => b
                    .bind(p, false)
                    .map_err(|e| format!("line {}: {e}", n + 1))?,
                Line::Control { text, .. } => b.control = Some(text),
                Line::Revoke { name, at } => {
                    b.revoked.insert(name);
                    b.stopped_at(at);
                }
                Line::Moved {
                    grams,
                    auto,
                    approval,
                    ..
                } => {
                    b.saw("offset", &approval);
                    b.note_moved(grams.parse().unwrap_or_default(), auto)
                }
                Line::Policy {
                    goods,
                    unhedged_base,
                    ..
                } => b
                    .desk
                    .set_base(goods, &unhedged_base)
                    .map_err(|e| format!("line {}: {e}", n + 1))?,
                Line::Milestone {
                    milestone,
                    signature,
                    mark,
                    ..
                } => {
                    b.check(&milestone, &signature)
                        .map_err(|e| format!("line {}: {e}", n + 1))?;
                    b.desk
                        .apply(milestone)
                        .map_err(|e| format!("line {}: {e}", n + 1))?;
                    let digest = event_digest(
                        b.chain_id,
                        b.desk_id,
                        b.desk.events().last().expect("just applied"),
                    );
                    if !b.booked.insert(digest) {
                        return Err(format!("line {}: a step booked twice", n + 1));
                    }
                    b.signatures.push(signature);
                    b.marks.push(mark.and_then(|k| k.parse().ok()));
                }
            }
        }
        Ok(b)
    }

    fn party(&self, name: &str) -> Option<&Party> {
        self.parties.iter().find(|p| p.name == name)
    }

    fn bind(&mut self, p: Party, write: bool) -> Result<(), String> {
        let addr = Address::from_str(&p.address).map_err(|e| format!("address: {e}"))?;
        let p = Party {
            name: p.name.trim().to_string(),
            address: format!("{addr:#x}"),
            approval: p.approval,
            kinds: p.kinds,
        };
        if p.name.is_empty() || p.name.len() > 80 || p.name.chars().any(char::is_control) {
            return Err("a counterparty needs a short name".into());
        }
        // A new name that reads like a live counterparty's (docs/103 S2-2: an invisible
        // character, a look-alike letter) is that name: another key under it is refused,
        // as under the exact name. Books already written replay as they were.
        if write {
            let want = crate::parties::name_key(&p.name);
            if let Some(o) = self.parties.iter().find(|o| {
                o.name != p.name
                    && o.address != p.address
                    && !self.revoked.contains(&o.name)
                    && crate::parties::name_key(&o.name) == want
            }) {
                return Err(format!(
                    "{} reads like {}, already approved as another key; approve it under a name that reads differently",
                    p.name, o.name
                ));
            }
        }
        // A stopped counterparty may be approved again, with its old key or a
        // new one: the owner's passkey signed this line as for a new one.
        // The same signer with other kinds is a new approval of its role.
        let again = self.revoked.contains(&p.name)
            || self
                .party(&p.name)
                .is_some_and(|old| old.address == p.address && old.kinds != p.kinds);
        match self.party(&p.name) {
            _ if again => {}
            Some(old) if old.address == p.address => return Ok(()),
            Some(old) => {
                return Err(format!(
                    "{} is already approved as {}; a different signer is a new counterparty, approve it under its own name",
                    p.name, old.address
                ));
            }
            None => {}
        }
        if write {
            self.append(&Line::Party(p.clone()))?;
        }
        if again {
            self.parties.retain(|x| x.name != p.name);
            self.revoked.remove(&p.name);
        }
        self.parties.push(p);
        Ok(())
    }

    /// The owner approves a counterparty. A trust decision: the route that
    /// calls this has checked the owner's passkey over [`party_challenge`]
    /// and passes the assertion in, to be kept with the line.
    pub fn approve(
        &mut self,
        name: &str,
        address: &str,
        approval: Option<Value>,
    ) -> Result<(), String> {
        self.approve_for(name, address, None, approval)
    }

    /// [`Book::approve`] for some kinds of step only (docs/91 R7); the route
    /// has checked the owner's passkey over [`party_challenge_v2`].
    pub fn approve_for(
        &mut self,
        name: &str,
        address: &str,
        kinds: Option<Vec<MilestoneKind>>,
        approval: Option<Value>,
    ) -> Result<(), String> {
        if kinds.as_ref().is_some_and(Vec::is_empty) {
            return Err("name at least one kind of step they may sign".into());
        }
        self.fresh("party", &approval)?;
        self.bind(
            Party {
                name: name.into(),
                address: address.into(),
                approval: approval.clone(),
                kinds,
            },
            true,
        )?;
        self.saw("party", &approval);
        Ok(())
    }

    /// An owner approval older than the last of its kind in this book is a
    /// replay: the service forgets spent challenges when it restarts, the
    /// book does not (docs/91 R11). Same second is allowed: a script approves
    /// three counterparties in one.
    fn fresh(&self, kind: &'static str, approval: &Option<Value>) -> Result<(), String> {
        if approval
            .as_ref()
            .and_then(|a| a["challenge"].as_str())
            .is_some_and(|c| self.spent.contains(&c.to_lowercase()))
        {
            return Err("this approval was already used on this desk; sign it again".into());
        }
        let issued = approval.as_ref().and_then(|a| a["issued"].as_u64());
        match (issued, self.last_issued.get(kind)) {
            (Some(i), Some(&last)) if i < last => {
                Err("this approval is older than your last one of its kind; sign it again".into())
            }
            _ => Ok(()),
        }
    }

    fn saw(&mut self, kind: &'static str, approval: &Option<Value>) {
        if let Some(c) = approval.as_ref().and_then(|a| a["challenge"].as_str()) {
            self.spent.insert(c.to_lowercase());
        }
        if let Some(i) = approval.as_ref().and_then(|a| a["issued"].as_u64()) {
            let last = self.last_issued.entry(kind).or_default();
            *last = (*last).max(i);
        }
    }

    /// The owner stops a counterparty: its signature books nothing from now on.
    pub fn revoke(&mut self, name: &str, at: u64) -> Result<(), String> {
        let name = name.trim();
        if self.party(name).is_none() {
            return Err(format!(
                "{name} is not an approved counterparty on this desk"
            ));
        }
        if !self.revoked.contains(name) {
            self.append(&Line::Revoke {
                name: name.into(),
                at,
            })?;
            self.revoked.insert(name.into());
            self.stopped_at(at);
        }
        Ok(())
    }

    /// A stop moves the party marker: no approval issued before it re-approves
    /// a counterparty (docs/92 T7).
    fn stopped_at(&mut self, at: u64) {
        let last = self.last_issued.entry("party").or_default();
        *last = (*last).max(at);
    }

    /// Record what the offset moved and whether the owner's passkey allowed it.
    pub fn moved(
        &mut self,
        grams: rust_decimal::Decimal,
        auto: bool,
        at: u64,
        approval: Option<Value>,
    ) -> Result<(), String> {
        let grams = grams.round_dp(3);
        self.append(&Line::Moved {
            grams: grams.to_string(),
            auto,
            at,
            approval: approval.clone(),
        })?;
        self.saw("offset", &approval);
        self.note_moved(grams, auto);
        Ok(())
    }

    /// Whether an owner approval's challenge was already used in this book
    /// (approvals, base, control, offset releases; docs/92 T7, docs/94 U8).
    pub fn is_spent(&self, approval: &Value) -> bool {
        approval["challenge"]
            .as_str()
            .is_some_and(|c| self.spent.contains(&c.to_lowercase()))
    }

    fn note_moved(&mut self, grams: rust_decimal::Decimal, auto: bool) {
        self.auto_moved = if auto {
            self.auto_moved + grams
        } else {
            rust_decimal::Decimal::ZERO
        };
    }

    /// The owner sets the stock kept unhedged because it is replaced every
    /// day anyway. A trust decision, like approving a counterparty.
    pub fn set_base(
        &mut self,
        goods: Goods,
        grams: &str,
        approval: Option<Value>,
    ) -> Result<(), String> {
        self.fresh("base", &approval)?;
        let mut next = self.desk.clone();
        next.set_base(goods, grams).map_err(|e| e.to_string())?;
        let issued = approval.clone();
        self.append(&Line::Policy {
            goods,
            unhedged_base: next
                .base(goods)
                .to_string()
                .trim_end_matches(" g gold")
                .to_string(),
            approval,
        })?;
        self.saw("base", &issued);
        self.desk = next;
        Ok(())
    }

    /// The owner states who holds the venue's master key. A trust decision.
    pub fn set_control(&mut self, text: &str, approval: Option<Value>) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() || text.len() > 200 || text.chars().any(char::is_control) {
            return Err("say in one short line who holds the venue's master key".into());
        }
        self.fresh("control", &approval)?;
        let issued = approval.clone();
        self.append(&Line::Control {
            text: text.into(),
            approval,
        })?;
        self.saw("control", &issued);
        self.control = Some(text.into());
        Ok(())
    }

    /// The signer a milestone must carry: none (owner-entered), or exactly the
    /// approved address of the counterparty it names.
    fn check(&self, m: &Milestone, signature: &str) -> Result<(), String> {
        if signature.is_empty() {
            if !m.signer.is_empty() {
                return Err("a milestone that names a signer carries its signature".into());
            }
            return Ok(());
        }
        let who = recover_event(self.chain_id, self.desk_id, m, signature)?;
        let party = self.party(&m.counterparty).ok_or_else(|| {
            format!(
                "{} is not an approved counterparty on this desk",
                m.counterparty
            )
        })?;
        if self.revoked.contains(&party.name) {
            return Err(format!(
                "{} was stopped on this desk; its signature books nothing until you approve it again",
                party.name
            ));
        }
        if let Some(kinds) = party.kinds.as_ref().filter(|k| !k.contains(&m.kind)) {
            let may: Vec<String> = kinds.iter().map(|k| kind_name(*k)).collect();
            return Err(format!(
                "{} may sign {} on this desk, not {}",
                party.name,
                may.join(", "),
                kind_name(m.kind)
            ));
        }
        if format!("{who:#x}") != party.address {
            return Err(format!(
                "signed by {who:#x}, but {} signs as {}",
                m.counterparty, party.address
            ));
        }
        if m.signer != party.address {
            return Err("the signer field does not match the signature".into());
        }
        Ok(())
    }

    /// Book a milestone. With a signature it must come from the approved
    /// counterparty; without one it is booked as the owner's own entry.
    /// Nothing is written unless the desk accepts it.
    pub fn record(&mut self, m: Milestone, signature: Option<&str>) -> Result<Milestone, String> {
        self.record_with(m, signature, None, None)
    }

    /// [`Book::record`], keeping the owner's passkey approval with an
    /// unsigned step (docs/89 M5) and the venue's mark seen at booking.
    pub fn record_with(
        &mut self,
        mut m: Milestone,
        signature: Option<&str>,
        owner: Option<Value>,
        mark: Option<rust_decimal::Decimal>,
    ) -> Result<Milestone, String> {
        let signature = signature.unwrap_or("").trim().to_string();
        m.signer = if signature.is_empty() {
            String::new()
        } else {
            format!(
                "{:#x}",
                recover_event(self.chain_id, self.desk_id, &m, &signature)?
            )
        };
        self.check(&m, &signature)?;
        let digest = event_digest(self.chain_id, self.desk_id, &m);
        if self.booked.contains(&digest) {
            return Err("this exact step is already booked; a signed step counts once".into());
        }
        let mut next = self.desk.clone();
        next.apply(m.clone()).map_err(|e| e.to_string())?;
        self.append(&Line::Milestone {
            milestone: m.clone(),
            signature: signature.clone(),
            owner,
            mark: mark.map(|k| k.to_string()),
        })?;
        self.desk = next;
        self.signatures.push(signature);
        self.marks.push(mark);
        self.booked.insert(digest);
        Ok(m)
    }

    fn append(&self, line: &Line) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        let text = serde_json::to_string(line).map_err(|e| e.to_string())?;
        writeln!(f, "{text}")
            .and_then(|_| f.sync_all())
            .map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

/// Where a tenant's book lives: `<app>/tenants/<policy>/desk.jsonl`.
pub fn book_path(app_dir: &Path, policy: &str) -> PathBuf {
    crate::level1::tenants_dir(app_dir)
        .join(policy.to_lowercase())
        .join("desk.jsonl")
}

/// The Hyperliquid testnet account the desk offsets on (`TAMEION_DESK_VENUE`).
/// One account for the service today: a per-customer trade-only key is the
/// founder's item (`docs/83` §8). `None` = no venue connected.
pub fn venue_account(policy: &str) -> Option<Address> {
    // One venue account for one desk (docs/89 C2): the service offsets only
    // for the account named in `TAMEION_DESK_POLICY`; every other account's
    // book is booked but never trades, and never claims this short.
    if let Ok(bound) = std::env::var("TAMEION_DESK_POLICY") {
        if !bound.trim().eq_ignore_ascii_case(policy.trim()) {
            return None;
        }
        return std::env::var("TAMEION_DESK_VENUE")
            .ok()
            .and_then(|a| Address::from_str(a.trim()).ok());
    }
    // Otherwise the desk its owner connected from the page: still one desk
    // for the one account that approved this Mac's trade-only key.
    let c = venue_claim()?;
    if !c["policy"].as_str()?.eq_ignore_ascii_case(policy.trim()) || c["connected"] == false {
        return None;
    }
    Address::from_str(c["account"].as_str()?).ok()
}

static APP_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Where the service keeps its files; set once at start (the venue claim lives there).
pub fn set_app_dir(dir: &Path) {
    let _ = APP_DIR.set(dir.to_path_buf());
}

/// `<app>/desk-venue.json`: the desk whose owner connected this Mac's exchange
/// account from the page (`POST /api/desk/venue`), with the owner's passkey.
fn venue_claim() -> Option<Value> {
    let p = APP_DIR.get()?.join("desk-venue.json");
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

/// What the owner's passkey signs to let the desk trade on an exchange account:
/// `keccak256("tameion.desk.venue.v1" ‖ chainId u64 ‖ policy ‖ account ‖ issued u64)`.
pub fn venue_challenge(chain_id: u64, policy: Address, account: Address, issued: u64) -> B256 {
    let mut m = tagged(b"tameion.desk.venue.v1", chain_id, policy);
    m.extend_from_slice(account.as_slice());
    m.extend_from_slice(&issued.to_be_bytes());
    keccak256(m)
}

/// This Mac's trade-only key and the one exchange account that approved it,
/// as Hyperliquid itself reports (`userRole`: an agent key serves one
/// account). Reads the key file only to derive the address.
pub async fn this_macs_agent() -> Result<(Address, Option<Address>), String> {
    let path = std::env::var("HEDGE_AGENT_KEY_FILE").unwrap_or_else(|_| {
        format!(
            "{}/.tameion/hedge-agent.key",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    agent_and_account(Path::new(&path)).await
}

/// The key `policy`'s desk trades with — its own when its owner connected the
/// account by `ApproveAgent` (docs/106), this Mac's otherwise — and the one
/// exchange account that approved it, as Hyperliquid reports.
pub async fn desk_agent(policy: &str) -> Result<(Address, Option<Address>), String> {
    let own = venue_claim()
        .filter(|c| {
            c["policy"]
                .as_str()
                .is_some_and(|p| p.eq_ignore_ascii_case(policy.trim()))
        })
        .and_then(|c| own_key_path(&c));
    match own {
        Some(p) => agent_and_account(&p).await,
        None => this_macs_agent().await,
    }
}

/// The desk's own key file when `claim` connected its account by
/// `ApproveAgent` and is still connected; `None` means this Mac's key.
fn own_key_path(claim: &Value) -> Option<PathBuf> {
    (claim["connected"] != false && claim["approval"]["approveAgent"].is_object())
        .then(|| {
            claim["policy"]
                .as_str()
                .map(crate::desk_live::agent_key_path)
        })
        .flatten()
}

/// The key the desk trades `account` with: its own when the claim connected
/// that account by `ApproveAgent`, else `None` (this Mac's key).
pub fn own_key_for(account: Address) -> Option<PathBuf> {
    venue_claim()
        .filter(|c| {
            c["account"]
                .as_str()
                .and_then(|a| Address::from_str(a).ok())
                == Some(account)
        })
        .and_then(|c| own_key_path(&c))
}

async fn agent_and_account(path: &Path) -> Result<(Address, Option<Address>), String> {
    let key = fs::read_to_string(path)
        .map_err(|_| format!("no trade-only key on this Mac ({})", path.display()))?;
    let agent = PrivateKeySigner::from_str(key.trim())
        .map_err(|_| "the trade-only key file does not hold a key".to_string())?
        .address();
    drop(key);
    let r: Value = reqwest::Client::new()
        .post("https://api.hyperliquid-testnet.xyz/info")
        .json(&json!({ "type": "userRole", "user": format!("{agent:#x}") }))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let user = (r["role"] == "agent")
        .then(|| {
            r["data"]["user"]
                .as_str()
                .and_then(|u| Address::from_str(u).ok())
        })
        .flatten();
    Ok((agent, user))
}

/// Connect `policy`'s desk to `account`, keeping the owner's approval. The
/// account belongs to the desk that claimed it first, and only a desk set up on
/// this Mac (`known`) may claim it first; a disconnect keeps the owner, so no
/// other account on the service can take it over (docs/92 T6). Moving it to
/// another desk is a hand edit of `<app>/desk-venue.json`.
pub fn connect_venue(
    policy: &str,
    account: Address,
    approval: Value,
    at: u64,
    known: &[String],
) -> Result<(), String> {
    let dir = APP_DIR
        .get()
        .ok_or("the service did not say where its files are")?;
    connect_venue_check(policy, &approval, known)?;
    let policy = policy.trim().to_lowercase();
    write_claim(
        dir,
        &json!({ "policy": policy, "account": format!("{account:#x}"), "approval": approval, "at": at, "connected": true }),
    )
}

/// Every refusal of [`connect_venue`], writing nothing: run it before anything
/// that cannot be taken back (the venue approving a key, docs/103 S3-2).
pub fn connect_venue_check(policy: &str, approval: &Value, known: &[String]) -> Result<(), String> {
    let policy = policy.trim().to_lowercase();
    // The same approval twice, or one issued before the owner's last disconnect, is a replay (docs/94 U8).
    if let Some(c) = venue_claim() {
        let used = c["approval"]["challenge"].as_str();
        if used.is_some() && used == approval["challenge"].as_str() {
            return Err("this approval was already used; sign again".into());
        }
        if approval["issued"]
            .as_u64()
            .zip(c["disconnectedAt"].as_u64())
            .is_some_and(|(i, d)| i < d)
        {
            return Err(
                "this approval was issued before the desk was disconnected; sign again".into(),
            );
        }
    }
    match venue_claim() {
        Some(c)
            if !c["policy"]
                .as_str()
                .unwrap_or("")
                .eq_ignore_ascii_case(&policy) =>
        {
            Err(format!(
                "this Mac's exchange account belongs to desk {}; moving it is done on this Mac by hand (runtime/app/desk-venue.json)",
                c["policy"].as_str().unwrap_or("?")
            ))
        }
        None if !known.contains(&policy) => Err(
            "only a desk set up on this Mac (scripts/desk_init.sh) can claim its exchange account"
                .into(),
        ),
        _ => Ok(()),
    }
}

/// The claim as it is now, to put back with [`restore_venue_claim`] when a
/// step after a new claim fails.
pub fn venue_claim_now() -> Option<Value> {
    venue_claim()
}

/// Put back a claim read with [`venue_claim_now`]; `None` removes the claim.
pub fn restore_venue_claim(old: Option<&Value>) -> Result<(), String> {
    let dir = APP_DIR
        .get()
        .ok_or("the service did not say where its files are")?;
    match old {
        Some(c) => write_claim(dir, c),
        None => match fs::remove_file(dir.join("desk-venue.json")) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
    }
}

/// Disconnect: the desk stops trading at once (trust shrinks) and keeps the
/// account, so only it can connect again. `false` when this desk does not hold it.
pub fn disconnect_venue(policy: &str, at: u64) -> Result<bool, String> {
    match (venue_claim(), APP_DIR.get()) {
        (Some(mut c), Some(dir))
            if c["policy"]
                .as_str()
                .is_some_and(|p| p.eq_ignore_ascii_case(policy.trim())) =>
        {
            c["connected"] = json!(false);
            c["disconnectedAt"] = json!(at);
            write_claim(dir, &c)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The desks set up on this Mac (`scripts/desk_init.sh` writes
/// `~/.tameion/desks/<name>.policy`; `TAMEION_DESKS_DIR` overrides the folder):
/// only one of them may claim this Mac's exchange account first (docs/92 T6).
pub fn desks_on_this_mac() -> Vec<String> {
    let dir = std::env::var("TAMEION_DESKS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".tameion/desks")
        });
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "policy"))
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .map(|p| p.trim().to_lowercase())
        .collect()
}

fn write_claim(dir: &Path, claim: &Value) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(dir.join("desk-venue.json"))
        .map_err(|e| e.to_string())?;
    f.write_all(claim.to_string().as_bytes())
        .map_err(|e| e.to_string())
}

/// The desk as the console reads it: every deal with where its goods are,
/// what each milestone says and who vouched for it, the exposure and the
/// short it implies, the approved counterparties and the root.
pub fn view(book: &Book, policy: &str) -> Result<Value, String> {
    let mut deals = Vec::new();
    for deal in book.desk.deals() {
        let h = book.desk.holding(&deal.id).map_err(|e| e.to_string())?;
        let total = h.total().map_err(|e| e.to_string())?;
        let milestones: Vec<Value> = book
            .desk
            .events()
            .iter()
            .zip(book.marks.iter().chain(std::iter::repeat(&None)))
            .filter(|(m, _)| m.deal == deal.id)
            .map(|(m, mark)| {
                json!({
                    "kind": m.kind, "quantity": m.quantity, "usdc": m.usdc, "counterparty": m.counterparty,
                    "document": m.document, "signer": m.signer, "signed": !m.signer.is_empty(), "at": m.at, "words": words(m),
                    "priceNote": mark.and_then(|k| price_note(m, k, price_band_bps())),
                })
            })
            .collect();
        deals.push(json!({
            "id": deal.id,
            "goods": deal.goods,
            "supplier": deal.supplier,
            "bought": deal.bought.to_string(),
            "cost": deal.cost.to_string(),
            "held": total.to_string(),
            "stage": h.stage(),
            "byStage": h.by_stage.iter().filter(|(_, m)| m.to_minor() > 0).map(|(st, m)| json!({ "stage": st, "words": st.words(), "class": st.class(), "grams": m.to_string() })).collect::<Vec<_>>(),
            "milestones": milestones,
        }));
    }
    let gold = Goods::Gold;
    Ok(json!({
        "deals": deals,
        "exposure": [{
            "goods": gold,
            "held": book.desk.exposure(gold).map_err(|e| e.to_string())?.to_string(),
            "market": gold.market(),
            "offsetTarget": book.desk.offset_target(gold).map_err(|e| e.to_string())?.round_dp(6).to_string(),
            "unhedgedBase": book.desk.base(gold).to_string(),
            "autoLimit": auto_limit(gold).to_string(),
            "autoMoved": format!("{} g", book.auto_moved.round_dp(3)),
        }],
        "parties": book.parties.iter().map(|p| json!({ "name": p.name, "address": p.address, "kinds": p.kinds, "stopped": book.revoked.contains(&p.name) })).collect::<Vec<_>>(),
        "root": book.desk.root().map_err(|e| e.to_string())?.to_hex(),
        "venueAccount": venue_account(policy).map(|a| format!("{a:#x}")),
        "control": book.control,
        "chainId": book.chain_id,
    }))
}

/// Anchor `root` on `c` with the relayer key: what a lender later finds in
/// the transaction's input and in `rootOf(relayer)`.
pub async fn anchor(c: &crate::watch::ChainPreset, root: B256) -> Result<Value, String> {
    let anchor = crate::level1::anchor_for(c)?;
    let key = crate::level1::relayer_key()?;
    let from = PrivateKeySigner::from_str(&key)
        .map_err(|e| e.to_string())?
        .address();
    let client =
        arc::ArcClient::with_signer(&crate::watch::rpc_for(c), &key).map_err(|e| e.to_string())?;
    drop(key);
    let tx = client
        .anchor_root(anchor, root)
        .await
        .map_err(|e| e.to_string())?;
    Ok(
        json!({ "chainId": c.chain_id, "contract": format!("{anchor:#x}"), "tx": format!("{tx:#x}"), "from": format!("{from:#x}") }),
    )
}

/// The anchor of a statement already issued over `root` on chain `chain_id`,
/// from the kits kept in `dir`: a root that has not moved is not anchored
/// again (docs/91 R15), so a statement asked twice spends the relayer's gas once.
pub fn anchored_before(dir: &Path, root: &str, chain_id: u64) -> Option<Value> {
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let kit: Value = serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok()?;
        let same = kit.pointer("/signed/statement/root")?.as_str()? == root
            && kit.pointer("/anchor/chainId")?.as_u64()? == chain_id;
        same.then(|| kit["anchor"].clone())
    })
}

/// Every earlier statement root of this desk, by the size of the book then,
/// from the kits kept in `dir`: what a lender holding one of them checks the
/// new root against (docs/89 H5 (3)).
pub fn earlier_roots(dir: &Path) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let kit: Value = serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok()?;
            Some((
                kit.pointer("/disclosure/leaves")?.as_u64()? as usize,
                kit.pointer("/signed/statement/root")?.as_str()?.to_string(),
            ))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Everything a lender needs for one deal, built from live venue reads:
/// the signed statement, the deal's milestones disclosed against the desk's
/// root, each with its counterparty's signature, the approved counterparties
/// and the transaction that anchored the root. `scripts/verify_desk.py`
/// checks it trusting only the chain and the venue.
pub async fn lender_kit(
    book: &Book,
    deal: &str,
    c: &crate::watch::ChainPreset,
    venue: Address,
    now: i64,
    reuse: Option<Value>,
    earlier: &[(usize, String)],
) -> Result<Value, String> {
    let goods = book
        .desk
        .deal(deal)
        .ok_or_else(|| format!("no deal {deal} on this desk"))?
        .goods;
    let since = book
        .desk
        .events()
        .iter()
        .find(|m| m.deal == deal)
        .map_or(now, |m| m.at);
    let mut inputs = statement_inputs(goods, venue, since, now).await?;
    if let Some(c) = &book.control {
        // The owner's sentence may end with its own full stop (docs/95 P8).
        inputs.hedge_control = format!(
            "{} The trader states: {}.",
            inputs.hedge_control,
            c.trim_end_matches('.')
        );
    }
    let st = book
        .desk
        .statement(deal, &inputs)
        .map_err(|e| e.to_string())?;
    // A short above the stock (a sale whose offset waits for the owner, a base raised) fails the lender's check m: the desk
    // does not sign what a lender must refuse (docs/101 Y2).
    let parse = |x: &str| rust_decimal::Decimal::from_str(x).unwrap_or_default();
    if parse(&st.short_contracts)
        > parse(&st.desk_held_contracts) + rust_decimal::Decimal::new(1, 3)
    {
        return Err(format!(
            "the exchange account is short {} oz, more than the {} oz the desk holds: bring the offset in line, then issue the statement",
            st.short_contracts, st.desk_held_contracts
        ));
    }
    let signed = sign_statement(c.chain_id, &st, &crate::statement::key()?)?;
    verify_statement(&signed)?;
    let disclosure = book.desk.disclose(deal).map_err(|e| e.to_string())?;
    let root = B256::from_str(&st.root).map_err(|e| e.to_string())?;
    // For each earlier statement of this desk, the path showing the book only
    // grew since (RFC 9162 consistency). Not filtered: a root this book does
    // not extend (a rewritten book) gets a path that fails, which is the point.
    let history: Vec<Value> = earlier
        .iter()
        .filter_map(|(m, r)| {
            let path = book.desk.consistency(*m).ok()??;
            Some(json!({ "leaves": m, "root": r, "consistency": path }))
        })
        .collect();
    Ok(json!({
        "signed": signed,
        "disclosure": disclosure,
        "milestones": signed_milestones(book, deal),
        "history": history,
        "parties": book.parties,
        // The desk every counterparty signature is bound to (EIP-712
        // verifyingContract, version 2).
        "desk": format!("{:#x}", book.desk_id),
        "anchor": match reuse {
            Some(a) => a,
            None => anchor(c, root).await?,
        },
    }))
}

/// One deal's milestones as booked, each with the counterparty's signature,
/// for the lender's kit: with these a lender re-checks every signature
/// against the EIP-712 `ShipmentEvent` and the approved address, instead of
/// reading "signed by" in the desk's own words.
pub fn signed_milestones(book: &Book, deal: &str) -> Vec<Value> {
    book.desk
        .events()
        .iter()
        .zip(&book.signatures)
        .filter(|(m, _)| m.deal == deal)
        .map(|(m, sig)| json!({ "milestone": m, "signature": sig }))
        .collect()
}

/// The position read back after an order, retried once after half a second:
/// the venue answered the order and then, once tonight, not the read that
/// followed it (docs/60 P-117). The error is returned, never swallowed.
pub async fn read_back(user: Address, market: &str) -> Result<rust_decimal::Decimal, String> {
    retried(|| venue_position(user, market)).await
}

/// Hyperliquid's testnet API drops a request now and then ("error sending
/// request", docs/60 P-129, P-130): every read the desk depends on is tried
/// three times, 0.5 s and 1.5 s apart, before it is reported.
async fn retried<T, F, Fut>(mut f: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut last = String::new();
    for wait in [0u64, 500, 1500] {
        if wait > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        }
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// The most a signed step may move the offset without the owner's tap
/// (`TAMEION_DESK_AUTO_GRAMS`, default 1,000 g; docs/60 P-114, docs/89 H1).
pub fn auto_limit(goods: Goods) -> ledger::Money {
    std::env::var("TAMEION_DESK_AUTO_GRAMS")
        .ok()
        .and_then(|g| ledger::Money::parse(&g, goods.unit()).ok())
        .unwrap_or(ledger::Money::minor(1_000_000, goods.unit()))
}

/// The venue position for `market` held by `user`, read from the venue.
pub async fn venue_position(user: Address, market: &str) -> Result<rust_decimal::Decimal, String> {
    use hedge::hl::{Venue, VenueApi};
    let snap = Venue::testnet()
        .account(user)
        .await
        .map_err(|e| e.to_string())?;
    Ok(snap.size(market))
}

/// Funding on `market` paid to (+) or by (−) `user` since `since` (unix
/// seconds), summed from the venue's own record.
pub async fn funding_since(
    user: Address,
    market: &str,
    since: i64,
) -> Result<rust_decimal::Decimal, String> {
    use hedge::hl::{Venue, VenueApi};
    let start = u64::try_from(since).unwrap_or(0).saturating_mul(1000);
    let entries = Venue::testnet()
        .funding_since(user, start)
        .await
        .map_err(|e| e.to_string())?;
    Ok(entries
        .iter()
        .filter(|f| f.delta.coin == market)
        .map(|f| f.delta.usdc)
        .sum())
}

/// The venue's mark for `market`.
pub async fn venue_mark(market: &str) -> Result<rust_decimal::Decimal, String> {
    use hedge::hl::{Venue, VenueApi};
    let prices = Venue::testnet().prices().await.map_err(|e| e.to_string())?;
    prices
        .get(market)
        .map(|c| c.mark_px)
        .ok_or_else(|| format!("{market} has no mark on the venue"))
}

/// One hedge step sized from the book: the short for `goods` becomes the
/// desk's exposure in venue units. Dry run unless `HEDGE_LIVE=1` and a
/// trade-only key exist (`hedge::driver`); every safety rule of the engine
/// applies, including read-back and HALT.
pub async fn offset(
    desk: &Desk,
    goods: Goods,
    user: Address,
) -> Result<hedge::engine::StepReport, String> {
    let size = desk.offset_target(goods).map_err(|e| e.to_string())?;
    // The desk's own key when its owner connected this account by
    // `ApproveAgent` (docs/106, docs/103 S3-1); this Mac's key otherwise.
    let env = hedge::driver::desk_env_with_key(
        user,
        vec![hedge::engine::Holding {
            coin: goods.market().into(),
            size,
        }],
        own_key_for(user).as_deref(),
    )?;
    let (report, _books) = hedge::driver::step(&env).await.map_err(|e| e.to_string())?;
    Ok(report)
}

/// The statement signed by the statement key, with what a verifier needs.
pub fn sign_statement(
    chain_id: u64,
    s: &Statement,
    key: &PrivateKeySigner,
) -> Result<Value, String> {
    let body = serde_json::to_value(s).map_err(|e| e.to_string())?;
    let hash = keccak256(crate::statement::canonical_json(&body).as_bytes());
    let root = B256::from_str(&s.root).map_err(|e| format!("root: {e}"))?;
    let digest = DeskStatement {
        deal: s.deal.clone(),
        root,
        statementHash: hash,
        at: u64::try_from(s.at).unwrap_or(0),
    }
    .eip712_signing_hash(&domain(chain_id));
    let sig = key.sign_hash_sync(&digest).map_err(|e| e.to_string())?;
    Ok(json!({
        "statement": body,
        "statementHash": format!("{hash:#x}"),
        "signer": format!("{:#x}", key.address()),
        "signature": format!("0x{}", alloy::hex::encode(sig.as_bytes())),
        "domain": { "name": DOMAIN_NAME, "version": DOMAIN_VERSION, "chainId": chain_id },
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" }, { "name": "version", "type": "string" }, { "name": "chainId", "type": "uint256" },
            ],
            "DeskStatement": [
                { "name": "deal", "type": "string" }, { "name": "root", "type": "bytes32" },
                { "name": "statementHash", "type": "bytes32" }, { "name": "at", "type": "uint64" },
            ],
        },
    }))
}

/// Who signed a statement produced by [`sign_statement`], after checking
/// that the hash covers the statement as served.
pub fn verify_statement(signed: &Value) -> Result<Address, String> {
    let body = signed.get("statement").ok_or("no statement")?;
    let s: Statement = serde_json::from_value(body.clone()).map_err(|e| e.to_string())?;
    let hash = keccak256(crate::statement::canonical_json(body).as_bytes());
    if signed.get("statementHash").and_then(Value::as_str) != Some(format!("{hash:#x}").as_str()) {
        return Err("the statement does not hash to the signed statementHash".into());
    }
    let chain_id = signed
        .pointer("/domain/chainId")
        .and_then(Value::as_u64)
        .ok_or("no chain id")?;
    let root = B256::from_str(&s.root).map_err(|e| format!("root: {e}"))?;
    let digest = DeskStatement {
        deal: s.deal.clone(),
        root,
        statementHash: hash,
        at: u64::try_from(s.at).unwrap_or(0),
    }
    .eip712_signing_hash(&domain(chain_id));
    let raw = alloy::hex::decode(
        signed
            .get("signature")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_start_matches("0x"),
    )
    .map_err(|e| format!("signature: {e}"))?;
    Signature::from_raw(&raw)
        .map_err(|e| format!("signature: {e}"))?
        .recover_address_from_prehash(&digest)
        .map_err(|e| format!("signature: {e}"))
}

/// Statement inputs read from the venue for `goods`: its mark, and the
/// desk's short as the venue reports it.
pub async fn statement_inputs(
    goods: Goods,
    user: Address,
    since: i64,
    at: i64,
) -> Result<StatementInputs, String> {
    Ok(StatementInputs {
        mark: retried(|| venue_mark(goods.market())).await?,
        offset_contracts: read_back(user, goods.market()).await?,
        stress_bps: 3000,
        basis_bps: 200,
        venue_account: format!("{user:#x}"),
        funding_usdc: retried(|| funding_since(user, goods.market(), since)).await?,
        hedge_control: format!(
            "Who can close this offset: the trader's desk, at any time; its key can trade on Hyperliquid account {user:#x} and cannot withdraw from it. Who can withdraw its margin: whoever holds that account's master key."
        ),
        at,
    })
}

/// How far from the venue's mark a priced step may be before it is named
/// (`TAMEION_DESK_PRICE_BAND_BPS`, default 500 = 5 %).
pub fn price_band_bps() -> u32 {
    std::env::var("TAMEION_DESK_PRICE_BAND_BPS")
        .ok()
        .and_then(|b| b.parse().ok())
        .unwrap_or(500)
}

/// A priced contract far from the market, in words: an inflated purchase is
/// how a borrowing base is padded, and a cheap sale how stock leaves quietly.
/// `None` for movements, or when the price is within `band_bps` of `mark`
/// (USDC per venue unit).
pub fn price_note(m: &Milestone, mark: rust_decimal::Decimal, band_bps: u32) -> Option<String> {
    use rust_decimal::Decimal;
    let usdc: Decimal = m.usdc.as_deref()?.parse().ok()?;
    let grams = ledger::Money::parse(&m.quantity, m.goods.unit()).ok()?;
    let contracts = ::desk::grams_to_contracts(m.goods, grams);
    if contracts.is_zero() || mark.is_zero() {
        return None;
    }
    let per = usdc / contracts;
    let dev_bps = (per - mark) / mark * Decimal::from(10_000);
    if dev_bps.abs() <= Decimal::from(band_bps) {
        return None;
    }
    let pct = (dev_bps / Decimal::from(100)).round_dp(1);
    let side = if pct.is_sign_positive() {
        "above"
    } else {
        "below"
    };
    Some(format!(
        "priced {}% {side} the market ({} vs {} USDC per troy ounce on {}): check it with {}",
        pct.abs(),
        per.round_dp(2),
        mark.round_dp(2),
        m.goods.market(),
        m.counterparty
    ))
}

/// A statement explained in words for the trader (docs/83 D6): written by
/// the model on this Mac, which sees the statement and nothing else, and
/// kept only if every number it wrote is one the statement holds (P-108:
/// the 8B model writes wrong facts). Otherwise the plain template.
pub async fn explain(
    st: &Statement,
    model: &crate::local::LocalReasoner,
    model_name: &str,
) -> Value {
    let pct = |bps: &str| {
        bps.parse::<f64>()
            .map(|b| format!("{:.1}", b / 100.0))
            .unwrap_or_default()
    };
    let facts = json!({
        // Rounded here, so the model copies numbers instead of computing them.
        "held": st.held, "stage": st.stage, "valueUsdc": round2(&st.value_usdc),
        "eligibleGrams": st.eligible.grams, "eligibleValueUsdc": round2(&st.eligible.value_usdc),
        "notEligibleGrams": st.ineligible.grams, "consignedGrams": st.consigned.grams,
        "offsetPercent": pct(&st.offset_share_bps), "maxLossUsdc": round2(&st.max_loss_usdc),
        "fallPercent": (st.stress_bps / 100).to_string(), "basisPercent": (st.basis_bps / 100).to_string(),
        "eligibleAfterLossUsdc": round2(&st.eligible_usdc), "fundingUsdc": round2(&st.funding_usdc),
    });
    let template = format!(
        "You hold {} of gold, {}. {}% of it is offset on the exchange. If gold fell {}%, the most you would lose is {} USDC. Your lender would count {} USDC after that loss.",
        st.held.trim_end_matches(" gold"),
        st.stage,
        pct(&st.offset_share_bps),
        st.stress_bps / 100,
        round2(&st.max_loss_usdc),
        round2(&st.eligible_usdc),
    );
    // The model writes the words and names the numbers; code puts the numbers
    // in (docs/60 P-119). A digit in its text, or a name that is not a fact,
    // sets the text aside: an 8B model both invented a number and put a true
    // number under the wrong label when it wrote numbers itself.
    let system = "You explain a gold dealer's statement for their lender to the dealer, in plain English, in 3 or 4 short sentences. \
        You never write a digit. Where a number belongs, write its name in braces from this list and nothing else: \
        {held}, {stage}, {valueUsdc}, {eligibleGrams}, {eligibleValueUsdc}, {notEligibleGrams}, {consignedGrams}, {offsetPercent}, \
        {maxLossUsdc}, {fallPercent}, {eligibleAfterLossUsdc}, {fundingUsdc}. \
        Names ending in Grams and held are in grams, names ending in Usdc are in USDC, names ending in Percent are percentages: \
        write the unit after the brace yourself (\"{held} g\", \"{maxLossUsdc} USDC\", \"{fallPercent}%\"). \
        Meanings: held = gold held now; valueUsdc = its value at the market; eligibleValueUsdc = the value of the part a lender counts, before any loss; \
        eligibleAfterLossUsdc = what the lender would count after the maximum loss; maxLossUsdc = the most the dealer loses if gold falls fallPercent; \
        offsetPercent = how much of the gold is offset on the exchange; fundingUsdc = what the exchange paid (positive) or charged for the offset so far. \
        Do not give advice.";
    let schema = json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] });
    let started = std::time::Instant::now();
    let answer = model
        .ask_structured(system, facts.to_string(), schema)
        .await;
    let ms = started.elapsed().as_millis();
    match answer
        .map_err(|e| e.to_string())
        .and_then(|raw| serde_json::from_str::<Value>(&raw).map_err(|e| e.to_string()))
        .and_then(|v| {
            v["text"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "no text".to_string())
        }) {
        Ok(words) => match render(&words, &facts) {
            Ok(text) => {
                json!({ "text": text, "by": model_name, "ms": ms, "checked": true, "words": words })
            }
            Err(why) => json!({
                "text": template, "by": "template", "ms": ms, "checked": true, "setAside": words,
                "rejected": format!("{model_name}: {why}; its text was set aside"),
            }),
        },
        Err(e) => {
            json!({ "text": template, "by": "template", "ms": ms, "checked": true, "rejected": format!("{model_name}: {e}") })
        }
    }
}

/// Put the facts into the model's words: `{name}` → the fact's value. Refused
/// when the words carry a digit of their own or name no known fact.
/// A number the model wrote itself: any digit in any script ("１０", "١٠"),
/// or a number word ("ten", "half", "none") — docs/91 R9. Words such as
/// "all" or "no" stay allowed: refusing them would refuse every sentence.
fn own_number(text: &str) -> bool {
    const WORDS: &[&str] = &[
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve", "twenty", "thirty", "forty", "fifty", "hundred", "thousand", "million",
        "billion", "half", "quarter", "double", "twice", "triple", "dozen", "none",
    ];
    text.chars().any(char::is_numeric)
        || text
            .split(|c: char| !c.is_alphabetic())
            .any(|w| WORDS.contains(&w.to_lowercase().as_str()))
}

fn render(words: &str, facts: &Value) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = words;
    while let Some(open) = rest.find('{') {
        let (before, after) = rest.split_at(open);
        if own_number(before) {
            return Err(format!(
                "it wrote a number of its own in \"{}\"",
                before.trim()
            ));
        }
        out.push_str(before);
        let close = after.find('}').ok_or("an unclosed {")?;
        let name = &after[1..close];
        let value = facts[name]
            .as_str()
            .ok_or_else(|| format!("it named {{{name}}}, which is not a fact"))?;
        // Bare numbers: the model writes the unit ("{held} g"), never the value.
        out.push_str(value.trim_end_matches(" g gold"));
        rest = &after[close + 1..];
    }
    if own_number(rest) {
        return Err(format!(
            "it wrote a number of its own in \"{}\"",
            rest.trim()
        ));
    }
    out.push_str(rest);
    Ok(out)
}

fn round2(s: &str) -> String {
    s.parse::<f64>()
        .map(|x| format!("{x:.2}"))
        .unwrap_or_else(|_| s.into())
}

/// Words for one milestone, for logs and the console.
pub fn words(m: &Milestone) -> String {
    let who = if m.signer.is_empty() {
        "entered by the owner (their passkey; no counterparty signature)".to_string()
    } else {
        format!("signed by {} ({})", m.counterparty, m.signer)
    };
    let what = match m.kind {
        MilestoneKind::Bought => format!(
            "bought {} g of {} for {} USDC",
            m.quantity,
            m.goods.name(),
            m.usdc.as_deref().unwrap_or("?")
        ),
        MilestoneKind::Shipped => format!("{} g of {} shipped", m.quantity, m.goods.name()),
        MilestoneKind::Arrived => format!("{} g of {} into stock", m.quantity, m.goods.name()),
        MilestoneKind::ToFabrication => {
            format!("{} g of {} to fabrication", m.quantity, m.goods.name())
        }
        MilestoneKind::Consigned => format!("{} g of {} consigned", m.quantity, m.goods.name()),
        MilestoneKind::Returned => {
            format!("{} g of {} back into stock", m.quantity, m.goods.name())
        }
        MilestoneKind::Sold => format!(
            "sold {} g of {} for {} USDC",
            m.quantity,
            m.goods.name(),
            m.usdc.as_deref().unwrap_or("?")
        ),
    };
    format!("{} · {what} · {} · {who}", m.deal, m.document)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESK: Address = Address::repeat_byte(0xd5);

    /// After `POST /api/desk/venue/approve` the claim carries `approveAgent`:
    /// the offset and `GET /api/desk/venue` use the desk's own key, never this
    /// Mac's (docs/103 S3-1); the old passkey connect keeps this Mac's key.
    #[test]
    fn a_desk_connected_by_approve_agent_trades_with_its_own_key() {
        let policy = "0xcd8d71d97c3a946a14ac3fb92db4fb86e2974308";
        let mut claim = json!({ "policy": policy, "account": "0x557e1e07652b75abaa667223b11704165fc94d09",
            "approval": { "issued": 1, "challenge": "0x01", "approveAgent": { "agent": "0x02", "name": "desk-cd8d71" } },
            "connected": true });
        assert_eq!(
            own_key_path(&claim),
            Some(crate::desk_live::agent_key_path(policy))
        );
        claim["connected"] = json!(false);
        assert_eq!(
            own_key_path(&claim),
            None,
            "a disconnected desk trades with nothing of its own"
        );
        claim["connected"] = json!(true);
        claim["approval"]
            .as_object_mut()
            .unwrap()
            .remove("approveAgent");
        assert_eq!(
            own_key_path(&claim),
            None,
            "connected by the owner's passkey alone: this Mac's key"
        );
    }

    fn ms(kind: MilestoneKind, q: &str, usdc: Option<&str>, who: &str, doc: &str) -> Milestone {
        Milestone {
            deal: "G-1".into(),
            kind,
            goods: Goods::Gold,
            quantity: q.into(),
            usdc: usdc.map(Into::into),
            counterparty: who.into(),
            document: doc.into(),
            signer: String::new(),
            at: 1_792_100_000,
        }
    }

    #[test]
    fn signed_milestones_are_booked_only_from_the_approved_signer() {
        let dir = std::env::temp_dir().join(format!("desk-test-{}", std::process::id()));
        let path = dir.join("book.jsonl");
        let _ = fs::remove_file(&path);
        let supplier = PrivateKeySigner::random();
        let carrier = PrivateKeySigner::random();
        let mallory = PrivateKeySigner::random();
        let mut b = Book::open(&path, 421_614, DESK).unwrap();
        b.approve("Supplier", &format!("{:#x}", supplier.address()), None)
            .unwrap();
        b.approve("Carrier", &format!("{:#x}", carrier.address()), None)
            .unwrap();
        assert!(
            b.approve("Supplier", &format!("{:#x}", mallory.address()), None)
                .is_err(),
            "no silent rebinding"
        );

        let bought = ms(
            MilestoneKind::Bought,
            "10",
            Some("1340"),
            "Supplier",
            "PO-1",
        );
        let sig = sign_event(421_614, DESK, &bought, &supplier).unwrap();
        let booked = b.record(bought, Some(&sig)).unwrap();
        assert_eq!(booked.signer, format!("{:#x}", supplier.address()));

        // A bill of lading signed by someone else is refused and not written.
        let bl = ms(MilestoneKind::Shipped, "10", None, "Carrier", "BL-1");
        let forged = sign_event(421_614, DESK, &bl, &mallory).unwrap();
        assert!(
            b.record(bl.clone(), Some(&forged))
                .unwrap_err()
                .contains("signs as")
        );
        // The same event signed for another chain recovers someone else.
        let other_chain = sign_event(1, DESK, &bl, &carrier).unwrap();
        assert!(b.record(bl.clone(), Some(&other_chain)).is_err());
        let good = sign_event(421_614, DESK, &bl, &carrier).unwrap();
        // Signed for another desk: recovers someone else here (docs/89 H3).
        let other_desk = sign_event(421_614, Address::repeat_byte(0xee), &bl, &carrier).unwrap();
        assert!(b.record(bl.clone(), Some(&other_desk)).is_err());
        b.record(bl.clone(), Some(&good)).unwrap();
        // The same signed step a second time is refused.
        assert!(
            b.record(bl, Some(&good))
                .unwrap_err()
                .contains("already booked")
        );

        // An unsigned entry is the owner's own, and says so.
        let arrived = b
            .record(
                ms(MilestoneKind::Arrived, "10", None, "Carrier", "AN-1"),
                None,
            )
            .unwrap();
        assert!(arrived.signer.is_empty());
        assert!(words(&arrived).contains("no counterparty signature"));

        // The file replays to the same book, every signature checked again.
        let again = Book::open(&path, 421_614, DESK).unwrap();
        assert_eq!(again.desk.root().unwrap(), b.desk.root().unwrap());
        assert_eq!(again.parties.len(), 2);
        // A hand-edited quantity no longer matches its signature: refused.
        let text = fs::read_to_string(&path).unwrap().replacen(
            "\"quantity\":\"10\"",
            "\"quantity\":\"9\"",
            1,
        );
        fs::write(&path, text).unwrap();
        assert!(Book::open(&path, 421_614, DESK).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_owner_challenges_are_the_documented_bytes() {
        // The same values are asserted in apps/console/test/guard.test.mts; the
        // base challenge was also computed with `cast keccak $(cast concat-hex …)`.
        let policy = Address::from_str("0xcd8d71d97c3a946a14ac3fb92db4fb86e2974308").unwrap();
        let m = Milestone {
            deal: "G-1".into(),
            kind: MilestoneKind::Bought,
            goods: Goods::Gold,
            quantity: "10.000".into(),
            usdc: Some("1334.45".into()),
            counterparty: "Workshop".into(),
            document: "RCPT-1".into(),
            signer: String::new(),
            at: 1_792_080_000,
        };
        let hex = |b: B256| format!("{b:#x}");
        assert_eq!(
            hex(base_challenge(
                421_614,
                policy,
                "gold",
                "100.000",
                1_792_080_000
            )),
            "0x905cd68699780501fcdb2cf2398368f9ebe2b63be1a3cc732f0a17413ce40770"
        );
        assert_eq!(
            hex(control_challenge(
                421_614,
                policy,
                "held by the lender",
                1_792_080_000
            )),
            "0x45958132365bccdd313e0a5dae8b8e9d1398e73180bdfef5efae171f6511ff62"
        );
        assert_eq!(
            hex(event_digest(421_614, policy, &m)),
            "0x2449d514c067381b192aff76f298745acf6fae3b0396c6c73678ac62caf35575"
        );
        assert_eq!(
            hex(step_challenge(
                421_614,
                policy,
                event_digest(421_614, policy, &m),
                1_792_080_000
            )),
            "0x2ad9d672ace341b3a1895860b3085bc92f02e97dd1cc2116ba6481d12ebfdfeb"
        );
        // `cast keccak $(cast concat-hex <tag> <chainId u64> <policy> <len u32> "0.321507" <issued u64>)`.
        assert_eq!(
            hex(offset_challenge(421_614, policy, "0.321507", 1_792_080_000)),
            "0xad7c937d74cea31d279418f85c0efd4660834ee9609af502dad3c8356f5b88cd"
        );
        // `cast keccak $(cast concat-hex <tag venue.v1> <chainId u64> <policy> <account> <issued u64>)`.
        assert_eq!(
            hex(venue_challenge(
                421_614,
                policy,
                Address::from_str("0x557e1e07652b75abaa667223b11704165fc94d09").unwrap(),
                1_792_080_000
            )),
            "0x882ca155c72b418b1a99c79b7e4a0fc7040677d090f58082ea1a73807bff85d1"
        );
        // `… <tag v2> … <len u32> "Vault" <address> 0x24 <issued u64>`: arrived (4) | returned (32).
        assert_eq!(
            hex(party_challenge_v2(
                421_614,
                policy,
                "Vault",
                Address::from_str("0x8b95fdedb647236da0f266231f71440d2714d542").unwrap(),
                &[MilestoneKind::Arrived, MilestoneKind::Returned],
                1_792_080_000
            )),
            "0xa4e74a38febef2dfe5e250cc6b23b498f0a89e5167df205e2ff27a8b993629b1"
        );
    }

    #[test]
    fn a_look_alike_name_for_another_key_is_refused_and_an_old_book_still_replays() {
        // docs/103 S2-2: "Lim Brothers Gold" with a Cyrillic о is not a second counterparty.
        let path = std::env::temp_dir()
            .join(format!("desk-alike-{}", std::process::id()))
            .join("book.jsonl");
        let _ = fs::remove_file(&path);
        let (a, b, c) = (
            Address::repeat_byte(0x11),
            Address::repeat_byte(0x22),
            Address::repeat_byte(0x33),
        );
        let mut book = Book::open(&path, 421_614, DESK).unwrap();
        book.approve("Lim Brothers Gold", &format!("{a:#x}"), None)
            .unwrap();
        let e = book
            .approve("Lim Br\u{043E}thers Gold", &format!("{b:#x}"), None)
            .unwrap_err();
        assert!(e.contains("reads like"), "{e}");
        book.approve("Harbour Vault KL", &format!("{b:#x}"), None)
            .unwrap();
        // A line written before the fold existed is kept as written when the book replays.
        let old = Line::Party(Party {
            name: "LIM BROTHERS GOLD".into(),
            address: format!("{c:#x}"),
            approval: None,
            kinds: None,
        });
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&old).unwrap()).unwrap();
        drop(f);
        assert_eq!(Book::open(&path, 421_614, DESK).unwrap().parties.len(), 3);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_stopped_counterparty_books_nothing_and_the_desk_counts_what_it_moved_alone() {
        let path = std::env::temp_dir()
            .join(format!("desk-revoke-{}", std::process::id()))
            .join("book.jsonl");
        let _ = fs::remove_file(&path);
        let supplier = PrivateKeySigner::random();
        let mut b = Book::open(&path, 421_614, DESK).unwrap();
        b.approve("Supplier", &format!("{:#x}", supplier.address()), None)
            .unwrap();
        let signed = |b: &mut Book, deal: &str| {
            let mut m = ms(MilestoneKind::Bought, "1", Some("134"), "Supplier", "PO");
            m.deal = deal.into();
            let sig = sign_event(421_614, DESK, &m, &supplier).unwrap();
            b.record(m, Some(&sig))
        };
        signed(&mut b, "D-1").unwrap();
        b.revoke("Supplier", 1).unwrap();
        assert!(signed(&mut b, "D-2").unwrap_err().contains("stopped"));
        assert!(b.revoke("Nobody", 1).is_err());
        // The book still opens: the step signed before the stop replays.
        let mut b = Book::open(&path, 421_614, DESK).unwrap();
        assert_eq!(b.desk.deals().count(), 1);
        assert!(signed(&mut b, "D-2").is_err());
        // Approved again (the owner's passkey, in the route), it books again.
        b.approve("Supplier", &format!("{:#x}", supplier.address()), None)
            .unwrap();
        signed(&mut b, "D-2").unwrap();
        assert_eq!(
            Book::open(&path, 421_614, DESK)
                .unwrap()
                .desk
                .deals()
                .count(),
            2
        );

        // A vault approved for receipts cannot sign a sale (docs/91 R7).
        let vault = PrivateKeySigner::random();
        b.approve_for(
            "Vault",
            &format!("{:#x}", vault.address()),
            Some(vec![MilestoneKind::Arrived, MilestoneKind::Returned]),
            None,
        )
        .unwrap();
        let mut sale = ms(MilestoneKind::Sold, "1", Some("140"), "Vault", "SO");
        sale.deal = "D-1".into();
        let sig = sign_event(421_614, DESK, &sale, &vault).unwrap();
        assert!(
            b.record(sale, Some(&sig))
                .unwrap_err()
                .contains("may sign arrived, returned on this desk, not sold")
        );
        let mut vr = ms(MilestoneKind::Arrived, "1", None, "Vault", "VR");
        vr.deal = "D-1".into();
        let sig = sign_event(421_614, DESK, &vr, &vault).unwrap();
        b.record(vr, Some(&sig)).unwrap();
        assert!(Book::open(&path, 421_614, DESK).is_ok());

        // An approval sent again after a stop does not undo it (docs/92 T7).
        let carrier = PrivateKeySigner::random();
        let ok_at = |i: u64, c: &str| Some(json!({ "issued": i, "challenge": c }));
        b.approve_for(
            "Carrier",
            &format!("{:#x}", carrier.address()),
            None,
            ok_at(1_000, "0xc1"),
        )
        .unwrap();
        b.revoke("Carrier", 1_060).unwrap();
        let again = |b: &mut Book| {
            b.approve_for(
                "Carrier",
                &format!("{:#x}", carrier.address()),
                None,
                ok_at(1_000, "0xc1"),
            )
        };
        assert!(again(&mut b).is_err(), "the same approval after the stop");
        assert!(
            again(&mut Book::open(&path, 421_614, DESK).unwrap()).is_err(),
            "nor after a restart"
        );
        b.approve_for(
            "Carrier",
            &format!("{:#x}", carrier.address()),
            None,
            ok_at(1_061, "0xc2"),
        )
        .unwrap();

        // An approval older than the last of its kind is a replay (docs/91 R11).
        let at = |i: u64| Some(json!({ "issued": i }));
        b.set_base(Goods::Gold, "100", at(200)).unwrap();
        assert!(
            b.set_base(Goods::Gold, "500", at(100))
                .unwrap_err()
                .contains("older")
        );
        b.set_base(Goods::Gold, "500", at(200)).unwrap();
        let b2 = Book::open(&path, 421_614, DESK).unwrap();
        assert!(b2.fresh("base", &at(150)).is_err() && b2.fresh("control", &at(150)).is_ok());

        // Moves without the owner add up; one the owner confirms resets them.
        let d = |x: &str| rust_decimal::Decimal::from_str(x).unwrap();
        b.moved(d("600"), true, 2, None).unwrap();
        b.moved(d("300.0004"), true, 3, None).unwrap();
        assert_eq!(b.auto_moved, d("900"));
        assert_eq!(
            Book::open(&path, 421_614, DESK).unwrap().auto_moved,
            d("900")
        );
        let release = json!({ "issued": 4, "challenge": "0x0ff5e7" });
        b.moved(d("5000"), false, 4, Some(release.clone())).unwrap();
        assert!(b.is_spent(&release));
        assert!(
            Book::open(&path, 421_614, DESK).unwrap().is_spent(&release),
            "a released move's approval stays spent after a restart"
        );
        assert!(
            Book::open(&path, 421_614, DESK)
                .unwrap()
                .auto_moved
                .is_zero()
        );

        // A long counts in full: the engine sells it, then opens the short.
        let g = |t: &str, p: &str| trade_grams(Goods::Gold, d(t), d(p)).round_dp(3);
        assert_eq!(g("0.321", "-0.321"), d("0"));
        assert_eq!(g("0.321", "0.1"), d("13.095"));
        assert_eq!(g("0", "-0.1"), d("3.110"));
    }

    #[test]
    fn one_desk_per_exchange_account_and_only_its_holder_disconnects() {
        // The only test that sets the service's app dir (a process-wide OnceLock).
        let dir = std::env::temp_dir().join(format!("desk-venue-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let _ = fs::remove_file(dir.join("desk-venue.json"));
        set_app_dir(&dir);
        if std::env::var("TAMEION_DESK_POLICY").is_ok() {
            return; // the environment wins over the page; nothing to check here
        }
        let acc = Address::repeat_byte(0x55);
        let (a, b) = (
            "0xAAaa000000000000000000000000000000000001",
            "0xbbbb000000000000000000000000000000000002",
        );
        let known = vec![a.to_lowercase()];
        assert_eq!(venue_account(a), None);
        // Only a desk set up on this Mac claims the account first (docs/92 T6).
        assert!(
            connect_venue(b, acc, json!({}), 1, &known)
                .unwrap_err()
                .contains("set up on this Mac")
        );
        connect_venue(a, acc, json!({}), 1, &known).unwrap();
        assert_eq!(venue_account(a), Some(acc));
        assert_eq!(
            venue_account(&a.to_lowercase()),
            Some(acc),
            "policies compare without case"
        );
        assert_eq!(
            venue_account(b),
            None,
            "another desk never claims this short"
        );
        assert!(
            connect_venue(b, acc, json!({}), 2, &known)
                .unwrap_err()
                .contains("belongs to desk")
        );
        assert!(
            !disconnect_venue(b, 5).unwrap(),
            "only the holder disconnects"
        );
        assert!(disconnect_venue(a, 5).unwrap());
        assert_eq!(venue_account(a), None, "disconnected: trades nothing");
        // Disconnected, the account still belongs to a: b cannot take it, a connects again.
        let all = vec![a.to_lowercase(), b.to_lowercase()];
        assert!(
            connect_venue(b, acc, json!({}), 3, &all)
                .unwrap_err()
                .contains("belongs to desk")
        );
        connect_venue(a, acc, json!({}), 4, &all).unwrap();
        assert_eq!(venue_account(a), Some(acc));
        // docs/94 U8: the same approval again, or one issued before a disconnect, is refused.
        let ok = json!({ "issued": 10, "challenge": "0xaa" });
        connect_venue(a, acc, ok.clone(), 10, &all).unwrap();
        assert!(
            connect_venue(a, acc, ok, 11, &all)
                .unwrap_err()
                .contains("already used")
        );
        disconnect_venue(a, 20).unwrap();
        let old = json!({ "issued": 15, "challenge": "0xbb" });
        assert!(
            connect_venue(a, acc, old, 21, &all)
                .unwrap_err()
                .contains("before the desk was disconnected")
        );

        // docs/103 S3-2: the same refusals, writing nothing, run before the venue sees a key.
        let before = venue_claim_now();
        assert!(
            connect_venue_check(b, &json!({ "issued": 30, "challenge": "0xcc" }), &all)
                .unwrap_err()
                .contains("belongs to desk")
        );
        assert!(
            connect_venue_check(a, &json!({ "issued": 30, "challenge": "0xaa" }), &all)
                .unwrap_err()
                .contains("already used")
        );
        assert_eq!(venue_claim_now(), before, "a check never writes");
        // docs/103 S3-1: after an ApproveAgent connect the offset signs with the desk's own key...
        let fresh = json!({ "issued": 40, "challenge": "0xdd", "approveAgent": { "agent": "0x01", "name": "desk-aaaa00" } });
        connect_venue_check(a, &fresh, &all).unwrap();
        connect_venue(a, acc, fresh, 40, &all).unwrap();
        assert_eq!(own_key_for(acc), Some(crate::desk_live::agent_key_path(a)));
        assert_eq!(
            own_key_for(Address::repeat_byte(0x66)),
            None,
            "another account: this Mac's key"
        );
        // ...and when the venue refuses, the claim goes back exactly as it was.
        restore_venue_claim(before.as_ref()).unwrap();
        assert_eq!(venue_claim_now(), before);
        assert_eq!(own_key_for(acc), None, "put back: no key of its own");
    }

    #[test]
    fn the_party_challenge_is_the_documented_bytes() {
        // Computed independently with `cast keccak $(cast concat-hex …)`; the
        // console's guard.ts is held to the same value.
        let ch = party_challenge(
            421_614,
            Address::from_str("0xcd8d71d97c3a946a14ac3fb92db4fb86e2974308").unwrap(),
            "Supplier",
            Address::from_str("0x85e1c3924ed413a7e24e3388ed2c8697fd6b8904").unwrap(),
            1_792_080_000,
        );
        assert_eq!(
            format!("{ch:#x}"),
            "0x65caa0468d12f8c25cf6907f40e52abda88fcb8518185efde9fd105f328aa883"
        );
    }

    #[test]
    fn the_model_names_the_numbers_and_code_writes_them() {
        let facts = json!({ "held": "10.000 g gold", "maxLossUsdc": "27.30", "fallPercent": "30" });
        assert_eq!(
            render("You hold {held} g. If gold fell {fallPercent}%, you lose at most {maxLossUsdc} USDC.", &facts).unwrap(),
            "You hold 10.000 g. If gold fell 30%, you lose at most 27.30 USDC."
        );
        assert!(
            render("You hold 6 grams.", &facts)
                .unwrap_err()
                .contains("number of its own")
        );
        assert!(
            render("You hold {grams}.", &facts)
                .unwrap_err()
                .contains("not a fact")
        );
        assert!(render("A fall of {fallPercent}% costs 27 USDC.", &facts).is_err());
        // docs/91 R9: number words and digits of other scripts are numbers too.
        for own in [
            "You hold ten grams.",
            "You hold １０ g.",
            "Half of it is offset.",
            "None of it is offset {held}.",
        ] {
            assert!(render(own, &facts).is_err(), "{own}");
        }
        assert!(render("All of it is in stock: {held} g.", &facts).is_ok());
    }

    #[test]
    fn a_price_far_from_the_market_is_named() {
        // 10 g = 0.3215 oz; at 4,150/oz the fair price is ~1,334.
        let mark = rust_decimal::Decimal::from(4150);
        let fair = ms(MilestoneKind::Bought, "10", Some("1340"), "Supplier", "PO");
        assert_eq!(price_note(&fair, mark, 500), None);
        let padded = ms(MilestoneKind::Bought, "10", Some("1500"), "Supplier", "PO");
        let note = price_note(&padded, mark, 500).unwrap();
        assert!(
            note.contains("12.4% above") && note.contains("Supplier"),
            "{note}"
        );
        let cheap = ms(MilestoneKind::Sold, "10", Some("1200"), "Buyer", "SO");
        assert!(price_note(&cheap, mark, 500).unwrap().contains("below"));
        assert_eq!(
            price_note(
                &ms(MilestoneKind::Arrived, "10", None, "Vault", "VR"),
                mark,
                500
            ),
            None
        );

        // The mark seen at booking is kept with the step, so the deal card
        // still names the padded price after the book is reopened.
        let path = std::env::temp_dir()
            .join(format!("desk-mark-{}", std::process::id()))
            .join("book.jsonl");
        let _ = fs::remove_file(&path);
        let supplier = PrivateKeySigner::random();
        let mut b = Book::open(&path, 421_614, DESK).unwrap();
        b.approve("Supplier", &format!("{:#x}", supplier.address()), None)
            .unwrap();
        let sig = sign_event(421_614, DESK, &padded, &supplier).unwrap();
        b.record_with(padded, Some(&sig), None, Some(mark)).unwrap();
        let note =
            |b: &Book| view(b, "0x").unwrap()["deals"][0]["milestones"][0]["priceNote"].clone();
        assert!(note(&b).as_str().unwrap().contains("12.4% above"));
        let b = Book::open(&path, 421_614, DESK).unwrap();
        assert!(note(&b).as_str().unwrap().contains("12.4% above"));
    }

    #[test]
    fn a_signed_statement_verifies_and_a_changed_number_does_not() {
        let mut d = Desk::new();
        d.apply(ms(
            MilestoneKind::Bought,
            "10",
            Some("1340"),
            "Supplier",
            "PO-1",
        ))
        .unwrap();
        let p = StatementInputs {
            mark: rust_decimal::Decimal::from(4150),
            offset_contracts: rust_decimal::Decimal::from_str("-0.321").unwrap(),
            stress_bps: 3000,
            basis_bps: 200,
            venue_account: "0x557e1e07652b75abaa667223b11704165fc94d09".into(),
            hedge_control: "trade-only key".into(),
            funding_usdc: rust_decimal::Decimal::ZERO,
            at: 1_792_100_100,
        };
        let s = d.statement("G-1", &p).unwrap();
        let key = PrivateKeySigner::random();
        let signed = sign_statement(421_614, &s, &key).unwrap();
        assert_eq!(verify_statement(&signed).unwrap(), key.address());
        let mut bad = signed.clone();
        bad["statement"]["eligibleUsdc"] = json!("999999");
        assert!(verify_statement(&bad).is_err());
    }
}
