//! Signed invoices from vendors, or from vendors' agents (docs/65 §5.4).
//!
//! A vendor (or the agent that bills for it) signs an invoice to one account
//! with the key of the address the money goes to, and posts it. The agent
//! checks it — signed by the payee itself, for this account on this chain,
//! terms inside the rules, not seen before, the payee approved or not, frozen
//! by the issuer or not, the amount as usual or not — and writes it down for
//! the owner, in words. It pays nothing: an invoice only becomes a bill the
//! agent may pay once the owner accepts it, and its payee still needs the
//! owner's passkey like any other. This replaces typing bills into "Add
//! bills" with an agent-to-agent hand-off; the trust decision stays a tap.
//!
//! The signature proves the invoice comes from whoever controls the payee
//! address, not that the address is the vendor the owner thinks it is: an
//! unknown payee is still a trust decision, said as one.

use crate::level1::{self, SeededBill, Tenant};
use alloy::primitives::{Address, B256, Signature, U256, keccak256};
use alloy::sol_types::SolValue;
use ledger::{Currency, Money};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The EIP-712 struct the payee signs. `everyDays = 0` is a one-off invoice.
pub const INVOICE_TYPE: &str = "Invoice(address payee,address policy,uint256 amount,uint64 dueAt,uint32 everyDays,uint16 discountBps,uint16 earlyDays,bytes32 ref,uint64 issuedAt)";
/// Version 2 (docs/81): the payee also signs who it says it is — a name and
/// its web domain — so the owner reads "Acme Ltd · acme.com", not `0x…`.
/// Domain version "2"; a body with `from` or `domain` is read as version 2.
pub const INVOICE_TYPE_V2: &str = "Invoice(address payee,address policy,uint256 amount,uint64 dueAt,uint32 everyDays,uint16 discountBps,uint16 earlyDays,bytes32 ref,uint64 issuedAt,string from,string domain)";

/// Largest invoice accepted, in USDC minor units (10 million USDC): above
/// this it is a typo or an attack, never a bill.
pub const MAX_AMOUNT_MINOR: u128 = 10_000_000 * 1_000_000;
/// How stale an invoice may be when it arrives, and how far its due date may
/// lie ahead.
const MAX_AGE_SECS: u64 = 90 * 86_400;
const MAX_AHEAD_SECS: u64 = 366 * 86_400;
/// Clock skew allowed on `issuedAt`.
const SKEW_SECS: u64 = 300;

/// One invoice, as signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoice {
    pub payee: Address,
    pub policy: Address,
    /// USDC minor units (six decimals).
    pub amount: u128,
    pub due_at: u64,
    pub every_days: u32,
    pub discount_bps: u16,
    pub early_days: u16,
    pub reference: B256,
    pub issued_at: u64,
    /// Version 2 only: the name and domain the payee signs for itself.
    pub issuer: Option<Issuer>,
}

/// Who the payee says it is (invoice version 2). Claimed, not proved: the
/// domain is checked separately (docs/81 phase B).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Issuer {
    pub from: String,
    pub domain: String,
}

/// A web domain as a person types it: lowercase labels of letters, digits and
/// inner hyphens, at least one dot, 253 characters at most.
pub fn valid_domain(d: &str) -> bool {
    d.len() <= 253
        && d.contains('.')
        && d.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// The invoice's identity on this account: `keccak256(abi.encode(
/// "tameion.invoice", payee, ref))`. It keys the store and is the action
/// hash a one-off payment is settled under. Not the bare `ref`: the vendor
/// chooses `ref`, and the contract's `executed` map is shared with the
/// agent's payments and standing orders — a vendor could set `ref` to a
/// standing order's next action hash and, once its own invoice was paid,
/// make that order unpayable (the session-5 review, docs/60 P-46). Nor may a
/// stranger claim another vendor's `ref` first: the id includes the payee.
pub fn id(payee: Address, reference: B256) -> B256 {
    keccak256(("tameion.invoice".to_string(), payee, reference).abi_encode())
}

/// The id of a stored line (lines written before ids carried only payee + ref).
pub fn line_id(v: &Value) -> Option<B256> {
    if let Some(i) = v["id"].as_str().and_then(|x| B256::from_str(x).ok()) {
        return Some(i);
    }
    let payee = v["payee"]
        .as_str()
        .and_then(|x| Address::from_str(x).ok())?;
    let r = v["ref"].as_str().and_then(|x| B256::from_str(x).ok())?;
    Some(id(payee, r))
}

/// The digest the payee signs: domain ("Tameion Invoice", "1", chain,
/// policy), the same construction as `arc::purchase_digest`.
pub fn digest(chain_id: u64, inv: &Invoice) -> B256 {
    let domain = keccak256(
        (
            keccak256(
                "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
            ),
            keccak256("Tameion Invoice"),
            keccak256(if inv.issuer.is_some() { "2" } else { "1" }),
            U256::from(chain_id),
            inv.policy,
        )
            .abi_encode(),
    );
    let head = (
        inv.payee,
        inv.policy,
        U256::from(inv.amount),
        U256::from(inv.due_at),
        U256::from(inv.every_days),
        U256::from(inv.discount_bps),
        U256::from(inv.early_days),
        inv.reference,
        U256::from(inv.issued_at),
    );
    let body = keccak256(match &inv.issuer {
        None => [keccak256(INVOICE_TYPE).abi_encode(), head.abi_encode()].concat(),
        // EIP-712 encodes a string as the keccak256 of its bytes.
        Some(i) => [
            keccak256(INVOICE_TYPE_V2).abi_encode(),
            head.abi_encode(),
            (keccak256(i.from.as_bytes()), keccak256(i.domain.as_bytes())).abi_encode(),
        ]
        .concat(),
    });
    keccak256([&[0x19u8, 0x01][..], domain.as_slice(), body.as_slice()].concat())
}

/// The body a vendor posts: the invoice's fields (amount in minor units, as
/// a string) and the payee's 65-byte signature.
pub fn to_json(inv: &Invoice, signature: &[u8]) -> Value {
    let mut v = json!({
        "policy": format!("{:#x}", inv.policy),
        "payee": format!("{:#x}", inv.payee),
        "amount": inv.amount.to_string(),
        "dueAt": inv.due_at,
        "everyDays": inv.every_days,
        "discountBps": inv.discount_bps,
        "earlyDays": inv.early_days,
        "ref": format!("{:#x}", inv.reference),
        "issuedAt": inv.issued_at,
        "signature": alloy::hex::encode_prefixed(signature),
    });
    if let Some(i) = &inv.issuer {
        v["from"] = json!(i.from);
        v["domain"] = json!(i.domain);
    }
    v
}

fn from_json(v: &Value) -> Result<(Invoice, Vec<u8>), String> {
    let addr = |k: &str| {
        v[k].as_str()
            .and_then(|a| Address::from_str(a.trim()).ok())
            .ok_or_else(|| format!("{k} is not an address"))
    };
    let int = |k: &str| {
        v[k].as_u64()
            .ok_or_else(|| format!("{k} is not a whole number"))
    };
    let small = |k: &str, max: u64| {
        int(k).and_then(|n| {
            (n <= max)
                .then_some(n)
                .ok_or_else(|| format!("{k} is too large"))
        })
    };
    let inv = Invoice {
        payee: addr("payee")?,
        policy: addr("policy")?,
        amount: v["amount"]
            .as_str()
            .and_then(|a| a.parse().ok())
            .ok_or("amount must be USDC minor units as a string (\"150000\" = 0.15 USDC)")?,
        due_at: int("dueAt")?,
        every_days: small("everyDays", u64::from(u32::MAX))? as u32,
        discount_bps: small("discountBps", u64::from(u16::MAX))? as u16,
        early_days: small("earlyDays", u64::from(u16::MAX))? as u16,
        reference: v["ref"]
            .as_str()
            .and_then(|r| B256::from_str(r.trim()).ok())
            .ok_or("ref is not 32 bytes of hex")?,
        issued_at: int("issuedAt")?,
        issuer: (v.get("from").is_some() || v.get("domain").is_some()).then(|| Issuer {
            from: v["from"].as_str().unwrap_or("").to_string(),
            domain: v["domain"].as_str().unwrap_or("").to_string(),
        }),
    };
    let sig = v["signature"]
        .as_str()
        .and_then(|s| alloy::hex::decode(s.trim()).ok())
        .ok_or("signature is not hex")?;
    Ok((inv, sig))
}

/// The invoice in `body`, if the payee signed it for `policy` on `chain_id`
/// and its terms are inside the rules. Pure: what it says no to needs no
/// chain.
pub fn verify(chain_id: u64, policy: Address, body: &Value, now: u64) -> Result<Invoice, String> {
    let (inv, sig) = from_json(body)?;
    if inv.policy != policy {
        return Err("this invoice is addressed to another account".into());
    }
    let signature = Signature::from_raw(&sig).map_err(|e| format!("signature: {e}"))?;
    let signer = signature
        .recover_address_from_prehash(&digest(chain_id, &inv))
        .map_err(|e| format!("signature: {e}"))?;
    // Any change to a field, the account or the chain recovers someone else.
    if signer != inv.payee {
        return Err(format!(
            "not signed by the payee: the signature is {signer:#x}'s, the money would go to {:#x}",
            inv.payee
        ));
    }
    if inv.amount == 0 || inv.amount > MAX_AMOUNT_MINOR {
        return Err("the amount must be more than zero and at most 10,000,000 USDC".into());
    }
    if inv.reference == B256::ZERO {
        return Err("the invoice needs its own reference".into());
    }
    if inv.issued_at > now.saturating_add(SKEW_SECS)
        || inv.issued_at.saturating_add(MAX_AGE_SECS) < now
    {
        return Err("the invoice is dated in the future or more than 90 days ago".into());
    }
    if inv.due_at.saturating_add(MAX_AGE_SECS) < now
        || inv.due_at > now.saturating_add(MAX_AHEAD_SECS)
    {
        return Err("the due date is more than 90 days past or a year ahead".into());
    }
    if let Some(i) = &inv.issuer {
        if i.from.chars().count() > 60
            || i.from.chars().any(char::is_control)
            || i.from.trim().to_ascii_lowercase().starts_with("0x")
        {
            return Err(
                "from is a name of at most 60 characters that does not look like an address".into(),
            );
        }
        if !i.domain.is_empty() && !valid_domain(&i.domain) {
            return Err("domain must be a lowercase web domain such as acme.com".into());
        }
        if i.from.trim().is_empty() && i.domain.is_empty() {
            return Err("a version 2 invoice names its sender: from, domain or both".into());
        }
    }
    if inv.every_days > 366 {
        return Err("a bill repeats at most once a year (everyDays 0..=366)".into());
    }
    // A one-off invoice is paid by the owner's tap at its full amount: a
    // discount on it would be shown and never applied (review, P-46).
    if inv.every_days == 0 && inv.discount_bps > 0 {
        return Err("an early-payment discount needs a repeating bill; a one-off invoice states its final amount".into());
    }
    if inv.discount_bps > crate::MAX_DISCOUNT_BPS
        || (inv.discount_bps > 0
            && (inv.early_days == 0
                || (inv.every_days > 0 && u32::from(inv.early_days) >= inv.every_days)))
    {
        return Err("early-payment terms must be at most 10% off, with an early window shorter than the billing interval".into());
    }
    Ok(inv)
}

/// What the agent found when it checked an invoice against the account.
#[derive(Debug, Clone, Default)]
pub struct Checks {
    /// The payee is already one the owner approved on the account.
    pub approved: bool,
    /// The stablecoin's issuer has frozen the payee.
    pub frozen: bool,
    /// What this payee usually bills, from the account's history.
    pub usual: Option<Money>,
    /// The account's limit for one payment by the agent.
    pub per_action: Option<Money>,
}

/// The checks in words for the owner, worst first.
pub fn words(inv: &Invoice, c: &Checks) -> Vec<String> {
    let amount = Money::minor(inv.amount as i128, Currency::Usdc);
    let mut w = Vec::new();
    if c.frozen {
        w.push(level1::frozen_words(&[format!("{:#x}", inv.payee)]));
    }
    // A one-off is paid by the owner's own signature and approves no one
    // (docs/60 P-46); only a repeating bill asks for the payee's approval.
    if !c.approved && inv.every_days > 0 {
        w.push(
            "Not one of your approved payees yet: accepting it approves this payee with your key."
                .into(),
        );
    }
    match c.usual {
        Some(u) if u.to_minor() != amount.to_minor() => w.push(format!(
            "This payee usually bills {u}; this invoice asks {amount}."
        )),
        Some(_) => w.push("Same amount this payee has billed before.".into()),
        None => w.push("No payment to this payee in the history the agent read.".into()),
    }
    // The ops lead's question: could the agent pay this? Not above the
    // contract's per-payment limit — say so before the owner decides
    // (docs/70 C9: a 1,000 USDC invoice to a 0.05 USDC-a-day account said
    // nothing about it).
    if let Some(cap) = c
        .per_action
        .filter(|cap| amount.to_minor() > cap.to_minor())
    {
        w.push(format!(
            "More than your limit for one payment ({cap}): the agent can never pay this by itself; only your tap can."
        ));
    }
    if inv.every_days == 0 {
        w.push("One payment, not a repeating bill.".into());
    } else {
        w.push(format!("Repeats every {} days.", inv.every_days));
    }
    if inv.discount_bps > 0 {
        w.push(format!(
            "{}% off when paid {} days early.",
            f64::from(inv.discount_bps) / 100.0,
            inv.early_days
        ));
    }
    w
}

/// The amount of the newest invoice from `payee` the owner accepted, if any.
fn paid_on_invoice(app_dir: &Path, policy: &str, payee: &str) -> Option<Money> {
    list(app_dir, policy)
        .iter()
        .find(|v| {
            v["status"] == "accepted"
                && v["payee"]
                    .as_str()
                    .is_some_and(|p| p.eq_ignore_ascii_case(payee))
        })
        .and_then(|v| v["amount"].as_str()?.parse::<i128>().ok())
        .map(|m| Money::minor(m, Currency::Usdc))
}

/// Where an account's invoices are written: one JSON line per arrival or
/// decision; the newest line for a reference is its state.
pub fn invoices_path(app_dir: &Path, policy: &str) -> PathBuf {
    level1::tenants_dir(app_dir).join(format!("{}.invoices.jsonl", policy.to_lowercase()))
}

fn append(app_dir: &Path, policy: &str, line: &Value) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(invoices_path(app_dir, policy))
        .map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

/// Every invoice of the account, its latest state folded in, newest first.
/// An arrival opens an invoice; only an `accepted` or `declined` line closes
/// it, and only while it is still `received` — a second arrival of the same
/// invoice (a replay racing the first) never reopens a decided one.
/// ponytail: the whole file read per call; an index when an account holds
/// thousands of invoices.
pub fn list(app_dir: &Path, policy: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for l in std::fs::read_to_string(invoices_path(app_dir, policy))
        .unwrap_or_default()
        .lines()
    {
        let Ok(mut v) = serde_json::from_str::<Value>(l) else {
            continue;
        };
        // A decision line from before ids (2026-10-14, only `ref`) closes the
        // invoice with that ref: ignoring it reopened paid invoices, and their
        // "Pay" would settle under the new id — paid twice (docs/60 P-46).
        let k = match line_id(&v) {
            Some(key) => format!("{key:#x}"),
            None => match out
                .iter()
                .find(|o| v["ref"].is_string() && o["ref"] == v["ref"])
            {
                Some(o) => o["id"].as_str().unwrap_or_default().to_string(),
                None => continue,
            },
        };
        match out.iter_mut().find(|o| o["id"] == k.as_str()) {
            Some(o)
                if o["status"] == "received"
                    && (v["status"] == "accepted" || v["status"] == "declined") =>
            {
                o["status"] = v["status"].clone();
                o["decidedAt"] = v["at"].clone();
            }
            Some(_) => {}
            None if v["status"] == "received" => {
                v["id"] = json!(k);
                out.push(v);
            }
            None => {}
        }
    }
    out.reverse();
    out
}

/// The invoices waiting for the owner.
pub fn pending(app_dir: &Path, policy: &str) -> Vec<Value> {
    list(app_dir, policy)
        .into_iter()
        .filter(|v| v["status"] == "received")
        .collect()
}

/// Whether the account has seen this invoice (this payee's `ref`) in any
/// state: a vendor re-sending it, or someone replaying it, is not a second bill.
pub fn seen(app_dir: &Path, policy: &str, key: B256) -> bool {
    let k = format!("{key:#x}");
    list(app_dir, policy).iter().any(|v| v["id"] == k.as_str())
}

/// Seen-check and append as one step: two copies of one signed body arriving
/// together (both pass `seen` while the chain is read) write one line.
/// ponytail: one lock for every account; per-account when invoices are busy.
static STORE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Verify, refuse a reference already seen, check against the account, and
/// write it down as received. `known` is what the account already pays,
/// (payee, amount) from the last pass, so "as usual" means the history.
/// Returns the line written; pays nothing.
pub async fn receive(
    app_dir: &Path,
    t: &Tenant,
    body: &Value,
    known: &[(String, Money)],
    now: u64,
) -> Result<Value, String> {
    let c = crate::watch::chain(&t.chain).ok_or("tenant chain unknown")?;
    let policy = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
    let inv = verify(c.chain_id, policy, body, now)?;
    let key = id(inv.payee, inv.reference);
    if seen(app_dir, &t.policy, key) {
        return Err("this invoice was received already".into());
    }
    let payee = format!("{:#x}", inv.payee);
    let approved = match level1::read_client(c) {
        Ok(client) if t.deployed => client.is_attested(policy, inv.payee).await.unwrap_or(false),
        _ => false,
    };
    let checks = Checks {
        approved,
        frozen: !level1::issuer_frozen(t, &[inv.payee]).await.is_empty(),
        // The last pass's recurring bills, else what the owner already paid
        // this payee on an invoice (docs/70 C6: two paid one-offs, then "No
        // payment to this payee").
        usual: known
            .iter()
            .find(|(who, _)| who.eq_ignore_ascii_case(&payee))
            .map(|(_, m)| *m)
            .or_else(|| paid_on_invoice(app_dir, &t.policy, &payee)),
        per_action: Money::parse(
            t.per_action.trim_end_matches(" USDC").trim(),
            Currency::Usdc,
        )
        .ok(),
    };
    let mut line = to_json(&inv, &[]);
    line["id"] = json!(format!("{key:#x}"));
    line["status"] = json!("received");
    line["at"] = json!(now);
    line["amountText"] = json!(Money::minor(inv.amount as i128, Currency::Usdc).to_string());
    line["approvedPayee"] = json!(checks.approved);
    line["frozen"] = json!(checks.frozen);
    line["notes"] = json!(words(&inv, &checks));
    // Kept as sent, so anyone can check the payee signed it.
    line["signature"] = body["signature"].clone();
    let _held = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if seen(app_dir, &t.policy, key) {
        return Err("this invoice was received already".into());
    }
    append(app_dir, &t.policy, &line)?;
    Ok(line)
}

/// The owner's decision on a received invoice: `accepted` or `declined`.
pub fn decide(
    app_dir: &Path,
    policy: &str,
    key: B256,
    accept: bool,
    now: u64,
) -> Result<Value, String> {
    let k = format!("{key:#x}");
    let _held = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let inv = pending(app_dir, policy)
        .into_iter()
        .find(|v| v["id"] == k.as_str())
        .ok_or("no invoice with this reference is waiting")?;
    if accept && inv["frozen"] == true {
        return Err("the issuer has frozen this payee; it cannot be paid".into());
    }
    let status = if accept { "accepted" } else { "declined" };
    append(
        app_dir,
        policy,
        &json!({ "id": k, "ref": inv["ref"], "payee": inv["payee"], "status": status, "at": now }),
    )?;
    let mut out = inv;
    out["status"] = json!(status);
    Ok(out)
}

/// An accepted repeating invoice as the bill "Add bills" would have made,
/// for `level1::set_bills` (which still requires the payee approved). A
/// one-off invoice is not a repeating bill: the owner pays it with one tap.
pub fn to_bill(line: &Value) -> Result<SeededBill, String> {
    let every_days = line["everyDays"].as_u64().unwrap_or(0) as u32;
    if every_days == 0 {
        return Err("a one-off invoice is paid by your tap, not added as a repeating bill".into());
    }
    let minor: i128 = line["amount"]
        .as_str()
        .and_then(|a| a.parse().ok())
        .ok_or("invoice line has no amount")?;
    Ok(SeededBill {
        payee: line["payee"].as_str().unwrap_or("").to_string(),
        amount: Money::minor(minor, Currency::Usdc)
            .to_string()
            .trim_end_matches(" USDC")
            .to_string(),
        every_days,
        next_due: line["dueAt"].as_u64().unwrap_or(0),
        discount_bps: line["discountBps"].as_u64().unwrap_or(0) as u16,
        early_days: line["earlyDays"].as_u64().unwrap_or(0) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::SignerSync;
    use alloy::signers::local::PrivateKeySigner;

    const CHAIN: u64 = 421_614;
    const NOW: u64 = 1_792_000_000;

    fn policy() -> Address {
        Address::from_str("0x26e6901e08b9928bbbc7d3523c9102da5e361e53").unwrap()
    }

    fn invoice(payee: Address) -> Invoice {
        Invoice {
            payee,
            policy: policy(),
            amount: 150_000,
            due_at: NOW + 10 * 86_400,
            every_days: 30,
            discount_bps: 200,
            early_days: 10,
            reference: keccak256("INV-2026-10-001"),
            issued_at: NOW - 60,
            issuer: None,
        }
    }

    fn signed(key: &PrivateKeySigner, chain: u64, inv: &Invoice) -> Value {
        let sig = key.sign_hash_sync(&digest(chain, inv)).unwrap();
        to_json(inv, &sig.as_bytes())
    }

    #[test]
    fn the_payees_own_signature_verifies_and_nothing_else_does() {
        let vendor = PrivateKeySigner::random();
        let inv = invoice(vendor.address());
        let body = signed(&vendor, CHAIN, &inv);
        assert_eq!(verify(CHAIN, policy(), &body, NOW), Ok(inv.clone()));

        // Someone else signs an invoice paying the vendor's address... or their own.
        let other = PrivateKeySigner::random();
        let e = verify(CHAIN, policy(), &signed(&other, CHAIN, &inv), NOW).unwrap_err();
        assert!(e.contains("not signed by the payee"), "{e}");

        // Another chain or another account.
        assert!(verify(CHAIN, policy(), &signed(&vendor, 5_042_002, &inv), NOW).is_err());
        let elsewhere = Address::from_str("0x7ebc3894ec793fa6c9225841992001e580da9273").unwrap();
        assert!(
            verify(CHAIN, elsewhere, &body, NOW)
                .unwrap_err()
                .contains("another account")
        );

        // Tampered after signing.
        let mut more = body.clone();
        more["amount"] = json!("1500000");
        assert!(
            verify(CHAIN, policy(), &more, NOW)
                .unwrap_err()
                .contains("not signed by the payee")
        );
    }

    #[test]
    fn terms_outside_the_rules_are_refused_even_when_signed() {
        let vendor = PrivateKeySigner::random();
        let mut inv = invoice(vendor.address());
        inv.discount_bps = crate::MAX_DISCOUNT_BPS + 1;
        assert!(
            verify(CHAIN, policy(), &signed(&vendor, CHAIN, &inv), NOW)
                .unwrap_err()
                .contains("10% off")
        );
        let mut inv = invoice(vendor.address());
        inv.early_days = 30;
        assert!(verify(CHAIN, policy(), &signed(&vendor, CHAIN, &inv), NOW).is_err());
        let mut inv = invoice(vendor.address());
        inv.amount = 0;
        assert!(verify(CHAIN, policy(), &signed(&vendor, CHAIN, &inv), NOW).is_err());
        let mut inv = invoice(vendor.address());
        inv.reference = B256::ZERO;
        assert!(verify(CHAIN, policy(), &signed(&vendor, CHAIN, &inv), NOW).is_err());
        let mut inv = invoice(vendor.address());
        inv.issued_at = NOW + 3_600;
        assert!(verify(CHAIN, policy(), &signed(&vendor, CHAIN, &inv), NOW).is_err());
        // A one-off invoice cannot carry a discount the owner's tap would not apply.
        let mut inv = invoice(vendor.address());
        inv.every_days = 0;
        assert!(
            verify(CHAIN, policy(), &signed(&vendor, CHAIN, &inv), NOW)
                .unwrap_err()
                .contains("one-off")
        );
        // Dates near u64::MAX do not overflow.
        let mut inv = invoice(vendor.address());
        inv.due_at = u64::MAX;
        assert!(verify(CHAIN, policy(), &signed(&vendor, CHAIN, &inv), NOW).is_err());
    }

    #[test]
    fn a_decision_written_before_ids_still_closes_its_invoice() {
        // Seen live 2026-10-15: lines from the first night carry only `ref`;
        // ignoring them reopened invoices already paid (docs/60 P-46).
        let dir =
            std::env::temp_dir().join(format!("tameion-invoice-legacy-{}", std::process::id()));
        std::fs::create_dir_all(level1::tenants_dir(&dir)).unwrap();
        let p = format!("{:#x}", policy());
        let vendor = PrivateKeySigner::random();
        let inv = invoice(vendor.address());
        let mut line = signed(&vendor, CHAIN, &inv);
        line["status"] = json!("received");
        line["at"] = json!(NOW);
        append(&dir, &p, &line).unwrap();
        append(
            &dir,
            &p,
            &json!({ "ref": format!("{:#x}", inv.reference), "status": "accepted", "at": NOW + 1 }),
        )
        .unwrap();
        assert!(pending(&dir, &p).is_empty());
        assert_eq!(list(&dir, &p)[0]["status"], "accepted");
        // docs/70 C6: a payee the owner already paid on an invoice is known.
        let vendor_hex = format!("{:#x}", vendor.address());
        assert_eq!(
            paid_on_invoice(&dir, &p, &vendor_hex.to_uppercase().replace("0X", "0x")),
            Some(Money::minor(150_000, Currency::Usdc))
        );
        assert_eq!(
            paid_on_invoice(&dir, &p, "0x0000000000000000000000000000000000005eed"),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reference_counts_once_and_an_accepted_one_becomes_the_import_bills_row() {
        let dir = std::env::temp_dir().join(format!("tameion-invoice-{}", std::process::id()));
        std::fs::create_dir_all(level1::tenants_dir(&dir)).unwrap();
        let p = format!("{:#x}", policy());
        let vendor = PrivateKeySigner::random();
        let inv = invoice(vendor.address());
        let mut line = signed(&vendor, CHAIN, &inv);
        line["status"] = json!("received");
        line["at"] = json!(NOW);
        append(&dir, &p, &line).unwrap();
        let key = id(vendor.address(), inv.reference);
        assert!(seen(&dir, &p, key));
        assert!(!seen(
            &dir,
            &p,
            id(vendor.address(), keccak256("INV-other"))
        ));
        assert!(
            !seen(&dir, &p, id(Address::repeat_byte(9), inv.reference)),
            "another payee's invoice with the same ref is a different invoice"
        );
        assert_ne!(
            key, inv.reference,
            "the vendor does not choose the action hash"
        );
        assert_eq!(pending(&dir, &p).len(), 1);

        let got = decide(&dir, &p, key, true, NOW + 5).unwrap();
        assert_eq!(got["status"], "accepted");
        assert!(pending(&dir, &p).is_empty());
        assert!(seen(&dir, &p, key), "a decided invoice is still seen");
        assert!(
            decide(&dir, &p, key, true, NOW + 6).is_err(),
            "decided once"
        );
        // A replayed arrival (it raced the first) never reopens it.
        append(&dir, &p, &line).unwrap();
        assert!(
            pending(&dir, &p).is_empty(),
            "a late copy does not reopen a decided invoice"
        );
        assert_eq!(
            to_bill(&got).unwrap(),
            SeededBill {
                payee: format!("{:#x}", vendor.address()),
                amount: "0.150000".into(),
                every_days: 30,
                next_due: NOW + 10 * 86_400,
                discount_bps: 200,
                early_days: 10,
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_owner_reads_the_checks_in_words() {
        let inv = invoice(Address::ZERO);
        let w = words(
            &inv,
            &Checks {
                approved: false,
                frozen: false,
                usual: Some(Money::minor(100_000, Currency::Usdc)),
                per_action: Some(Money::minor(50_000, Currency::Usdc)),
            },
        );
        assert!(
            w.iter().any(|x| x.contains("only your tap can")),
            "an amount over the per-payment limit is said: {w:?}"
        );
        assert!(w[0].starts_with("Not one of your approved payees"));
        assert!(w[1].contains("usually bills 0.100000 USDC"), "{w:?}");
        assert!(
            w.iter()
                .any(|x| x.contains("2% off when paid 10 days early"))
        );
        for x in &w {
            for banned in ["calldata", "nonce", "relayer", "root", "attest"] {
                assert!(!x.contains(banned), "{x}");
            }
        }
        // A one-off is paid by the owner's signature: no approval is asked.
        let once = Invoice {
            every_days: 0,
            discount_bps: 0,
            ..inv
        };
        let w = words(&once, &Checks::default());
        assert!(!w.iter().any(|x| x.contains("approved payees")), "{w:?}");
    }

    #[test]
    fn version_two_signs_who_the_payee_says_it_is() {
        let vendor = PrivateKeySigner::random();
        let mut inv = invoice(vendor.address());
        inv.issuer = Some(Issuer {
            from: "Acme Design Ltd".into(),
            domain: "acme.example".into(),
        });
        let body = signed(&vendor, CHAIN, &inv);
        assert_eq!(body["from"], "Acme Design Ltd");
        assert_eq!(verify(CHAIN, policy(), &body, NOW), Ok(inv.clone()));
        // The name is signed: changed after signing, the signature is someone else's.
        let mut forged = body.clone();
        forged["from"] = json!("Landlord");
        assert!(
            verify(CHAIN, policy(), &forged, NOW)
                .unwrap_err()
                .contains("not signed by the payee")
        );
        // Dropping the name turns it into a version 1 body, which the v2 signature does not match.
        let mut stripped = body.clone();
        stripped.as_object_mut().unwrap().remove("from");
        stripped.as_object_mut().unwrap().remove("domain");
        assert!(verify(CHAIN, policy(), &stripped, NOW).is_err());
        // Names that read as addresses, and domains that are not domains, are refused.
        for (from, domain) in [
            ("0xdeadbeef", ""),
            ("Acme", "ACME.COM"),
            ("Acme", "localhost"),
            ("", ""),
        ] {
            inv.issuer = Some(Issuer {
                from: from.into(),
                domain: domain.into(),
            });
            assert!(
                verify(CHAIN, policy(), &signed(&vendor, CHAIN, &inv), NOW).is_err(),
                "{from} {domain}"
            );
        }
        assert!(valid_domain("pay.acme-design.co.uk"));
        assert!(!valid_domain("-acme.com") && !valid_domain("acme..com"));
    }
}
