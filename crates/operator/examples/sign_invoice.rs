//! What a vendor (or the vendor's agent) posts to an account: one invoice,
//! signed with the key of the address the money goes to (docs/65 §5.4).
//!
//! ```text
//! cargo run -p operator --example sign_invoice -- <vendor-key-file> <chain-id> <policy> \
//!     <amount-minor> <due-in-days> <every-days> [discount-bps early-days] [reference]
//! cargo run -p operator --example sign_invoice -- bind <vendor-key-file> <chain-id> <policy> <ml-dsa-seed-file>
//! ```
//!
//! Prints the JSON body for `POST /api/level1/invoice`. The key is read from
//! the file and never printed; `every-days` 0 is a one-off invoice. With
//! `INVOICE_FROM` and/or `INVOICE_DOMAIN` set it signs version 2: the vendor's
//! name and domain, which the owner reads instead of the address (docs/81).
//! With `INVOICE_ALG` it signs version 3 (docs/117 §2): `secp256k1-eip712` with
//! the vendor key, or `ml-dsa-65` with the ML-DSA-65 key whose 32-byte seed
//! (hex) is in `MLDSA_SEED_FILE`; the body then carries that public key.
//! `bind` prints the body for `POST /api/level1/key-binding`: the vendor key
//! binds the ML-DSA-65 key's sha256 to its address on that account.

use alloy::primitives::{Address, keccak256};
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use ml_dsa::{KeyExport, Keypair, MlDsa65, SignatureEncoding, Signer, SigningKey};
use operator::invoice::{
    Invoice, Issuer, KeyBinding, binding_digest, binding_json, digest, to_json,
};
use operator::sig::{Alg, key_hash};
use std::str::FromStr;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn vendor(file: &str) -> PrivateKeySigner {
    let key = std::fs::read_to_string(file).expect("vendor key file");
    PrivateKeySigner::from_str(key.trim()).expect("vendor key")
}

/// The ML-DSA-65 key from its seed file (64 hex characters); never printed.
fn ml_dsa(file: &str) -> SigningKey<MlDsa65> {
    let hex = std::fs::read_to_string(file).expect("ML-DSA seed file");
    let seed: [u8; 32] = alloy::hex::decode(hex.trim())
        .expect("seed hex")
        .try_into()
        .expect("a 32-byte seed");
    SigningKey::<MlDsa65>::from_seed(&seed.into())
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.first().map(String::as_str) == Some("bind") && a.len() == 5 {
        let vendor = vendor(&a[1]);
        let pk = ml_dsa(&a[4]).verifying_key().to_bytes();
        let b = KeyBinding {
            payee: vendor.address(),
            policy: Address::from_str(&a[3]).expect("policy address"),
            alg: Alg::MlDsa65,
            pk_sha256: key_hash(&pk),
            bound_at: now(),
        };
        let chain: u64 = a[2].parse().expect("chain id");
        let sig = vendor
            .sign_hash_sync(&binding_digest(chain, &b))
            .expect("sign");
        println!("{}", binding_json(&b, &sig.as_bytes()));
        return;
    }
    if a.len() < 6 {
        eprintln!(
            "usage: sign_invoice <vendor-key-file> <chain-id> <policy> <amount-minor> <due-in-days> <every-days> [discount-bps early-days] [reference]\n       sign_invoice bind <vendor-key-file> <chain-id> <policy> <ml-dsa-seed-file>"
        );
        std::process::exit(2);
    }
    let vendor = vendor(&a[0]);
    let num = |i: usize| -> u64 { a[i].parse().expect("a whole number") };
    let now = now();
    let reference = a.get(8).cloned().unwrap_or_else(|| format!("INV-{now}"));
    let alg = std::env::var("INVOICE_ALG").ok().map(|s| {
        s.parse::<Alg>()
            .expect("INVOICE_ALG: secp256k1-eip712 or ml-dsa-65")
    });
    let inv = Invoice {
        payee: vendor.address(),
        policy: Address::from_str(&a[2]).expect("policy address"),
        amount: a[3].parse().expect("amount in minor units"),
        due_at: now + num(4) * 86_400,
        every_days: num(5) as u32,
        discount_bps: a.get(6).map_or(0, |s| s.parse().expect("discount bps")),
        early_days: a.get(7).map_or(0, |s| s.parse().expect("early days")),
        reference: keccak256(reference.as_bytes()),
        issued_at: now,
        issuer: match (
            std::env::var("INVOICE_FROM"),
            std::env::var("INVOICE_DOMAIN"),
        ) {
            (Err(_), Err(_)) => None,
            (f, d) => Some(Issuer {
                from: f.unwrap_or_default(),
                domain: d.unwrap_or_default(),
            }),
        },
        alg,
        key_hash: None,
    };
    let d = digest(num(1), &inv);
    let body = if alg == Some(Alg::MlDsa65) {
        let sk = ml_dsa(&std::env::var("MLDSA_SEED_FILE").expect("MLDSA_SEED_FILE"));
        let mut body = to_json(&inv, &sk.sign(d.as_slice()).to_bytes());
        body["publicKey"] = alloy::hex::encode_prefixed(sk.verifying_key().to_bytes()).into();
        body
    } else {
        to_json(&inv, &vendor.sign_hash_sync(&d).expect("sign").as_bytes())
    };
    println!("{body}");
}
