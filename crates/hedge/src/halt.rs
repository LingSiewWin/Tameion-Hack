//! The halt file. Present means: do not open or grow any position.
//!
//! Written by the engine when a position read back from the venue does not
//! match what it placed, or when a decision cannot be booked. Only a human
//! removes it -- and since 2026-10-13 (docs/46 "HALT with an owner") removing
//! it is a signed act: [`clear`] appends one line to `HALT.history` carrying
//! the clearer's address, their reason, the Keccak-256 of the HALT file they
//! cleared and an EIP-191 signature over that digest, then deletes `HALT`.
//! `is_halted` is still just "does the file exist": the engine's refusal needs
//! no key; only the clearing does.

use alloy::primitives::{Address, B256, keccak256};
use alloy::signers::{Signature, SignerSync, local::PrivateKeySigner};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_NAME: &str = "HALT";
pub const HISTORY_FILE: &str = "HALT.history";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What `clear` signs: the digest of the HALT file's bytes, so the signature
/// names exactly which halt (which reasons) was cleared.
pub fn digest_of(halt_contents: &[u8]) -> B256 {
    keccak256(halt_contents)
}

/// Clear the halt with a signature. Refuses when there is no HALT. Writes the
/// history line before deleting the file, so a crash in between leaves the
/// halt in place with its clearing on record, never the reverse.
pub fn clear(dir: &Path, signer: &PrivateKeySigner, reason: &str) -> io::Result<Cleared> {
    let p = path(dir);
    let contents = fs::read(&p)?;
    let digest = digest_of(&contents);
    let sig = signer
        .sign_message_sync(digest.as_slice())
        .map_err(|e| io::Error::other(e.to_string()))?;
    let at = now_secs();
    let reason = reason.replace(['\n', '\t'], " ");
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(HISTORY_FILE))?;
    writeln!(
        f,
        "{at}\t{:#x}\t{reason}\tkeccak={digest:#x}\tsig=0x{}",
        signer.address(),
        hex(&sig.as_bytes())
    )?;
    f.sync_all()?;
    fs::remove_file(&p)?;
    Ok(Cleared {
        at,
        by: signer.address(),
        digest,
        reasons_cleared: String::from_utf8_lossy(&contents).lines().count(),
    })
}

/// A record of one clearing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleared {
    pub at: u64,
    pub by: Address,
    pub digest: B256,
    pub reasons_cleared: usize,
}

/// One line of `HALT.history`, checked: the signature must recover to the
/// address the line names. A line that does not is reported, not skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryLine {
    pub at: u64,
    pub by: Address,
    pub reason: String,
    pub digest: B256,
    pub signature_valid: bool,
}

pub fn history(dir: &Path) -> io::Result<Vec<HistoryLine>> {
    let p = dir.join(HISTORY_FILE);
    if !p.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(p)?;
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let parts: Vec<&str> = line.split('\t').collect();
        let bad = |what: &str| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("HALT.history: {what}: {line}"),
            )
        };
        if parts.len() != 5 {
            return Err(bad("expected 5 tab-separated fields"));
        }
        let at: u64 = parts[0].parse().map_err(|_| bad("time"))?;
        let by: Address = parts[1].parse().map_err(|_| bad("address"))?;
        let digest: B256 = parts[3]
            .strip_prefix("keccak=")
            .ok_or_else(|| bad("digest"))?
            .parse()
            .map_err(|_| bad("digest hex"))?;
        let sig_hex = parts[4].strip_prefix("sig=").ok_or_else(|| bad("sig"))?;
        let sig: Signature = sig_hex.parse().map_err(|_| bad("sig hex"))?;
        let signature_valid = sig
            .recover_address_from_msg(digest.as_slice())
            .map(|a| a == by)
            .unwrap_or(false);
        out.push(HistoryLine {
            at,
            by,
            reason: parts[2].to_string(),
            digest,
            signature_valid,
        });
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

pub fn is_halted(dir: &Path) -> bool {
    path(dir).exists()
}

/// Write (or append to) the halt file with a timestamped reason.
pub fn halt(dir: &Path, reason: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path(dir))?;
    let now = now_secs();
    writeln!(f, "{now}\t{reason}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hedge-halt-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn clearing_is_signed_named_and_checkable() {
        let d = dir("clear");
        halt(&d, "read back 0, expected -0.5").unwrap();
        halt(&d, "second reason").unwrap();
        assert!(is_halted(&d));
        let who = PrivateKeySigner::random();
        let c = clear(&d, &who, "venue showed the fill late;\tre-read matched").unwrap();
        assert!(!is_halted(&d), "HALT removed");
        assert_eq!(c.by, who.address());
        assert_eq!(c.reasons_cleared, 2);
        let h = history(&d).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].by, who.address());
        assert!(h[0].signature_valid);
        assert_eq!(h[0].digest, c.digest);
        assert!(!h[0].reason.contains('\t'), "reason is one field");
        // Nothing to clear now.
        assert!(clear(&d, &who, "again").is_err());
    }

    #[test]
    fn a_history_line_with_a_swapped_address_fails_its_check() {
        let d = dir("tamper");
        halt(&d, "x").unwrap();
        let who = PrivateKeySigner::random();
        clear(&d, &who, "ok").unwrap();
        let p = d.join(HISTORY_FILE);
        let text = fs::read_to_string(&p).unwrap();
        let other = PrivateKeySigner::random().address();
        let swapped = text.replace(&format!("{:#x}", who.address()), &format!("{other:#x}"));
        fs::write(&p, swapped).unwrap();
        let h = history(&d).unwrap();
        assert_eq!(h[0].by, other);
        assert!(
            !h[0].signature_valid,
            "the signature does not recover to the claimed address"
        );
    }
}
