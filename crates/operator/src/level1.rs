//! Level 1 of the trust ladder as a service (docs/52): a treasurer's own
//! policy, with their passkey as the only key that can add a payee or
//! withdraw, the agent in *Propose* mode (it prepares each payment; the phone
//! signs), run for them by this service. Everything the contracts already
//! enforce (caps, attested payees, delay rule, Mode) applies unchanged.
//!
//! Who holds what: the treasurer's passkey is the principal; this service's
//! relayer key is the operator (routine, cannot withdraw, cannot add a
//! payee) and pays gas; a fresh agent key per tenant (kept under the app
//! directory, mode 600) proposes payments and can send only to attested
//! payees inside the caps, and in Propose mode cannot send at all.
//!
//! Contracts are deployed from `contracts/bytecode/` (`scripts/snapshot_bytecode.sh`)
//! so the host needs no forge.

use crate::decide::{Configured, Decider};
use crate::local::Choice;
use crate::reasoner::{Reasoner, RuleBased};
use crate::watch::{ChainPreset, rpc_for};
use crate::{Config, Operator};
use alloy::primitives::{Address, B256, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
use arc::{ArcClient, PasskeyAuth, PreparedCall, passkey_challenge, prepare_payee_batch};
use ledger::{Currency, Money, Seconds, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub const PASSKEY_PRINCIPAL_BYTECODE: &str =
    include_str!("../../../contracts/bytecode/PasskeyPrincipal.hex");
pub const BUDGET_POLICY_BYTECODE: &str =
    include_str!("../../../contracts/bytecode/BudgetPolicy.hex");
/// The policy on a chain where USDC is an ERC-20 and gas is ETH (PLAN 5.58).
pub const BUDGET_POLICY_TOKEN_BYTECODE: &str =
    include_str!("../../../contracts/bytecode/BudgetPolicyToken.hex");
/// The pull variant (PLAN 5.26): the money stays in the bill wallet and the
/// policy `transferFrom`s it inside the same caps.
pub const BUDGET_POLICY_PULL_BYTECODE: &str =
    include_str!("../../../contracts/bytecode/BudgetPolicyPull.hex");
/// The policy code a company's own wallet runs by EIP-7702 (PLAN 5.28): the
/// same policy, initialised by the wallet itself, at the wallet's address.
pub const TAMEION_DELEGATE_BYTECODE: &str =
    include_str!("../../../contracts/bytecode/TameionDelegate.hex");
/// Arc's native USDC answers the ERC-20 interface here (`decimals() == 6`,
/// `approve`, `allowance`, `transferFrom`; checked 2026-10-14).
pub const ARC_USDC_FACADE: &str = "0x3600000000000000000000000000000000000000";
/// ERC-8004 IdentityRegistry, same address on every chain it is deployed to.
pub const REGISTRY: &str = "0x8004A818BFB912233c491871b3d84c89A494BD9e";

/// One treasurer's deployment, as kept on disk under `tenants/<policy>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub chain: String,
    /// `hold` (the policy holds the USDC) or `pull` (the money stays in the
    /// bill wallet and the policy pulls inside an allowance; PLAN 5.26).
    #[serde(default = "kind_hold")]
    pub kind: String,
    pub owner: String,
    pub policy: String,
    pub agent: String,
    pub operator: String,
    pub watched: String,
    pub recovery: String,
    pub per_action: String,
    pub daily: String,
    pub floor: String,
    pub delay_secs: u64,
    pub mode: u8,
    pub created_block: u64,
    pub history_from_block: u64,
    pub created: u64,
    /// Lazy deploy (PLAN 5.32): `false` while the addresses are only
    /// reserved — nothing on chain until money arrives or the owner taps.
    #[serde(default = "default_true")]
    pub deployed: bool,
    /// PLAN 5.38: no agent key exists for this account (agent = 0x…dEaD).
    #[serde(default)]
    pub keyless: bool,
    /// PLAN 5.45: the partner that brought this account (`/?partner=<id>`),
    /// if any; the partner sees it in its list, never its keys or money.
    #[serde(default)]
    pub partner: Option<String>,
    /// PLAN 5.7: devices that asked to become co-owners (qx, qy, label, at),
    /// waiting for the current owners to add them.
    #[serde(default)]
    pub join_requests: Vec<(String, String, String, u64)>,
    /// The passkey's public key, kept so a reserved tenant can be
    /// materialised later with the same deterministic owner address.
    #[serde(default)]
    pub qx: Option<String>,
    #[serde(default)]
    pub qy: Option<String>,
    pub owner_tx: Option<String>,
    pub policy_tx: Option<String>,
    /// The relayer's gas top-up to the agent key, when one was sent.
    #[serde(default)]
    pub agent_gas_tx: Option<String>,
    /// Where the agent reports after each pass (docs/53 5.3): an https URL
    /// that receives a JSON summary. Set at deploy or by the admin.
    #[serde(default)]
    pub notify_url: Option<String>,
    /// Bills the owner imported (PLAN 5.8); only payees the policy attests.
    #[serde(default)]
    pub bills: Vec<SeededBill>,
    /// Secret the webhook body is HMAC-SHA256 signed with (docs/56 #8),
    /// minted when a notify URL is first set and shown to the setter once.
    #[serde(default)]
    pub notify_secret: Option<String>,
    /// A Telegram chat the agent reports to after each pass (PLAN 5.10),
    /// with the host's bot token in `TAMEION_TELEGRAM_BOT_TOKEN`.
    #[serde(default)]
    pub telegram_chat_id: Option<String>,
    /// Capability tokens the owner's passkey minted for other agents (PLAN
    /// 5.31): scopes, expiry, label, and the sha256 of the secret — never the
    /// secret. Once one exists, read / run / prepare / import routes for this
    /// tenant require a token with the right scope.
    #[serde(default)]
    pub agent_tokens: Vec<AgentToken>,
    /// docs/79 §6a: the account's handle (`acme`), unique across this
    /// service's records; `None` for an account opened from a wallet.
    #[serde(default)]
    pub name: Option<String>,
    /// docs/79 §6a: the wallets whose history is the account's, at most 5.
    /// `watched` is frozen at reserve (it is in the account's CREATE2 salt);
    /// a name-first account reserves with `watched = 0x0` and lists its
    /// sources here. Read them through [`history_sources`], never directly.
    #[serde(default)]
    pub wallets: Vec<String>,
    /// The reserve named `recovery` itself (the zero address included), so a
    /// connected wallet never replaces it.
    #[serde(default)]
    pub recovery_explicit: bool,
    /// The spare key proposed for an account reserved without one: its first
    /// connected wallet (docs/79 §6a #4). Not `recovery`, which stays what the
    /// owner contract holds or will hold: recovery is a `PasskeyPrincipal`
    /// constructor argument, inside the creation code kept at reserve, so
    /// changing it would move the owner and with it the account's address
    /// (the same reason `watched` is frozen). Only the passkey can name it on
    /// chain (`setRecovery`, one tap through `prepare-recovery`); the record
    /// takes it as `recovery` when that call goes through `execute` (docs/60 P-88).
    #[serde(default)]
    pub spare_key: Option<String>,
    /// The owner typed no caps: while reserved they follow the bills the
    /// service knows ([`refresh_provisional_caps`]); at deploy the operator
    /// sets them on the policy (a raise waits the delay, like any raise).
    #[serde(default)]
    pub caps_provisional: bool,
    /// sha256 (hex) of the reserve token (docs/60 P-87): minted at a new
    /// reserve and returned once. It — not the passkey's public key, which the
    /// public tenant route serves — lets the browser that opened the account
    /// change a reserved account's wallets and first bills, and stays the
    /// owner's device token for `/wallets` after deploy (P-93). The passkey
    /// re-issues it on another device (`/api/level1/reserve-token`).
    #[serde(default)]
    pub reserve_token_hash: Option<String>,
    /// docs/81: the names the owner gave addresses, written only from the
    /// device that opened the account (`/api/level1/contacts`), read only by
    /// it or a checked agent key; never in the public view.
    #[serde(default)]
    pub contacts: Vec<crate::parties::Contact>,
}

/// A fresh reserve token for `t` (32 random bytes, hex); the record keeps only
/// its sha256, the caller hands the token out once.
pub fn mint_reserve_token(t: &mut Tenant) -> String {
    let tok = new_secret();
    t.reserve_token_hash = Some(token_hash(&tok));
    tok
}

/// Is `presented` this account's device token (docs/60 P-87, P-93)? Compared
/// in constant time. Where it counts is the caller's: a reserved account's
/// wallets and first bills, a deployed account's wallets.
pub fn reserve_token_ok(t: &Tenant, presented: Option<&str>) -> bool {
    let (Some(want), Some(p)) = (
        t.reserve_token_hash.as_deref(),
        presented.map(str::trim).filter(|p| !p.is_empty()),
    ) else {
        return false;
    };
    let got = token_hash(p);
    want.len() == got.len()
        && want
            .bytes()
            .zip(got.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// The 32 bytes the passkey signs to get a new reserve token for a reserved
/// account on a device that does not hold it: a tag, the chain, the policy and
/// when it was issued (accepted for ten minutes, once).
pub fn reserve_token_challenge(chain_id: u64, policy: Address, issued: u64) -> B256 {
    let mut m = b"tameion.reserve-token.v1".to_vec();
    m.extend_from_slice(&chain_id.to_be_bytes());
    m.extend_from_slice(policy.as_slice());
    m.extend_from_slice(&issued.to_be_bytes());
    alloy::primitives::keccak256(m)
}

/// A handle (docs/79 §6a #1): lowercase `[a-z0-9-]{3,32}`, no `-` at either end.
pub fn valid_name(s: &str) -> bool {
    (3..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// The records holding `name`, oldest first.
// ponytail: reads every tenant file per lookup; keep a name index when the
// service holds thousands of accounts.
pub fn by_name(app_dir: &Path, name: &str) -> Vec<Tenant> {
    list(app_dir)
        .into_iter()
        .filter(|t| t.name.as_deref() == Some(name))
        .collect()
}

/// The wallets whose history is this account's: `wallets`, then `watched`,
/// de-duplicated, **never the zero address** — every USDC mint is a Transfer
/// from it, so reading its logs reads the whole token (docs/79 §6a #3).
pub fn history_sources(t: &Tenant) -> Vec<Address> {
    let mut out: Vec<Address> = Vec::new();
    for a in t.wallets.iter().chain(std::iter::once(&t.watched)) {
        if let Ok(a) = Address::from_str(a.trim())
            && !a.is_zero()
            && !out.contains(&a)
        {
            out.push(a);
        }
    }
    out
}

/// Where the read-only preview of a reserved account's wallets is kept.
pub fn preview_path(app_dir: &Path, policy: &str) -> PathBuf {
    tenants_dir(app_dir).join(format!("{}.preview.json", policy.to_lowercase()))
}

/// The history sources as the preview and the history cache record them.
pub fn sources_key(t: &Tenant) -> Value {
    let mut s: Vec<String> = history_sources(t)
        .iter()
        .map(|a| format!("{a:#x}"))
        .collect();
    s.sort();
    json!(s)
}

/// The caps a name-first account reserves with before any bill is known.
pub const PROVISIONAL_PER_ACTION_MINOR: i128 = 50_000_000;
pub const PROVISIONAL_DAILY_MINOR: i128 = 100_000_000;

/// docs/79 §6a #5: while reserved and the owner typed no caps, the caps
/// follow the bills the service knows -- the bills typed into the record and
/// the bills the read of its wallets found (the preview, if it read the
/// wallets the record lists now) -- at twice the largest, per payment and per
/// day; none known: 50 and 100 USDC. The floor stays as reserved.
pub fn refresh_provisional_caps(app_dir: &Path, t: &mut Tenant) {
    if t.deployed || !t.caps_provisional {
        return;
    }
    let minor = |s: &str| {
        Money::parse(s.trim_end_matches(" USDC").trim(), Currency::Usdc)
            .map(|m| m.to_minor())
            .ok()
    };
    let found: Vec<i128> = fs::read_to_string(preview_path(app_dir, &t.policy))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(|v| v["sources"] == sources_key(t))
        .and_then(|v| v["bills"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|b| b["amount"].as_str().and_then(minor))
        .collect();
    let largest = t
        .bills
        .iter()
        .filter_map(|b| minor(&b.amount))
        .chain(found)
        .max()
        .filter(|m| *m > 0);
    let (per, day) = match largest {
        Some(m) => (m.saturating_mul(2), m.saturating_mul(2)),
        None => (PROVISIONAL_PER_ACTION_MINOR, PROVISIONAL_DAILY_MINOR),
    };
    t.per_action = Money::minor(per, Currency::Usdc).to_string();
    t.daily = Money::minor(day, Currency::Usdc).to_string();
}

/// One minted capability (PLAN 5.31).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentToken {
    pub id: String,
    pub label: String,
    pub scopes: Vec<String>,
    pub expires: u64,
    pub issued: u64,
    /// sha256 of the bearer secret, hex.
    pub hash: String,
}

/// The scopes a token may carry, and what each unlocks.
pub const TOKEN_SCOPES: &[(&str, &str)] = &[
    (
        "read",
        "the inbox, the records and the events behind a root",
    ),
    ("run", "ask the agent to check the bills now"),
    (
        "prepare",
        "calldata for the owner's passkey (payees, limits, mode, funding, orders, renewals)",
    ),
    ("import", "tell the agent about bills"),
    (
        "buy",
        "buy a paid API (x402) from a payee you approved, inside the caps",
    ),
];

/// Which scope a route needs, or `None` when it is open (the passkey signs
/// it anyway, the chain bounds it, or it is the token flow itself).
pub fn scope_for(method: &str, path: &str) -> Option<&'static str> {
    let p = path.strip_prefix("/api/level1/")?;
    let p = p.split('?').next().unwrap_or(p);
    Some(match (method, p) {
        // `records` and `record` are not here: the anchored roots are what the
        // page tells anyone they can check, so a key for the agents never hides
        // them (docs/60 P-176). On a locked account `record` serves an outsider
        // each event's leaf hash, enough to rebuild the root, and the events only
        // to `read` or the owner's device (P-212). Bills, payees' names,
        // purchases and the statement stay behind `read`.
        ("GET", "inbox") | ("GET", "purchases.jsonl") | ("GET", "statement") => "read",
        ("POST", "run") => "run",
        // `wallets` is not here: its handler gates it (the device token or an
        // `import` agent key, never open; docs/60 P-93), because this gate
        // would refuse a device-token holder on an account with agent keys.
        ("POST", "bills") | ("POST", "invoice-decide") => "import",
        ("POST", "buy") => "buy",
        ("POST", x)
            if x.starts_with("prepare-")
                && !x.starts_with("prepare-token")
                && x != "prepare-notify" =>
        {
            "prepare"
        }
        ("POST", "fund") => "prepare",
        _ => return None,
    })
}

pub fn token_hash(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    alloy::hex::encode(Sha256::digest(secret.trim().as_bytes()))
}

/// A partner (PLAN 5.45): an accounting firm or a payments company that
/// brings its clients. Kept in `partners.json` beside the tenants; only the
/// sha256 of its key is stored (`scripts/partner.sh add` prints the key once).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Partner {
    pub id: String,
    pub name: String,
    pub token_hash: String,
    pub created: u64,
}

pub fn partners(app_dir: &Path) -> Vec<Partner> {
    fs::read_to_string(app_dir.join("partners.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The partner a bearer key belongs to, if any.
pub fn partner_for_key(app_dir: &Path, key: &str) -> Option<Partner> {
    let h = token_hash(key);
    partners(app_dir).into_iter().find(|p| p.token_hash == h)
}

/// The 32 bytes the passkey signs to mint a token: a tag, the chain, the
/// policy, the scopes, the label, the expiry and when it was issued.
pub fn token_challenge(
    chain_id: u64,
    policy: Address,
    scopes: &[String],
    label: &str,
    expires: u64,
    issued: u64,
) -> B256 {
    let mut m = b"tameion.token.v1".to_vec();
    m.extend_from_slice(&chain_id.to_be_bytes());
    m.extend_from_slice(policy.as_slice());
    let joined = scopes.join(",");
    m.extend_from_slice(&(joined.len() as u32).to_be_bytes());
    m.extend_from_slice(joined.as_bytes());
    m.extend_from_slice(&(label.len() as u32).to_be_bytes());
    m.extend_from_slice(label.as_bytes());
    m.extend_from_slice(&expires.to_be_bytes());
    m.extend_from_slice(&issued.to_be_bytes());
    alloy::primitives::keccak256(m)
}

/// The 32 bytes the passkey signs to accept or decline invoice `id`: a tag,
/// the chain, the policy, the invoice id, the decision (1 = accept) and when it
/// was issued. Accepting a repeating invoice turns it into a bill the agent pays
/// in Autopilot, so on an account without agent keys only the owner's passkey
/// may decide (audit 2026-10-15, docs/60 P-73: a vendor could accept its own).
pub fn invoice_decide_challenge(
    chain_id: u64,
    policy: Address,
    id: B256,
    accept: bool,
    issued: u64,
) -> B256 {
    let mut m = b"tameion.invoice-decide.v1".to_vec();
    m.extend_from_slice(&chain_id.to_be_bytes());
    m.extend_from_slice(policy.as_slice());
    m.extend_from_slice(id.as_slice());
    m.push(u8::from(accept));
    m.extend_from_slice(&issued.to_be_bytes());
    alloy::primitives::keccak256(m)
}

/// The 32 bytes the passkey signs to revoke token `id`.
pub fn token_revoke_challenge(chain_id: u64, policy: Address, id: &str, issued: u64) -> B256 {
    let mut m = b"tameion.token-revoke.v1".to_vec();
    m.extend_from_slice(&chain_id.to_be_bytes());
    m.extend_from_slice(policy.as_slice());
    m.extend_from_slice(id.as_bytes());
    m.extend_from_slice(&issued.to_be_bytes());
    alloy::primitives::keccak256(m)
}

/// Does `bearer` unlock `scope` on this tenant now? A tenant with no tokens is
/// open (the owner never asked for gating). Otherwise the token must exist,
/// be unexpired, and carry the scope.
pub fn token_allows(
    t: &Tenant,
    bearer: Option<&str>,
    scope: &str,
    now: u64,
) -> Result<(), &'static str> {
    // Buying is closed until the owner opens it (PLAN 5.54): an account with
    // no agent keys is open to read and run, never to spend on services.
    if t.agent_tokens.is_empty() {
        return if scope == "buy" {
            Err(
                "buying a service needs an agent key with the \"buy\" scope; the owner mints one with the passkey on the account page",
            )
        } else {
            Ok(())
        };
    }
    let Some(b) = bearer.map(str::trim).filter(|b| !b.is_empty()) else {
        return Err(
            "this account's owner requires an agent key: send Authorization: Bearer <token> (the owner mints one on the account page)",
        );
    };
    let h = token_hash(b);
    let Some(tok) = t.agent_tokens.iter().find(|x| x.hash == h) else {
        return Err("unknown agent key");
    };
    if now >= tok.expires {
        return Err("this agent key has expired; the owner can mint a new one");
    }
    if !tok.scopes.iter().any(|s| s == scope) {
        return Err("this agent key does not carry the scope this call needs");
    }
    Ok(())
}

/// One imported bill: who, how much, how often, and when the next one is due.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SeededBill {
    pub payee: String,
    pub amount: String,
    pub every_days: u32,
    pub next_due: u64,
    /// Early-payment terms on the invoice (PLAN 5.56): this many basis points
    /// off when paid at least `early_days` before the due date; 0 = none.
    #[serde(default)]
    pub discount_bps: u16,
    #[serde(default)]
    pub early_days: u32,
}

/// The early-payment terms among imported bills, for the pass (PLAN 5.56).
pub fn early_terms(bills: &[SeededBill]) -> Vec<crate::EarlyTerms> {
    bills
        .iter()
        .filter(|b| b.discount_bps > 0 && b.early_days > 0)
        .filter_map(|b| {
            Some(crate::EarlyTerms {
                counterparty: ledger::Counterparty::new(&b.payee),
                lead: Seconds::days(i64::from(b.early_days))?,
                discount_bps: b.discount_bps,
            })
        })
        .collect()
}

impl Tenant {
    /// The tenant as a reply may carry it: everything a page needs, never
    /// the webhook secret (an HMAC key) nor the agent-key hashes. Found
    /// leaking on the public tenant route 2026-10-14 (docs/60 P-17).
    pub fn public(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(o) = v.as_object_mut() {
            o.remove("notify_secret");
            o.remove("agent_tokens");
            o.remove("reserve_token_hash");
            o.remove("contacts");
            // Audit 2026-10-15 (docs/60 P-70): this view is served without a
            // key, even for a locked account. A webhook URL is often itself a
            // bearer secret (a Slack hook), and the chat id, the imported bills
            // and the devices asking to join are the owner's business; the
            // pages read them from the gated inbox. Kept here as yes/no and counts.
            o.remove("notify_url");
            o.remove("telegram_chat_id");
            o.remove("bills");
            o.remove("join_requests");
            o.insert(
                "notifySet".into(),
                Value::Bool(self.notify_secret.is_some()),
            );
            o.insert("telegramSet".into(), json!(self.telegram_chat_id.is_some()));
            o.insert("billCount".into(), json!(self.bills.len()));
            o.insert("agentTokenCount".into(), json!(self.agent_tokens.len()));
        }
        v
    }
}

fn kind_hold() -> String {
    "hold".into()
}

fn default_true() -> bool {
    true
}

pub struct DeployRequest {
    pub chain: &'static ChainPreset,
    /// Pull mode: deploy `BudgetPolicyPull` with `watched` as the source.
    pub pull: bool,
    pub qx: B256,
    pub qy: B256,
    pub recovery: Address,
    pub watched: Address,
    pub per_action: Money,
    pub daily: Money,
    pub floor: Money,
    pub days: u32,
    pub notify_url: Option<String>,
    /// docs/79 §6a: the handle, already validated and found free by the caller.
    pub name: Option<String>,
    /// The body named `recovery` itself.
    pub recovery_explicit: bool,
    /// The body named no caps: `per_action`/`daily` are the provisional
    /// default and follow the known bills until deploy.
    pub caps_provisional: bool,
    /// No agent key at all (PLAN 5.38): the policy's agent is
    /// [`KEYLESS_AGENT`]; the account stays in Propose.
    pub keyless: bool,
    /// The company's own wallet becomes the account by EIP-7702 (PLAN 5.28):
    /// the policy's address is `watched` itself; the wallet signs one
    /// authorization to [`delegate_address`] and initialises itself.
    pub wallet: bool,
}

/// The agent of a keyless account (PLAN 5.38, docs/62 F3b): an address no
/// one holds a key for, so `pay` can never be called on that policy. The
/// owner pays (one tap per bill, or one tap for all), standing orders pay
/// themselves, and the service only reads, proposes and anchors the log.
pub const KEYLESS_AGENT: &str = "0x000000000000000000000000000000000000dEaD";

/// The CREATE2 salt of a passkey's principal: the key itself under a version
/// tag, so the same passkey gets the same contract address on every chain.
pub fn principal_salt(qx: B256, qy: B256) -> B256 {
    let mut m = b"tameion.principal.v1".to_vec();
    m.extend_from_slice(qx.as_slice());
    m.extend_from_slice(qy.as_slice());
    alloy::primitives::keccak256(m)
}

/// The CREATE2 salt of the 7702 delegate implementation: one per code
/// version, the same address on every chain with the EIP-7997 factory.
pub fn delegate_salt() -> B256 {
    alloy::primitives::keccak256(b"tameion.delegate.v1")
}

/// Where the delegate implementation lives (or will, after the first wallet
/// account on a chain puts it there through the factory).
pub fn delegate_address() -> Result<Address, String> {
    let factory = arc::EIP7997_FACTORY
        .parse::<Address>()
        .expect("factory address");
    let code = alloy::hex::decode(TAMEION_DELEGATE_BYTECODE.trim()).map_err(|e| e.to_string())?;
    Ok(arc::predict_create2(factory, delegate_salt(), &code))
}

/// The call the wallet makes to itself in the delegating transaction:
/// `initialize(owner, agent, operator, registry, agentId, caps, floor, delay,
/// Propose)` — the same terms a deployed policy would have.
pub fn delegate_initialize_calldata(t: &Tenant) -> Result<String, String> {
    use alloy::sol_types::SolCall;
    alloy::sol! {
        function initialize(address principal, address agent, address operator, address identityRegistry, uint256 agentId, uint256 perActionCap, uint256 dailyCap, uint256 reserveFloor, uint256 delay, uint8 mode) external;
    }
    let a = |x: &str| Address::from_str(x).map_err(|e| e.to_string());
    let m = |x: &str| {
        Money::parse(x.trim_end_matches(" USDC"), Currency::Usdc)
            .map(money_u256)
            .map_err(|e| e.to_string())
    };
    let call = initializeCall {
        principal: a(&t.owner)?,
        agent: a(&t.agent)?,
        operator: a(&t.operator)?,
        identityRegistry: Address::from_str(REGISTRY).expect("registry"),
        agentId: agent_id(),
        perActionCap: m(&t.per_action)?,
        dailyCap: m(&t.daily)?,
        reserveFloor: m(&t.floor)?,
        delay: U256::from(t.delay_secs),
        mode: 1,
    };
    Ok(format!("0x{}", alloy::hex::encode(call.abi_encode())))
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// The key that pays gas and acts as operator. Testnet: the treasury key by
/// default; a hosted deployment sets its own.
pub fn relayer_key() -> Result<String, String> {
    // A host whose secrets are environment variables (Fly, Docker) sets the
    // key itself; a laptop keeps it in a mode-600 file outside the tree.
    if let Ok(k) = std::env::var("TAMEION_RELAYER_KEY") {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return Ok(k);
        }
    }
    let f = std::env::var("TAMEION_RELAYER_KEY_FILE")
        .unwrap_or_else(|_| format!("{}/.tameion/treasury.key", home()));
    fs::read_to_string(&f)
        .map(|k| k.trim().to_string())
        .map_err(|e| format!("relayer key {f}: {e}"))
}

pub fn relayer_address() -> Result<Address, String> {
    let k = relayer_key()?;
    PrivateKeySigner::from_str(&k)
        .map(|s| s.address())
        .map_err(|e| e.to_string())
}

/// `MemoryAnchor` for the chain: `TAMEION_ANCHOR`, else the Arc testnet
/// anchor recorded by `scripts/prove_arc.sh deploy`.
pub fn anchor_for(c: &ChainPreset) -> Result<Address, String> {
    let s = match std::env::var("TAMEION_ANCHOR") {
        Ok(a) => a,
        Err(_) if c.key == "arc-testnet" => {
            fs::read_to_string("runtime/arc_anchor.addr").map_err(|_| {
                "TAMEION_ANCHOR not set and runtime/arc_anchor.addr missing".to_string()
            })?
        }
        // Other chains (PLAN 5.58): runtime/<key>_anchor.addr.
        Err(_) => {
            let p = format!("runtime/{}_anchor.addr", c.key);
            fs::read_to_string(&p).map_err(|_| format!("TAMEION_ANCHOR not set and {p} missing"))?
        }
    };
    Address::from_str(s.trim()).map_err(|e| e.to_string())
}

pub fn agent_id() -> U256 {
    std::env::var("TAMEION_AGENT_ID")
        .ok()
        .and_then(|v| U256::from_str(&v).ok())
        .unwrap_or(U256::from(896_918u64))
}

/// The delay on trust-expanding changes (docs/46 #3). 60 s on testnets so a
/// proof fits a session; 172800 (48 h) is the recommended production value.
pub fn delay_secs() -> u64 {
    std::env::var("TAMEION_POLICY_DELAY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60)
}

pub fn tenants_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("tenants")
}

/// Write the tenant through a temporary file and a rename, so a reader sees
/// the old file or the new one, never half of one (docs/60 P-221: a read that
/// landed mid-write found no tenant, and the record route then served a locked
/// account's events in full).
pub fn save(app_dir: &Path, t: &Tenant) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let d = tenants_dir(app_dir);
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let file = d.join(format!("{}.json", t.policy.to_lowercase()));
    let tmp = d.join(format!(
        "{}.json.{}-{}.tmp",
        t.policy.to_lowercase(),
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::write(
        &tmp,
        serde_json::to_string_pretty(t).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(&tmp, &file).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Whether this policy has a tenant file here, readable or not: a gate that
/// finds the file but cannot read it refuses rather than opening (P-221).
pub fn tenant_file_exists(app_dir: &Path, policy: &str) -> bool {
    Address::from_str(policy.trim())
        .is_ok_and(|a| tenants_dir(app_dir).join(format!("{a:#x}.json")).exists())
}

/// The tenant whose policy is `policy`. Only an address names a file: a
/// string like "./0x…" or "../x" once resolved to a tenant while the token
/// gate, which parses addresses, saw no tenant and let the call through
/// (docs/60 P-45, the session-5 review's critical finding).
pub fn load(app_dir: &Path, policy: &str) -> Option<Tenant> {
    let a = Address::from_str(policy.trim()).ok()?;
    let p = tenants_dir(app_dir).join(format!("{a:#x}.json"));
    serde_json::from_str(&fs::read_to_string(p).ok()?).ok()
}

pub fn list(app_dir: &Path) -> Vec<Tenant> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(tenants_dir(app_dir)) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "json")
                && let Ok(s) = fs::read_to_string(e.path())
                && let Ok(t) = serde_json::from_str::<Tenant>(&s)
            {
                out.push(t);
            }
        }
    }
    out.sort_by_key(|t| t.created);
    out
}

fn agent_key_path(app_dir: &Path, owner: Address) -> PathBuf {
    tenants_dir(app_dir)
        .join(format!("{owner:#x}"))
        .join("agent.key")
}

/// Where a reserved contract's exact creation code is kept (docs/60 P-28):
/// `tenants/<owner>/initcode-<address>.hex`. A lazily reserved address is a
/// CREATE2 of the code as it was at reservation; recomputing it from a later
/// build would land somewhere else and strand whatever was sent there.
fn initcode_path(app_dir: &Path, owner: Address, at: Address) -> PathBuf {
    tenants_dir(app_dir)
        .join(format!("{owner:#x}"))
        .join(format!("initcode-{at:#x}.hex"))
}

fn keep_initcode(app_dir: &Path, owner: Address, at: Address, code: &[u8]) -> Result<(), String> {
    let p = initcode_path(app_dir, owner, at);
    fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&p, format!("{}\n", alloy::hex::encode_prefixed(code))).map_err(|e| e.to_string())
}

/// The creation code that lands at the reserved `at`: the kept file, or —
/// for a reservation made before codes were kept — today's code, but only
/// if it still predicts `at`. Never deploys to an address nobody reserved.
fn initcode_for(
    app_dir: &Path,
    owner: Address,
    at: Address,
    salt: B256,
    today: Vec<u8>,
) -> Result<Vec<u8>, String> {
    if let Ok(hex) = fs::read_to_string(initcode_path(app_dir, owner, at)) {
        return alloy::hex::decode(hex.trim()).map_err(|e| e.to_string());
    }
    let factory = arc::EIP7997_FACTORY
        .parse::<Address>()
        .expect("factory address");
    if arc::predict_create2(factory, salt, &today) != at {
        return Err(format!(
            "{at:#x} was reserved under an earlier build of the contracts and its creation code was not kept; nothing was sent (docs/60 P-28)"
        ));
    }
    Ok(today)
}

fn money_u256(m: Money) -> U256 {
    U256::from(m.to_minor().max(0) as u128)
}

fn signed_client(c: &ChainPreset, key: &str) -> Result<ArcClient, String> {
    let mut client = ArcClient::with_signer(&rpc_for(c), key)
        .map_err(|e| e.to_string())?
        .with_log_chunk(c.log_span);
    if let Some(t) = c.usdc {
        client = client.with_token(Address::from_str(t).expect("preset token"));
    }
    Ok(client)
}

/// Which of `who` the tenant's stablecoin issuer has frozen (PLAN 5.47, F10),
/// as `0x…` strings. Asked before a payee is attested or imported, so a
/// frozen address never becomes a bill; empty when the token has no list or
/// the chain is unknown.
pub async fn issuer_frozen(t: &Tenant, who: &[Address]) -> Vec<String> {
    let Some(c) = crate::watch::chain(&t.chain) else {
        return Vec::new();
    };
    match read_only_client(c) {
        Ok(client) => client.issuer_blocked_among(who).await.into_iter().collect(),
        Err(_) => Vec::new(),
    }
}

/// The words a frozen payee is refused with.
pub fn frozen_words(frozen: &[String]) -> String {
    format!(
        "Circle has frozen {} on USDC: the token refuses transfers to it, so it cannot be a payee here",
        frozen.join(", ")
    )
}

/// A read-only client for `c` (the x402 buyer's chain reads).
pub fn read_client(c: &ChainPreset) -> Result<ArcClient, String> {
    read_only_client(c)
}

/// The agent key already kept for `owner`, never minting one.
pub fn agent_key(app_dir: &Path, owner: Address) -> Option<PrivateKeySigner> {
    let k = fs::read_to_string(agent_key_path(app_dir, owner)).ok()?;
    PrivateKeySigner::from_str(k.trim()).ok()
}

fn read_only_client(c: &ChainPreset) -> Result<ArcClient, String> {
    crate::watch::client_for(c).map_err(|e| e.to_string())
}

/// A read-only client for a person waiting on a page: a stalled read fails in
/// seconds (`ArcClient::read_only_quick`) instead of retrying for a minute
/// (docs/110 item 8). The account page's inbox and status use it.
pub fn quick_client(c: &ChainPreset) -> Result<ArcClient, String> {
    let mut client = ArcClient::read_only_quick(&crate::watch::rpc_for(c))
        .map_err(|e| e.to_string())?
        .with_log_chunk(c.log_span);
    if let Some(t) = c.usdc {
        client = client.with_token(Address::from_str(t).map_err(|e| e.to_string())?);
    }
    Ok(client)
}

/// Deploy the treasurer's `PasskeyPrincipal` and `BudgetPolicy` (Propose mode)
/// with the relayer key; generate and keep the tenant's agent key; record
/// the tenant. The policy holds nothing until the treasurer funds it.
/// The PasskeyPrincipal creation code for a passkey, recovery address and delay.
fn principal_initcode(
    qx: B256,
    qy: B256,
    recovery: Address,
    delay: u64,
) -> Result<Vec<u8>, String> {
    let mut code =
        alloy::hex::decode(PASSKEY_PRINCIPAL_BYTECODE.trim()).map_err(|e| e.to_string())?;
    code.extend_from_slice(&(qx, qy, recovery, U256::from(delay)).abi_encode());
    Ok(code)
}

/// The policy's creation code (hold or pull) for its constructor arguments.
#[allow(clippy::too_many_arguments)]
fn policy_initcode(
    c: &ChainPreset,
    pull: bool,
    watched: Address,
    owner: Address,
    agent: Address,
    operator: Address,
    per_action: Money,
    daily: Money,
    floor: Money,
    delay: u64,
) -> Result<Vec<u8>, String> {
    let registry = Address::from_str(REGISTRY).expect("registry");
    let (mut code, args) = if pull {
        let usdc = Address::from_str(c.usdc.unwrap_or(ARC_USDC_FACADE)).expect("usdc address");
        (
            alloy::hex::decode(BUDGET_POLICY_PULL_BYTECODE.trim()).map_err(|e| e.to_string())?,
            (
                usdc,
                watched,
                owner,
                agent,
                operator,
                registry,
                agent_id(),
                money_u256(per_action),
                money_u256(daily),
                money_u256(floor),
                U256::from(delay),
                U256::from(1u8),
            )
                .abi_encode(),
        )
    } else if let Some(token) = c.usdc {
        // ERC-20 USDC (PLAN 5.58): the token comes first in the constructor.
        let usdc = Address::from_str(token).expect("usdc address");
        (
            alloy::hex::decode(BUDGET_POLICY_TOKEN_BYTECODE.trim()).map_err(|e| e.to_string())?,
            (
                usdc,
                owner,
                agent,
                operator,
                registry,
                agent_id(),
                money_u256(per_action),
                money_u256(daily),
                money_u256(floor),
                U256::from(delay),
                U256::from(1u8),
            )
                .abi_encode(),
        )
    } else {
        (
            alloy::hex::decode(BUDGET_POLICY_BYTECODE.trim()).map_err(|e| e.to_string())?,
            (
                owner,
                agent,
                operator,
                registry,
                agent_id(),
                money_u256(per_action),
                money_u256(daily),
                money_u256(floor),
                U256::from(delay),
                // Mode.Propose; a uint8 is the same 32-byte word as a uint256 here.
                U256::from(1u8),
            )
                .abi_encode(),
        )
    };
    code.extend_from_slice(&args);
    Ok(code)
}

/// The CREATE2 salt of a policy: one per (owner, bill wallet, kind), so the
/// address is known before anything is sent and a second deploy is a no-op.
pub fn policy_salt(owner: Address, watched: Address, pull: bool) -> B256 {
    let mut m = b"tameion.policy.v1".to_vec();
    m.extend_from_slice(owner.as_slice());
    m.extend_from_slice(watched.as_slice());
    m.push(u8::from(pull));
    alloy::primitives::keccak256(m)
}

/// The agent key for `owner`: reused when the file exists (one passkey, many
/// policies, one agent), minted otherwise.
fn agent_signer(app_dir: &Path, owner: Address) -> Result<PrivateKeySigner, String> {
    let kp = agent_key_path(app_dir, owner);
    match fs::read_to_string(&kp) {
        Ok(k) => PrivateKeySigner::from_str(k.trim())
            .map_err(|e| format!("agent key {}: {e}", kp.display())),
        Err(_) => {
            let fresh = PrivateKeySigner::random();
            fs::create_dir_all(kp.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::write(
                &kp,
                format!("{}\n", alloy::hex::encode_prefixed(fresh.to_bytes())),
            )
            .map_err(|e| e.to_string())?;
            fs::set_permissions(&kp, fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
            Ok(fresh)
        }
    }
}

/// Reserve a tenant without sending anything (PLAN 5.32, lazy deploy): the
/// owner's and the policy's addresses are computed from the factory, the
/// agent key is minted, the file is written with `deployed: false`. Money
/// may be sent or allowed to the policy's address before it exists; the
/// first pass or the first passkey action materialises both contracts.
/// A new record comes with its reserve token (docs/60 P-87); an existing one
/// handed back to the same passkey's public key comes without.
pub async fn reserve(app_dir: &Path, r: DeployRequest) -> Result<(Tenant, Option<String>), String> {
    // Pull draws from `watched`; a wallet account *is* `watched`. Neither
    // means anything with the zero address (docs/79 §6a #2).
    if (r.pull || r.wallet) && r.watched.is_zero() {
        return Err("a pull or wallet account needs the wallet it draws from (watched)".into());
    }
    // The wallet-as-account code pays native USDC (PLAN 5.28): Arc only.
    if r.wallet && r.chain.usdc.is_some() {
        return Err(format!(
            "making the wallet itself the account works on Arc only for now; on {} keep the money in the wallet (pull) or in the account",
            r.chain.name
        ));
    }
    let client = read_only_client(r.chain)?;
    client
        .expect_chain(r.chain.chain_id)
        .await
        .map_err(|e| e.to_string())?;
    let operator = relayer_address()?;
    let head = client.block_number().await.map_err(|e| e.to_string())?;
    let delay = delay_secs();
    let factory = arc::EIP7997_FACTORY
        .parse::<Address>()
        .expect("factory address");
    // One passkey, one owner (PLAN 5.35): a passkey that already has an owner
    // contract on this chain keeps it, whatever build of the code put it
    // there. Predicting from today's code would give a second owner at a new
    // address each time the contracts change (seen 2026-10-14: 0x7b84… then
    // 0xc7a9… for the same key; docs/60 P-33).
    let same_key = |t: &Tenant| {
        t.chain == r.chain.key
            && t.qx
                .as_deref()
                .is_some_and(|x| B256::from_str(x).ok() == Some(r.qx))
            && t.qy
                .as_deref()
                .is_some_and(|y| B256::from_str(y).ok() == Some(r.qy))
    };
    // Reused only if the chain shows this key AND this recovery address: an
    // owner contract someone else deployed for the same public key with their
    // own recovery would let them take the account over by recovery (audit
    // 2026-10-15, docs/60 P-75).
    let mut existing = None;
    for t in list(app_dir).into_iter().filter(same_key) {
        if let Ok(o) = Address::from_str(&t.owner)
            && !client.code_at(o).await.unwrap_or_default().is_empty()
            && client
                .passkey_owner(o)
                .await
                .is_ok_and(|(x, y, rec)| x == r.qx && y == r.qy && rec == r.recovery)
        {
            existing = Some(o);
            break;
        }
    }
    let owner = match existing {
        Some(o) => o,
        None => {
            let owner_code = principal_initcode(r.qx, r.qy, r.recovery, delay)?;
            let o = arc::predict_create2(factory, principal_salt(r.qx, r.qy), &owner_code);
            keep_initcode(app_dir, o, o, &owner_code)?;
            o
        }
    };
    // A keyless account mints no agent key at all (PLAN 5.38).
    let agent_addr = if r.keyless {
        Address::from_str(KEYLESS_AGENT).expect("keyless agent")
    } else {
        agent_signer(app_dir, owner)?.address()
    };
    // A wallet account (PLAN 5.28) is the wallet itself; nothing to predict.
    // Its delegate code must exist before the wallet points at it: a
    // delegation to an empty address would run an empty `initialize` and
    // leave the wallet uninitialised without an error. Once per chain.
    let policy = if r.wallet {
        let code =
            alloy::hex::decode(TAMEION_DELEGATE_BYTECODE.trim()).map_err(|e| e.to_string())?;
        signed_client(r.chain, &relayer_key()?)?
            .deploy_deterministic(delegate_salt(), &code)
            .await
            .map_err(|e| format!("deploy TameionDelegate: {e}"))?;
        r.watched
    } else {
        let code = policy_initcode(
            r.chain,
            r.pull,
            r.watched,
            owner,
            agent_addr,
            operator,
            r.per_action,
            r.daily,
            r.floor,
            delay,
        )?;
        let at = arc::predict_create2(factory, policy_salt(owner, r.watched, r.pull), &code);
        keep_initcode(app_dir, owner, at, &code)?;
        at
    };

    let now = client
        .block_timestamp(head)
        .await
        .map_err(|e| e.to_string())?;
    let probe = head.saturating_sub(10_000);
    let then = client
        .block_timestamp(probe)
        .await
        .map_err(|e| e.to_string())?;
    let spb = ((now - then).as_i64().max(1) as f64) / ((head - probe).max(1) as f64);
    let window = ((r.days as f64) * 86_400.0 / spb).ceil() as u64;

    let mut t = Tenant {
        chain: r.chain.key.to_string(),
        kind: if r.wallet {
            "wallet".into()
        } else if r.pull {
            "pull".into()
        } else {
            "hold".into()
        },
        deployed: false,
        keyless: r.keyless,
        partner: None,
        join_requests: Vec::new(),
        qx: Some(format!("{:#x}", r.qx)),
        qy: Some(format!("{:#x}", r.qy)),
        owner: format!("{owner:#x}"),
        policy: format!("{policy:#x}"),
        agent: format!("{agent_addr:#x}"),
        operator: format!("{operator:#x}"),
        watched: format!("{:#x}", r.watched),
        recovery: format!("{:#x}", r.recovery),
        per_action: r.per_action.to_string(),
        daily: r.daily.to_string(),
        floor: r.floor.to_string(),
        delay_secs: delay,
        mode: 1,
        created_block: head,
        history_from_block: head.saturating_sub(window),
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        owner_tx: None,
        policy_tx: None,
        agent_gas_tx: None,
        notify_url: r.notify_url.clone(),
        notify_secret: r.notify_url.as_ref().map(|_| new_secret()),
        bills: Vec::new(),
        telegram_chat_id: None,
        agent_tokens: Vec::new(),
        name: r.name.clone(),
        // An account opened from a wallet reads it, as before (§6a #2).
        wallets: if r.watched.is_zero() {
            Vec::new()
        } else {
            vec![format!("{:#x}", r.watched)]
        },
        recovery_explicit: r.recovery_explicit,
        spare_key: None,
        caps_provisional: r.caps_provisional,
        reserve_token_hash: None,
        contacts: Vec::new(),
    };
    // Never over an existing account's record (audit 2026-10-15, P-75): it
    // carries the owner, the agent keys that lock it, bills and settings. The
    // same passkey asking again gets its account back unchanged -- and without
    // a reserve token: qx/qy are public, so matching them proves nothing
    // (docs/60 P-87). A new token takes the passkey (`reserve_token_challenge`).
    if let Some(old) = load(app_dir, &t.policy) {
        if old.qx == t.qx && old.qy == t.qy && old.owner.eq_ignore_ascii_case(&t.owner) {
            return Ok((old, None));
        }
        return Err("an account already exists at that address".into());
    }
    let tok = mint_reserve_token(&mut t);
    save(app_dir, &t)?;
    Ok((t, Some(tok)))
}

/// Put a reserved tenant on chain (idempotent): the principal and the policy
/// through the factory at the addresses already recorded, then gas for the
/// agent. Called before the first pass and before the first passkey action.
pub async fn ensure_deployed(app_dir: &Path, t: &mut Tenant) -> Result<(), String> {
    if t.deployed {
        return Ok(());
    }
    let c = crate::watch::chain(&t.chain).ok_or("tenant chain unknown")?;
    let (Some(qx), Some(qy)) = (t.qx.as_deref(), t.qy.as_deref()) else {
        return Err(
            "this tenant was reserved without its passkey key; it cannot be materialised".into(),
        );
    };
    let qx = B256::from_str(qx).map_err(|e| e.to_string())?;
    let qy = B256::from_str(qy).map_err(|e| e.to_string())?;
    let recovery = Address::from_str(&t.recovery).map_err(|e| e.to_string())?;
    let watched = Address::from_str(&t.watched).map_err(|e| e.to_string())?;
    let owner = Address::from_str(&t.owner).map_err(|e| e.to_string())?;
    let agent = Address::from_str(&t.agent).map_err(|e| e.to_string())?;
    let per_action = Money::parse(t.per_action.trim_end_matches(" USDC"), Currency::Usdc)
        .map_err(|e| e.to_string())?;
    let daily = Money::parse(t.daily.trim_end_matches(" USDC"), Currency::Usdc)
        .map_err(|e| e.to_string())?;
    let floor = Money::parse(t.floor.trim_end_matches(" USDC"), Currency::Usdc)
        .map_err(|e| e.to_string())?;
    let relayer = relayer_key()?;
    let client = signed_client(c, &relayer)?;
    let operator = client.address().ok_or("relayer has no address")?;
    // Say it in words before the node says "-32003 insufficient funds" half
    // way through (docs/60 P-14): the relayer pays for both contracts.
    let have = client
        .native_wei(operator)
        .await
        .map_err(|e| format!("relayer balance: {e}"))?;
    // An owner already on chain (a second account for the same passkey)
    // leaves only the policy to pay for: half the floor.
    let owner_live = !client.code_at(owner).await.unwrap_or_default().is_empty();
    let need = if owner_live {
        relayer_floor_wei(c) / U256::from(2)
    } else {
        relayer_floor_wei(c)
    };
    if have < need {
        return Err(format!(
            "the relayer {operator:#x} holds {} and needs at least {} on {} to put an account on chain; top it up and run the check again",
            native_text(c, have),
            native_text(c, need),
            c.name
        ));
    }
    // An owner already live (this passkey's other account) is not redeployed.
    if !owner_live {
        let (got_owner, owner_tx) = client
            .deploy_deterministic(
                principal_salt(qx, qy),
                &initcode_for(
                    app_dir,
                    owner,
                    owner,
                    principal_salt(qx, qy),
                    principal_initcode(qx, qy, recovery, t.delay_secs)?,
                )?,
            )
            .await
            .map_err(|e| format!("deploy PasskeyPrincipal: {e}"))?;
        if got_owner != owner {
            return Err(format!(
                "the owner contract landed at {got_owner:#x}, not the reserved {owner:#x}"
            ));
        }
        // Record each step as it lands: a failure on the next one (gas, RPC)
        // must not lose this hash, and the retry is idempotent anyway.
        if owner_tx.is_some() {
            t.owner_tx = owner_tx.map(|h| format!("{h:#x}"));
            save(app_dir, t)?;
        }
    }
    if t.kind == "wallet" {
        // PLAN 5.28: the account is the wallet itself. Put the delegate code on
        // chain (once per chain, through the factory), then check the wallet
        // did its part: its code points at the delegate and it initialised
        // itself with this owner. The service cannot sign for the wallet.
        let code =
            alloy::hex::decode(TAMEION_DELEGATE_BYTECODE.trim()).map_err(|e| e.to_string())?;
        let (implementation, impl_tx) = client
            .deploy_deterministic(delegate_salt(), &code)
            .await
            .map_err(|e| format!("deploy TameionDelegate: {e}"))?;
        if impl_tx.is_some() {
            t.policy_tx = impl_tx.map(|h| format!("{h:#x}"));
            save(app_dir, t)?;
        }
        let wallet = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
        let mut want = vec![0xef, 0x01, 0x00];
        want.extend_from_slice(implementation.as_slice());
        let have = client.code_at(wallet).await.map_err(|e| e.to_string())?;
        let principal_now = client.principal(wallet).await.ok();
        if have != want || principal_now != Some(owner) {
            return Err(format!(
                "the wallet {wallet:#x} has not become the account yet: it signs one EIP-7702 authorization to {implementation:#x} and calls initialize on itself in the same transaction (data: {})",
                delegate_initialize_calldata(t)?
            ));
        }
    } else {
        let pull = t.kind == "pull";
        let reserved = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
        let code = initcode_for(
            app_dir,
            owner,
            reserved,
            policy_salt(owner, watched, pull),
            policy_initcode(
                c,
                pull,
                watched,
                owner,
                agent,
                operator,
                per_action,
                daily,
                floor,
                t.delay_secs,
            )?,
        )?;
        let (got_policy, policy_tx) = client
            .deploy_deterministic(policy_salt(owner, watched, pull), &code)
            .await
            .map_err(|e| format!("deploy BudgetPolicy: {e}"))?;
        if got_policy != reserved {
            return Err(format!(
                "the policy landed at {got_policy:#x}, not the reserved {reserved:#x}"
            ));
        }
        if policy_tx.is_some() {
            t.policy_tx = policy_tx.map(|h| format!("{h:#x}"));
            save(app_dir, t)?;
        }
        // docs/79 §6a #5: provisional caps followed the bills after the
        // creation code was kept (caps are constructor arguments, so the code
        // could not change without moving the account). The operator sets
        // them now: a cut applies at once, a raise waits the delay like any
        // raise. Idempotent: a retry finds them equal and sends nothing.
        if t.caps_provisional {
            let (on_per, on_day, on_floor) =
                client.caps(reserved).await.map_err(|e| e.to_string())?;
            if (on_per, on_day, on_floor) != (per_action, daily, floor) {
                client
                    .set_limits(reserved, per_action, daily, floor)
                    .await
                    .map_err(|e| format!("set the provisional caps: {e}"))?;
            }
        }
    }
    // Gas for the agent only if it needs it: one passkey, many policies,
    // one agent key — the second policy must not pay for gas the key holds.
    // A keyless account's agent never transacts: no gas to give it.
    let gas_tx = if t.keyless {
        None
    } else {
        top_up_agent_gas(c, agent).await?
    };
    t.deployed = true;
    // From here the caps change only by the owner's tap (or the operator's cut).
    t.caps_provisional = false;
    t.agent_gas_tx = gas_tx.map(|h| format!("{h:#x}"));
    save(app_dir, t)
}

/// Reserve and put on chain at once (the service's `/deploy` without `lazy`).
pub async fn deploy(app_dir: &Path, r: DeployRequest) -> Result<Tenant, String> {
    let (mut t, _) = reserve(app_dir, r).await?;
    ensure_deployed(app_dir, &mut t).await?;
    Ok(t)
}

/// What the passkey must sign to approve `data` on `target` as the owner's
/// next call: the nonce read from the chain and the contract's own formula.
pub async fn challenge(
    c: &ChainPreset,
    owner: Address,
    target: Address,
    data: &str,
) -> Result<(U256, B256), String> {
    let client = read_only_client(c)?;
    let nonce = client.passkey_nonce(owner).await.map_err(refused)?;
    let call = PreparedCall {
        to: target,
        data: data.to_string(),
        value: U256::ZERO,
        summary: String::new(),
    };
    let ch = passkey_challenge(c.chain_id, owner, nonce, &call).map_err(|e| e.to_string())?;
    Ok((nonce, ch))
}

/// Approvals collected for an owner whose threshold is above one (PLAN 5.7):
/// the call, the nonce it was signed for, and each key's assertion. Kept in
/// `tenants/<owner>/approvals.json`; an entry whose nonce has moved on is
/// stale and dropped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingApproval {
    pub nonce: String,
    pub challenge: String,
    pub target: String,
    pub data: String,
    pub created: u64,
    /// (key index, the assertion JSON as the page posted it)
    pub approvals: Vec<(u64, Value)>,
}

fn approvals_path(app_dir: &Path, owner: Address) -> PathBuf {
    tenants_dir(app_dir)
        .join(format!("{owner:#x}"))
        .join("approvals.json")
}

fn load_approvals(app_dir: &Path, owner: Address) -> Vec<PendingApproval> {
    fs::read_to_string(approvals_path(app_dir, owner))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_approvals(app_dir: &Path, owner: Address, list: &[PendingApproval]) -> Result<(), String> {
    let p = approvals_path(app_dir, owner);
    fs::create_dir_all(p.parent().expect("parent")).map_err(|e| e.to_string())?;
    fs::write(
        &p,
        serde_json::to_string_pretty(list).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// The calls still waiting for more of the owner's keys, at the current nonce.
/// Read for the inbox, so a stalled node costs seconds (docs/110 item 8).
pub async fn pending_approvals(
    c: &ChainPreset,
    app_dir: &Path,
    owner: Address,
) -> Vec<PendingApproval> {
    let Ok(client) = quick_client(c) else {
        return Vec::new();
    };
    let Ok(nonce) = client.passkey_nonce(owner).await else {
        return Vec::new();
    };
    load_approvals(app_dir, owner)
        .into_iter()
        .filter(|a| a.nonce == nonce.to_string())
        .collect()
}

/// A refusal by the chain, in the owner's words (docs/70 C10). The node's
/// message carries the revert data: the contract's own error (its selector,
/// also inside the passkey contract's `CallFailed(bytes)`) or a token's
/// `Error(string)`. Nothing is paid when this happens, and the words say so.
/// Every call the service relays for a passkey ends here, so the classic
/// page, the console and an agent read the same sentence. A message that is
/// not a refusal (no revert, no revert data) is returned as it is.
/// The hash of a transaction that was sent but whose receipt could not be read (`crates/arc` `Confirmed`):
/// "transaction 0x<64 hex> was sent…". Only that shape, so "nothing was sent" is not taken for it.
pub fn sent_unread(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    lower.match_indices("transaction 0x").find_map(|(i, _)| {
        let hash = lower.get(i + 14..i + 78)?;
        (hash.bytes().all(|b| b.is_ascii_hexdigit()) && lower[i + 78..].starts_with(" was sent"))
            .then(|| format!("0x{hash}"))
    })
}

pub fn refusal_words(raw: &str) -> String {
    const BALANCE: &str =
        "The account holds less than this payment, so nothing was paid. Add money and tap again.";
    const ALLOWANCE: &str = "Your wallet allows the account less than this payment, so nothing was paid. Allow more and tap again.";
    const WORDS: &[(&[&str], &str)] = &[
        (
            &[
                "InsufficientBalance(uint256,uint256)",
                "BelowFloor(uint256,uint256,uint256)",
            ],
            BALANCE,
        ),
        (&["InsufficientAllowance(uint256,uint256)"], ALLOWANCE),
        (
            &["OverPerActionCap(uint256,uint256)"],
            "Over the limit for one payment, so nothing was paid. Raise the limit or pay it in parts.",
        ),
        (
            &[
                "OverDailyCap(uint256,uint256)",
                "OverDailyCap(uint256,uint256,uint256)",
            ],
            "Over today's limit, so nothing was paid. It can go tomorrow, or raise the limit.",
        ),
        (
            &["BelowReserveFloor(uint256,uint256)"],
            "It would take the account below the reserve you keep, so nothing was paid.",
        ),
        (
            &["AlreadyExecuted(bytes32)"],
            "Already paid — a payment is never made twice.",
        ),
        (
            &["PayeeNotAttested(address)", "NotAttested(address)"],
            "Not one of your approved payees yet, so nothing was paid.",
        ),
        (
            &["PayeeCoolingOff(address,uint256)"],
            "A payee approved just now can be paid once the waiting period you set has passed.",
        ),
        (
            &["Paused()"],
            "The account is paused, so nothing was paid. Resume it to pay.",
        ),
        (
            &["TermsChanged(bytes32,bytes32)"],
            "Your limits or keys changed since this was prepared, so nothing was paid. Refresh and tap again.",
        ),
        (
            &["ApprovalExpired(uint64)"],
            "This approval expired, so nothing was paid. Refresh and tap again.",
        ),
        (
            &["AgentLapsed(uint64)"],
            "The agent's authority lapsed. Renew it to let the agent pay.",
        ),
        (
            &["BadPasskeySignature()"],
            "That passkey is not this account's owner, so nothing happened.",
        ),
        (
            &["NeedsMoreApprovals(uint8,uint8)"],
            "More owners need to approve this first.",
        ),
        (
            &["WindowFull()"],
            "Too many payments in this window; try again shortly.",
        ),
        // Arc's USDC refuses a short transfer without a reason; the policy
        // reports it as TransferFailed() (seen live 2026-10-15, docs/70).
        (
            &["TransferFailed()"],
            "USDC did not move, most often because the account holds less than this payment, so nothing was paid. Check the balance and tap again.",
        ),
        (
            &["TooEarly(uint64)"],
            "The waiting period you set has not passed yet, so nothing changed. Try again when it ends.",
        ),
        (
            &["NothingPending()"],
            "Nothing is waiting to apply or cancel.",
        ),
        (
            &["CapAboveDailyCap(uint256,uint256)", "CapAboveDailyCap()"],
            "The limit for one payment cannot be above the daily limit, so nothing changed.",
        ),
        (
            &["ScheduleNotDue(uint256,uint64)"],
            "This standing order is not due yet, so nothing was paid.",
        ),
        (
            &["ScheduleInactive(uint256)"],
            "This standing order has ended, so nothing was paid.",
        ),
        (
            &["IntervalTooShort(uint64)"],
            "That is too often for a standing order; pick a longer interval.",
        ),
        (&["AlreadyApproved()"], "This key already approved it."),
    ];
    // The revert data: every 0x-prefixed run of at least 8 hex digits.
    let lower = raw.to_ascii_lowercase();
    let hexes: Vec<&str> = lower
        .split("0x")
        .skip(1)
        .map(|s| {
            s.split(|c: char| !c.is_ascii_hexdigit())
                .next()
                .unwrap_or("")
        })
        .filter(|h| h.len() >= 8)
        .collect();
    let reverted = lower.contains("revert");
    // Sent but not confirmed in time (arc `confirmed()`): the transaction may
    // still land. Never "nothing was paid" here — a second tap signs a new
    // nonce, and a standing order or a withdrawal would then happen twice
    // (audit 2026-10-15, docs/60 P-73). Say where to look, with the hash.
    // The same when its receipt could not be read after the send (docs/99 W3; only arc's own words, docs/100 X10).
    if !reverted && (lower.contains("not confirmed") || sent_unread(raw).is_some()) {
        let tx = hexes
            .iter()
            .find(|h| h.len() == 64)
            .map(|h| format!(" (0x{h})"))
            .unwrap_or_default();
        return format!(
            "Sent, but the network has not confirmed it yet{tx}. Check Recent or the explorer before tapping again — it may still go through."
        );
    }
    // A public node that throttles or drops the call (docs/60 P-12, P-50):
    // the call may or may not have gone out, so the words say where to look
    // instead of claiming nothing was paid. A payment bound to its action
    // hash cannot pay twice (AlreadyExecuted); other calls (a standing order,
    // a withdrawal) can, so the words say to look first.
    if !reverted
        && [
            "rate limit",
            "-32005",
            "429",
            "too many requests",
            "timed out",
            "error sending request",
        ]
        .iter()
        .any(|w| lower.contains(w))
    {
        return "The network is busy and did not answer. Check Recent in a minute before tapping again — it may have gone through.".into();
    }
    if hexes.is_empty() && !reverted {
        return raw.to_string();
    }
    for (sigs, words) in WORDS {
        let hit = sigs.iter().any(|sig| {
            let sel = alloy::hex::encode(&alloy::primitives::keccak256(sig.as_bytes())[..4]);
            hexes.iter().any(|h| h.contains(&sel))
        });
        if hit {
            return (*words).to_string();
        }
    }
    // Error(string): a token's own reason, e.g. "transfer amount exceeds balance".
    let reason = hexes
        .iter()
        .find_map(|h| {
            let at = h.find("08c379a0")?;
            let bytes = alloy::hex::decode(&h[at..h.len() - (h.len() - at) % 2]).ok()?;
            alloy::sol_types::decode_revert_reason(&bytes)
        })
        .map(|r| r.trim_start_matches("revert: ").to_string())
        .unwrap_or_default();
    let text = format!("{reason} {lower}");
    if text.contains("exceeds balance") {
        return BALANCE.into();
    }
    if text.contains("exceeds allowance") {
        return ALLOWANCE.into();
    }
    if reason.is_empty() {
        "The chain refused this, so nothing was paid. Refresh and try again.".into()
    } else {
        format!("The chain refused this, so nothing was paid ({reason}). Refresh and try again.")
    }
}

/// A relay error for the person who tapped: the words above, and the node's
/// own text in the service log for whoever runs it.
fn refused(e: impl std::fmt::Display) -> String {
    let raw = e.to_string();
    let words = refusal_words(&raw);
    if words != raw {
        eprintln!("level1: the chain refused a relayed call: {raw}");
    }
    words
}

/// The keys this service holds: the relayer (every account's operator), the
/// statement signer and the accounts' agent keys. None of them is ever a
/// guardian or the recovery address (docs/102 D10, docs/104 §A row 4).
pub fn our_addresses(app_dir: &Path) -> Vec<Address> {
    let mut v: Vec<Address> = list(app_dir)
        .iter()
        .filter_map(|t| Address::from_str(&t.agent).ok())
        .collect();
    v.extend(relayer_address().ok());
    v.extend(crate::statement::signer_address().ok());
    v.retain(|a| !a.is_zero());
    v.sort();
    v.dedup();
    v
}

/// Why an owner-contract call must not be relayed, if it must not:
/// `setGuardians` or `setRecovery` naming one of `ours`, or `setGuardians`
/// naming the account itself (`itself`: the owner and its policies), a slot
/// that can never vote. Any other call, or a call to another contract,
/// passes. Pure, so it is tested (docs/103 S1-4, S1-5).
pub fn guardian_call_refusal(
    owner: Address,
    target: Address,
    data: &[u8],
    ours: &[Address],
    itself: &[Address],
) -> Option<String> {
    use alloy::sol_types::SolCall;
    alloy::sol! {
        function setGuardians(address[] list, uint8 threshold) external;
        function setRecovery(address next) external;
    }
    if target != owner {
        return None;
    }
    let (named, guardians) = if let Ok(c) = setGuardiansCall::abi_decode(data) {
        (c.list, true)
    } else if let Ok(c) = setRecoveryCall::abi_decode(data) {
        (vec![c.next], false)
    } else {
        return None;
    };
    if named.iter().any(|a| ours.contains(a)) {
        return Some("that is Tameion's own key; a guardian or recovery address is a wallet you, or someone you trust, holds".into());
    }
    (guardians && named.iter().any(|a| itself.contains(a)))
        .then(|| "that is this account itself; a guardian is a wallet that can sign".to_string())
}

/// The owner's words for a replacement key waiting out the delay: when it
/// takes over, who proposed it, and the one tap that stops it (docs/103 S1-2).
pub fn key_replacement_text(eta: u64, by: Address, now: u64) -> String {
    let left = eta.saturating_sub(now);
    let when = match left {
        0..90 => format!("in {left} s"),
        90..5_400 => format!("in about {} min", left.div_ceil(60)),
        5_400..172_800 => format!("in about {} hours", left.div_ceil(3_600)),
        _ => format!("in about {} days", left.div_ceil(86_400)),
    };
    let at = if left < 86_400 {
        format!(
            " (at {:02}:{:02} UTC)",
            eta % 86_400 / 3_600,
            eta % 3_600 / 60
        )
    } else {
        String::new()
    };
    let by = format!("{by:x}");
    format!(
        "A new key is waiting to replace yours: it takes over {when}{at}, proposed by wallet ••{}. If you did not ask for it, open Settings → Owners → Keys and tap Cancel it now before then.",
        by[by.len() - 4..].to_uppercase()
    )
}

/// [`guardian_call_refusal`] with this service's keys and the account's own
/// addresses read from the records: the one check every relay and the
/// prepare route share.
pub fn guardian_refusal(
    app_dir: &Path,
    owner: Address,
    target: Address,
    data: &[u8],
) -> Option<String> {
    let mut itself = vec![owner];
    itself.extend(
        list(app_dir)
            .iter()
            .filter(|t| Address::from_str(&t.owner).ok() == Some(owner))
            .filter_map(|t| Address::from_str(&t.policy).ok()),
    );
    guardian_call_refusal(owner, target, data, &our_addresses(app_dir), &itself)
}

/// Relay a passkey-approved call for an owner with co-owners (PLAN 5.7).
/// The assertion is checked here against the key it claims before anything
/// is stored or sent. Threshold one: relayed at once (`execute` for key 0,
/// `executeAs` for a co-owner). Above one: kept until that many distinct keys
/// approved the same call at the same nonce, then relayed with `executeWith`.
/// Answers `{tx, explorer}` or `{pending, have, need, challenge}`.
pub async fn execute_by_key(
    c: &ChainPreset,
    app_dir: &Path,
    owner: Address,
    target: Address,
    data: &str,
    key_index: u64,
    auth_json: &Value,
) -> Result<Value, String> {
    let auth = auth_from_json(auth_json)?;
    // Every relay of an owner call passes here: a passkey-signed call that
    // would make one of our keys a guardian is refused even when the page's
    // prepare route was skipped (docs/103 S1-4).
    let raw =
        alloy::hex::decode(data.trim().trim_start_matches("0x")).map_err(|e| e.to_string())?;
    if let Some(e) = guardian_refusal(app_dir, owner, target, &raw) {
        return Err(e);
    }
    let reader = read_only_client(c)?;
    let (threshold, keys) = reader.owner_signers(owner).await.map_err(refused)?;
    let (nonce, challenge) = challenge(c, owner, target, data).await?;
    // The page names its key's index; a page that lost it sends 0. If that key
    // does not verify, the one that does is used, so a co-owner is never told
    // they are not an owner (audit 2026-10-15, docs/60 P-73).
    let key_index = match keys.get(key_index as usize) {
        Some(&(x, y)) if verify_passkey_assertion(x, y, challenge, &auth).is_ok() => key_index,
        _ => keys
            .iter()
            .position(|&(x, y)| verify_passkey_assertion(x, y, challenge, &auth).is_ok())
            .map_or(key_index, |i| i as u64),
    };
    let Some(&(kx, ky)) = keys.get(key_index as usize) else {
        return Err(
            "this device's key is not one of this account's owners; ask an owner to add it".into(),
        );
    };
    // The page asked for the challenge with the same call, so a different
    // one here means the owner's nonce moved in between: another approval
    // (another tab, device or owner) landed first. In words, not mechanism.
    verify_passkey_assertion(kx, ky, challenge, &auth).map_err(|e| {
        eprintln!("level1: passkey assertion refused: {e}");
        if e.contains("different challenge") {
            "Something else was approved on this account a moment ago, so this tap was not used and nothing was paid. Tap again.".to_string()
        } else {
            "That passkey is not this account's owner, so nothing happened.".to_string()
        }
    })?;
    let bytes = alloy::hex::decode(data.trim_start_matches("0x")).map_err(|e| e.to_string())?;
    let client = signed_client(c, &relayer_key()?)?;
    let done = |tx: B256| json!({ "tx": format!("{tx:#x}"), "explorer": format!("{}/tx/{tx:#x}", c.explorer) });
    if threshold <= 1 {
        let tx = if key_index == 0 {
            client
                .execute_as_passkey(owner, target, U256::ZERO, bytes, &auth)
                .await
        } else {
            client
                .execute_as_key(owner, key_index, target, U256::ZERO, bytes, &auth)
                .await
        }
        .map_err(refused)?;
        return Ok(done(tx));
    }
    // One approval at a time: two co-owners approving together read, change
    // and wrote the same file, and one approval was lost (audit 2026-10-15,
    // docs/60 P-73). Held across the send, so the last approval executes once.
    // ponytail: one lock for every account; per-owner locks if many owners approve at once.
    static APPROVALS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _held = APPROVALS.lock().await;
    let mut list: Vec<PendingApproval> = load_approvals(app_dir, owner)
        .into_iter()
        .filter(|a| a.nonce == nonce.to_string())
        .collect();
    let ch = format!("{challenge:#x}");
    let i = match list.iter().position(|a| a.challenge == ch) {
        Some(i) => i,
        None => {
            list.push(PendingApproval {
                nonce: nonce.to_string(),
                challenge: ch.clone(),
                target: format!("{target:#x}"),
                data: data.to_string(),
                created: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                approvals: Vec::new(),
            });
            list.len() - 1
        }
    };
    if !list[i].approvals.iter().any(|(k, _)| *k == key_index) {
        list[i].approvals.push((key_index, auth_json.clone()));
    }
    let have = list[i].approvals.len();
    if have < threshold as usize {
        save_approvals(app_dir, owner, &list)?;
        return Ok(json!({ "pending": true, "have": have, "need": threshold, "challenge": ch }));
    }
    let mut approvals: Vec<(u64, PasskeyAuth)> = list[i]
        .approvals
        .iter()
        .map(|(k, a)| auth_from_json(a).map(|a| (*k, a)))
        .collect::<Result<_, _>>()?;
    approvals.sort_by_key(|(k, _)| *k);
    let tx = client
        .execute_with_keys(owner, target, U256::ZERO, bytes, &approvals)
        .await
        .map_err(refused)?;
    list.remove(i);
    save_approvals(app_dir, owner, &list)?;
    Ok(done(tx))
}

/// Parse the assertion JSON the page posts (`r`, `s`, `challengeIndex`,
/// `typeIndex`, `authenticatorData`, `clientDataJSON`).
pub fn auth_from_json(v: &Value) -> Result<PasskeyAuth, String> {
    let b256 = |k: &str| -> Result<B256, String> {
        v[k].as_str()
            .ok_or_else(|| format!("auth.{k} missing"))?
            .parse()
            .map_err(|e| format!("auth.{k}: {e}"))
    };
    let idx = |k: &str| -> Result<U256, String> {
        match &v[k] {
            Value::Number(n) => n
                .as_u64()
                .map(U256::from)
                .ok_or_else(|| format!("auth.{k} not an integer")),
            Value::String(s) => U256::from_str(s).map_err(|e| format!("auth.{k}: {e}")),
            _ => Err(format!("auth.{k} missing")),
        }
    };
    Ok(PasskeyAuth {
        r: b256("r")?,
        s: b256("s")?,
        challenge_index: idx("challengeIndex")?,
        type_index: idx("typeIndex")?,
        authenticator_data: alloy::hex::decode(
            v["authenticatorData"]
                .as_str()
                .ok_or("auth.authenticatorData missing")?
                .trim_start_matches("0x"),
        )
        .map_err(|e| format!("auth.authenticatorData: {e}"))?,
        client_data_json: v["clientDataJSON"]
            .as_str()
            .ok_or("auth.clientDataJSON missing")?
            .to_string(),
    })
}

/// Calldata to attest several payees in one passkey tap.
pub fn prepare_payees(policy: Address, payees: &[Address]) -> PreparedCall {
    prepare_payee_batch(policy, payees)
}

/// The same, with `allow = false` to revoke: instant on the contract.
pub fn prepare_payees_with(policy: Address, payees: &[Address], allow: bool) -> PreparedCall {
    arc::prepare_payee_batch_with(policy, payees, allow)
}

/// Calldata for the principal to cancel a pending raise, for the passkey.
pub fn prepare_cancel(policy: Address) -> PreparedCall {
    arc::prepare_cancel_pending(policy)
}

/// Calldata for the passkey to set its recovery address (target: the owner).
pub fn prepare_recovery(owner: Address, recovery: Address) -> PreparedCall {
    arc::prepare_set_recovery(owner, recovery)
}

/// Calldata for the principal to set the agent's limits with the passkey.
pub fn prepare_limits(
    policy: Address,
    per_action: Money,
    daily: Money,
    floor: Money,
) -> Result<PreparedCall, String> {
    arc::prepare_set_limits(policy, per_action, daily, floor)
}

/// Calldata for the principal to withdraw with the passkey.
pub fn prepare_withdraw(
    policy: Address,
    to: Address,
    amount: Money,
) -> Result<PreparedCall, String> {
    arc::prepare_withdraw(policy, to, amount)
}

/// Calldata for the principal to set the autonomy level with the passkey:
/// 0 Watch, 1 Propose, 2 Autopilot. The contract applies a lowering at once
/// and schedules a raise behind the delay, whoever asks.
pub fn prepare_mode(policy: Address, mode: u8) -> Result<PreparedCall, String> {
    arc::prepare_set_mode(policy, mode)
}

// ---- Where the agent reports, set by the owner's passkey (PLAN 5.3)

/// The 32 bytes the owner's passkey signs to point the agent's reports at
/// `url` / `chat`: a domain tag, the chain, the policy, both strings with
/// their lengths, and when it was issued (the service accepts it for ten
/// minutes, once). Nothing on chain; the key is the same one that moves
/// money, read from the contract.
pub fn notify_challenge(
    chain_id: u64,
    policy: Address,
    url: &str,
    chat: &str,
    issued: u64,
) -> B256 {
    let mut m = Vec::with_capacity(96 + url.len() + chat.len());
    m.extend_from_slice(b"tameion.notify.v1");
    m.extend_from_slice(&chain_id.to_be_bytes());
    m.extend_from_slice(policy.as_slice());
    m.extend_from_slice(&(url.len() as u32).to_be_bytes());
    m.extend_from_slice(url.as_bytes());
    m.extend_from_slice(&(chat.len() as u32).to_be_bytes());
    m.extend_from_slice(chat.as_bytes());
    m.extend_from_slice(&issued.to_be_bytes());
    alloy::primitives::keccak256(m)
}

/// base64url without padding, as WebAuthn writes the challenge into
/// `clientDataJSON`.
pub fn b64url(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.len();
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let v = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(v >> 18) as usize & 63] as char);
        out.push(T[(v >> 12) as usize & 63] as char);
        if n > 1 {
            out.push(T[(v >> 6) as usize & 63] as char);
        }
        if n > 2 {
            out.push(T[v as usize & 63] as char);
        }
    }
    out
}

/// Check a WebAuthn assertion the way the contract would, off chain: the
/// client data is a `webauthn.get` over exactly `challenge`, and the P-256
/// signature over sha256(authenticatorData || sha256(clientDataJSON))
/// verifies against (`qx`, `qy`). Returns why not, in words.
pub fn verify_passkey_assertion(
    qx: B256,
    qy: B256,
    challenge: B256,
    auth: &PasskeyAuth,
) -> Result<(), String> {
    use p256::ecdsa::signature::hazmat::PrehashVerifier;
    use sha2::{Digest, Sha256};
    let cd = &auth.client_data_json;
    if !cd.contains("\"type\":\"webauthn.get\"") {
        return Err("the assertion is not a webauthn.get".into());
    }
    let want = format!("\"challenge\":\"{}\"", b64url(challenge.as_slice()));
    if !cd.contains(&want) {
        return Err("the assertion signs a different challenge".into());
    }
    let cd_hash = Sha256::digest(cd.as_bytes());
    let mut h = Sha256::new();
    h.update(&auth.authenticator_data);
    h.update(cd_hash);
    let msg = h.finalize();
    let point = p256::EncodedPoint::from_affine_coordinates(&qx.0.into(), &qy.0.into(), false);
    let key = p256::ecdsa::VerifyingKey::from_encoded_point(&point)
        .map_err(|_| "the owner's key is not a point on P-256")?;
    let sig = p256::ecdsa::Signature::from_scalars(auth.r.0, auth.s.0)
        .map_err(|_| "the signature is not a pair of P-256 scalars")?;
    key.verify_prehash(&msg, &sig)
        .map_err(|_| "the signature does not verify against the owner's key".into())
}

/// Store where the agent reports; a new URL gets a new secret, returned
/// once. `chat`: `None` leaves Telegram alone, `Some(None)` clears it.
pub fn apply_notify(
    t: &mut Tenant,
    url: Option<&str>,
    chat: Option<Option<String>>,
    rotate: bool,
) -> Result<Option<String>, String> {
    let url = url.map(str::trim).filter(|u| !u.is_empty());
    if url.is_some_and(|u| !(u.starts_with("https://") || u.starts_with("http://"))) {
        return Err("url must be http(s)".into());
    }
    // A different receiver gets a different secret: the old one must not be
    // able to forge reports to the new one.
    let changed = t.notify_url.as_deref() != url;
    t.notify_url = url.map(String::from);
    let mut minted = None;
    if t.notify_url.is_some() && (t.notify_secret.is_none() || rotate || changed) {
        let s = new_secret();
        t.notify_secret = Some(s.clone());
        minted = Some(s);
    }
    if let Some(c) = chat {
        t.telegram_chat_id = c.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    }
    Ok(minted)
}

/// A fresh webhook secret: 32 random bytes, hex.
pub fn new_secret() -> String {
    let k = PrivateKeySigner::random();
    alloy::hex::encode(k.to_bytes())
}

/// `sha256=<hex>` over `timestamp.body` with the tenant's secret, the shape
/// a receiver recomputes before trusting a report.
pub fn webhook_signature(secret_hex: &str, timestamp: u64, body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let key = alloy::hex::decode(secret_hex).unwrap_or_else(|_| secret_hex.as_bytes().to_vec());
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("hmac accepts any key length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("sha256={}", alloy::hex::encode(mac.finalize().into_bytes()))
}

/// Imported bills as obligations: the owner's word stands in for history
/// (confidence 1.0, no evidence transactions) until history exists.
pub fn seeds(bills: &[SeededBill]) -> Vec<ledger::Obligation> {
    bills
        .iter()
        .filter_map(|b| {
            let amount = Money::parse(&b.amount, Currency::Usdc).ok()?;
            let interval = Seconds::days(i64::from(b.every_days.max(1)))?;
            let next_due = Timestamp::from_unix(b.next_due as i64);
            Some(ledger::Obligation {
                counterparty: ledger::Counterparty::new(&b.payee),
                kind: ledger::ObligationKind::Subscription,
                typical_amount: amount,
                interval,
                last_paid: next_due - interval,
                next_due,
                confidence: ledger::Confidence::new(1.0)?,
                sample_count: 0,
                interval_cv: 0.0,
                amount_cv: 0.0,
                evidence: Vec::new(),
            })
        })
        .collect()
}

/// Replace a tenant's imported bills. Only payees the policy attests are
/// kept: attesting is the passkey's act, so a stranger cannot make the
/// agent propose a payee the owner never approved. Returns what was kept
/// and what was refused, with the reason.
pub async fn set_bills(
    app_dir: &Path,
    t: &Tenant,
    bills: Vec<SeededBill>,
) -> Result<Value, String> {
    let c = crate::watch::chain(&t.chain).ok_or("tenant chain unknown")?;
    let policy = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
    let client = read_only_client(c)?;
    let mut kept = Vec::new();
    let mut refused = Vec::new();
    for b in bills {
        let Ok(payee) = Address::from_str(b.payee.trim()) else {
            refused.push(json!({ "bill": b, "why": "payee is not an address" }));
            continue;
        };
        if Money::parse(&b.amount, Currency::Usdc).is_err() {
            refused.push(json!({ "bill": b, "why": "amount is not a USDC amount" }));
            continue;
        }
        if b.every_days == 0 || b.every_days > 366 {
            refused.push(json!({ "bill": b, "why": "every_days must be 1..=366" }));
            continue;
        }
        if b.discount_bps > crate::MAX_DISCOUNT_BPS
            || (b.discount_bps > 0 && (b.early_days == 0 || b.early_days >= b.every_days))
        {
            refused.push(json!({ "bill": b, "why": "early-payment terms must be at most 10% off, and the early window shorter than the billing interval" }));
            continue;
        }
        let attested = client
            .is_attested(policy, payee)
            .await
            .map_err(|e| e.to_string())?;
        if !attested {
            refused.push(json!({ "bill": b, "why": "payee is not attested on the policy; approve it with the passkey first" }));
            continue;
        }
        kept.push(SeededBill {
            payee: format!("{payee:#x}"),
            ..b
        });
    }
    // Read the account again after the chain reads above: a token minted or
    // revoked meanwhile must not be overwritten by the copy this call started
    // with (code review, 2026-10-15).
    // ponytail: reload-then-save narrows the window, it does not close it; a
    // per-tenant lock around every load-modify-save if writes ever contend.
    let mut t = load(app_dir, &t.policy).unwrap_or_else(|| t.clone());
    // Added to what the account already has: a payee named again replaces
    // its earlier bill, the others stay (docs/60 P-39: a second "Add bills"
    // used to drop the first).
    t.bills.retain(|old| {
        !kept
            .iter()
            .any(|k| k.payee.eq_ignore_ascii_case(&old.payee))
    });
    t.bills.extend(kept.clone());
    save(app_dir, &t)?;
    Ok(json!({ "policy": t.policy, "bills": kept, "refused": refused }))
}

/// Which of these action hashes the policy has settled (by the owner's
/// `settleAsPrincipal` or the agent's `pay`). Read-only.
pub async fn executed_hashes(
    t: &Tenant,
    hashes: &[B256],
) -> Result<std::collections::BTreeSet<B256>, String> {
    let c = crate::watch::chain(&t.chain).ok_or("tenant chain unknown")?;
    let policy = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
    let client = quick_client(c)?;
    // `executed` never turns back to false: a hash seen executed is not read
    // again, and the rest are read eight at a time, not one after another
    // (docs/110 item 8: on a slow node the inbox spent most of 38 s here).
    static SEEN: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeSet<(Address, B256)>>,
    > = std::sync::OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    let mut done: std::collections::BTreeSet<B256> = {
        let seen = seen.lock().unwrap_or_else(|e| e.into_inner());
        hashes
            .iter()
            .filter(|h| seen.contains(&(policy, **h)))
            .copied()
            .collect()
    };
    let unknown: Vec<B256> = hashes
        .iter()
        .filter(|h| !done.contains(*h))
        .copied()
        .collect();
    let answers = arc::eight_at_a_time(unknown, |h| {
        let client = client.clone();
        async move { (h, client.executed(policy, h).await) }
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    for (h, r) in answers {
        if r.map_err(|e| e.to_string())? {
            seen.insert((policy, h));
            done.insert(h);
        }
    }
    Ok(done)
}

/// How many of the agent's past proposals the owner settled (docs/53 5.5):
/// each prepared payment carries its action hash; `executed(hash)` on the
/// policy is true once the owner's passkey settled it. Read-only.
pub async fn approved_count(t: &Tenant, hashes: &[B256]) -> Result<usize, String> {
    let c = crate::watch::chain(&t.chain).ok_or("tenant chain unknown")?;
    let policy = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
    let client = read_only_client(c)?;
    let mut n = 0;
    for h in hashes {
        if client
            .executed(policy, *h)
            .await
            .map_err(|e| e.to_string())?
        {
            n += 1;
        }
    }
    Ok(n)
}

/// The operator's side of the autonomy level, signed with the relayer key:
/// `pause` (Watch, instant), `propose` (instant when lowering, scheduled when
/// raising), `autopilot` (scheduled behind the delay), `apply` (anyone, once
/// the delay has passed), `status` (read only). The principal's
/// `cancelPending` goes through the passkey, never through here.
pub async fn mode_action(t: &Tenant, action: &str) -> Result<Value, String> {
    let c = crate::watch::chain(&t.chain).ok_or("tenant chain unknown")?;
    let policy = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
    if action == "status" && !t.deployed {
        // Reserved, not on chain: the caps are what was asked, the mode is
        // Propose, and the balance is what the reserved address can already
        // reach (pull: the wallet's balance within the allowance it granted
        // the address; hold: USDC already sent to the address).
        let client = quick_client(c)?;
        let watched = Address::from_str(&t.watched).map_err(|e| e.to_string())?;
        let balance = if t.kind == "pull" {
            let held = client
                .usdc_balance(watched)
                .await
                .map_err(|e| e.to_string())?;
            let allowed = client
                .usdc_allowance(watched, policy)
                .await
                .map_err(|e| e.to_string())?;
            if held.to_minor() < allowed.to_minor() {
                held
            } else {
                allowed
            }
        } else {
            client
                .usdc_balance(policy)
                .await
                .map_err(|e| e.to_string())?
        };
        return Ok(json!({
            "balance": balance.to_string(),
            "caps": { "perAction": t.per_action, "daily": t.daily, "floor": t.floor },
            "delaySecs": t.delay_secs,
            "explorer": c.explorer,
            "explorerTx": Value::Null,
            "mode": "Propose",
            "pending": Value::Null,
            "tx": Value::Null,
            "deployed": false,
        }));
    }
    let client = if action == "status" {
        quick_client(c)?
    } else {
        signed_client(c, &relayer_key()?)?
    };
    let tx = match action {
        "status" => None,
        "pause" => Some(
            client
                .set_mode(policy, 0)
                .await
                .map_err(|e| e.to_string())?,
        ),
        "propose" => Some(
            client
                .set_mode(policy, 1)
                .await
                .map_err(|e| e.to_string())?,
        ),
        "autopilot" => Some(
            client
                .set_mode(policy, 2)
                .await
                .map_err(|e| e.to_string())?,
        ),
        "apply" => Some(
            client
                .apply_pending(policy)
                .await
                .map_err(|e| e.to_string())?,
        ),
        other => {
            return Err(format!(
                "unknown action {other:?}; use status, pause, propose, autopilot or apply"
            ));
        }
    };
    // `balanceUsdc()` on the contract, not the policy address's own USDC: in
    // pull mode the policy holds nothing and the reach is min(wallet balance,
    // allowance), which is what the page's Protected and Runway must use.
    // Read together: the inbox waits on this (docs/60 P-31).
    let (g, balance, caps) = tokio::join!(
        client.governance(policy),
        client.policy_balance(policy),
        client.caps(policy),
    );
    let g = g.map_err(|e| e.to_string())?;
    let balance = balance.map_err(|e| e.to_string())?;
    let (per, day, floor) = caps.map_err(|e| e.to_string())?;
    Ok(json!({
        "balance": balance.to_string(),
        "caps": { "perAction": per.to_string(), "daily": day.to_string(), "floor": floor.to_string() },
        "tx": tx.map(|h| format!("{h:#x}")),
        "explorerTx": tx.map(|h| format!("{}/tx/{h:#x}", c.explorer)),
        "mode": g.mode.label(),
        "delaySecs": g.delay_secs,
        // Only what is pending (bit 0 limits, bit 1 mode): a mode-only change
        // printed its unused cap slots, "0.000000 USDC" (docs/60 P-181).
        "pending": g.pending.as_ref().map(|p| {
            let caps = [p.per_action_cap.to_string(), p.daily_cap.to_string(), p.reserve_floor.to_string()];
            let mut v = json!({
                "kinds": p.kinds,
                "eta": p.eta_unix,
                "words": crate::agent_api::pending_words(u64::from(p.kinds), p.mode.label(), [&caps[0], &caps[1], &caps[2]], p.eta_unix),
            });
            if p.kinds & 1 != 0 {
                v["perActionCap"] = json!(caps[0]);
                v["dailyCap"] = json!(caps[1]);
                v["reserveFloor"] = json!(caps[2]);
            }
            if p.kinds & 2 != 0 {
                v["mode"] = json!(p.mode.label());
            }
            v
        }),
        "explorer": c.explorer,
    }))
}

/// What the relayer gives a tenant's agent for gas, in native wei:
/// `TAMEION_AGENT_GAS_WEI`, default 0.05 (Arc testnet prices a payment or an
/// anchor at about 0.003–0.006 USDC; the first default, 0.01, ran dry on
/// the third pass of the first night).
pub fn agent_gas_wei(c: &ChainPreset) -> U256 {
    if eth_gas(c) {
        // 0.00012 ETH: ~30 payments or anchors at Arbitrum Sepolia's 0.02 gwei,
        // refilled below the floor. Was 0.0003: most of a new account's cost,
        // and the relayer's 0.0023 ETH covered five accounts (2026-10-15).
        return U256::from(120_000_000_000_000u128);
    }
    std::env::var("TAMEION_AGENT_GAS_WEI")
        .ok()
        .and_then(|v| U256::from_str(&v).ok())
        .unwrap_or(U256::from(50_000_000_000_000_000u128))
}

/// Whether gas on `c` is ETH rather than USDC (PLAN 5.58): the amounts
/// below are in a different asset there and an Arc number would be absurd.
fn eth_gas(c: &ChainPreset) -> bool {
    c.usdc.is_some()
}

/// A native amount in words: USDC on Arc, ETH elsewhere.
fn native_text(c: &ChainPreset, wei: U256) -> String {
    if eth_gas(c) {
        let micro = (wei / U256::from(1_000_000_000_000u128)).to::<u128>();
        format!("{}.{:06} ETH", micro / 1_000_000, micro % 1_000_000)
    } else {
        arc::usdc_from_wei(wei).to_string()
    }
}

/// Below this the agent cannot be sure of one more transaction:
/// `TAMEION_AGENT_GAS_FLOOR_WEI`, default 0.02.
pub fn agent_gas_floor_wei(c: &ChainPreset) -> U256 {
    if eth_gas(c) {
        // ~7 transactions' margin at 0.02 gwei.
        return U256::from(30_000_000_000_000u128);
    }
    std::env::var("TAMEION_AGENT_GAS_FLOOR_WEI")
        .ok()
        .and_then(|v| U256::from_str(&v).ok())
        .unwrap_or(U256::from(20_000_000_000_000_000u128))
}

/// What the relayer must hold before it tries to put a tenant on chain:
/// `TAMEION_RELAYER_FLOOR_WEI`, default 0.1 (one policy deploy is ~2 M gas;
/// the node's estimate asks for more than it uses). Found the hard way —
/// docs/60 P-14.
pub fn relayer_floor_wei(c: &ChainPreset) -> U256 {
    if eth_gas(c) {
        // Two contracts, ~5 M gas, at a few hundredths of a gwei: 0.001 ETH
        // leaves room for the node's estimate.
        return U256::from(1_000_000_000_000_000u128);
    }
    std::env::var("TAMEION_RELAYER_FLOOR_WEI")
        .ok()
        .and_then(|v| U256::from_str(&v).ok())
        .unwrap_or(U256::from(100_000_000_000_000_000u128))
}

/// Keep the agent able to pay and anchor: if its native balance is under
/// the floor, the relayer sends it the usual amount first. Returns the
/// top-up transaction, if one was needed.
async fn top_up_agent_gas(c: &ChainPreset, agent: Address) -> Result<Option<B256>, String> {
    let relayer = signed_client(c, &relayer_key()?)?;
    let have = relayer
        .native_wei(agent)
        .await
        .map_err(|e| format!("agent balance: {e}"))?;
    if have >= agent_gas_floor_wei(c) || agent_gas_wei(c).is_zero() {
        return Ok(None);
    }
    relayer
        .send_native(agent, agent_gas_wei(c))
        .await
        .map(Some)
        .map_err(|e| format!("top up agent gas: {e}"))
}

/// The pass's configuration for a tenant, in one place so a pass and the
/// read-only [`plan`] act on the same bills, keys and offered references.
fn pass_config(
    app_dir: &Path,
    t: &Tenant,
    c: &ChainPreset,
    anchor: Address,
) -> Result<Config, String> {
    let policy = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
    let mut config = Config::new(policy, policy, anchor);
    config.also_history = history_sources(t);
    config.history_from_block = t.history_from_block;
    // The policy did not exist before its own deploy block.
    config.policy_from_block = Some(t.created_block);
    config.horizon = Seconds::days(30).expect("30 days");
    config.detection.dust_line = Money::parse(c.dust, Currency::Usdc).ok();
    config.seeded = seeds(&t.bills);
    config.early = early_terms(&t.bills);
    config.audit_dir = Some(
        tenants_dir(app_dir)
            .join(format!("{policy:#x}"))
            .join("log"),
    );
    config.principal_id = format!("tenant:{policy:#x}");
    config.offered = offered_cycles(app_dir, &t.policy);
    Ok(config)
}

/// The policy's own transfers joined to the watched wallets' history, as the
/// pass learns from them.
async fn with_policy_transfers(
    client: &ArcClient,
    t: &Tenant,
    history: &mut Vec<crate::Transfer>,
    head: u64,
) -> Result<(), String> {
    let policy = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
    history.extend(
        client
            .transfers(policy, t.created_block.min(head), head)
            .await
            .map_err(|e| e.to_string())?,
    );
    // A wallet account is its own policy (policy == watched): its transfers
    // were just read twice, and a doubled sample count inflates confidence.
    crate::watch::dedup_transfers(history);
    // Money moving between the policy and the owner's wallets is the owner's
    // own, not evidence of a payee (the operator's own rule, kept here).
    let own: std::collections::BTreeSet<String> = std::iter::once(&policy)
        .chain(&history_sources(t))
        .map(|a| format!("{a:#x}"))
        .collect();
    history.retain(|tr| !own.contains(&tr.counterparty.to_string().to_lowercase()));
    history.sort_by_key(|tr| tr.at);
    Ok(())
}

/// What the next pass would act on, read only (docs/60 P-210): every bill,
/// stepped past the cycles already paid under its own key or a reference the
/// service offered for it, those due now, and those set aside with why. Reads
/// the cached history and the chain; sends nothing, writes nothing.
pub async fn plan(app_dir: &Path, t: &Tenant) -> Result<Value, String> {
    let c = crate::watch::chain(&t.chain).ok_or("tenant chain unknown")?;
    let client = read_only_client(c)?;
    let config = pass_config(app_dir, t, c, anchor_for(c)?)?;
    let head = client.block_number().await.map_err(|e| e.to_string())?;
    let now = client
        .block_timestamp(head)
        .await
        .map_err(|e| e.to_string())?;
    let mut history = load_history(app_dir, t)
        .map(|h| h.transfers)
        .unwrap_or_default();
    with_policy_transfers(&client, t, &mut history, head).await?;
    let offered = config.offered.len();
    let op = crate::Operator::new(client, config, RuleBased).map_err(|e| e.to_string())?;
    let p = op
        .obligations_due(&history, now)
        .await
        .map_err(|e| e.to_string())?;
    let row = |o: &crate::Obligation| json!({ "cycleKey": o.cycle_key(), "amount": o.typical_amount.to_string(), "nextDue": o.next_due.as_unix(), "intervalSecs": o.interval.as_i64() });
    Ok(json!({
        "policy": t.policy, "now": now.as_unix(), "head": head, "offeredReferences": offered,
        "obligations": p.obligations.iter().map(row).collect::<Vec<_>>(),
        "due": p.due.iter().map(row).collect::<Vec<_>>(),
        "aside": p.aside.iter().map(|(o, why)| json!({ "cycleKey": o.cycle_key(), "why": why })).collect::<Vec<_>>(),
    }))
}

/// Whether this bill-cycle (`payee@due`) is already paid by any path the
/// service knows (docs/60 P-211): under its own settlement key (the agent's
/// pay, the owner's tap since P-175, Pay all, an early payment) or under a
/// reference a pass offered for it (the owner's tap before P-175). Read only.
pub async fn cycle_paid(app_dir: &Path, t: &Tenant, cycle: &str) -> Result<bool, String> {
    let policy = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
    let key = B256::from(
        *ledger::settlement_key_for(&ledger::Idempotency {
            key: cycle.to_lowercase(),
            scope: format!("tenant:{policy:#x}"),
        })
        .as_bytes(),
    );
    let mut refs: Vec<B256> = offered_cycles(app_dir, &t.policy)
        .into_iter()
        .filter(|(_, c)| c.eq_ignore_ascii_case(cycle))
        .map(|(h, _)| h)
        .collect();
    refs.push(key);
    Ok(!executed_hashes(t, &refs).await?.is_empty())
}

/// Every reference this service's passes offered the owner for a bill-cycle,
/// with the cycle (`payee@due`): each Needs-you row's proposal hash and
/// settlement key, from the finished passes' records in `jobs/`. The owner's
/// tap was bound to the proposal hash before docs/60 P-175, so only these say
/// which cycle a hand payment paid (P-210).
pub fn offered_cycles(app_dir: &Path, policy: &str) -> std::collections::BTreeMap<B256, String> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(rd) = fs::read_dir(app_dir.join("jobs")) else {
        return out;
    };
    for e in rd.flatten() {
        let Some(v) = fs::read_to_string(e.path())
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        else {
            continue;
        };
        if v["request"]["level1"] != "run"
            || !v["request"]["policy"]
                .as_str()
                .is_some_and(|p| p.eq_ignore_ascii_case(policy))
            || v["status"] != "done"
        {
            continue;
        }
        for row in v["report"]["pendingSignature"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(cycle) = row["cycleKey"].as_str() else {
                continue;
            };
            for k in ["actionHash", "settlementKey"] {
                if let Some(h) = row[k].as_str().and_then(|h| B256::from_str(h).ok()) {
                    out.insert(h, cycle.to_lowercase());
                }
            }
        }
    }
    out
}

/// One pass of the agent for a tenant: read the watched wallet's history,
/// infer, verify against the policy's caps, and -- in Propose mode -- hand
/// every payment to the owner as ready-to-sign calldata. Anchors the root
/// under the tenant's agent key; the audit record goes under the tenant.
pub async fn run(
    app_dir: &Path,
    t: &Tenant,
    progress: impl Fn(crate::watch::Progress),
) -> Result<Value, String> {
    let c = crate::watch::chain(&t.chain).ok_or("tenant chain unknown")?;
    // Lazy deploy (PLAN 5.32): the first pass puts a reserved tenant on chain.
    let mut materialised = t.clone();
    ensure_deployed(app_dir, &mut materialised).await?;
    let t = &materialised;
    let owner = Address::from_str(&t.owner).map_err(|e| e.to_string())?;
    let policy = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
    // The wallets whose history is the account's: never the zero address a
    // name-first account reserves with (docs/79 §6a #3). None yet: the pass
    // reads only the policy's own transfers.
    let sources = history_sources(t);
    // A keyless account (PLAN 5.38) has no agent key: the pass reads,
    // proposes, triggers due standing orders (anyone may) and anchors its log
    // with the relayer key, which holds no authority over this policy.
    let client = if t.keyless {
        signed_client(c, &relayer_key()?)?
    } else {
        let key = fs::read_to_string(agent_key_path(app_dir, owner))
            .map_err(|e| format!("tenant agent key: {e}"))?;
        let client = signed_client(c, key.trim())?;
        drop(key);
        client
    };
    let anchor = anchor_for(c)?;
    let agent_addr = Address::from_str(&t.agent).map_err(|e| e.to_string())?;
    let gas_top_up = if t.keyless {
        None
    } else {
        top_up_agent_gas(c, agent_addr).await?
    };

    let config = pass_config(app_dir, t, c, anchor)?;

    let head = client.block_number().await.map_err(|e| e.to_string())?;
    let now: Timestamp = client
        .block_timestamp(head)
        .await
        .map_err(|e| e.to_string())?;

    // The history, read here with progress rather than inside the operator:
    // the watched wallet's, kept between passes so a pass reads only what
    // is new plus, within the time budget, what is still older than the
    // cache (docs/60 P-38: re-reading 90 days every minute was an hour on
    // Arbitrum's public node and a rate-limited node on Arc); the policy's
    // own transfers only since its deploy block (it did not exist before).
    let total = head.saturating_sub(t.history_from_block) + 1;
    progress(crate::watch::Progress {
        phase: "reading the watched wallet's history".into(),
        scanned_blocks: 0,
        total_blocks: total,
        transfers_so_far: 0,
        eta_cap_secs: None,
    });
    // Older history within the scan budget (`TAMEION_PASS_SCAN_SECS`,
    // default 60): a first check answers in about a minute with the newest
    // weeks and the passes after it reach further back.
    let budget = std::env::var("TAMEION_PASS_SCAN_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60u64);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(budget);
    let mut cache = load_history(app_dir, t).unwrap_or(History {
        lo: head.saturating_add(1),
        hi: head,
        transfers: Vec::new(),
    });
    if sources.is_empty() {
        // Nothing to read: an empty address list must never reach a log
        // filter (it would match every transfer of the token).
        cache.lo = t.history_from_block;
        cache.hi = head;
    }
    if cache.hi < head {
        // New blocks since the last pass, a margin re-read for reorgs; no
        // deadline, so the cache never has a hole.
        let from = cache
            .hi
            .saturating_sub(HISTORY_REORG_MARGIN)
            .max(t.history_from_block);
        let (new, reached) = crate::watch::scan_transfers_budgeted(
            &client, c, &sources, from, head, None, &progress,
        )
        .await
        .map_err(|e| e.to_string())?;
        cache.transfers.extend(new);
        // Only a complete read moves `hi`: an index that stopped at its page
        // cap left blocks between `from` and `reached` unread, and the next
        // pass reads them again rather than skipping them for good.
        if reached <= from {
            cache.hi = head;
        }
    }
    if cache.lo > t.history_from_block {
        let (old, reached) = crate::watch::scan_transfers_budgeted(
            &client,
            c,
            &sources,
            t.history_from_block,
            cache.lo - 1,
            Some(deadline),
            &progress,
        )
        .await
        .map_err(|e| e.to_string())?;
        cache.transfers.extend(old);
        cache.lo = reached;
    }
    crate::watch::dedup_transfers(&mut cache.transfers);
    save_history(app_dir, t, &cache);
    let reached = cache.lo;
    let mut history = cache.transfers;
    let history_note = if reached > t.history_from_block {
        let since = client
            .block_timestamp(reached)
            .await
            .map(|ts| (now.as_unix() - ts.as_unix()).max(0) / 86_400)
            .unwrap_or(0);
        Some(format!(
            "read the newest {since} days of your wallet so far; each check reads further back"
        ))
    } else {
        None
    };
    with_policy_transfers(&client, t, &mut history, head).await?;
    progress(crate::watch::Progress {
        phase: "inferring the bills, verifying each against the policy, deciding".into(),
        scanned_blocks: total,
        total_blocks: total,
        transfers_so_far: history.len(),
        eta_cap_secs: None,
    });

    // PLAN 5.54: what the agent bought since the last anchored record joins
    // this pass's record, so every purchase and its reason is under a root.
    let (mut bought, bought_upto) = crate::x402::unanchored_events(app_dir, &t.policy);
    // Receipt v1 (docs/76 §6): settled purchases are bill evidence. They are
    // detected here, beside the pass, not inside it: the pass keeps the
    // agent's own payments out of what it learns schedules from (an agent
    // that learns from its outputs moves its own due dates, `arc::agent_payments`),
    // and an API bought per call is never paid unprompted on a cadence. So
    // a seller bought from on a rhythm is reported as a bill — runway and the
    // owner's list see it — and the pass does not pay it.
    let receipts = crate::x402::settled_transfers(app_dir, &t.policy);
    let receipt_bills = crate::x402::receipt_bills(&history, &receipts);
    // TAMEION_REASONER (docs/86); unset keeps Claude-if-keyed-else-rule. The
    // decider goes with it (docs/112): a cloud choice with a key gets the
    // model, local or rule the rule; one memory file per account.
    let choice = crate::local::choose();
    let decider = Configured::for_choice(&choice).with_memory(
        tenants_dir(app_dir)
            .join(format!("{policy:#x}"))
            .join("decisions.jsonl"),
    );
    // Invoices waiting for the owner are decision points too: decided here,
    // before the pass, so their records sit under this pass's root.
    let (invoice_events, invoice_records) = decide_invoices(
        app_dir,
        t,
        &client,
        config.detection,
        &history,
        &decider,
        now,
    )
    .await;
    bought.extend(invoice_events);
    let mut report = match choice {
        Choice::Local(r) => drive(client, config, r, &decider, now, history, bought).await?,
        Choice::Cloud(r) => drive(client, config, r, &decider, now, history, bought).await?,
        Choice::CloudRedacted(r) => {
            drive(client, config, r, &decider, now, history, bought).await?
        }
        Choice::Rule => drive(client, config, RuleBased, &decider, now, history, bought).await?,
    };
    if !report["anchorTx"].is_null() {
        crate::x402::mark_anchored(app_dir, &t.policy, bought_upto);
    }
    // The invoices' decisions hold once the pass that records them did: kept
    // (so the next pass does not decide them again), and a refused one is
    // declined -- declining pays nothing, and the owner is not left a chore.
    for r in &invoice_records {
        remember_invoice_decision(app_dir, &t.policy, r);
        if r["decision"]["action"] == "refuse"
            && let Some(id) = r["id"].as_str().and_then(|i| B256::from_str(i).ok())
        {
            let _ = crate::invoice::decide(app_dir, &t.policy, id, false, now.as_unix() as u64);
            if let Some(list) = report["blocked"].as_array_mut() {
                list.push(json!({
                    "counterparty": r["counterparty"], "amount": r["amount"], "invoice": r["id"],
                    "problems": [r["decision"]["why"]],
                }));
            }
        }
    }
    if let Some(list) = report["decisions"].as_array_mut() {
        list.extend(invoice_records);
    }
    if let Some(list) = report["obligationsList"].as_array_mut() {
        let listed: std::collections::BTreeSet<String> = list
            .iter()
            .filter_map(|o| o["counterparty"].as_str().map(str::to_lowercase))
            .collect();
        for o in receipt_bills
            .iter()
            .filter(|o| !listed.contains(o.counterparty.as_str()))
        {
            list.push(json!({
                "counterparty": o.counterparty.to_string(),
                "amount": o.typical_amount.to_string(),
                "intervalSecs": o.interval.as_i64(),
                "nextDue": o.next_due.as_unix(),
                "confidence": o.confidence.get(),
                "samples": o.sample_count,
                "evidence": o.evidence,
                "provenance": "x402 receipt",
            }));
        }
        report["obligations"] = json!(list.len());
    }
    report["agentGasTopUpTx"] = json!(gas_top_up.map(|h| format!("{h:#x}")));
    report["historyReadFromBlock"] = json!(reached);
    report["historyNote"] = json!(history_note);
    Ok(report)
}

/// Where an account keeps the agent's decision on each invoice (docs/112):
/// one JSON line per invoice, written once the pass that recorded it held.
pub fn invoice_decisions_path(app_dir: &Path, policy: &str) -> PathBuf {
    tenants_dir(app_dir).join(format!("{}.invoice-decisions.jsonl", policy.to_lowercase()))
}

/// The agent's decisions on the account's invoices, oldest first: `id`,
/// `decision` (`action`, `basis`, `why`, `by`, `model`), `note`, the `case`.
pub fn invoice_decisions(app_dir: &Path, policy: &str) -> Vec<Value> {
    fs::read_to_string(invoice_decisions_path(app_dir, policy))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn remember_invoice_decision(app_dir: &Path, policy: &str, record: &Value) {
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(invoice_decisions_path(app_dir, policy))
    {
        let _ = writeln!(f, "{record}");
    }
}

/// docs/112: the decision on every invoice still waiting for the owner and
/// not decided before, as events for this pass's record and as report
/// lines. `ask` leaves the invoice in the owner's list with the agent's why
/// (an Escalation: the statement's "needs you"); `refuse` is a blocked
/// Decision (the statement's "refused"), declined by `run` once the pass held.
async fn decide_invoices<D: Decider + Sync>(
    app_dir: &Path,
    t: &Tenant,
    client: &ArcClient,
    detection: ledger::DetectionConfig,
    history: &[ledger::Transfer],
    decider: &D,
    now: Timestamp,
) -> (Vec<(u64, ledger::EventBody)>, Vec<Value>) {
    let decided: std::collections::BTreeSet<String> = invoice_decisions(app_dir, &t.policy)
        .iter()
        .filter_map(|v| v["id"].as_str().map(str::to_string))
        .collect();
    let pending: Vec<Value> = crate::invoice::pending(app_dir, &t.policy)
        .into_iter()
        .filter(|l| l["id"].as_str().is_some_and(|i| !decided.contains(i)))
        .collect();
    let Ok(policy) = Address::from_str(&t.policy) else {
        return (Vec::new(), Vec::new());
    };
    if pending.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let payees: Vec<Address> = pending
        .iter()
        .filter_map(|l| l["payee"].as_str().and_then(|p| Address::from_str(p).ok()))
        .collect();
    let (since, balance, authority) = tokio::join!(
        client.attested_since_among(policy, &payees),
        client.policy_balance(policy),
        client.authority(policy),
    );
    let since: std::collections::BTreeMap<String, u64> = since
        .map(|v| v.into_iter().map(|(a, s)| (format!("{a:#x}"), s)).collect())
        .unwrap_or_default();
    let balance = balance.unwrap_or_else(|_| Money::zero(Currency::Usdc));
    // The per-payment cap the contract holds: above it the agent never pays.
    let cap = authority
        .ok()
        .and_then(|a| u128::try_from(a.per_action_cap.to_minor()).ok());
    // Whom history shows the owner paid: the references for the look-alike rule.
    let known: Vec<String> = history
        .iter()
        .filter(|tr| tr.direction == ledger::Direction::Out && !detection.is_dust(tr.amount))
        .map(|tr| tr.counterparty.as_str().to_lowercase())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut events = Vec::new();
    let mut records = Vec::new();
    for line in &pending {
        let payee = line["payee"].as_str().unwrap_or("").to_lowercase();
        let Some(case) = crate::decide::invoice_case(
            line,
            history,
            detection,
            since.get(&payee).copied(),
            &known,
            cap,
            now.as_unix(),
        ) else {
            continue;
        };
        let (d, note) = crate::decide::decide_or_rule(decider, &case).await;
        let amount = Money::minor(
            i128::try_from(case.amount_minor).unwrap_or(0),
            Currency::Usdc,
        );
        let counterparty = ledger::Counterparty::new(&payee);
        let said = crate::decide::record_line(&d, note.as_deref());
        let body = if d.action == crate::decide::Action::Refuse {
            ledger::EventBody::Decision {
                counterparty,
                amount,
                decision: ledger::Decision::Block,
                action_hash: ledger::ActionHash::from_bytes(
                    B256::from_str(&case.id).map(|b| b.0).unwrap_or_default(),
                ),
                confidence: ledger::Confidence::clamped(case.confidence),
                balance_seen: balance,
                weakest_evidence: Some(ledger::Provenance::CounterpartyAsserted),
                evidence: vec![format!("invoice:{}", case.id)],
                note: format!("Invoice {} refused and declined. {said}", case.id),
            }
        } else {
            ledger::EventBody::Escalation {
                counterparty,
                amount,
                reason: format!("invoice: {}", d.why),
                note: format!(
                    "Invoice {} is the owner's to accept or decline. {said}",
                    case.id
                ),
            }
        };
        events.push((now.as_unix().max(0) as u64, body));
        records.push(json!({
            "kind": "invoice", "id": case.id, "counterparty": payee, "amount": amount.to_string(),
            "decision": d, "note": note, "allowed": case.allowed, "case": case,
        }));
    }
    (events, records)
}

/// Blocks re-read below the cache's newest block on every pass, in case
/// the chain reorganised them (testnet finality is seconds; this is cheap).
const HISTORY_REORG_MARGIN: u64 = 64;

/// The watched wallet's transfers read so far, blocks `lo..=hi`.
struct History {
    lo: u64,
    hi: u64,
    transfers: Vec<ledger::obligations::Transfer>,
}

fn history_path(app_dir: &Path, policy: &str) -> PathBuf {
    tenants_dir(app_dir).join(format!("{}.history.json", policy.to_lowercase()))
}

/// The cache for `t`, if it was read for the same chain and wallet.
fn load_history(app_dir: &Path, t: &Tenant) -> Option<History> {
    use ledger::obligations::{Counterparty, Direction, Transfer};
    let v: Value =
        serde_json::from_str(&fs::read_to_string(history_path(app_dir, &t.policy)).ok()?).ok()?;
    // Read for the same chain and the same wallets; a cache from before
    // `sources` was kept names its one wallet as `watched`.
    let read_for = match &v["sources"] {
        Value::Null => {
            let w = Address::from_str(v["watched"].as_str()?.trim()).ok()?;
            json!([format!("{w:#x}")])
        }
        s => s.clone(),
    };
    if v["chain"] != json!(t.chain) || read_for != sources_key(t) {
        return None;
    }
    let transfers = v["transfers"]
        .as_array()?
        .iter()
        .map(|r| {
            Some(Transfer {
                tx_hash: r["tx"].as_str()?.to_string(),
                counterparty: Counterparty::new(r["cp"].as_str()?),
                amount: Money::minor(r["minor"].as_str()?.parse().ok()?, Currency::Usdc),
                at: Timestamp::from_unix(r["at"].as_i64()?),
                direction: if r["out"].as_bool()? {
                    Direction::Out
                } else {
                    Direction::In
                },
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(History {
        lo: v["lo"].as_u64()?,
        hi: v["hi"].as_u64()?,
        transfers,
    })
}

fn save_history(app_dir: &Path, t: &Tenant, h: &History) {
    use ledger::obligations::Direction;
    let rows: Vec<Value> = h
        .transfers
        .iter()
        .map(|r| {
            json!({
                "tx": r.tx_hash,
                "cp": r.counterparty.as_str(),
                "minor": r.amount.to_minor().to_string(),
                "at": r.at.as_unix(),
                "out": r.direction == Direction::Out,
            })
        })
        .collect();
    let body = json!({ "chain": t.chain, "watched": t.watched, "sources": sources_key(t), "lo": h.lo, "hi": h.hi, "transfers": rows });
    // ponytail: the whole file rewritten each pass; a wallet with tens of
    // thousands of transfers wants an append-only file instead.
    let path = history_path(app_dir, &t.policy);
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, body.to_string()).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

async fn drive<R: Reasoner, D: Decider + Sync>(
    client: ArcClient,
    config: Config,
    reasoner: R,
    decider: &D,
    now: Timestamp,
    history: Vec<ledger::Transfer>,
    bought: Vec<(u64, ledger::EventBody)>,
) -> Result<Value, String> {
    let mut op = Operator::new(client, config, reasoner).map_err(|e| e.to_string())?;
    for (at, body) in bought {
        op.log_mut().append(
            Timestamp::from_unix(at.min(now.as_unix() as u64) as i64),
            body,
        );
    }
    let outcome = op
        .run_once_deciding(now, Some(history), true, decider)
        .await
        .map_err(|e| e.to_string())?;
    // docs/112: the decider's sentence leads the reasons a Needs-you or a
    // refused row shows (the account page joins `problems` into its why).
    let why = |o: &ledger::Obligation| {
        outcome
            .decisions
            .iter()
            .find(|(c, ..)| c.id == o.cycle_key())
            .map(|(_, d, _)| d.why.clone())
    };
    let reasons = |o: &ledger::Obligation, v: &ledger::Verdict| {
        why(o)
            .into_iter()
            .chain(v.problems().map(|c| format!("{}: {}", c.name, c.detail)))
            .collect::<Vec<_>>()
    };
    let fallbacks = outcome
        .decisions
        .iter()
        .filter(|(_, d, note)| note.is_some() && d.by != ledger::ActorKind::Model)
        .count();
    Ok(json!({
        "now": now.as_unix(),
        "balance": outcome.balance.to_string(),
        "transfers": outcome.transfers_seen,
        "obligations": outcome.obligations_found,
        "reasoner": outcome.reasoner,
        "mode": outcome.mode.map(|m| m.label().to_string()),
        "agentExpiry": outcome.agent_expiry,
        "settled": outcome.settled.iter().map(|(o, tx)| {
            let saved = outcome.early.iter().find(|(k, _)| *k == o.cycle_key()).map(|(_, m)| m.to_string());
            json!({ "counterparty": o.counterparty.to_string(), "amount": o.typical_amount.to_string(), "tx": format!("{tx:#x}"), "early": saved.is_some(), "saved": saved, "dueAt": o.next_due.as_unix() })
        }).collect::<Vec<_>>(),
        "escalated": outcome.escalated.iter().map(|(o, v)| json!({ "counterparty": o.counterparty.to_string(), "amount": o.typical_amount.to_string(), "decision": v.decision.to_string(), "problems": reasons(o, v) })).collect::<Vec<_>>(),
        "blocked": outcome.blocked.iter().map(|(o, v)| json!({ "counterparty": o.counterparty.to_string(), "amount": o.typical_amount.to_string(), "problems": reasons(o, v) })).collect::<Vec<_>>(),
        "decider": outcome.decider,
        "deciderFallbacks": fallbacks,
        "decisions": outcome.decisions.iter().map(|(c, d, note)| json!({
            // The address, as every other row of the report; the case's label beside it.
            "kind": "bill", "id": c.id, "counterparty": c.id.split('@').next().unwrap_or(&c.payee), "label": c.payee,
            "amount": format!("{} USDC", crate::decide::usdc(c.amount_minor)),
            "decision": d, "note": note, "allowed": c.allowed, "case": c,
        })).collect::<Vec<_>>(),
        "pendingSignature": outcome.pending_signature.iter().map(|p| json!({
            "cycleKey": p.cycle_key,
            "actionHash": p.action_hash,
            "counterparty": p.counterparty,
            "amount": p.amount.to_string(),
            "settlementKey": p.settlement_key,
            "stale": p.stale,
            "why": outcome.decisions.iter().find(|(c, ..)| c.id == p.cycle_key).map(|(_, d, _)| d.why.clone()),
            "to": format!("{:#x}", p.call.to),
            "data": p.call.data,
            "summary": p.call.summary,
            "attest": p.attest.as_ref().map(|a| json!({ "to": format!("{:#x}", a.to), "data": a.data, "summary": a.summary })),
        })).collect::<Vec<_>>(),
        "staleEscalations": outcome.stale_escalations,
        "obligationsList": outcome.obligations.iter().map(|o| json!({
            "counterparty": o.counterparty.to_string(),
            "amount": o.typical_amount.to_string(),
            "intervalSecs": o.interval.as_i64(),
            "nextDue": o.next_due.as_unix(),
            "confidence": o.confidence.get(),
            "samples": o.sample_count,
            // Where the evidence came from: the owner's payment history (or an
            // import, samples 0); receipt bills are appended by `run` as "x402 receipt".
            "provenance": if o.sample_count == 0 { "imported" } else { "history" },
        })).collect::<Vec<_>>(),
        "root": outcome.root.to_hex(),
        "anchorTx": outcome.anchor_tx.map(|t| format!("{t:#x}")),
        "anchorError": outcome.anchor_error,
        "caughtUp": outcome.caught_up.iter().map(|(r, t)| json!({ "root": r.to_hex(), "tx": format!("{t:#x}") })).collect::<Vec<_>>(),
        "log": crate::replay(op.log()),
    }))
}

#[cfg(test)]
mod invoice_decide_challenge_tests {
    use super::*;
    use std::str::FromStr;

    /// Vectors computed independently with `cast keccak` over the documented
    /// byte layout (docs/60 P-73), so the pages' ports have a fixed target.
    #[test]
    fn the_decision_challenge_matches_its_documented_bytes() {
        let policy = Address::from_str("0x35a2e2701d4d72be257bf7708c67d1433c60342f").unwrap();
        let id =
            B256::from_str("0x8b9badaf6d88b27f468085fc9e1558d02152496bf86113f2909554ba95a9543a")
                .unwrap();
        assert_eq!(
            format!(
                "{:#x}",
                invoice_decide_challenge(421_614, policy, id, true, 1_792_003_905)
            ),
            "0x724f3c25f4d9271c46dfff4adc4b5713e5d7ec53e2fb735a87481c1991ce0350"
        );
        assert_eq!(
            format!(
                "{:#x}",
                invoice_decide_challenge(421_614, policy, id, false, 1_792_003_905)
            ),
            "0x9ae991b8cc4f74a7dc47299af29c5885d7e2e6f67487f70240db00b52b7bf63a"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_keys_and_the_account_itself_are_never_guardians() {
        let a = |n: u8| Address::repeat_byte(n);
        let (owner, policy, relayer, g1, g2) = (a(1), a(2), a(9), a(5), a(6));
        let (ours, itself) = ([relayer], [owner, policy]);
        let guardians = |l: Vec<Address>| arc::set_guardians_calldata(&l, 2);
        let recovery = |r: Address| {
            let mut d =
                alloy::primitives::keccak256("setRecovery(address)".as_bytes())[..4].to_vec();
            d.extend_from_slice(r.into_word().as_slice());
            d
        };
        let refuse = |target: Address, data: &[u8]| {
            guardian_call_refusal(owner, target, data, &ours, &itself)
        };
        // The reviewer's call (docs/103 S1-4): our relayer beside a real guardian.
        assert!(
            refuse(owner, &guardians(vec![relayer, g1]))
                .unwrap()
                .contains("Tameion's own key")
        );
        assert!(
            refuse(owner, &recovery(relayer))
                .unwrap()
                .contains("Tameion's own key")
        );
        // The account itself as a guardian (S1-5): a slot that can never vote.
        assert!(
            refuse(owner, &guardians(vec![g1, owner]))
                .unwrap()
                .contains("this account itself")
        );
        assert!(
            refuse(owner, &guardians(vec![policy, g2]))
                .unwrap()
                .contains("this account itself")
        );
        // What the Keys panel sends, clearing the recovery address, and any other call pass.
        assert_eq!(refuse(owner, &guardians(vec![g1, g2])), None);
        assert_eq!(refuse(owner, &recovery(Address::ZERO)), None);
        assert_eq!(refuse(owner, &[0xde, 0xad, 0xbe, 0xef]), None);
        // The same bytes sent to another contract are not an owner call.
        assert_eq!(refuse(policy, &guardians(vec![relayer, g1])), None);
    }

    #[test]
    fn a_waiting_replacement_key_is_told_with_its_deadline_and_the_way_to_stop_it() {
        let by = Address::from_str("0x67B13190c7E25d98950FC3153623560e66c56E0F").unwrap();
        // The testnet delay: a minute.
        let t = key_replacement_text(1_792_141_968, by, 1_792_141_908);
        assert!(t.contains("in 60 s") && t.contains("(at 09:12 UTC)"), "{t}");
        assert!(
            t.contains("wallet ••6E0F") && t.ends_with("tap Cancel it now before then."),
            "{t}"
        );
        // A production delay of days: no clock time, the day count.
        let t = key_replacement_text(1_000 + 3 * 86_400, by, 1_000);
        assert!(t.contains("in about 3 days") && !t.contains("UTC"), "{t}");
        // Already due: still said, never a negative wait.
        assert!(key_replacement_text(5, by, 10).contains("in 0 s"));
    }

    #[test]
    fn a_refusal_reaches_the_owner_in_words_and_anything_else_as_it_is() {
        // The live string from docs/70 C10: the passkey contract's
        // CallFailed(bytes) wrapping the token's Error(string).
        let live = "contract call: server returned an error response: error code 3: execution reverted, data: \"0xa5fa8d2b0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000008408c379a00000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000002645524332303a207472616e7366657220616d6f756e7420657863656564732062616c616e6365000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000\"";
        assert!(refusal_words(live).starts_with("The account holds less than this payment"));
        // A contract error by its selector, wrapped or not.
        let sel = alloy::hex::encode(
            &alloy::primitives::keccak256("OverDailyCap(uint256,uint256)".as_bytes())[..4],
        );
        let daily = format!("execution reverted, data: \"0x{sel}{}\"", "0".repeat(128));
        assert!(refusal_words(&daily).starts_with("Over today's limit"));
        // Live on Arc, 2026-10-15: CallFailed(bytes) wrapping TransferFailed().
        let arc = "contract call: server returned an error response: error code 3: execution reverted, data: \"0xa5fa8d2b0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000490b8ec1800000000000000000000000000000000000000000000000000000000\"";
        assert!(refusal_words(arc).starts_with("USDC did not move"));
        // A token reason we have no sentence for is named, and nothing-paid is said.
        let other = alloy::sol_types::Revert::from("ERC20: something else");
        let data = alloy::hex::encode(alloy::sol_types::SolError::abi_encode(&other));
        let words = refusal_words(&format!("execution reverted, data: \"0x{data}\""));
        assert_eq!(
            words,
            "The chain refused this, so nothing was paid (ERC20: something else). Refresh and try again."
        );
        // A throttled public node (seen live 2026-10-15 on the challenge read):
        // where to look, never "nothing was paid".
        let busy = "contract call: Max retries exceeded server returned an error response: error code -32005: rate limit exceeded";
        assert!(refusal_words(busy).starts_with("The network is busy"));
        assert!(!refusal_words(busy).contains("nothing was paid"));
        // sent, not confirmed: may still land, so never "nothing was paid" (P-73)
        let sent = "transaction 0x9558d9ad0ba91a301e85c70847fd758dd7906bb37b94b9eabdf064028dface57 was sent but not confirmed within 90 s; check it on the explorer before sending again";
        assert!(
            refusal_words(sent)
                .starts_with("Sent, but the network has not confirmed it yet (0x9558d9ad")
        );
        assert!(!refusal_words(sent).contains("nothing was paid"));
        // sent, receipt read failed (arc `Confirmed`, docs/99 W3): the same words, never "refused"
        for why in [
            "null response",
            "deserialization error",
            "HTTP error 502",
            "connection closed",
        ] {
            let failed = format!(
                "transaction 0x9558d9ad0ba91a301e85c70847fd758dd7906bb37b94b9eabdf064028dface57 was sent; reading its receipt failed ({why}); check it on the explorer before sending again"
            );
            assert!(
                refusal_words(&failed)
                    .starts_with("Sent, but the network has not confirmed it yet (0x9558d9ad"),
                "{why}"
            );
        }
        // "nothing was sent" is not a send (docs/100 X10)
        assert_eq!(sent_unread("the relayer refused: nothing was sent"), None);
        assert!(sent_unread("transaction 0x9558d9ad0ba91a301e85c70847fd758dd7906bb37b94b9eabdf064028dface57 was sent; reading its receipt failed (x)").is_some());
        // Not a refusal: unchanged, so a real reason is never hidden.
        let plain = "this device's key is not one of this account's owners; ask an owner to add it";
        assert_eq!(refusal_words(plain), plain);
    }

    #[test]
    fn agent_tokens_gate_by_scope_and_expiry_and_an_open_tenant_stays_open() {
        let mut t: Tenant = serde_json::from_value(serde_json::json!({
            "chain": "arc-testnet", "owner": "0x1", "policy": "0x2", "agent": "0x3", "operator": "0x4",
            "watched": "0x5", "recovery": "0x0", "per_action": "1", "daily": "1", "floor": "0",
            "delay_secs": 60, "mode": 1, "created_block": 0, "history_from_block": 0, "created": 0
        }))
        .unwrap();
        assert_eq!(
            token_allows(&t, None, "run", 100),
            Ok(()),
            "no tokens: open"
        );
        assert!(
            token_allows(&t, None, "buy", 100).is_err(),
            "no tokens: buying stays closed until the owner opens it (PLAN 5.54)"
        );
        assert_eq!(scope_for("POST", "/api/level1/buy"), Some("buy"));
        t.agent_tokens.push(AgentToken {
            id: "t1".into(),
            label: "ops bot".into(),
            scopes: vec!["read".into(), "run".into()],
            expires: 1_000,
            issued: 1,
            hash: token_hash("s3cret"),
        });
        assert!(
            token_allows(&t, None, "read", 100).is_err(),
            "a token exists: the bearer is required"
        );
        assert!(token_allows(&t, Some("wrong"), "read", 100).is_err());
        assert_eq!(token_allows(&t, Some("s3cret"), "read", 100), Ok(()));
        assert_eq!(
            token_allows(&t, Some(" s3cret\n"), "run", 100),
            Ok(()),
            "whitespace around the secret is ignored"
        );
        assert!(
            token_allows(&t, Some("s3cret"), "prepare", 100).is_err(),
            "scope not carried"
        );
        assert!(
            token_allows(&t, Some("s3cret"), "read", 1_000).is_err(),
            "expired"
        );
        assert_eq!(
            scope_for("GET", "/api/level1/inbox?policy=0x2"),
            Some("read")
        );
        assert_eq!(scope_for("POST", "/api/level1/run"), Some("run"));
        assert_eq!(
            scope_for("POST", "/api/level1/prepare-limits"),
            Some("prepare")
        );
        assert_eq!(scope_for("POST", "/api/level1/bills"), Some("import"));
        // Gated in its handler, never open (docs/60 P-93).
        assert_eq!(scope_for("POST", "/api/level1/wallets"), None);
        assert_eq!(
            scope_for("POST", "/api/level1/prepare-token"),
            None,
            "the token flow itself is open"
        );
        assert_eq!(
            scope_for("POST", "/api/level1/execute"),
            None,
            "the passkey signs it"
        );
        assert_eq!(
            scope_for("POST", "/api/level1/pay-due"),
            None,
            "anyone may, the chain bounds it"
        );
        assert_eq!(scope_for("POST", "/api/watch"), None);
        // docs/60 P-176: what "anyone can check" stays open on a keyed account.
        assert_eq!(scope_for("GET", "/api/level1/records?policy=0x2"), None);
        assert_eq!(
            scope_for("GET", "/api/level1/record?policy=0x2&root=0x3"),
            None
        );
        assert_eq!(
            scope_for("GET", "/api/level1/statement?policy=0x2"),
            Some("read")
        );
        let a = token_challenge(
            5_042_002,
            Address::repeat_byte(2),
            &["read".into()],
            "ops",
            2_000,
            1_000,
        );
        assert_ne!(
            a,
            token_challenge(
                5_042_002,
                Address::repeat_byte(2),
                &["read".into(), "run".into()],
                "ops",
                2_000,
                1_000
            )
        );
        assert_ne!(
            a,
            token_revoke_challenge(5_042_002, Address::repeat_byte(2), "t1", 1_000)
        );
    }

    #[test]
    fn a_new_receiver_gets_a_new_secret_and_the_same_one_keeps_it() {
        let mut t: Tenant = serde_json::from_value(serde_json::json!({
            "chain": "arc-testnet", "owner": "0x1", "policy": "0x2", "agent": "0x3", "operator": "0x4",
            "watched": "0x5", "recovery": "0x0", "per_action": "1", "daily": "1", "floor": "0",
            "delay_secs": 60, "mode": 1, "created_block": 0, "history_from_block": 0, "created": 0
        }))
        .unwrap();
        assert!(apply_notify(&mut t, Some("ftp://x"), None, false).is_err());
        let first = apply_notify(&mut t, Some("https://a.example/h"), None, false).unwrap();
        assert!(first.is_some(), "a first URL mints a secret");
        assert!(
            apply_notify(&mut t, Some("https://a.example/h"), None, false)
                .unwrap()
                .is_none(),
            "same URL keeps it"
        );
        let second = apply_notify(
            &mut t,
            Some("https://b.example/h"),
            Some(Some(" 42 ".into())),
            false,
        )
        .unwrap();
        assert!(
            second.is_some() && second != first,
            "a new URL gets a new secret"
        );
        assert_eq!(t.telegram_chat_id.as_deref(), Some("42"));
        assert!(
            apply_notify(&mut t, Some(""), Some(None), false)
                .unwrap()
                .is_none()
        );
        assert!(t.notify_url.is_none() && t.telegram_chat_id.is_none());
    }

    #[test]
    fn a_passkey_assertion_verifies_against_the_contracts_key_and_only_that_challenge() {
        use p256::ecdsa::signature::hazmat::PrehashSigner;
        use sha2::{Digest, Sha256};
        let sk = p256::ecdsa::SigningKey::from_slice(&[7u8; 32]).unwrap();
        let pt = sk.verifying_key().to_encoded_point(false);
        let qx = B256::from_slice(pt.x().unwrap());
        let qy = B256::from_slice(pt.y().unwrap());
        let policy = Address::from_str("0x1645951793f7ca2ae36d7a3eeae90ef93e1932ab").unwrap();
        let ch = notify_challenge(
            5_042_002,
            policy,
            "https://x.example/hook",
            "",
            1_791_900_000,
        );
        assert_ne!(
            ch,
            notify_challenge(
                5_042_002,
                policy,
                "https://x.example/hook",
                "",
                1_791_900_001
            )
        );
        let cd = format!(
            "{{\"type\":\"webauthn.get\",\"challenge\":\"{}\",\"origin\":\"http://localhost:7880\"}}",
            b64url(ch.as_slice())
        );
        let ad = vec![0xaa; 37];
        let mut h = Sha256::new();
        h.update(&ad);
        h.update(Sha256::digest(cd.as_bytes()));
        let sig: p256::ecdsa::Signature = sk.sign_prehash(&h.finalize()).unwrap();
        let (r, s) = sig.split_bytes();
        let auth = PasskeyAuth {
            r: B256::from_slice(&r),
            s: B256::from_slice(&s),
            challenge_index: U256::from(cd.find("\"challenge\"").unwrap()),
            type_index: U256::from(0),
            authenticator_data: ad.clone(),
            client_data_json: cd.clone(),
        };
        assert_eq!(verify_passkey_assertion(qx, qy, ch, &auth), Ok(()));
        let other = notify_challenge(
            5_042_002,
            policy,
            "https://evil.example/hook",
            "",
            1_791_900_000,
        );
        assert!(
            verify_passkey_assertion(qx, qy, other, &auth)
                .unwrap_err()
                .contains("different challenge")
        );
        let mut bad = auth.clone();
        bad.authenticator_data[0] ^= 1;
        assert!(
            verify_passkey_assertion(qx, qy, ch, &bad)
                .unwrap_err()
                .contains("does not verify")
        );
        assert!(verify_passkey_assertion(B256::ZERO, qy, ch, &auth).is_err());
        assert_eq!(b64url(b""), "");
        assert_eq!(b64url(b"f"), "Zg");
        assert_eq!(b64url(b"fo"), "Zm8");
        assert_eq!(b64url(b"foo"), "Zm9v");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn the_webhook_signature_is_hmac_sha256_over_timestamp_dot_body() {
        // HMAC-SHA256(key="key", msg="1700000000.{}") — fixed vector, computed
        // once with Python's hmac; a receiver in any language reproduces it.
        let sig = webhook_signature("6b6579", 1_700_000_000, b"{}");
        assert!(sig.starts_with("sha256="));
        assert_eq!(sig.len(), 7 + 64);
        // Changing one byte of the body changes the signature.
        assert_ne!(sig, webhook_signature("6b6579", 1_700_000_000, b"{ }"));
        // The same inputs always sign the same.
        assert_eq!(sig, webhook_signature("6b6579", 1_700_000_000, b"{}"));
    }

    #[test]
    fn the_snapshotted_bytecode_is_hex_and_present() {
        for (name, hex) in [
            ("PasskeyPrincipal", PASSKEY_PRINCIPAL_BYTECODE),
            ("BudgetPolicy", BUDGET_POLICY_BYTECODE),
        ] {
            let bytes = alloy::hex::decode(hex.trim()).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(
                bytes.len() > 3_000,
                "{name}: creation code is {} bytes",
                bytes.len()
            );
        }
    }

    #[test]
    fn constructor_args_encode_as_a_plain_tuple() {
        let args = (B256::ZERO, B256::ZERO, Address::ZERO, U256::from(60u64)).abi_encode();
        assert_eq!(args.len(), 4 * 32);
        let args = (
            Address::ZERO,
            Address::ZERO,
            Address::ZERO,
            Address::ZERO,
            U256::from(1u64),
            U256::from(1u64),
            U256::from(2u64),
            U256::ZERO,
            U256::from(60u64),
            U256::from(1u8),
        )
            .abi_encode();
        assert_eq!(args.len(), 10 * 32);
        assert_eq!(
            args[args.len() - 1],
            1,
            "mode Propose lands in the last word"
        );
    }

    #[test]
    fn an_assertion_json_round_trips() {
        let v = json!({
            "r": format!("{:#x}", B256::repeat_byte(1)),
            "s": format!("{:#x}", B256::repeat_byte(2)),
            "challengeIndex": 23,
            "typeIndex": 1,
            "authenticatorData": "0x0102",
            "clientDataJSON": "{\"type\":\"webauthn.get\"}"
        });
        let a = auth_from_json(&v).unwrap();
        assert_eq!(a.challenge_index, U256::from(23u64));
        assert_eq!(a.authenticator_data, vec![1, 2]);
        assert!(auth_from_json(&json!({})).is_err());
    }

    #[test]
    fn a_public_tenant_carries_no_secret() {
        let mut t: Tenant = serde_json::from_value(json!({
            "chain": "arc-testnet", "owner": "0x1", "policy": "0x2", "agent": "0x3", "operator": "0x4",
            "watched": "0x5", "recovery": "0x6", "per_action": "1", "daily": "2", "floor": "0",
            "delay_secs": 60, "mode": 1, "created_block": 1, "history_from_block": 1, "created": 0,
            "owner_tx": null, "policy_tx": null, "agent_gas_tx": null, "notify_url": "https://x",
            "notify_secret": "deadbeef", "telegram_chat_id": null
        }))
        .unwrap();
        t.agent_tokens.push(AgentToken {
            id: "tok-1".into(),
            label: "ops".into(),
            scopes: vec!["read".into()],
            hash: "ab".repeat(32),
            issued: 0,
            expires: 1,
        });
        let v = t.public();
        let text = v.to_string();
        assert!(!text.contains("deadbeef"));
        assert!(!text.contains(&"ab".repeat(32)));
        assert_eq!(v["notifySet"], true);
        assert_eq!(v["agentTokenCount"], 1);
        // nor the webhook URL, chat id, bills or join requests (P-70)
        for k in ["notify_url", "telegram_chat_id", "bills", "join_requests"] {
            assert!(v.get(k).is_none(), "{k} must not be public");
        }
        assert_eq!(v["policy"], "0x2");
    }

    #[test]
    fn policy_salt_is_per_owner_wallet_and_kind() {
        let o = Address::repeat_byte(1);
        let w = Address::repeat_byte(2);
        assert_eq!(policy_salt(o, w, false), policy_salt(o, w, false));
        assert_ne!(policy_salt(o, w, false), policy_salt(o, w, true));
        assert_ne!(policy_salt(o, w, false), policy_salt(w, o, false));
    }

    #[test]
    fn a_tenant_file_without_the_deployed_field_is_deployed() {
        let t: Tenant = serde_json::from_value(json!({
            "chain": "arc-testnet", "owner": "0x1", "policy": "0x2", "agent": "0x3", "operator": "0x4",
            "watched": "0x5", "recovery": "0x6", "per_action": "1", "daily": "2", "floor": "0",
            "delay_secs": 60, "mode": 1, "created_block": 1, "history_from_block": 1, "created": 0,
            "owner_tx": null, "policy_tx": null, "agent_gas_tx": null, "notify_url": null,
            "notify_secret": null, "telegram_chat_id": null
        }))
        .unwrap();
        assert!(t.deployed);
        assert!(t.qx.is_none());
    }

    #[test]
    fn tenants_save_and_load_by_policy_in_any_case() {
        let dir = std::env::temp_dir().join(format!("tameion-l1-{}", std::process::id()));
        let t = Tenant {
            chain: "arc-testnet".into(),
            kind: "hold".into(),
            deployed: true,
            keyless: false,
            partner: None,
            join_requests: Vec::new(),
            qx: None,
            qy: None,
            agent_tokens: Vec::new(),
            owner: "0x1".into(),
            policy: "0x00000000000000000000000000000000000AbCdE".into(),
            agent: "0x2".into(),
            operator: "0x3".into(),
            watched: "0x4".into(),
            recovery: "0x5".into(),
            per_action: "1".into(),
            daily: "2".into(),
            floor: "0".into(),
            delay_secs: 60,
            mode: 1,
            created_block: 10,
            history_from_block: 1,
            created: 0,
            owner_tx: None,
            policy_tx: None,
            agent_gas_tx: None,
            notify_url: None,
            bills: Vec::new(),
            notify_secret: None,
            telegram_chat_id: None,
            name: None,
            wallets: Vec::new(),
            recovery_explicit: false,
            spare_key: None,
            caps_provisional: false,
            reserve_token_hash: None,
            contacts: Vec::new(),
        };
        save(&dir, &t).unwrap();
        let p = "0x00000000000000000000000000000000000abcde";
        assert_eq!(
            load(&dir, p).unwrap().policy,
            "0x00000000000000000000000000000000000AbCdE"
        );
        assert!(load(&dir, &p.to_uppercase().replace("0X", "0x")).is_some());
        // Only an address names a file (docs/60 P-45).
        assert!(load(&dir, &format!("./{p}")).is_none());
        assert!(load(&dir, "../tenants/x").is_none());
        assert_eq!(list(&dir).len(), 1);
        let _ = fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod atomic_save_tests {
    use super::*;

    /// docs/60 P-221: a reader racing the writer sees a whole tenant every time.
    #[test]
    fn a_tenant_read_during_a_save_is_never_half_a_file() {
        let dir = std::env::temp_dir().join(format!("tameion-atomic-{}", std::process::id()));
        fs::create_dir_all(tenants_dir(&dir)).unwrap();
        let policy = "0x00000000000000000000000000000000000000e1";
        let mut t: Tenant = serde_json::from_value(json!({
            "chain": "arc-testnet", "owner": "0x1", "policy": policy, "agent": "0x3", "operator": "0x4",
            "watched": "0x5", "recovery": "0x0", "per_action": "1", "daily": "1", "floor": "0",
            "delay_secs": 60, "mode": 1, "created_block": 0, "history_from_block": 0, "created": 0
        }))
        .unwrap();
        // A big file, so a write takes long enough to be caught half done.
        t.join_requests = (0..2000)
            .map(|i| {
                (
                    format!("0x{i:064x}"),
                    format!("0x{i:064x}"),
                    "a device".repeat(8),
                    i,
                )
            })
            .collect();
        save(&dir, &t).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = {
            let (dir, t, stop) = (dir.clone(), t.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    save(&dir, &t).unwrap();
                }
            })
        };
        let mut misses = 0;
        for _ in 0..300 {
            if load(&dir, policy).is_none() {
                misses += 1;
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        writer.join().unwrap();
        let leftovers = fs::read_dir(tenants_dir(&dir)).unwrap().count();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(misses, 0, "a read saw a half-written tenant");
        assert_eq!(leftovers, 1, "no temporary file left behind");
    }
}

#[cfg(test)]
mod offered_cycles_tests {
    use super::*;

    /// docs/60 P-210: the references each finished pass of this account offered
    /// for a bill-cycle, and nothing from another account or a failed pass.
    #[test]
    fn the_passes_offered_references_name_their_bill_cycle() {
        let dir = std::env::temp_dir().join(format!("tameion-offered-{}", std::process::id()));
        let jobs = dir.join("jobs");
        fs::create_dir_all(&jobs).unwrap();
        let cycle = "0x179dca89f69faef5765d2e7445aa5452b8159520@1792156029";
        let row = |h: u8| json!({ "actionHash": format!("{:#x}", B256::repeat_byte(h)), "settlementKey": format!("{:#x}", B256::repeat_byte(0xf3)), "cycleKey": cycle });
        let job = |policy: &str, status: &str, h: u8| json!({ "status": status, "request": { "level1": "run", "policy": policy }, "report": { "pendingSignature": [row(h)] } });
        let mine = "0x48274c7904edb23f75ea607c932d5c441c1a80e3";
        fs::write(jobs.join("a.json"), job(mine, "done", 0x85).to_string()).unwrap();
        fs::write(
            jobs.join("b.json"),
            job(&mine.to_uppercase().replace("0X", "0x"), "done", 0xd1).to_string(),
        )
        .unwrap();
        fs::write(jobs.join("c.json"), job(mine, "failed", 0x01).to_string()).unwrap();
        fs::write(
            jobs.join("d.json"),
            job("0x0000000000000000000000000000000000000001", "done", 0x02).to_string(),
        )
        .unwrap();
        let got = offered_cycles(&dir, mine);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(got.len(), 3, "{got:?}");
        for h in [0x85, 0xd1, 0xf3] {
            assert_eq!(
                got.get(&B256::repeat_byte(h)).map(String::as_str),
                Some(cycle)
            );
        }
    }
}

#[cfg(test)]
mod history_cache_tests {
    use super::*;

    #[test]
    fn the_history_cache_round_trips_and_belongs_to_one_wallet() {
        let dir = std::env::temp_dir().join(format!("tameion-hist-{}", std::process::id()));
        fs::create_dir_all(tenants_dir(&dir)).unwrap();
        let w5 = format!("{:#x}", Address::repeat_byte(5));
        let mut t: Tenant = serde_json::from_value(json!({
            "chain": "arbitrum-sepolia", "owner": "0x1", "policy": "0xABC", "agent": "0x3", "operator": "0x4",
            "watched": w5, "recovery": "0x0", "per_action": "1", "daily": "1", "floor": "0",
            "delay_secs": 60, "mode": 1, "created_block": 0, "history_from_block": 0, "created": 0
        }))
        .unwrap();
        let tr = ledger::obligations::Transfer {
            tx_hash: "0xaa".into(),
            counterparty: ledger::obligations::Counterparty::new("0xBEEF"),
            amount: Money::minor(1_960, Currency::Usdc),
            at: Timestamp::from_unix(1_791_988_210),
            direction: ledger::obligations::Direction::Out,
        };
        save_history(
            &dir,
            &t,
            &History {
                lo: 10,
                hi: 20,
                transfers: vec![tr.clone()],
            },
        );
        let h = load_history(&dir, &t).expect("cache");
        assert_eq!((h.lo, h.hi), (10, 20));
        assert_eq!(h.transfers, vec![tr]);
        // A cache written before `sources` was kept still belongs to its wallet.
        let p = history_path(&dir, &t.policy);
        let mut old: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        old.as_object_mut().unwrap().remove("sources");
        fs::write(&p, old.to_string()).unwrap();
        assert!(load_history(&dir, &t).is_some(), "old cache, same wallet");
        // The same wallet listed again in `wallets` reads the same history.
        t.wallets = vec![w5.clone()];
        assert!(load_history(&dir, &t).is_some());
        // Another wallet connected, or another watched wallet (or chain),
        // never reuses this history.
        t.wallets.push(format!("{:#x}", Address::repeat_byte(7)));
        assert!(load_history(&dir, &t).is_none());
        t.wallets.clear();
        t.watched = format!("{:#x}", Address::repeat_byte(6));
        assert!(load_history(&dir, &t).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod account_name_tests {
    //! docs/79 §6a Service 7: names, history sources, provisional caps.
    use super::*;

    fn named(watched: Address, wallets: &[Address]) -> Tenant {
        serde_json::from_value(json!({
            "chain": "arbitrum-sepolia", "owner": "0x1", "policy": format!("{:#x}", Address::repeat_byte(0xc1)),
            "agent": "0x3", "operator": "0x4", "watched": format!("{watched:#x}"), "recovery": format!("{:#x}", Address::ZERO),
            "per_action": "50.000000 USDC", "daily": "100.000000 USDC", "floor": "0.000000 USDC",
            "delay_secs": 60, "mode": 1, "created_block": 0, "history_from_block": 0, "created": 0,
            "deployed": false, "name": "acme", "caps_provisional": true,
            "wallets": wallets.iter().map(|a| format!("{a:#x}")).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn a_reserve_token_opens_only_its_reserved_account() {
        // docs/60 P-87: the gate is a secret the record keeps hashed, not the
        // public key the tenant route serves.
        let mut t = named(Address::ZERO, &[]);
        assert!(!reserve_token_ok(&t, Some("anything")), "no token minted");
        let tok = mint_reserve_token(&mut t);
        assert_eq!(tok.len(), 64);
        assert_eq!(
            t.reserve_token_hash.as_deref(),
            Some(token_hash(&tok).as_str())
        );
        assert!(reserve_token_ok(&t, Some(&tok)));
        assert!(
            !reserve_token_ok(&t, Some(&token_hash(&tok))),
            "the hash is not the token"
        );
        for bad in [None, Some(""), Some("  "), Some("00")] {
            assert!(!reserve_token_ok(&t, bad), "{bad:?}");
        }
        let v = t.public();
        assert!(v.get("reserve_token_hash").is_none() && !v.to_string().contains(&tok));
        // A second mint replaces the first.
        let again = mint_reserve_token(&mut t);
        assert!(!reserve_token_ok(&t, Some(&tok)) && reserve_token_ok(&t, Some(&again)));
        // After deploy it stays the owner's device token for /wallets (P-93).
        t.deployed = true;
        assert!(reserve_token_ok(&t, Some(&again)));
    }

    #[test]
    fn the_reserve_token_challenge_matches_its_documented_bytes() {
        // cast keccak 0x<"tameion.reserve-token.v1"> ‖ 421614 u64 ‖ 0x…d1 ‖ 0x67000000 u64
        let policy = Address::from_str("0x00000000000000000000000000000000000000d1").unwrap();
        assert_eq!(
            format!(
                "{:#x}",
                reserve_token_challenge(421_614, policy, 0x6700_0000)
            ),
            "0x44f87f27ab1211749325aa73f4c89335569eeaa17fb35672b066790ebf5192e5"
        );
    }

    #[test]
    fn a_name_is_lowercase_letters_digits_and_inner_dashes() {
        for ok in ["acme", "a-1", "uat-1729000000", &"x".repeat(32)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "ac",
            "Acme",
            "-acme",
            "acme-",
            "ac me",
            "acmé",
            "a_b",
            &"x".repeat(33),
            "",
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn an_old_record_loads_unchanged_and_reads_its_watched_wallet() {
        let w = Address::repeat_byte(5);
        let t: Tenant = serde_json::from_value(json!({
            "chain": "arc-testnet", "owner": "0x1", "policy": "0xABC", "agent": "0x3", "operator": "0x4",
            "watched": format!("{w:#x}"), "recovery": format!("{w:#x}"), "per_action": "1", "daily": "1", "floor": "0",
            "delay_secs": 60, "mode": 1, "created_block": 0, "history_from_block": 0, "created": 0
        }))
        .unwrap();
        assert_eq!(t.name, None);
        assert!(t.wallets.is_empty() && t.spare_key.is_none());
        assert!(!t.recovery_explicit && !t.caps_provisional);
        assert_eq!(history_sources(&t), vec![w]);
    }

    #[test]
    fn a_pass_on_a_name_first_account_reads_only_its_wallets_never_the_zero_address() {
        let (a, b) = (Address::repeat_byte(0xa), Address::repeat_byte(0xb));
        // Reserved by name: watched is 0x0 and stays in the salt only.
        assert!(history_sources(&named(Address::ZERO, &[])).is_empty());
        assert_eq!(history_sources(&named(Address::ZERO, &[a, b])), vec![a, b]);
        // A zero address that slipped into the list is dropped too.
        assert_eq!(
            history_sources(&named(Address::ZERO, &[Address::ZERO, a])),
            vec![a]
        );
        // Opened from a wallet: that wallet once, whether listed or not.
        assert_eq!(history_sources(&named(a, &[a])), vec![a]);
        assert_eq!(history_sources(&named(a, &[b])), vec![b, a]);
    }

    #[test]
    fn provisional_caps_follow_the_known_bills_until_deploy() {
        let dir = std::env::temp_dir().join(format!("tameion-caps-{}", std::process::id()));
        fs::create_dir_all(tenants_dir(&dir)).unwrap();
        let a = Address::repeat_byte(0xa);
        let mut t = named(Address::ZERO, &[a]);
        let caps = |t: &Tenant| (t.per_action.clone(), t.daily.clone());
        let usdc = |n: &str| Money::parse(n, Currency::Usdc).unwrap().to_string();
        // Nothing known: the small default.
        refresh_provisional_caps(&dir, &mut t);
        assert_eq!(caps(&t), (usdc("50"), usdc("100")));
        // A typed bill: twice the largest, per payment and per day.
        t.bills.push(SeededBill {
            payee: format!("{:#x}", Address::repeat_byte(0x11)),
            amount: "30".into(),
            every_days: 30,
            next_due: 0,
            discount_bps: 0,
            early_days: 0,
        });
        refresh_provisional_caps(&dir, &mut t);
        assert_eq!(caps(&t), (usdc("60"), usdc("60")));
        // A bill the read of these wallets found, larger: it leads.
        let preview = |sources: Value| json!({ "status": "done", "sources": sources, "bills": [{ "amount": "70.000000 USDC" }] });
        fs::write(
            preview_path(&dir, &t.policy),
            preview(sources_key(&t)).to_string(),
        )
        .unwrap();
        refresh_provisional_caps(&dir, &mut t);
        assert_eq!(caps(&t), (usdc("140"), usdc("140")));
        // A read of other wallets is not this account's.
        fs::write(
            preview_path(&dir, &t.policy),
            preview(json!(["0x00"])).to_string(),
        )
        .unwrap();
        refresh_provisional_caps(&dir, &mut t);
        assert_eq!(caps(&t), (usdc("60"), usdc("60")));
        // Typed caps, or an account on chain, are never moved.
        t.caps_provisional = false;
        t.bills[0].amount = "500".into();
        refresh_provisional_caps(&dir, &mut t);
        assert_eq!(caps(&t), (usdc("60"), usdc("60")));
        t.caps_provisional = true;
        t.deployed = true;
        refresh_provisional_caps(&dir, &mut t);
        assert_eq!(caps(&t), (usdc("60"), usdc("60")));
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod second_chain_tests {
    //! PLAN 5.58: an account on a chain where USDC is an ERC-20 and gas is ETH.
    use super::*;

    fn arb() -> &'static ChainPreset {
        crate::watch::chain("arbitrum-sepolia").expect("preset")
    }

    #[test]
    fn an_erc20_chain_gets_the_token_policy_with_its_usdc_first() {
        let usdc = Money::parse("1", Currency::Usdc).unwrap();
        let code = policy_initcode(
            arb(),
            false,
            Address::repeat_byte(1),
            Address::repeat_byte(2),
            Address::repeat_byte(3),
            Address::repeat_byte(4),
            usdc,
            usdc,
            Money::zero(Currency::Usdc),
            60,
        )
        .unwrap();
        let token = alloy::hex::decode(BUDGET_POLICY_TOKEN_BYTECODE.trim()).unwrap();
        assert!(
            code.starts_with(&token),
            "the ERC-20 policy, not the native one"
        );
        let first_arg = &code[token.len()..token.len() + 32];
        assert_eq!(
            &first_arg[12..],
            Address::from_str(arb().usdc.unwrap()).unwrap().as_slice(),
            "the chain's USDC is the first constructor argument"
        );
    }

    #[test]
    fn gas_amounts_are_in_eth_off_arc_and_in_usdc_on_it() {
        let arc = crate::watch::chain("arc-testnet").unwrap();
        assert!(relayer_floor_wei(arb()) < relayer_floor_wei(arc) / U256::from(50));
        assert!(agent_gas_wei(arb()) > agent_gas_floor_wei(arb()));
        assert_eq!(
            native_text(arb(), U256::from(300_000_000_000_000u128)),
            "0.000300 ETH"
        );
    }

    #[tokio::test]
    async fn the_wallet_account_is_refused_off_arc_before_anything_is_sent() {
        let one = Money::parse("1", Currency::Usdc).unwrap();
        let r = DeployRequest {
            chain: arb(),
            pull: false,
            qx: B256::repeat_byte(1),
            qy: B256::repeat_byte(2),
            recovery: Address::ZERO,
            watched: Address::repeat_byte(9),
            per_action: one,
            daily: one,
            floor: Money::zero(Currency::Usdc),
            days: 7,
            notify_url: None,
            name: None,
            recovery_explicit: false,
            caps_provisional: false,
            keyless: false,
            wallet: true,
        };
        let dir = std::env::temp_dir().join(format!("tameion-2chain-{}", std::process::id()));
        let e = reserve(&dir, r).await.unwrap_err();
        assert!(e.contains("Arc only"), "{e}");
        assert!(!dir.exists(), "nothing written");
    }
}
