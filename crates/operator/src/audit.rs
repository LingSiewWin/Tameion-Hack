//! The record an auditor re-hashes (`docs/46` #1).
//!
//! The decision log is anchored on chain as one 32-byte root, but until this
//! module existed nothing durable held the events behind that root: a month
//! later there was a root on Arc and nothing to hash. Now every pass that
//! anchors writes the whole log to `<dir>/<root>.events.jsonl` — one line per
//! event with its canonical bytes, the bytes the leaf is hashed from — and
//! appends one line to `<dir>/anchors.jsonl` naming the root, the anchoring
//! transaction and the agent address the contract keys it under.
//!
//! A reader needs no `Event` decoder: leaves come from the canonical bytes
//! (`ledger::log::leaf_from_canonical`), the root from the leaves, and the
//! file name must equal the recomputed root. `kind` and `note` are carried for
//! humans and are not hashed; a changed note shows up as a root mismatch only
//! if the canonical bytes were changed too, which is the point.
//!
//! `TAMEION_LOG_DIR` overrides the directory (default `runtime/log`).

use ledger::log::{DecisionLog, Hash32, leaf_from_canonical, root_from_leaves};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub const ANCHORS_FILE: &str = "anchors.jsonl";

/// Where the audit record lives.
pub fn dir() -> PathBuf {
    std::env::var("TAMEION_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("runtime/log"))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|c| {
            let hi = (c[0] as char).to_digit(16)?;
            let lo = (c[1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

/// Write the whole log as `<dir>/<root>.events.jsonl`. Idempotent: the same
/// log produces the same file, written to a temp name and renamed so a
/// reader never sees a half-written record. Returns the path.
pub fn persist(dir: &Path, log: &DecisionLog) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let root = log.root().to_hex();
    let path = dir.join(format!("{root}.events.jsonl"));
    let tmp = dir.join(format!(".{root}.events.jsonl.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        for e in log.events() {
            let line = json!({
                "seq": e.seq,
                "at": e.at.as_unix(),
                "kind": e.body.kind(),
                "note": e.body.note(),
                "canonical": hex(&e.canonical()),
            });
            writeln!(f, "{line}")?;
        }
        f.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Append one line to `<dir>/anchors.jsonl`: which root was anchored, by which
/// transaction, under which agent address, at which chain contract and when.
/// `tx` is `None` when the pass computed a root but did not anchor it.
pub fn append_anchor(
    dir: &Path,
    root: Hash32,
    tx: Option<String>,
    agent: Option<String>,
    anchor_contract: String,
    at_unix: i64,
    events: usize,
) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(ANCHORS_FILE))?;
    let line = json!({
        "root": root.to_hex(),
        "anchorTx": tx,
        "agent": agent,
        "anchor": anchor_contract,
        "at": at_unix,
        "events": events,
        "file": format!("{}.events.jsonl", root.to_hex()),
    });
    writeln!(f, "{line}")
}

/// One line of an events file, as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub seq: u64,
    pub at: i64,
    pub kind: String,
    pub note: String,
    pub canonical: Vec<u8>,
    /// The event's leaf hash, when only that was served: a locked account's
    /// record as an outsider gets it (docs/60 P-212). `None` when the canonical
    /// bytes are here; the leaf is then computed from them, never taken.
    pub leaf: Option<Hash32>,
}

/// Read an events file. Refuses a line that is not JSON, lacks a field, or
/// whose `seq` is not its position: a file with a gap or a duplicate cannot
/// pass as the record of a log that never issued that sequence.
pub fn load(path: &Path) -> io::Result<Vec<Line>> {
    let f = fs::File::open(path)?;
    let bad = |n: usize, what: &str| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: line {n}: {what}", path.display()),
        )
    };
    let mut out = Vec::new();
    for (i, line) in BufReader::new(f).lines().enumerate() {
        let n = i + 1;
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).map_err(|e| bad(n, &e.to_string()))?;
        let seq = v["seq"].as_u64().ok_or_else(|| bad(n, "seq missing"))?;
        if seq as usize != out.len() {
            return Err(bad(n, &format!("seq {seq} out of order")));
        }
        // A line with the canonical bytes is checked from them; a line with
        // only a leaf hash (docs/60 P-220) is checked from the leaf.
        let (canonical, leaf) = match v["canonical"].as_str() {
            Some(c) => (unhex(c).ok_or_else(|| bad(n, "canonical not hex"))?, None),
            None => (
                Vec::new(),
                Some(
                    v["leaf"]
                        .as_str()
                        .and_then(Hash32::from_hex)
                        .ok_or_else(|| bad(n, "neither canonical nor leaf"))?,
                ),
            ),
        };
        out.push(Line {
            seq,
            at: match (v["at"].as_i64(), leaf) {
                (Some(at), _) => at,
                (None, Some(_)) => 0,
                (None, None) => return Err(bad(n, "at missing")),
            },
            kind: v["kind"].as_str().unwrap_or("").to_string(),
            note: v["note"].as_str().unwrap_or("").to_string(),
            canonical,
            leaf,
        });
    }
    Ok(out)
}

/// Leaves and root recomputed from canonical bytes alone, or from the leaf
/// hashes a locked account's public record serves instead (docs/60 P-220).
pub fn recompute(lines: &[Line]) -> (Vec<Hash32>, Hash32) {
    let leaves: Vec<Hash32> = lines
        .iter()
        .map(|l| l.leaf.unwrap_or_else(|| leaf_from_canonical(&l.canonical)))
        .collect();
    let root = root_from_leaves(&leaves);
    (leaves, root)
}

/// The root an events file claims, from its name: `<root>.events.jsonl`.
pub fn root_from_filename(path: &Path) -> Option<Hash32> {
    let name = path.file_name()?.to_str()?;
    Hash32::from_hex(name.strip_suffix(".events.jsonl")?)
}

/// Every anchor line, oldest first; empty when nothing was anchored yet.
pub fn anchors(dir: &Path) -> io::Result<Vec<Value>> {
    let path = dir.join(ANCHORS_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for line in BufReader::new(fs::File::open(path)?).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            out.push(serde_json::from_str(&line)?);
        }
    }
    Ok(out)
}

/// The most recent anchor line, if any.
pub fn last_anchor(dir: &Path) -> io::Result<Option<Value>> {
    Ok(anchors(dir)?.pop())
}

/// Records persisted but never anchored (a pass that paid and then could not
/// anchor: gas, RPC), oldest first, with their event counts. The next pass
/// anchors them before its own root, so no record stays unanchored.
pub fn unanchored(dir: &Path) -> io::Result<Vec<(Hash32, usize)>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut anchored = std::collections::BTreeSet::new();
    let path = dir.join(ANCHORS_FILE);
    if path.exists() {
        for line in BufReader::new(fs::File::open(path)?).lines() {
            let line = line?;
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && !v["anchorTx"].is_null()
                && let Some(r) = v["root"].as_str()
            {
                anchored.insert(r.trim_start_matches("0x").to_ascii_lowercase());
            }
        }
    }
    let mut out = Vec::new();
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        let Some(root) = root_from_filename(&p) else {
            continue;
        };
        if anchored.contains(root.to_hex().trim_start_matches("0x")) {
            continue;
        }
        let modified = fs::metadata(&p)?.modified()?;
        let events = fs::read_to_string(&p)?
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();
        out.push((modified, root, events));
    }
    out.sort_by_key(|(m, _, _)| *m);
    Ok(out.into_iter().map(|(_, r, n)| (r, n)).collect())
}

/// Every anchor line whose `root` is `root`.
pub fn anchors_for(dir: &Path, root: Hash32) -> io::Result<Vec<Value>> {
    let want = root.to_hex();
    Ok(anchors(dir)?
        .into_iter()
        .filter(|v| v["root"].as_str() == Some(want.as_str()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ledger::log::{EventBody, path_from_leaves, verify_inclusion};
    use ledger::money::{Currency, Money};
    use ledger::time::Timestamp;

    fn sample_log() -> DecisionLog {
        let mut log = DecisionLog::new();
        for i in 0..3u64 {
            log.append(
                Timestamp::from_unix(1_700_000_000 + i as i64),
                EventBody::Observation {
                    balance: Money::parse("1.000000", Currency::Usdc).unwrap(),
                    transfers_seen: i as usize,
                    obligations_found: 0,
                    total_due: Money::parse("0", Currency::Usdc).unwrap(),
                    note: format!("obs {i}"),
                },
            );
        }
        log
    }

    /// docs/60 P-220: the public record of a locked account (each event's `seq`
    /// and leaf hash) recomputes to the same root as the events; a line with
    /// neither bytes nor a leaf is refused, and bytes win over a stated leaf.
    #[test]
    fn the_leaf_hashes_alone_recompute_the_same_root() {
        let dir = std::env::temp_dir().join(format!("tameion-audit-leaves-{}", std::process::id()));
        let log = sample_log();
        let path = persist(&dir, &log).unwrap();
        let full = load(&path).unwrap();
        let public: String = full
            .iter()
            .map(|l| {
                format!(
                    "{{\"seq\":{},\"leaf\":\"{}\"}}\n",
                    l.seq,
                    leaf_from_canonical(&l.canonical).to_hex()
                )
            })
            .collect();
        let leaves_file = dir.join("public.events.jsonl");
        fs::write(&leaves_file, &public).unwrap();
        let lines = load(&leaves_file).unwrap();
        assert!(
            lines
                .iter()
                .all(|l| l.canonical.is_empty() && l.leaf.is_some())
        );
        assert_eq!(recompute(&lines).1, log.root());
        fs::write(&leaves_file, "{\"seq\":0}\n").unwrap();
        assert!(load(&leaves_file).is_err(), "neither bytes nor a leaf");
        let lying = format!(
            "{{\"seq\":0,\"at\":1,\"canonical\":\"0x00\",\"leaf\":\"{}\"}}\n",
            log.root().to_hex()
        );
        fs::write(&leaves_file, lying).unwrap();
        assert_eq!(
            recompute(&load(&leaves_file).unwrap()).0,
            vec![leaf_from_canonical(&[0])]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_persisted_log_recomputes_to_the_same_root_and_file_name() {
        let dir = std::env::temp_dir().join(format!("tameion-audit-{}", std::process::id()));
        let log = sample_log();
        let path = persist(&dir, &log).unwrap();
        assert_eq!(root_from_filename(&path), Some(log.root()));
        let lines = load(&path).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].note, "obs 1");
        let (leaves, root) = recompute(&lines);
        assert_eq!(root, log.root());
        let p = path_from_leaves(&leaves, 2).unwrap();
        assert!(verify_inclusion(leaves[2], &p, root));
        // idempotent
        let again = persist(&dir, &log).unwrap();
        assert_eq!(again, path);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_edited_canonical_byte_changes_the_root_and_a_gap_is_refused() {
        let dir = std::env::temp_dir().join(format!("tameion-audit-edit-{}", std::process::id()));
        let log = sample_log();
        let path = persist(&dir, &log).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        // flip the last hex digit of line 1's canonical bytes
        let l = &lines[1];
        let i = l.rfind("\"canonical\":\"0x").unwrap() + 15;
        let end = l[i..].find('"').unwrap() + i;
        let mut hexs: Vec<char> = l[i..end].chars().collect();
        let last = hexs.len() - 1;
        hexs[last] = if hexs[last] == '0' { '1' } else { '0' };
        let edited: String = hexs.into_iter().collect();
        lines[1] = format!("{}{}{}", &l[..i], edited, &l[end..]);
        fs::write(&path, lines.join("\n")).unwrap();
        let (_, root) = recompute(&load(&path).unwrap());
        assert_ne!(root, log.root(), "an edited byte must move the root");
        assert_ne!(Some(root), root_from_filename(&path));
        // drop a line: seq out of order is refused with its line number
        let mut gapped: Vec<String> = text.lines().map(String::from).collect();
        gapped.remove(1);
        fs::write(&path, gapped.join("\n")).unwrap();
        let err = load(&path).unwrap_err().to_string();
        assert!(
            err.contains("line 2") && err.contains("out of order"),
            "{err}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn anchors_are_appended_and_found_by_root() {
        let dir = std::env::temp_dir().join(format!("tameion-audit-anc-{}", std::process::id()));
        let log = sample_log();
        append_anchor(
            &dir,
            log.root(),
            Some("0xabc".into()),
            Some("0xagent".into()),
            "0xanchor".into(),
            1_700_000_000,
            3,
        )
        .unwrap();
        let found = anchors_for(&dir, log.root()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["anchorTx"], "0xabc");
        assert_eq!(
            last_anchor(&dir).unwrap().unwrap()["root"],
            log.root().to_hex()
        );
        assert!(anchors_for(&dir, Hash32::ZERO).unwrap().is_empty());
        let _ = fs::remove_dir_all(dir);
    }
}
