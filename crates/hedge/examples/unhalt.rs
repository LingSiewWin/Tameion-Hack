//! Clear the hedge HALT with a signature and a reason (docs/46, "HALT with an
//! owner"). The engine refuses to open positions while `runtime/hedge/HALT`
//! exists; removing it by hand left no trace of who did it or why. This
//! appends a signed line to `HALT.history` first, then removes the file.
//!
//!   HALT_REASON="venue showed the fill late; re-read matched" \
//!   cargo run --locked -q -p hedge --example unhalt
//!
//! Environment:
//!   HALT_REASON                 required
//!   HEDGE_HALT_DIR              default runtime/hedge
//!   TAMEION_OPERATOR_KEY_FILE   the key that signs the clearing (default
//!                               ~/.tameion/treasury.key: on Arc the treasury is
//!                               also the operator). Never printed.
//!
//! Prints the history afterwards with each line's signature check.

use hedge::halt;
use hedge::hl::parse_signer;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir =
        PathBuf::from(std::env::var("HEDGE_HALT_DIR").unwrap_or_else(|_| "runtime/hedge".into()));
    let reason = std::env::var("HALT_REASON")
        .map_err(|_| "HALT_REASON must be set: say why the halt is cleared")?;
    if reason.trim().is_empty() {
        return Err("HALT_REASON is empty".into());
    }
    let key_file = std::env::var("TAMEION_OPERATOR_KEY_FILE").unwrap_or_else(|_| {
        format!(
            "{}/.tameion/treasury.key",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let key_hex =
        std::fs::read_to_string(&key_file).map_err(|e| format!("cannot read {key_file}: {e}"))?;
    let signer = parse_signer(&key_hex)?;
    drop(key_hex);

    println!("# unhalt {}", dir.display());
    if !halt::is_halted(&dir) {
        println!("no HALT present: nothing to clear");
    } else {
        let c = halt::clear(&dir, &signer, &reason)?;
        println!(
            "cleared {} halt reason(s) at {}  by {:#x}  keccak {:#x}",
            c.reasons_cleared, c.at, c.by, c.digest
        );
    }
    println!("## HALT.history");
    for h in halt::history(&dir)? {
        println!(
            "{}  {:#x}  {}  keccak {:#x}  signature {}",
            h.at,
            h.by,
            h.reason,
            h.digest,
            if h.signature_valid {
                "VALID"
            } else {
                "INVALID"
            }
        );
    }
    Ok(())
}
