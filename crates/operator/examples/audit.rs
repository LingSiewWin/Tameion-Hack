//! Recompute the anchored root from the events on disk and compare it with
//! the chain. No key, no database: the events file, one `rootOf` read, and
//! the anchors index (`docs/46` #1).
//!
//!   cargo run --locked -q -p operator --example audit
//!
//! Environment:
//!   AUDIT_EVENTS   path of `<root>.events.jsonl` (default: the file named by
//!                  the last line of `<TAMEION_LOG_DIR>/anchors.jsonl`)
//!   TAMEION_LOG_DIR  default `runtime/log`
//!   RPC_URL        default Arc testnet
//!   ANCHOR         MemoryAnchor address (default `runtime/arc_anchor.addr`)
//!   AGENT          address the root is keyed under (default: the anchors
//!                  line's `agent`, else `~/.tameion/agent.address`)
//!   AUDIT_SEQ      print and verify the inclusion proof for one event
//!
//! Exit 0: file name, recomputed root and chain agree (or the chain has moved
//! on and this root's anchoring transaction is on record). Exit 2 otherwise.

use alloy::primitives::{Address, B256};
use arc::ArcClient;
use ledger::log::{Hash32, path_from_leaves, verify_inclusion};
use operator::audit;
use std::path::PathBuf;
use std::str::FromStr;

fn read_trim(p: &str) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = audit::dir();
    let last = audit::last_anchor(&dir)?;
    let file = match std::env::var("AUDIT_EVENTS") {
        Ok(p) => PathBuf::from(p),
        Err(_) => {
            let l = last.as_ref().ok_or(
                "no AUDIT_EVENTS and no anchors.jsonl yet: run a pass first (scripts/demo.sh or prove_arc.sh)",
            )?;
            dir.join(
                l["file"]
                    .as_str()
                    .ok_or("anchors.jsonl line lacks `file`")?,
            )
        }
    };
    let rpc = std::env::var("RPC_URL").unwrap_or_else(|_| arc::ARC_TESTNET_RPC.to_string());
    let anchor = std::env::var("ANCHOR")
        .ok()
        .or_else(|| read_trim("runtime/arc_anchor.addr"))
        .ok_or("ANCHOR not set and runtime/arc_anchor.addr missing")?;
    let agent = std::env::var("AGENT")
        .ok()
        .or_else(|| {
            last.as_ref()
                .and_then(|l| l["agent"].as_str().map(String::from))
        })
        .or_else(|| {
            let home = std::env::var("HOME").ok()?;
            read_trim(&format!("{home}/.tameion/agent.address"))
        })
        .ok_or("AGENT not set and no agent recorded")?;

    println!("# audit {}", file.display());
    let lines = audit::load(&file)?;
    for l in &lines {
        println!("[{:04}] {:<11} {}", l.seq, l.kind, l.note);
    }
    let (leaves, root) = audit::recompute(&lines);
    let named = audit::root_from_filename(&file);
    println!("events          : {}", lines.len());
    println!("recomputed root : {}", root.to_hex());
    let name_ok = named == Some(root);
    println!(
        "file name       : {}   {}",
        named
            .map(|h| h.to_hex())
            .unwrap_or_else(|| "(not <root>.events.jsonl)".into()),
        if name_ok {
            "MATCH"
        } else {
            "MISMATCH — edited or renamed"
        }
    );

    let client = ArcClient::read_only(&rpc)?;
    let chain = client
        .anchored_root(Address::from_str(&anchor)?, Address::from_str(&agent)?)
        .await?;
    let chain = Hash32::from_bytes(chain.0);
    let chain_ok = chain == root;
    // An earlier record: `rootOf` holds only the latest root, so the record
    // is checked by the transaction that anchored it -- read from the chain
    // (sender, target, status, the root in the calldata), whether the hash
    // came from `ANCHOR_TX` or from a local anchors.jsonl.
    let mut anchored_elsewhere = None;
    if !chain_ok {
        let mut candidates: Vec<String> = std::env::var("ANCHOR_TX").ok().into_iter().collect();
        for a in audit::anchors_for(&dir, root)? {
            if let Some(tx) = a["anchorTx"].as_str() {
                candidates.push(tx.to_string());
            }
        }
        for tx in candidates {
            let Ok(h) = B256::from_str(tx.trim()) else {
                continue;
            };
            if client
                .anchored_in(
                    h,
                    Address::from_str(&anchor)?,
                    Address::from_str(&agent)?,
                    B256::from(*root.as_bytes()),
                )
                .await
                .unwrap_or(false)
            {
                anchored_elsewhere = Some(tx);
                break;
            }
        }
    }
    println!(
        "chain rootOf    : {}   (anchor {anchor}, agent {agent})   {}",
        chain.to_hex(),
        if chain_ok {
            "MATCH".to_string()
        } else if let Some(tx) = &anchored_elsewhere {
            format!(
                "LATER ROOT ON CHAIN — this root was anchored in {tx}: checked on chain (sent by the agent to the anchor, succeeded, the root in its calldata)"
            )
        } else {
            "DIFFERS — no anchoring transaction on record for this root".to_string()
        }
    );

    if let Ok(seq) = std::env::var("AUDIT_SEQ") {
        let seq: usize = seq.parse()?;
        let path = path_from_leaves(&leaves, seq).ok_or("AUDIT_SEQ out of range")?;
        let ok = verify_inclusion(leaves[seq], &path, root);
        println!(
            "inclusion [{seq:04}]: leaf {} path {} steps   {}",
            leaves[seq].to_hex(),
            path.len(),
            if ok { "VERIFIED" } else { "FAILED" }
        );
        for (i, step) in path.iter().enumerate() {
            println!(
                "  {i}: {} {}",
                if step.sibling_is_left {
                    "left "
                } else {
                    "right"
                },
                step.sibling.to_hex()
            );
        }
        if !ok {
            std::process::exit(2);
        }
    }

    if name_ok && (chain_ok || anchored_elsewhere.is_some()) {
        println!("RESULT          : PASS");
        Ok(())
    } else {
        println!("RESULT          : FAIL");
        std::process::exit(2);
    }
}
