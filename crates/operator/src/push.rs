//! The agent tells you on your phone: standard Web Push to the installed
//! account page (docs/53 §5 item 1 without anybody's bot token; PLAN 5.51
//! made `/` installable).
//!
//! Design, grounded in the RFC texts (read 2026-10-14 from rfc-editor.org):
//!
//! - **Encryption, RFC 8291 + RFC 8188 (`aes128gcm`).** One record. A fresh
//!   P-256 key pair (`as_private`, `as_public`) and a random 16-byte salt per
//!   message. `ecdh_secret = ECDH(as_private, ua_public)`;
//!   `PRK_key = HMAC-SHA-256(auth_secret, ecdh_secret)`;
//!   `IKM = HMAC-SHA-256(PRK_key, "WebPush: info" || 0x00 || ua_public || as_public || 0x01)`;
//!   `PRK = HMAC-SHA-256(salt, IKM)`;
//!   `CEK = HMAC-SHA-256(PRK, "Content-Encoding: aes128gcm" || 0x00 || 0x01)[..16]`;
//!   `NONCE = HMAC-SHA-256(PRK, "Content-Encoding: nonce" || 0x00 || 0x01)[..12]`;
//!   ciphertext = AES-128-GCM(CEK, NONCE, plaintext || 0x02). Body = salt (16)
//!   || rs (4, big-endian, 4096) || idlen (1) = 65 || `as_public` (65) ||
//!   ciphertext; at most 3993 octets of plaintext (RFC 8291 §4). The test
//!   below reproduces the RFC 8291 §5 message byte for byte from its
//!   Appendix A inputs.
//! - **Identity, RFC 8292 (VAPID).** `Authorization: vapid t=<JWT>, k=<key>`:
//!   an ES256 JWT `{"typ":"JWT","alg":"ES256"}` over
//!   `{"aud": <push endpoint origin>, "exp": now + 12 h (≤ 24 h), "sub": ...}`
//!   and `k` = the uncompressed public key, base64url. One key pair per host,
//!   made on first use and kept at `~/.tameion/vapid.key` (mode 600, outside
//!   the tree, never printed); the page subscribes with its public half.
//! - **Crates.** Nothing new in the graph: `p256` (ECDSA; ECDH as one point
//!   multiplication, since its `ecdh` feature would add `hkdf`), `hmac` and
//!   `sha2` for HKDF's two HMAC steps, `ring`'s AES-128-GCM (already compiled
//!   in through rustls), `base64`.
//! - **Who may subscribe.** The owner's passkey signs a challenge over the
//!   subscription (domain-separated from the webhook's), checked against the
//!   key the contract holds — the same rule as the report address. A
//!   stranger cannot point the agent's reports at their own browser.
//! - **What is sent.** A line or two ("Paid 0.35 USDC to 2 payees · 2 bills
//!   need you"): what the webhook already carries, no addresses. Sent only
//!   when it differs from the last one (an hourly pass with the same two
//!   bills waiting does not ring every hour); a subscription the push
//!   service says is gone (404/410) is dropped. TTL one day.

use alloy::primitives::{Address, B256};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use serde_json::{Value, json};
use sha2::Sha256;
use std::path::{Path, PathBuf};

/// Record size written into the header; one record always fits (≤ 3993
/// octets of plaintext, RFC 8291 §4).
const RS: u32 = 4096;

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    for p in parts {
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

fn random<const N: usize>() -> Result<[u8; N], String> {
    use ring::rand::SecureRandom;
    let mut b = [0u8; N];
    ring::rand::SystemRandom::new()
        .fill(&mut b)
        .map_err(|_| "no system randomness".to_string())?;
    Ok(b)
}

/// The ECDH shared secret: the x coordinate of `their * mine` (SEC 1
/// §3.3.1), with p256's own arithmetic — the crate's `ecdh` feature would
/// add `hkdf` to the graph for one multiplication.
fn ecdh(mine: &SecretKey, their: &PublicKey) -> [u8; 32] {
    use p256::elliptic_curve::point::AffineCoordinates;
    let shared = (their.to_projective() * *mine.to_nonzero_scalar()).to_affine();
    shared.x().into()
}

fn uncompressed(p: &PublicKey) -> Vec<u8> {
    p.to_encoded_point(false).as_bytes().to_vec()
}

/// CEK and NONCE from the ECDH secret (RFC 8291 §3.4, both sides).
fn content_key(
    ecdh_secret: &[u8],
    auth_secret: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8],
) -> Result<(ring::aead::LessSafeKey, ring::aead::Nonce), String> {
    let prk_key = hmac(auth_secret, &[ecdh_secret]);
    let ikm = hmac(&prk_key, &[b"WebPush: info\0", ua_public, as_public, &[1]]);
    let prk = hmac(salt, &[&ikm]);
    let cek = hmac(&prk, &[b"Content-Encoding: aes128gcm\0", &[1]]);
    let nonce = hmac(&prk, &[b"Content-Encoding: nonce\0", &[1]]);
    let key =
        ring::aead::UnboundKey::new(&ring::aead::AES_128_GCM, &cek[..16]).map_err(|_| "AES key")?;
    let n: [u8; 12] = nonce[..12].try_into().expect("12 bytes");
    Ok((
        ring::aead::LessSafeKey::new(key),
        ring::aead::Nonce::assume_unique_for_key(n),
    ))
}

/// The `aes128gcm` body for one push message (RFC 8291 §3.4, RFC 8188 §2):
/// header (salt, rs, keyid = `as_public`) then the one sealed record. Pure
/// given its inputs, so the RFC's example checks it byte for byte.
pub fn encrypt(
    ua_public: &[u8],
    auth_secret: &[u8],
    as_secret: &SecretKey,
    salt: &[u8; 16],
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    if plaintext.len() > 3993 {
        return Err("a push message carries at most 3993 octets".into());
    }
    let ua = PublicKey::from_sec1_bytes(ua_public).map_err(|_| "p256dh is not a P-256 key")?;
    let as_public = uncompressed(&as_secret.public_key());
    let (key, nonce) = content_key(
        &ecdh(as_secret, &ua),
        auth_secret,
        ua_public,
        &as_public,
        salt,
    )?;
    let mut record = plaintext.to_vec();
    record.push(0x02); // last (and only) record, no padding
    key.seal_in_place_append_tag(nonce, ring::aead::Aad::empty(), &mut record)
        .map_err(|_| "AES-GCM seal")?;

    let mut out = Vec::with_capacity(86 + record.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&RS.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(&as_public);
    out.extend_from_slice(&record);
    Ok(out)
}

/// `Authorization: vapid t=<JWT>, k=<public key>` for `endpoint` (RFC 8292
/// §2–3): ES256 over `aud` = the endpoint's origin, `exp` twelve hours out.
pub fn vapid_header(vapid: &SecretKey, endpoint: &str, now: u64) -> Result<String, String> {
    use p256::ecdsa::signature::Signer;
    let u = reqwest::Url::parse(endpoint).map_err(|e| e.to_string())?;
    let aud = u.origin().ascii_serialization();
    let head = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = B64.encode(
        json!({ "aud": aud, "exp": now + 12 * 3600, "sub": "mailto:push@tameion.app" }).to_string(),
    );
    let signing = format!("{head}.{claims}");
    let sig: p256::ecdsa::Signature = p256::ecdsa::SigningKey::from(vapid).sign(signing.as_bytes());
    Ok(format!(
        "vapid t={signing}.{}, k={}",
        B64.encode(sig.to_bytes()),
        B64.encode(uncompressed(&vapid.public_key()))
    ))
}

/// The host's VAPID key: `~/.tameion/vapid.key` (hex scalar, mode 600),
/// made on first use. Never printed; only its public half leaves.
pub fn vapid_key() -> Result<SecretKey, String> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var("TAMEION_VAPID_KEY")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".tameion/vapid.key")
        });
    if let Ok(hex) = std::fs::read_to_string(&path) {
        let b = alloy::hex::decode(hex.trim()).map_err(|_| "vapid.key is not hex")?;
        return SecretKey::from_slice(&b).map_err(|_| "vapid.key is not a P-256 key".into());
    }
    let key = loop {
        if let Ok(k) = SecretKey::from_slice(&random::<32>()?) {
            break k;
        }
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, alloy::hex::encode(key.to_bytes())).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    Ok(key)
}

/// The public half the page subscribes with (base64url, uncompressed).
pub fn vapid_public() -> Result<String, String> {
    Ok(B64.encode(uncompressed(&vapid_key()?.public_key())))
}

/// What the owner's passkey signs to add this browser: a domain tag of its
/// own (never a webhook challenge), the chain, the account, the
/// subscription, when it was issued.
pub fn challenge(
    chain_id: u64,
    policy: Address,
    endpoint: &str,
    p256dh: &str,
    auth: &str,
    issued: u64,
) -> B256 {
    let mut m = Vec::new();
    m.extend_from_slice(b"tameion.push.v1");
    m.extend_from_slice(&chain_id.to_be_bytes());
    m.extend_from_slice(policy.as_slice());
    for s in [endpoint, p256dh, auth] {
        m.extend_from_slice(&(s.len() as u32).to_be_bytes());
        m.extend_from_slice(s.as_bytes());
    }
    m.extend_from_slice(&issued.to_be_bytes());
    alloy::primitives::keccak256(m)
}

/// A subscription as the browser gives it (`PushSubscription.toJSON()`),
/// checked: an https endpoint, a P-256 key, a 16-byte secret.
pub fn subscription(v: &Value) -> Result<(String, String, String), String> {
    let endpoint = v["endpoint"].as_str().unwrap_or("").trim().to_string();
    let p256dh = v["keys"]["p256dh"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    let auth = v["keys"]["auth"].as_str().unwrap_or("").trim().to_string();
    if !endpoint.starts_with("https://") {
        return Err("the subscription's endpoint must be https".into());
    }
    let k = B64.decode(&p256dh).map_err(|_| "p256dh is not base64url")?;
    PublicKey::from_sec1_bytes(&k).map_err(|_| "p256dh is not a P-256 key")?;
    if B64
        .decode(&auth)
        .map_err(|_| "auth is not base64url")?
        .len()
        != 16
    {
        return Err("auth must be 16 bytes".into());
    }
    Ok((endpoint, p256dh, auth))
}

fn path(app_dir: &Path, policy: &str) -> PathBuf {
    crate::level1::tenants_dir(app_dir).join(format!("{}.push.json", policy.to_lowercase()))
}

fn load(app_dir: &Path, policy: &str) -> Value {
    std::fs::read_to_string(path(app_dir, policy))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({ "subs": [], "last": "" }))
}

fn store(app_dir: &Path, policy: &str, v: &Value) -> Result<(), String> {
    std::fs::write(path(app_dir, policy), v.to_string()).map_err(|e| e.to_string())
}

/// Add (or refresh) one browser for `policy`; returns how many it has.
pub fn add(
    app_dir: &Path,
    policy: &str,
    endpoint: &str,
    p256dh: &str,
    auth: &str,
) -> Result<usize, String> {
    let mut v = load(app_dir, policy);
    let mut subs: Vec<Value> = v["subs"].as_array().cloned().unwrap_or_default();
    subs.retain(|s| s["endpoint"] != endpoint);
    subs.push(json!({ "endpoint": endpoint, "p256dh": p256dh, "auth": auth }));
    let n = subs.len();
    v["subs"] = json!(subs);
    store(app_dir, policy, &v)?;
    Ok(n)
}

/// How many browsers `policy` tells.
pub fn count(app_dir: &Path, policy: &str) -> usize {
    load(app_dir, policy)["subs"].as_array().map_or(0, Vec::len)
}

/// The line a pass is worth telling, or `None` when nothing happened that a
/// person needs: paid, waiting for a tap, or the check failed. Amounts as
/// the webhook carries them, summed; no addresses.
pub fn line(status: &str, report: Option<&Value>) -> Option<String> {
    if status == "failed" {
        return Some(
            "The agent's check did not finish; nothing was paid. It tries again on the next check."
                .into(),
        );
    }
    let r = report?;
    let paid: Vec<i128> = r["settled"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            ledger::Money::parse(
                s["amount"].as_str()?.trim_end_matches(" USDC").trim(),
                ledger::Currency::Usdc,
            )
            .ok()
            .map(|m| m.to_minor())
        })
        .collect();
    let waiting = r["pendingSignature"].as_array().map_or(0, Vec::len);
    let mut parts = Vec::new();
    if !paid.is_empty() {
        let total = ledger::Money::minor(paid.iter().sum(), ledger::Currency::Usdc).to_string();
        let n = paid.len();
        parts.push(format!(
            "Paid {} to {n} payee{}",
            human(&total),
            if n == 1 { "" } else { "s" }
        ));
    }
    if waiting > 0 {
        parts.push(format!(
            "{waiting} bill{} need{} you",
            if waiting == 1 { "" } else { "s" },
            if waiting == 1 { "s" } else { "" }
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// "0.350000 USDC" -> "0.35 USDC", the page's rule.
pub fn human(m: &str) -> String {
    let v = m.trim_end_matches(" USDC");
    let Some((w, f)) = v.split_once('.') else {
        return m.to_string();
    };
    let f = f.trim_end_matches('0');
    format!("{w}.{f:0<2} USDC")
}

/// Push `text` to every browser of `policy`, unless it is the same line as
/// last time. Best effort: a push service that says the subscription is gone
/// (404, 410) loses it; anything else is printed and kept.
pub async fn tell(app_dir: &Path, policy: &str, text: &str) {
    let mut v = load(app_dir, policy);
    let subs: Vec<Value> = v["subs"].as_array().cloned().unwrap_or_default();
    if subs.is_empty() || v["last"] == text {
        return;
    }
    let Ok(vapid) = vapid_key() else {
        println!("push {policy}: no VAPID key");
        return;
    };
    let payload = json!({ "title": "Tameion", "body": text, "policy": policy }).to_string();
    let mut kept = Vec::new();
    for s in subs {
        match send(&s, payload.as_bytes(), &vapid).await {
            Ok(404) | Ok(410) => println!("push {policy}: a browser unsubscribed; dropped"),
            Ok(code) => {
                println!("push {policy} -> {code}");
                kept.push(s);
            }
            Err(e) => {
                println!("push {policy} failed: {e}");
                kept.push(s);
            }
        }
    }
    v["subs"] = json!(kept);
    v["last"] = json!(text);
    let _ = store(app_dir, policy, &v);
}

/// One encrypted message to one browser; the push service's status code.
pub async fn send(sub: &Value, payload: &[u8], vapid: &SecretKey) -> Result<u16, String> {
    let endpoint = sub["endpoint"].as_str().ok_or("no endpoint")?;
    let ua = B64
        .decode(sub["p256dh"].as_str().unwrap_or(""))
        .map_err(|_| "p256dh")?;
    let auth = B64
        .decode(sub["auth"].as_str().unwrap_or(""))
        .map_err(|_| "auth")?;
    let eph = loop {
        if let Ok(k) = SecretKey::from_slice(&random::<32>()?) {
            break k;
        }
    };
    let body = encrypt(&ua, &auth, &eph, &random::<16>()?, payload)?;
    let (url, client) = crate::x402::public_client(
        endpoint,
        std::time::Duration::from_secs(10),
        "tameion-push/1",
    )
    .await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let resp = client
        .post(url)
        .header("TTL", "86400")
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("Authorization", vapid_header(vapid, endpoint, now)?)
        .body(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.status().as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Vec<u8> {
        B64.decode(s.split_whitespace().collect::<String>())
            .unwrap()
    }

    /// RFC 8291 §5 and Appendix A, byte for byte: the same inputs give the
    /// same message, and the user agent's private key decrypts it.
    #[test]
    fn rfc8291_example_byte_for_byte() {
        let as_secret =
            SecretKey::from_slice(&b("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        assert_eq!(
            uncompressed(&as_secret.public_key()),
            b(
                "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIg Dll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
            )
        );
        let ua_public = b(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV- JvLexhqUzORcx aOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        );
        let salt: [u8; 16] = b("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let out = encrypt(
            &ua_public,
            &b("BTBZMqHH6r4Tts7J_aSIgg"),
            &as_secret,
            &salt,
            b"When I grow up, I want to be a watermelon",
        )
        .unwrap();
        let expected = b(
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml
             mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT
             pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
        );
        assert_eq!(out, expected);
        // 86 header + 41 plaintext + 1 delimiter + 16 tag. (The RFC's sample
        // request says "Content-Length: 145"; its own body decodes to 144.)
        assert_eq!(out.len(), 144);
        // And the receiver opens it with its own key, as a browser would.
        let ua_secret =
            SecretKey::from_slice(&b("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94")).unwrap();
        let as_public = PublicKey::from_sec1_bytes(&out[21..86]).unwrap();
        let (key, nonce) = content_key(
            &ecdh(&ua_secret, &as_public),
            &b("BTBZMqHH6r4Tts7J_aSIgg"),
            &ua_public,
            &out[21..86],
            &out[..16],
        )
        .unwrap();
        let mut record = out[86..].to_vec();
        let plain = key
            .open_in_place(nonce, ring::aead::Aad::empty(), &mut record)
            .unwrap();
        assert_eq!(plain, b"When I grow up, I want to be a watermelon\x02");
    }

    /// Live, by hand (needs a real browser's subscription and the network):
    /// `TAMEION_PUSH_FILE=<tenants/<policy>.push.json> cargo test -p operator
    /// --lib push::tests::live -- --ignored --nocapture` sends one message to
    /// the first browser there; 201 is the push service accepting it.
    #[tokio::test]
    #[ignore]
    async fn live_push_reaches_the_push_service() {
        let f = std::env::var("TAMEION_PUSH_FILE").expect("TAMEION_PUSH_FILE");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        let msg =
            json!({ "title": "Tameion", "body": "Paid 0.35 USDC to 2 payees · 1 bill needs you" });
        let code = send(
            &v["subs"][0],
            msg.to_string().as_bytes(),
            &vapid_key().unwrap(),
        )
        .await
        .unwrap();
        println!("push service answered {code}");
        assert_eq!(code, 201);
    }

    #[test]
    fn vapid_header_verifies_with_its_own_key() {
        use p256::ecdsa::signature::Verifier;
        let k = SecretKey::from_slice(&[7u8; 32]).unwrap();
        let h = vapid_header(&k, "https://fcm.googleapis.com/fcm/send/abc", 1_000).unwrap();
        let t = h
            .strip_prefix("vapid t=")
            .unwrap()
            .split(", k=")
            .next()
            .unwrap();
        let (signing, sig) = t.rsplit_once('.').unwrap();
        let claims: Value =
            serde_json::from_slice(&B64.decode(signing.split('.').nth(1).unwrap()).unwrap())
                .unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["exp"], 1_000 + 12 * 3600);
        let sig = p256::ecdsa::Signature::from_slice(&B64.decode(sig).unwrap()).unwrap();
        p256::ecdsa::VerifyingKey::from(k.public_key())
            .verify(signing.as_bytes(), &sig)
            .unwrap();
    }

    #[test]
    fn a_pass_reads_as_one_line_and_quiet_passes_say_nothing() {
        let r = json!({ "settled": [{ "amount": "0.200000 USDC" }, { "amount": "0.150000 USDC" }], "pendingSignature": [{}] });
        assert_eq!(
            line("done", Some(&r)).unwrap(),
            "Paid 0.35 USDC to 2 payees · 1 bill needs you"
        );
        assert_eq!(
            line(
                "done",
                Some(&json!({ "settled": [], "pendingSignature": [] }))
            ),
            None
        );
        assert!(line("failed", None).unwrap().contains("nothing was paid"));
    }
}
