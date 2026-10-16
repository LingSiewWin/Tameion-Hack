//! A counterparty joins a desk by a link (`docs/105`, D11 of `docs/102`): the
//! owner types no address.
//!
//! The owner mints a one-use invite for a role and sends the link. The
//! counterparty's page makes its key and signs [`DeskJoin`] on the desk's own
//! EIP-712 domain — the one its steps use (version 2, `verifyingContract` =
//! the desk), so a join signed for one trader cannot be shown to another — and
//! the service keeps the answer beside the invite. Nothing here approves
//! anyone: an answer is one row in front of the owner, whose passkey approves
//! it through `POST /api/desk/party` like any other counterparty.

use crate::desk::{DOMAIN_NAME, EVENT_DOMAIN_VERSION, Party};
use alloy::primitives::{Address, B256, Signature, keccak256};
use alloy::sol_types::{SolStruct, eip712_domain};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;

alloy::sol! {
    /// What a counterparty's key signs to ask to sign steps for a desk.
    struct DeskJoin {
        string name;
        string role;
        string invite;
        uint64 at;
    }
}

/// The roles an invite can name. What each may sign is the owner's page's
/// table (`apps/console/src/components/desk/trust.tsx`, `ROLES`), shown beside
/// the row and signed by the owner's passkey with the approval itself.
pub const ROLES: [&str; 6] = [
    "Supplier", "Carrier", "Vault", "Workshop", "Retailer", "Buyer",
];
/// An invite is answered within a week, once; after that it is gone.
pub const TTL_SECS: u64 = 7 * 86_400;
/// Open invites per desk: a desk with more is asked to dismiss some.
const MAX_OPEN: usize = 50;

/// The counterparty's answer: the name it chose and the key that signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub name: String,
    pub address: String,
    pub at: u64,
    pub signature: String,
    pub received: u64,
}

/// One invite: only the hash of its secret is kept, never the secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub id: String,
    pub role: String,
    pub created: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dismissed: bool,
}

/// One writer at a time for every desk's invites file.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn file(dir: &Path, policy: &str) -> PathBuf {
    crate::level1::tenants_dir(dir)
        .join(policy.trim().to_lowercase())
        .join("invites.json")
}

fn load(dir: &Path, policy: &str) -> Vec<Invite> {
    std::fs::read_to_string(file(dir, policy))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(dir: &Path, policy: &str, all: &[Invite]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let p = file(dir, policy);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string(all).map_err(|e| e.to_string())?;
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&p)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| e.to_string())
}

const fn live(i: &Invite, now: u64) -> bool {
    now.saturating_sub(i.created) <= TTL_SECS
}

fn id_of(secret: &str) -> String {
    format!("{:#x}", keccak256(secret.trim().as_bytes()))
}

/// The digest a counterparty's key signs to join `desk` as `name` in `role`,
/// answering the invite whose secret is `invite`.
pub fn join_digest(
    chain_id: u64,
    desk: Address,
    name: &str,
    role: &str,
    invite: &str,
    at: u64,
) -> B256 {
    DeskJoin {
        name: name.into(),
        role: role.into(),
        invite: invite.into(),
        at,
    }
    .eip712_signing_hash(&eip712_domain! {
        name: DOMAIN_NAME,
        version: EVENT_DOMAIN_VERSION,
        chain_id: chain_id,
        verifying_contract: desk,
    })
}

/// A new invite for `role`: the secret goes into the link once and is
/// returned with when it lapses; only its hash stays on disk.
pub fn mint(
    dir: &Path,
    policy: &str,
    role: &str,
    now: u64,
) -> Result<(String, String, u64), String> {
    let role = ROLES
        .iter()
        .find(|r| r.eq_ignore_ascii_case(role.trim()))
        .ok_or_else(|| format!("an invite is for one of: {}", ROLES.join(", ")))?;
    let secret = crate::level1::new_secret();
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(dir, policy);
    all.retain(|i| live(i, now));
    if all
        .iter()
        .filter(|i| i.answer.is_none() && !i.dismissed)
        .count()
        >= MAX_OPEN
    {
        return Err(format!(
            "{MAX_OPEN} invites are still open on this desk; they lapse after a week"
        ));
    }
    all.push(Invite {
        id: id_of(&secret),
        role: (*role).to_string(),
        created: now,
        answer: None,
        dismissed: false,
    });
    save(dir, policy, &all)?;
    Ok((secret, (*role).to_string(), now + TTL_SECS))
}

/// The counterparty's answer to an invite, `{invite, name, address, at,
/// signature}` from its page. Checked: the invite is this desk's, live and not
/// answered by another key; the key at `address` signed [`DeskJoin`] for this
/// desk, this invite's role and this name, after the invite was made (a return
/// link may be opened hours later; the invite is good once, so it is not replayed); the name
/// is a name (`parties::clean_name`) and not another key's on this desk. The
/// same answer sent twice is answered the same (a retry).
#[allow(clippy::too_many_arguments)]
pub fn propose(
    dir: &Path,
    policy: &str,
    chain_id: u64,
    desk: Address,
    parties: &[Party],
    stopped: &HashSet<String>,
    v: &Value,
    now: u64,
) -> Result<Value, String> {
    let secret = v["invite"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("the link carries no invite: ask the trader for a new one")?;
    let raw_name = v["name"].as_str().unwrap_or("");
    let name = crate::parties::clean_name(raw_name)?;
    if name != raw_name {
        return Err(
            "this name changes when it is cleaned (invisible characters, compatibility forms, extra spaces): sign the cleaned name".into(),
        );
    }
    let address = v["address"]
        .as_str()
        .and_then(|a| Address::from_str(a.trim()).ok())
        .filter(|a| !a.is_zero())
        .ok_or("address must be the signing key's address")?;
    let at = v["at"]
        .as_u64()
        .ok_or("at: when it was signed, in seconds")?;
    if at > now + 60 {
        return Err("this answer is dated in the future: check the phone's clock".into());
    }
    let signature = v["signature"].as_str().unwrap_or("").trim();
    let bytes = alloy::hex::decode(signature.trim_start_matches("0x"))
        .map_err(|e| format!("signature: {e}"))?;
    let sig = Signature::from_raw(&bytes).map_err(|e| format!("signature: {e}"))?;
    let id = id_of(secret);
    let key = format!("{address:#x}");

    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(dir, policy);
    let Some(ix) = all
        .iter()
        .position(|i| i.id == id && live(i, now) && !i.dismissed)
    else {
        return Err(
            "this invite is not open on that desk (used, lapsed or never made): ask the trader for a new link"
                .into(),
        );
    };
    if at < all[ix].created {
        return Err("this answer was signed before the invite was made".into());
    }
    let role = all[ix].role.clone();
    let who = sig
        .recover_address_from_prehash(&join_digest(chain_id, desk, &name, &role, secret, at))
        .map_err(|e| format!("signature: {e}"))?;
    if who != address {
        return Err(format!(
            "the signature is not by that key; it was signed for another desk, role or name (signed by {who:#x})"
        ));
    }
    let reply = || json!({ "proposed": { "name": name, "role": role, "address": key } });
    if let Some(a) = &all[ix].answer {
        return if a.address == key && a.name == name {
            Ok(reply())
        } else {
            Err("this invite was already answered: ask the trader for a new link".into())
        };
    }
    // A name another key holds — or one that reads like it (docs/103 S2-2: an invisible
    // character, a Cyrillic letter) — would put two identical rows in front of the owner:
    // refused here, to the one who can change it. A stopped counterparty's name may be taken again.
    let want = crate::parties::name_key(&name);
    let alike = |other: &str| crate::parties::name_key(other) == want;
    let answers = || {
        all.iter()
            .filter(|i| live(i, now) && !i.dismissed)
            .filter_map(|i| i.answer.as_ref())
    };
    let held = parties
        .iter()
        .any(|p| alike(&p.name) && p.address != key && !stopped.contains(&p.name))
        || answers().any(|a| alike(&a.name) && a.address != key);
    if held {
        return Err(format!(
            "{name} is already another key's name on this desk, or reads like it: choose another name"
        ));
    }
    // One key, one name (docs/103 S2-5): a key already on the desk under another name would
    // sign for two roles, the purchase and the arrival both.
    if let Some(other) = parties
        .iter()
        .find(|p| p.address == key && p.name != name && !stopped.contains(&p.name))
        .map(|p| p.name.clone())
        .or_else(|| {
            answers()
                .find(|a| a.address == key && a.name != name)
                .map(|a| a.name.clone())
        })
    {
        return Err(format!(
            "this key already signs for this desk as {other}: one key, one name; make a new key for this invite"
        ));
    }
    all[ix].answer = Some(Answer {
        name: name.clone(),
        address: key.clone(),
        at,
        signature: signature.to_string(),
        received: now,
    });
    save(dir, policy, &all)?;
    Ok(reply())
}

/// The answers waiting for the owner: live, not dismissed, and not yet a
/// counterparty under that name and key.
pub fn pending(
    dir: &Path,
    policy: &str,
    parties: &[Party],
    stopped: &HashSet<String>,
    now: u64,
) -> Vec<Value> {
    load(dir, policy)
        .into_iter()
        .filter(|i| live(i, now) && !i.dismissed)
        .filter_map(|i| {
            let a = i.answer?;
            let approved = parties
                .iter()
                .any(|p| p.name == a.name && p.address == a.address && !stopped.contains(&p.name));
            (!approved).then(|| {
                json!({ "id": i.id, "role": i.role, "name": a.name, "address": a.address, "at": a.at, "received": a.received, "invited": i.created })
            })
        })
        .collect()
}

/// The owner turns an answer (or an open invite) down: trust does not grow,
/// so the device is enough. `false` when there is no such invite.
pub fn dismiss(dir: &Path, policy: &str, id: &str) -> Result<bool, String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(dir, policy);
    let Some(i) = all
        .iter_mut()
        .find(|i| i.id.eq_ignore_ascii_case(id.trim()))
    else {
        return Ok(false);
    };
    i.dismissed = true;
    save(dir, policy, &all)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::{SignerSync, local::PrivateKeySigner};

    const DESK: Address = Address::repeat_byte(0xd5);
    const CHAIN: u64 = 421_614;
    const NOW: u64 = 1_792_100_000;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("desk-invite-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn answer(key: &PrivateKeySigner, secret: &str, name: &str, role: &str, at: u64) -> Value {
        let sig = key
            .sign_hash_sync(&join_digest(CHAIN, DESK, name, role, secret, at))
            .unwrap();
        json!({ "invite": secret, "name": name, "address": format!("{:#x}", key.address()), "at": at,
                "signature": format!("0x{}", alloy::hex::encode(sig.as_bytes())) })
    }

    #[test]
    fn the_join_digest_is_the_one_the_page_signs() {
        // Printed by viem's hashTypedData with the same domain and message
        // (apps/console/test/names.test.mts holds the same vector).
        let d = join_digest(
            CHAIN,
            DESK,
            "Kim Seng Bullion",
            "Supplier",
            "ab12",
            1_792_100_000,
        );
        assert_eq!(
            format!("{d:#x}"),
            "0x1dd441fc93b8b4e0f674e5357cf5f95594b246acc50d8e8701fcb58bd3a3dc4f"
        );
    }

    #[test]
    fn an_invite_is_answered_once_by_the_key_that_signed_it() {
        let d = dir("once");
        let p = format!("{DESK:#x}");
        let (secret, role, expires) = mint(&d, &p, "supplier", NOW).unwrap();
        assert_eq!((role.as_str(), expires), ("Supplier", NOW + TTL_SECS));
        let key = PrivateKeySigner::random();
        let none = HashSet::new();

        // Signed for another role, another desk's invite, or by another key: refused.
        let wrong_role = answer(&key, &secret, "Kim Seng", "Vault", NOW);
        assert!(propose(&d, &p, CHAIN, DESK, &[], &none, &wrong_role, NOW).is_err());
        let mut forged = answer(&key, &secret, "Kim Seng", "Supplier", NOW);
        forged["address"] = json!(format!("{:#x}", PrivateKeySigner::random().address()));
        assert!(propose(&d, &p, CHAIN, DESK, &[], &none, &forged, NOW).is_err());
        let no_invite = answer(&key, "not-the-secret", "Kim Seng", "Supplier", NOW);
        assert!(propose(&d, &p, CHAIN, DESK, &[], &none, &no_invite, NOW).is_err());
        // Signed before the invite existed, dated in the future, or a name that looks like an address: refused.
        let before = answer(&key, &secret, "Kim Seng", "Supplier", NOW - 1);
        assert!(propose(&d, &p, CHAIN, DESK, &[], &none, &before, NOW).is_err());
        let future = answer(&key, &secret, "Kim Seng", "Supplier", NOW + 3_600);
        assert!(propose(&d, &p, CHAIN, DESK, &[], &none, &future, NOW).is_err());
        let hexy = answer(&key, &secret, "0xKim", "Supplier", NOW);
        assert!(propose(&d, &p, CHAIN, DESK, &[], &none, &hexy, NOW).is_err());

        // Opened two hours after it was signed (a return link sent by WhatsApp): still good.
        let good = answer(&key, &secret, "Kim Seng", "Supplier", NOW);
        let r = propose(&d, &p, CHAIN, DESK, &[], &none, &good, NOW + 7_200).unwrap();
        assert_eq!(r["proposed"]["role"], "Supplier");
        // A retry is answered the same; another key on the same invite is refused.
        assert!(propose(&d, &p, CHAIN, DESK, &[], &none, &good, NOW + 5).is_ok());
        let other = answer(
            &PrivateKeySigner::random(),
            &secret,
            "Kim Seng 2",
            "Supplier",
            NOW,
        );
        assert!(propose(&d, &p, CHAIN, DESK, &[], &none, &other, NOW).is_err());

        let rows = pending(&d, &p, &[], &none, NOW);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "Kim Seng");
        assert_eq!(rows[0]["address"], format!("{:#x}", key.address()));
        // Approved under that name and key: no longer waiting.
        let party = Party {
            name: "Kim Seng".into(),
            address: format!("{:#x}", key.address()),
            approval: None,
            kinds: None,
        };
        assert!(pending(&d, &p, &[party], &none, NOW).is_empty());
        // The secret is not on disk, only its hash.
        let disk = std::fs::read_to_string(file(&d, &p)).unwrap();
        assert!(!disk.contains(&secret) && disk.contains(&id_of(&secret)));
        // A week later the invite is gone.
        assert!(pending(&d, &p, &[], &none, NOW + TTL_SECS + 1).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_name_held_by_another_key_is_refused_and_a_dismissed_answer_waits_no_more() {
        let d = dir("names");
        let p = format!("{DESK:#x}");
        let held = Party {
            name: "Supplier".into(),
            address: format!("{:#x}", Address::repeat_byte(0x11)),
            approval: None,
            kinds: None,
        };
        let none = HashSet::new();
        let (secret, _, _) = mint(&d, &p, "Supplier", NOW).unwrap();
        let key = PrivateKeySigner::random();
        let a = answer(&key, &secret, "Supplier", "Supplier", NOW);
        assert!(
            propose(
                &d,
                &p,
                CHAIN,
                DESK,
                std::slice::from_ref(&held),
                &none,
                &a,
                NOW
            )
            .is_err()
        );
        // Once that party is stopped, its name may be taken by a new key.
        let stopped: HashSet<String> = ["Supplier".to_string()].into();
        assert!(propose(&d, &p, CHAIN, DESK, &[held], &stopped, &a, NOW).is_ok());
        let id = pending(&d, &p, &[], &none, NOW)[0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(dismiss(&d, &p, &id).unwrap());
        assert!(pending(&d, &p, &[], &none, NOW).is_empty());
        assert!(!dismiss(&d, &p, "0x00").unwrap());
        assert!(mint(&d, &p, "Banker", NOW).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_look_alike_name_and_a_second_name_for_one_key_are_refused() {
        // docs/103 S2-2 and S2-5, on the reviewer's own name.
        let d = dir("alike");
        let p = format!("{DESK:#x}");
        let none = HashSet::new();
        let first = PrivateKeySigner::random();
        let (s1, _, _) = mint(&d, &p, "Supplier", NOW).unwrap();
        let ok = answer(&first, &s1, "Lim Brothers Gold", "Supplier", NOW);
        assert!(propose(&d, &p, CHAIN, DESK, &[], &none, &ok, NOW).is_ok());
        let (s2, _, _) = mint(&d, &p, "Supplier", NOW).unwrap();
        let stranger = PrivateKeySigner::random();
        // The live attack: + U+200B. It does not survive cleaning, so the signed name is not the name.
        let zwsp = answer(&stranger, &s2, "Lim Brothers Gold\u{200B}", "Supplier", NOW);
        let e = propose(&d, &p, CHAIN, DESK, &[], &none, &zwsp, NOW).unwrap_err();
        assert!(e.contains("invisible"), "{e}");
        // A Cyrillic о survives cleaning; the fold catches it, against the pending answer and the approved party alike.
        let cyr = answer(&stranger, &s2, "Lim Br\u{043E}thers Gold", "Supplier", NOW);
        let e = propose(&d, &p, CHAIN, DESK, &[], &none, &cyr, NOW).unwrap_err();
        assert!(e.contains("reads like it"), "{e}");
        let approved = Party {
            name: "Lim Brothers Gold".into(),
            address: format!("{:#x}", first.address()),
            approval: None,
            kinds: None,
        };
        assert!(
            propose(
                &d,
                &p,
                CHAIN,
                DESK,
                std::slice::from_ref(&approved),
                &none,
                &cyr,
                NOW
            )
            .is_err()
        );
        assert_eq!(
            pending(&d, &p, &[], &none, NOW).len(),
            1,
            "one row, not two identical ones"
        );
        // One key, one name: the approved supplier's key cannot answer a vault invite as another name.
        let (s3, _, _) = mint(&d, &p, "Vault", NOW).unwrap();
        let second = answer(&first, &s3, "Harbour Vault", "Vault", NOW);
        let e = propose(&d, &p, CHAIN, DESK, &[approved], &none, &second, NOW).unwrap_err();
        assert!(e.contains("one key, one name"), "{e}");
        // The row says which invite it answers.
        assert_eq!(pending(&d, &p, &[], &none, NOW)[0]["invited"], NOW);
        let _ = std::fs::remove_dir_all(&d);
    }
}
