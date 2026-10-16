//! The 32 bytes the owner's passkey signs for a desk's trust decision
//! (docs/89 M5), for scripts and other tools: the same functions the service
//! verifies with, so nothing is computed twice in two languages by hand.
//!
//!     desk_challenge party   <chain-id> <desk> <name> <address> [kinds,comma,separated] <issued>
//!     desk_challenge base    <chain-id> <desk> <goods> <grams> <issued>
//!     desk_challenge control <chain-id> <desk> '<text>' <issued>
//!     desk_challenge step    <chain-id> <desk> '<milestone json>' <issued>
//!     desk_challenge offset  <chain-id> <desk> <target short, as GET /api/desk shows it> <issued>
//!     desk_challenge venue   <chain-id> <desk> <exchange account> <issued>
//!
//! Prints `0x…`. Sign it with the owner's key (`target/desk-se sign <it>`).

use alloy::primitives::Address;
use std::str::FromStr;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let usage =
        "usage: desk_challenge party|base|control|step|offset|venue <chain-id> <desk> … <issued>";
    let (kind, rest) = a.split_first().ok_or(usage)?;
    let chain: u64 = rest.first().ok_or(usage)?.parse()?;
    let desk = Address::from_str(rest.get(1).ok_or(usage)?)?;
    let issued: u64 = rest.last().ok_or(usage)?.parse()?;
    let ch = match (kind.as_str(), &rest[2..rest.len() - 1]) {
        ("party", [name, addr]) => operator::desk::party_challenge(
            chain,
            desk,
            name.trim(),
            Address::from_str(addr)?,
            issued,
        ),
        ("party", [name, addr, kinds]) => {
            let kinds: Vec<desk::MilestoneKind> = kinds
                .split(',')
                .map(|k| serde_json::from_value(serde_json::Value::String(k.trim().into())))
                .collect::<Result<_, _>>()?;
            operator::desk::party_challenge_v2(
                chain,
                desk,
                name.trim(),
                Address::from_str(addr)?,
                &kinds,
                issued,
            )
        }
        ("base", [goods, grams]) => {
            operator::desk::base_challenge(chain, desk, goods, grams.trim(), issued)
        }
        ("control", [text]) => operator::desk::control_challenge(chain, desk, text.trim(), issued),
        ("offset", [target]) => {
            operator::desk::offset_challenge(chain, desk, target.trim(), issued)
        }
        ("venue", [account]) => {
            operator::desk::venue_challenge(chain, desk, Address::from_str(account)?, issued)
        }
        ("step", [json]) => {
            let mut m: desk::Milestone = serde_json::from_str(json)?;
            m.signer = String::new();
            operator::desk::step_challenge(
                chain,
                desk,
                operator::desk::event_digest(chain, desk, &m),
                issued,
            )
        }
        _ => return Err(usage.into()),
    };
    println!("{ch:#x}");
    Ok(())
}
