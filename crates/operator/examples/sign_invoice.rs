//! What a vendor (or the vendor's agent) posts to an account: one invoice,
//! signed with the key of the address the money goes to (docs/65 §5.4).
//!
//! ```text
//! cargo run -p operator --example sign_invoice -- <vendor-key-file> <chain-id> <policy> \
//!     <amount-minor> <due-in-days> <every-days> [discount-bps early-days] [reference]
//! ```
//!
//! Prints the JSON body for `POST /api/level1/invoice`. The key is read from
//! the file and never printed; `every-days` 0 is a one-off invoice. With
//! `INVOICE_FROM` and/or `INVOICE_DOMAIN` set it signs version 2: the vendor's
//! name and domain, which the owner reads instead of the address (docs/81).

use alloy::primitives::{Address, keccak256};
use alloy::signers::SignerSync;
use alloy::signers::local::PrivateKeySigner;
use operator::invoice::{Invoice, Issuer, digest, to_json};
use std::str::FromStr;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 6 {
        eprintln!(
            "usage: sign_invoice <vendor-key-file> <chain-id> <policy> <amount-minor> <due-in-days> <every-days> [discount-bps early-days] [reference]"
        );
        std::process::exit(2);
    }
    let key = std::fs::read_to_string(&a[0]).expect("vendor key file");
    let vendor = PrivateKeySigner::from_str(key.trim()).expect("vendor key");
    drop(key);
    let num = |i: usize| -> u64 { a[i].parse().expect("a whole number") };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let reference = a.get(8).cloned().unwrap_or_else(|| format!("INV-{now}"));
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
    };
    let sig = vendor.sign_hash_sync(&digest(num(1), &inv)).expect("sign");
    println!("{}", to_json(&inv, &sig.as_bytes()));
}
