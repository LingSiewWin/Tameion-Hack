//! One-time setup: create a trade-only API wallet for the treasury on
//! Hyperliquid **testnet** and approve it with the master key.
//!
//! Reads the master key once, generates a fresh agent key, saves the agent
//! key (mode 600) before approving so a successful approval is never lost,
//! approves it on the venue, then reads the approved agents back from the
//! venue as confirmation. Prints addresses only. Never prints a key.
//!
//! Env:
//! - `TAMEION_TREASURY_KEY_FILE` master key (default `~/.tameion/treasury.key`)
//! - `HEDGE_AGENT_KEY_FILE`      where to save the agent key
//!   (default `~/.tameion/hedge-agent.key`)
//! - `HEDGE_AGENT_NAME`          venue-side name (default `tameion-hedge`)
//!
//! Refuses to overwrite an existing agent key file: approving a second agent
//! under the same name deregisters the first at the venue.

use hedge::hl::{Venue, parse_signer};
use hypersdk::hypercore::PrivateKeySigner;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let master_file = std::env::var("TAMEION_TREASURY_KEY_FILE")
        .unwrap_or_else(|_| format!("{}/.tameion/treasury.key", home()));
    let agent_file = PathBuf::from(
        std::env::var("HEDGE_AGENT_KEY_FILE")
            .unwrap_or_else(|_| format!("{}/.tameion/hedge-agent.key", home())),
    );
    let name = std::env::var("HEDGE_AGENT_NAME").unwrap_or_else(|_| "tameion-hedge".into());

    if agent_file.exists() {
        eprintln!(
            "refusing: {} already exists. Remove it on purpose if you mean to rotate the agent.",
            agent_file.display()
        );
        std::process::exit(2);
    }
    let master_hex = match fs::read_to_string(&master_file) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("refusing: cannot read master key {master_file}: {e}");
            std::process::exit(2);
        }
    };
    let master = parse_signer(&master_hex)?;
    drop(master_hex);

    let venue = Venue::testnet();
    let agent = PrivateKeySigner::random();

    // Save first, approve second. A saved-but-unapproved key is harmless; an
    // approved-but-unsaved key is an agent nobody can use or revoke cleanly.
    if let Some(dir) = agent_file.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&agent_file, format!("{}\n", agent.to_bytes()))?;
    fs::set_permissions(&agent_file, fs::Permissions::from_mode(0o600))?;

    println!("# approve API wallet — Hyperliquid {:?}", venue.chain());
    println!("master (treasury): {}", master.address());
    println!("agent:             {}  name={name:?}", agent.address());
    println!("agent key saved:   {} (mode 600)", agent_file.display());

    if let Err(e) = venue.approve_agent(&master, agent.address(), &name).await {
        let _ = fs::remove_file(&agent_file);
        eprintln!("approval failed, agent key file removed: {e}");
        eprintln!(
            "most common cause: the master account has never deposited on testnet (\"Must deposit before performing actions\"). Fund it at https://app.hyperliquid-testnet.xyz/faucet first."
        );
        std::process::exit(3);
    }
    println!("approved at venue: ok");

    let agents = venue.api_agents(master.address()).await?;
    println!("\n## agents the venue now reports for {}", master.address());
    for a in &agents {
        println!(
            "  {} name={:?} valid_until={:?}{}",
            a.address,
            a.name,
            a.valid_until,
            if a.address == agent.address() {
                "   <- this one"
            } else {
                ""
            }
        );
    }
    if !agents.iter().any(|a| a.address == agent.address()) {
        eprintln!(
            "WARNING: venue did not list the new agent yet; re-run `api_agents` before trading."
        );
        std::process::exit(4);
    }
    println!("\nnext: scripts/prove_hedge.sh live");
    Ok(())
}
