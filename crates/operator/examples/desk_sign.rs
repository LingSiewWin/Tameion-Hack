//! A counterparty signs its milestone for a desk (`docs/83` W5): the supplier
//! its priced purchase, the vault its receipt, the buyer its sale.
//!
//!     cargo run -q -p operator --example desk_sign -- <key-file> <chain-id> <desk-account> '<milestone json>'
//!
//! Prints `{"milestone": …, "signature": "0x…"}`, ready for
//! `POST /api/desk/milestone` with the desk's `policy` added. The key is read
//! from a file (mode 600) and never printed.

use alloy::signers::local::PrivateKeySigner;
use std::str::FromStr;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [key_file, chain, desk, milestone] = a.as_slice() else {
        return Err(
            "usage: desk_sign <key-file> <chain-id> <desk-account> '<milestone json>'".into(),
        );
    };
    // The step is bound to this desk (EIP-712 verifyingContract, docs/89 H3).
    let desk = alloy::primitives::Address::from_str(desk.trim())?;
    let key = PrivateKeySigner::from_str(std::fs::read_to_string(key_file)?.trim())?;
    let mut m: desk::Milestone = serde_json::from_str(milestone)?;
    m.signer = format!("{:#x}", key.address());
    let sig = operator::desk::sign_event(chain.parse()?, desk, &m, &key)?;
    println!(
        "{}",
        serde_json::json!({ "milestone": m, "signature": sig })
    );
    Ok(())
}
