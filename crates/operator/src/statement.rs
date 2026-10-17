//! The signed weekly statement (`docs/76` §1d, §7 row 8): what the agent
//! paid, refused and handed to the owner in one ISO week, decoded from the
//! canonical bytes of the anchored record, hashed, and signed EIP-712 by a
//! key that does nothing else. An accountant or another agent checks it with
//! `cast` and two public nodes (`scripts/verify_statement.sh`) and never has
//! to take the service's word a second time.
//!
//! Lines come from the events behind every anchored root of the account
//! (`<tenants>/<policy>/log/<root>.events.jsonl`, written by [`crate::audit`]):
//! a `Settlement` is "paid", a `Decision` that blocked is "refused", an
//! `Escalation` is "needs-you". Observations, skips and the hedge events move
//! no money and are not lines. The fields are read from the canonical bytes
//! the leaf is hashed from, never from the `note` column, so a line cannot
//! say something the root does not vouch for.
//!
//! `linesHash` is Keccak-256 of the canonical JSON of `lines`: keys sorted,
//! no whitespace, strings escaped the way `serde_json` and Python's `json`
//! both do (`json.dumps(lines, sort_keys=True, separators=(",", ":"),
//! ensure_ascii=False)` reproduces it byte for byte).

use crate::audit;
use alloy::primitives::{Address, B256, keccak256};
use alloy::signers::{SignerSync, local::PrivateKeySigner};
use alloy::sol_types::{SolStruct, eip712_domain};
use ledger::money::{Currency, Money};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;

alloy::sol! {
    /// What the statement key signs. The week is `[from, to)` in unix
    /// seconds; `root` is the account's latest anchored record as of `to`.
    struct Statement {
        address policy;
        uint256 chainId;
        uint64 from;
        uint64 to;
        bytes32 root;
        bytes32 linesHash;
    }
}

pub const DOMAIN_NAME: &str = "Tameion Statement";
pub const DOMAIN_VERSION: &str = "1";
const EVENT_VERSION: &str = "air-log/v0.1";
const DAY: i64 = 86_400;

// ---- ISO weeks, proleptic Gregorian, UTC (Howard Hinnant's day algorithms) ----

pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Monday = 0 … Sunday = 6. 1970-01-01 was a Thursday.
fn weekday(days: i64) -> i64 {
    (days + 3).rem_euclid(7)
}

/// The Monday that starts ISO week 1 of `year`: the week holding 4 January.
fn week1_monday(year: i64) -> i64 {
    let jan4 = days_from_civil(year, 1, 4);
    jan4 - weekday(jan4)
}

/// `[from, to)` in unix seconds for an ISO week label `YYYY-Www`: Monday
/// 00:00 UTC to the next Monday. `None` for a malformed label or a week
/// the year does not have (most years have 52).
pub fn week_bounds(label: &str) -> Option<(i64, i64)> {
    let (y, w) = label.trim().split_once("-W")?;
    let (year, week) = (y.parse::<i64>().ok()?, w.parse::<i64>().ok()?);
    if y.len() != 4 || w.len() != 2 || !(1..=53).contains(&week) {
        return None;
    }
    let from = week1_monday(year) + (week - 1) * 7;
    if from >= week1_monday(year + 1) {
        return None;
    }
    Some((from * DAY, (from + 7) * DAY))
}

/// The ISO week label holding `unix`, e.g. `2026-W42`.
pub fn week_of(unix: i64) -> String {
    let days = unix.div_euclid(DAY);
    // The ISO year is the year of the week's Thursday.
    let thursday = days - weekday(days) + 3;
    let (year, _, _) = civil_from_days(thursday);
    format!("{year}-W{:02}", (thursday - week1_monday(year)) / 7 + 1)
}

/// `2026-10-15T03:00:36Z` for a unix time.
pub fn iso8601(unix: i64) -> String {
    let (y, m, d) = civil_from_days(unix.div_euclid(DAY));
    let s = unix.rem_euclid(DAY);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
}

// ---- the canonical bytes, read back (mirror of `ledger::log::Event::canonical`) ----

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_be_bytes)
    }
    fn i128(&mut self) -> Option<i128> {
        self.take(16)
            .and_then(|b| b.try_into().ok())
            .map(i128::from_be_bytes)
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u64()?;
        self.take(usize::try_from(n).ok()?)
    }
    fn str(&mut self) -> Option<String> {
        std::str::from_utf8(self.bytes()?).ok().map(String::from)
    }
    fn money(&mut self) -> Option<Money> {
        let minor = self.i128()?;
        let ccy = match self.u8()? {
            1 => Currency::Usdc,
            2 => Currency::Eurc,
            3 => Currency::Usd,
            4 => Currency::Eur,
            _ => return None,
        };
        Some(Money::minor(minor, ccy))
    }
}

/// One line of the statement, as decoded from an event's canonical bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub at: i64,
    pub seq: u64,
    /// The anchored root whose record holds this event.
    pub root: String,
    /// `paid`, `refused` or `needs-you`.
    pub kind: &'static str,
    pub payee: String,
    pub amount: Money,
    /// The settling transaction, for a `paid` line.
    pub tx: Option<String>,
    /// The agent's reason (refused, needs-you) or note (paid), as recorded.
    pub detail: String,
    /// `Some("owner")` for a payment the owner settled with the passkey, read
    /// from the chain; `None` for a line decoded from the agent's record.
    pub by: Option<&'static str>,
}

impl Line {
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "at": self.at, "seq": self.seq, "root": self.root, "kind": self.kind,
            "payee": self.payee, "amount": self.amount.to_string(), "tx": self.tx, "detail": self.detail,
        });
        if let Some(by) = self.by {
            v["paidBy"] = json!(by);
        }
        v
    }
}

/// `Ok(Some)` for a settlement, a blocked decision or an escalation;
/// `Ok(None)` for any other event; `Err` when the bytes do not read as an
/// event this code knows, so a statement is never quietly short a line.
fn decode(canonical: &[u8], root: &str) -> Result<Option<Line>, String> {
    let mut r = Reader(canonical);
    let short = || "canonical bytes end early".to_string();
    let version = r.str().ok_or_else(short)?;
    if version != EVENT_VERSION {
        return Err(format!(
            "event version {version:?} is not {EVENT_VERSION:?}"
        ));
    }
    let seq = r.u64().ok_or_else(short)?;
    let at = i64::try_from(r.u64().ok_or_else(short)?).map_err(|_| short())?;
    let line = |kind, payee, amount, tx, detail| Line {
        at,
        seq,
        root: root.to_string(),
        kind,
        payee,
        amount,
        tx,
        detail,
        by: None,
    };
    Ok(match r.u8().ok_or_else(short)? {
        // Decision: counterparty, amount, verdict, action hash, confidence,
        // balance seen, weakest evidence, evidence list, note.
        2 => {
            let payee = r.str().ok_or_else(short)?;
            let amount = r.money().ok_or_else(short)?;
            let verdict = r.u8().ok_or_else(short)?;
            r.bytes().ok_or_else(short)?;
            r.u64().ok_or_else(short)?;
            r.money().ok_or_else(short)?;
            if r.u8().ok_or_else(short)? == 1 {
                r.u8().ok_or_else(short)?;
            }
            let n = r.u64().ok_or_else(short)?;
            for _ in 0..n {
                r.str().ok_or_else(short)?;
            }
            let note = r.str().ok_or_else(short)?;
            (verdict == 3).then(|| line("refused", payee, amount, None, note))
        }
        // Settlement: counterparty, cycle key, amount, tx, action hash,
        // settlement key, note.
        3 => {
            let payee = r.str().ok_or_else(short)?;
            r.str().ok_or_else(short)?;
            let amount = r.money().ok_or_else(short)?;
            let tx = r.str().ok_or_else(short)?;
            r.bytes().ok_or_else(short)?;
            r.bytes().ok_or_else(short)?;
            let note = r.str().ok_or_else(short)?;
            Some(line("paid", payee, amount, Some(tx), note))
        }
        // Escalation: counterparty, amount, reason, note.
        4 => {
            let payee = r.str().ok_or_else(short)?;
            let amount = r.money().ok_or_else(short)?;
            let reason = r.str().ok_or_else(short)?;
            Some(line("needs-you", payee, amount, None, reason))
        }
        _ => None,
    })
}

/// Every statement line in `[from, to)` across the account's anchored
/// records, oldest first (ties by root, then sequence), so two readers of
/// the same files get the same bytes.
// ponytail: reads every events file of the account per request; an index
// by week when an account has thousands of passes.
pub fn lines_for(log_dir: &Path, from: i64, to: i64) -> Result<Vec<Line>, String> {
    let mut out = Vec::new();
    if !log_dir.exists() {
        return Ok(out);
    }
    for e in fs::read_dir(log_dir).map_err(|e| e.to_string())? {
        let p = e.map_err(|e| e.to_string())?.path();
        let Some(root) = audit::root_from_filename(&p) else {
            continue;
        };
        let root = root.to_hex();
        for l in audit::load(&p).map_err(|e| e.to_string())? {
            let decoded = decode(&l.canonical, &root)
                .map_err(|e| format!("{}: seq {}: {e}", p.display(), l.seq))?;
            if let Some(line) = decoded
                && line.at >= from
                && line.at < to
            {
                out.push(line);
            }
        }
    }
    out.sort_by(|a, b| (a.at, &a.root, a.seq).cmp(&(b.at, &b.root, b.seq)));
    Ok(out)
}

// ---- hashing and signing ----

/// Compact JSON with object keys sorted, whatever map the serializer uses.
pub fn canonical_json(v: &Value) -> String {
    fn go(v: &Value, out: &mut String) {
        match v {
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    go(item, out);
                }
                out.push(']');
            }
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String(k.clone()).to_string());
                    out.push(':');
                    go(&map[k], out);
                }
                out.push('}');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut s = String::new();
    go(v, &mut s);
    s
}

pub fn lines_hash(lines: &[Value]) -> B256 {
    keccak256(canonical_json(&Value::Array(lines.to_vec())).as_bytes())
}

/// The EIP-712 digest the statement key signs.
pub fn digest(
    policy: Address,
    chain_id: u64,
    from: u64,
    to: u64,
    root: B256,
    lines_hash: B256,
) -> B256 {
    let domain = eip712_domain! {
        name: DOMAIN_NAME,
        version: DOMAIN_VERSION,
        chain_id: chain_id,
    };
    Statement {
        policy,
        chainId: alloy::primitives::U256::from(chain_id),
        from,
        to,
        root,
        linesHash: lines_hash,
    }
    .eip712_signing_hash(&domain)
}

/// The types block a verifier feeds `cast wallet verify --data`, as served
/// in every statement so nobody has to look them up here.
pub fn typed_data_types() -> Value {
    json!({
        "EIP712Domain": [
            { "name": "name", "type": "string" }, { "name": "version", "type": "string" }, { "name": "chainId", "type": "uint256" },
        ],
        "Statement": [
            { "name": "policy", "type": "address" }, { "name": "chainId", "type": "uint256" },
            { "name": "from", "type": "uint64" }, { "name": "to", "type": "uint64" },
            { "name": "root", "type": "bytes32" }, { "name": "linesHash", "type": "bytes32" },
        ],
    })
}

fn key_path() -> String {
    std::env::var("TAMEION_STATEMENT_KEY_FILE").unwrap_or_else(|_| {
        format!(
            "{}/.tameion/statement.key",
            std::env::var("HOME").unwrap_or_default()
        )
    })
}

/// The statement key: `~/.tameion/statement.key` (or
/// `TAMEION_STATEMENT_KEY_FILE`), made on first use, mode 600. It signs
/// statements and nothing else -- never the relayer's or the treasury's key,
/// so a reader who learns this address learns no key that can move money.
pub fn key() -> Result<PrivateKeySigner, String> {
    let path = key_path();
    match fs::read_to_string(&path) {
        Ok(k) => {
            PrivateKeySigner::from_str(k.trim()).map_err(|e| format!("statement key {path}: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            use std::os::unix::fs::OpenOptionsExt;
            if let Some(dir) = Path::new(&path).parent() {
                fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let fresh = PrivateKeySigner::random();
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut f) => {
                    writeln!(f, "0x{}", alloy::hex::encode(fresh.to_bytes()))
                        .map_err(|e| format!("statement key {path}: {e}"))?;
                    Ok(fresh)
                }
                // Two requests made the key at once: the first one's wins.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => key(),
                Err(e) => Err(format!("statement key {path}: {e}")),
            }
        }
        Err(e) => Err(format!("statement key {path}: {e}")),
    }
}

/// The address readers check signatures against (`/api/level1/config`).
pub fn signer_address() -> Result<Address, String> {
    key().map(|k| k.address())
}

/// What the statement is about: the account, its chain, the week.
pub struct Scope<'a> {
    pub policy: Address,
    pub chain_key: &'a str,
    pub chain_id: u64,
    pub explorer: &'a str,
    pub week: &'a str,
    pub from: i64,
    pub to: i64,
}

/// A payment the owner settled with the passkey (`SettledByPrincipal`), read
/// from the chain: the agent's record never holds it, so in "asks before
/// paying" a statement built from the record alone said "paid 0" while the
/// chain showed the payments (docs/60 P-177).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerPaid {
    pub at: i64,
    pub payee: String,
    pub amount: Money,
    pub tx: String,
}

/// The whole statement for one account and week, signed.
pub fn build(log_dir: &Path, s: &Scope<'_>) -> Result<Value, String> {
    build_with(log_dir, s, &[])
}

/// [`build`] with the owner's own payments of the week as `paid` lines
/// (`paidBy: "owner"`, with the transaction anyone can check), each with the
/// agent's latest ask for that payee and amount as its reason.
pub fn build_with(log_dir: &Path, s: &Scope<'_>, owner: &[OwnerPaid]) -> Result<Value, String> {
    let mut lines = lines_for(log_dir, s.from, s.to)?;
    let mut added = Vec::new();
    for o in owner.iter().filter(|o| o.at >= s.from && o.at < s.to) {
        let asked = lines
            .iter()
            .rev()
            .find(|l| {
                l.kind == "needs-you"
                    && l.payee.eq_ignore_ascii_case(&o.payee)
                    && l.amount == o.amount
                    && l.at <= o.at
            })
            .map(|l| l.detail.clone());
        added.push(Line {
            at: o.at,
            seq: 0,
            root: String::new(),
            kind: "paid",
            payee: o.payee.clone(),
            amount: o.amount,
            tx: Some(o.tx.clone()),
            detail: match asked {
                Some(r) => format!("paid by the owner with the passkey; the agent asked: {r}"),
                None => "paid by the owner with the passkey".into(),
            },
            by: Some("owner"),
        });
    }
    lines.extend(added);
    lines.sort_by(|a, b| (a.at, &a.root, a.seq).cmp(&(b.at, &b.root, b.seq)));
    let line_values: Vec<Value> = lines.iter().map(Line::to_json).collect();
    // Totals count every line; the amount sums USDC, the only currency the
    // accounts settle in. A line in another currency still shows its own.
    let total = |kind: &str| {
        let mine = lines.iter().filter(|l| l.kind == kind);
        let minor: i128 = mine
            .clone()
            .filter(|l| l.amount.currency() == Currency::Usdc)
            .map(|l| l.amount.to_minor())
            .sum();
        json!({ "count": mine.count(), "amount": Money::minor(minor, Currency::Usdc).to_string() })
    };
    // The record the week stands on: the newest anchored root as of `to`
    // (what `rootOf` holds while this is the current week); failing one with
    // a transaction, the newest root at all; failing any, zero.
    let anchors = audit::anchors(log_dir).map_err(|e| e.to_string())?;
    let before: Vec<&Value> = anchors
        .iter()
        .filter(|a| a["at"].as_i64().is_some_and(|at| at < s.to))
        .collect();
    let anchor = before
        .iter()
        .rev()
        .find(|a| !a["anchorTx"].is_null())
        .or(before.last())
        .copied();
    let root = anchor
        .and_then(|a| a["root"].as_str())
        .and_then(|r| B256::from_str(r).ok())
        .unwrap_or(B256::ZERO);
    let lines_hash = lines_hash(&line_values);
    let hash = digest(
        s.policy,
        s.chain_id,
        s.from as u64,
        s.to as u64,
        root,
        lines_hash,
    );
    let signer = key()?;
    let sig = signer.sign_hash_sync(&hash).map_err(|e| e.to_string())?;
    Ok(json!({
        "policy": format!("{:#x}", s.policy),
        "chain": { "key": s.chain_key, "chainId": s.chain_id, "explorer": s.explorer },
        "week": { "label": s.week, "from": s.from, "to": s.to },
        "lines": line_values,
        "totals": { "paid": total("paid"), "refused": total("refused"), "needsYou": total("needs-you") },
        "record": {
            "eventsFrom": lines.first().map(|l| l.at),
            "eventsTo": lines.last().map(|l| l.at),
            "root": format!("{root:#x}"),
            "anchorTx": anchor.map(|a| a["anchorTx"].clone()).unwrap_or(Value::Null),
            "agent": anchor.map(|a| a["agent"].clone()).unwrap_or(Value::Null),
            "anchor": anchor.map(|a| a["anchor"].clone()).unwrap_or(Value::Null),
            "linesHash": format!("{lines_hash:#x}"),
        },
        "signature": {
            "scheme": "eip712",
            "signer": format!("{:#x}", signer.address()),
            "domain": { "name": DOMAIN_NAME, "version": DOMAIN_VERSION, "chainId": s.chain_id },
            "primaryType": "Statement",
            "types": typed_data_types(),
            "hash": format!("{hash:#x}"),
            "sig": format!("0x{}", alloy::hex::encode(sig.as_bytes())),
        },
        "verify": "scripts/verify_statement.sh <this JSON> <this host>",
    }))
}

/// The same statement as CSV for an accountant's import: one line per row,
/// the signature and what it covers in trailing `#` comment lines.
pub fn csv(statement: &Value) -> String {
    // A field with a comma, a quote or a line break is quoted (RFC 4180).
    let field = |s: &str| {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let mut out = String::from("date,payee,amount,kind,tx,paid_by,reason\n");
    for l in statement["lines"].as_array().into_iter().flatten() {
        let kind = l["kind"].as_str().unwrap_or("");
        let by = match (kind, l["paidBy"].as_str()) {
            (_, Some(b)) => b,
            ("paid", None) => "agent",
            _ => "",
        };
        out.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            iso8601(l["at"].as_i64().unwrap_or(0)),
            field(l["payee"].as_str().unwrap_or("")),
            field(l["amount"].as_str().unwrap_or("")),
            field(kind),
            field(l["tx"].as_str().unwrap_or("")),
            by,
            field(l["detail"].as_str().unwrap_or("")),
        ));
    }
    let r = &statement["record"];
    let sg = &statement["signature"];
    out.push_str(&format!(
        "# statement {} for {} on {} (chainId {}); week {}..{} unix; root {}; linesHash {}\n",
        statement["week"]["label"].as_str().unwrap_or(""),
        statement["policy"].as_str().unwrap_or(""),
        statement["chain"]["key"].as_str().unwrap_or(""),
        statement["chain"]["chainId"],
        statement["week"]["from"],
        statement["week"]["to"],
        r["root"].as_str().unwrap_or(""),
        r["linesHash"].as_str().unwrap_or(""),
    ));
    out.push_str(&format!(
        "# signature eip712 signer={} hash={} sig={}\n",
        sg["signer"].as_str().unwrap_or(""),
        sg["hash"].as_str().unwrap_or(""),
        sg["sig"].as_str().unwrap_or(""),
    ));
    out.push_str("# verify: fetch the same week without format=csv and run scripts/verify_statement.sh on it\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Signature;
    use ledger::log::{DecisionLog, EventBody};
    use ledger::time::Timestamp;
    use ledger::{ActionHash, Confidence, Counterparty, Decision};

    fn usdc(s: &str) -> Money {
        Money::parse(s, Currency::Usdc).unwrap()
    }

    #[test]
    fn iso_week_bounds_are_monday_to_monday_utc() {
        // python3: calendar.timegm(date.fromisocalendar(2026, 42, 1).timetuple())
        assert_eq!(
            week_bounds("2026-W42"),
            Some((1_791_763_200, 1_792_368_000))
        );
        // 2020 has a week 53 (Mon 2020-12-28), so does 2026 (a year that
        // starts on a Thursday); 2025 does not.
        assert_eq!(
            week_bounds("2020-W53"),
            Some((1_609_113_600, 1_609_718_400))
        );
        assert_eq!(
            week_bounds("2026-W53"),
            Some((1_798_416_000, 1_799_020_800))
        );
        assert_eq!(week_bounds("2025-W53"), None);
        assert_eq!(week_bounds("2026-W00"), None);
        assert_eq!(week_bounds("2026-42"), None);
        assert_eq!(week_bounds("26-W42"), None);
        // Thu 2026-10-15 03:26:50Z is in week 42; New Year's Day 2021 is still 2020-W53.
        assert_eq!(week_of(1_792_034_810), "2026-W42");
        assert_eq!(week_of(1_609_459_200), "2020-W53");
        assert_eq!(week_of(1_609_113_600 - 1), "2020-W52");
        // Round trip across a few years, including the boundaries of each week.
        for label in ["2024-W01", "2025-W52", "2026-W01", "2026-W42", "2020-W53"] {
            let (from, to) = week_bounds(label).unwrap();
            assert_eq!(week_of(from), label);
            assert_eq!(week_of(to - 1), label);
            assert_ne!(week_of(to), label);
        }
        assert_eq!(iso8601(1_792_034_810), "2026-10-15T03:26:50Z");
    }

    #[test]
    fn the_lines_hash_is_the_same_whatever_the_key_order_and_moves_with_one_byte() {
        let a = json!({ "b": 1, "a": "x" });
        let mut b = serde_json::Map::new();
        b.insert("a".into(), json!("x"));
        b.insert("b".into(), json!(1));
        let b = Value::Object(b);
        assert_eq!(canonical_json(&a), r#"{"a":"x","b":1}"#);
        assert_eq!(lines_hash(std::slice::from_ref(&a)), lines_hash(&[b]));
        // cast keccak '[{"a":"x","b":1}]'
        assert_eq!(
            format!("{:#x}", lines_hash(&[a])),
            "0xd411b846dbc516737626f779ed3d4bd2c1dd78437534536e24a64aa6f10f7797"
        );
        assert_ne!(
            lines_hash(&[json!({ "a": "x", "b": 1 })]),
            lines_hash(&[json!({ "a": "y", "b": 1 })])
        );
        // Strings are escaped the way serde_json does, which Python's json
        // (ensure_ascii=False) matches: quotes, backslashes, control bytes.
        assert_eq!(
            canonical_json(&json!(["a\"b\\c\n\u{1f}é"])),
            "[\"a\\\"b\\\\c\\n\\u001fé\"]"
        );
    }

    /// Vector made with Foundry's own EIP-712 encoder, using the public
    /// anvil account 0 (0xf39F…2266, key 0xac09…ff80):
    ///
    ///   cast wallet sign --mnemonic "test test test test test test test test test test test junk" --mnemonic-index 0 \
    ///     --data '{"types":{"EIP712Domain":[{"name":"name","type":"string"},{"name":"version","type":"string"},{"name":"chainId","type":"uint256"}],"Statement":[{"name":"policy","type":"address"},{"name":"chainId","type":"uint256"},{"name":"from","type":"uint64"},{"name":"to","type":"uint64"},{"name":"root","type":"bytes32"},{"name":"linesHash","type":"bytes32"}]},"primaryType":"Statement","domain":{"name":"Tameion Statement","version":"1","chainId":421614},"message":{"policy":"0x35a2e2701d4d72be257bf7708c67d1433c60342f","chainId":421614,"from":1791763200,"to":1792368000,"root":"0x1111111111111111111111111111111111111111111111111111111111111111","linesHash":"0x2222222222222222222222222222222222222222222222222222222222222222"}}'
    ///
    /// If our digest differed from cast's in any byte, the signature would
    /// recover to some other address.
    #[test]
    fn the_eip712_digest_is_the_one_cast_signs() {
        let sig = Signature::from_str(
            "0x6115295afab8df00ed0f83125c46cad02ec54fec1bc9930b3a73ea29c99e0f046d7fd6e6253ad53a5ac237e612fa4ecd519aaabae0fadd9f6d72eb28308a860d1c",
        )
        .unwrap();
        let d = digest(
            Address::from_str("0x35a2e2701d4d72be257bf7708c67d1433c60342f").unwrap(),
            421_614,
            1_791_763_200,
            1_792_368_000,
            B256::repeat_byte(0x11),
            B256::repeat_byte(0x22),
        );
        assert_eq!(
            sig.recover_address_from_prehash(&d).unwrap(),
            Address::from_str("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap()
        );
        // One field moved, one other address.
        let d2 = digest(
            Address::from_str("0x35a2e2701d4d72be257bf7708c67d1433c60342f").unwrap(),
            421_614,
            1_791_763_200,
            1_792_368_001,
            B256::repeat_byte(0x11),
            B256::repeat_byte(0x22),
        );
        assert_ne!(
            sig.recover_address_from_prehash(&d2).unwrap(),
            Address::from_str("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap()
        );
    }

    #[test]
    fn lines_are_decoded_from_the_canonical_bytes_and_cut_to_the_week() {
        let dir = std::env::temp_dir().join(format!("tameion-statement-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (from, to) = week_bounds("2026-W42").unwrap();
        let who = Counterparty::new("0x1111111111111111111111111111111111111111");
        let mut log = DecisionLog::new();
        log.append(
            Timestamp::from_unix(from + 10),
            EventBody::Observation {
                balance: usdc("1"),
                transfers_seen: 0,
                obligations_found: 0,
                total_due: usdc("0"),
                note: "obs".into(),
            },
        );
        log.append(
            Timestamp::from_unix(from + 20),
            EventBody::Decision {
                counterparty: who.clone(),
                amount: usdc("0.5"),
                decision: Decision::Block,
                action_hash: ActionHash::from_bytes([7u8; 32]),
                confidence: Confidence::new(0.9).unwrap(),
                balance_seen: usdc("1"),
                weakest_evidence: None,
                evidence: vec!["e1".into(), "e2".into()],
                note: "over the cap".into(),
            },
        );
        log.append(
            Timestamp::from_unix(from + 30),
            EventBody::Settlement {
                counterparty: who.clone(),
                cycle_key: "c".into(),
                amount: usdc("0.003"),
                tx_hash: "0xabc".into(),
                action_hash: ActionHash::from_bytes([1u8; 32]),
                settlement_key: ActionHash::from_bytes([2u8; 32]),
                note: "paid".into(),
            },
        );
        log.append(
            Timestamp::from_unix(from + 40),
            EventBody::Escalation {
                counterparty: who.clone(),
                amount: usdc("0.2"),
                reason: "not attested".into(),
                note: "long note".into(),
            },
        );
        // Next week: not a line of this one.
        log.append(
            Timestamp::from_unix(to),
            EventBody::Settlement {
                counterparty: who,
                cycle_key: "c".into(),
                amount: usdc("9"),
                tx_hash: "0xdef".into(),
                action_hash: ActionHash::from_bytes([1u8; 32]),
                settlement_key: ActionHash::from_bytes([2u8; 32]),
                note: "later".into(),
            },
        );
        let path = audit::persist(&dir, &log).unwrap();
        let lines = lines_for(&dir, from, to).unwrap();
        let kinds: Vec<&str> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, ["refused", "paid", "needs-you"]);
        assert_eq!(lines[0].amount, usdc("0.5"));
        assert_eq!(lines[0].detail, "over the cap");
        assert_eq!(lines[1].tx.as_deref(), Some("0xabc"));
        assert_eq!(lines[1].amount.to_string(), "0.003000 USDC");
        assert_eq!(lines[2].detail, "not attested");
        assert!(lines.iter().all(|l| l.root == log.root().to_hex()));
        assert_eq!(lines_for(&dir, to, to + 7 * DAY).unwrap().len(), 1);
        // A root this code cannot read is an error, not a shorter statement.
        let text = fs::read_to_string(&path).unwrap();
        let edited = text.replacen("\"canonical\":\"0x", "\"canonical\":\"0x00", 1);
        fs::write(&path, edited).unwrap();
        assert!(lines_for(&dir, from, to).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_built_statement_verifies_and_one_flipped_byte_does_not() {
        let dir = std::env::temp_dir().join(format!("tameion-statement-b-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let keyfile = dir.join("statement.key");
        // SAFETY: tests in this module that read the key run with this set;
        // the variable is only read by `key_path`.
        unsafe { std::env::set_var("TAMEION_STATEMENT_KEY_FILE", &keyfile) };
        let (from, to) = week_bounds("2026-W42").unwrap();
        let mut log = DecisionLog::new();
        log.append(
            Timestamp::from_unix(from + 30),
            EventBody::Settlement {
                counterparty: Counterparty::new("0x2222222222222222222222222222222222222222"),
                cycle_key: "c".into(),
                amount: usdc("0.003"),
                tx_hash: "0xabc".into(),
                action_hash: ActionHash::from_bytes([1u8; 32]),
                settlement_key: ActionHash::from_bytes([2u8; 32]),
                note: "paid".into(),
            },
        );
        let log_dir = dir.join("log");
        audit::persist(&log_dir, &log).unwrap();
        audit::append_anchor(
            &log_dir,
            log.root(),
            Some("0xtx".into()),
            Some("0xagent".into()),
            "0xanchor".into(),
            from + 31,
            1,
        )
        .unwrap();
        let policy = Address::from_str("0x35a2e2701d4d72be257bf7708c67d1433c60342f").unwrap();
        let scope = Scope {
            policy,
            chain_key: "arbitrum-sepolia",
            chain_id: 421_614,
            explorer: "https://sepolia.arbiscan.io",
            week: "2026-W42",
            from,
            to,
        };
        let st = build(&log_dir, &scope).unwrap();
        // The key was made on first use, mode 600, and is the advertised signer.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&keyfile).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            st["signature"]["signer"],
            format!("{:#x}", signer_address().unwrap())
        );
        assert_eq!(st["totals"]["paid"]["count"], 1);
        assert_eq!(st["totals"]["paid"]["amount"], "0.003000 USDC");
        assert_eq!(st["record"]["root"], log.root().to_hex());
        assert_eq!(st["record"]["anchorTx"], "0xtx");
        // Recompute what a verifier recomputes.
        let lines = st["lines"].as_array().unwrap();
        let lh = lines_hash(lines);
        assert_eq!(st["record"]["linesHash"], format!("{lh:#x}"));
        let d = digest(
            policy,
            421_614,
            from as u64,
            to as u64,
            B256::from(*log.root().as_bytes()),
            lh,
        );
        assert_eq!(st["signature"]["hash"], format!("{d:#x}"));
        let sig = Signature::from_str(st["signature"]["sig"].as_str().unwrap()).unwrap();
        let signer = Address::from_str(st["signature"]["signer"].as_str().unwrap()).unwrap();
        assert_eq!(sig.recover_address_from_prehash(&d).unwrap(), signer);
        // One byte of one line flipped: the lines hash, and so the signer, move.
        let mut tampered = lines.clone();
        tampered[0]["amount"] = json!("0.003001 USDC");
        let lh2 = lines_hash(&tampered);
        assert_ne!(lh, lh2);
        let d2 = digest(
            policy,
            421_614,
            from as u64,
            to as u64,
            B256::from(*log.root().as_bytes()),
            lh2,
        );
        assert_ne!(sig.recover_address_from_prehash(&d2).unwrap(), signer);
        // CSV: one row, who paid and why, the signature in a comment.
        let c = csv(&st);
        assert!(c.starts_with("date,payee,amount,kind,tx,paid_by,reason\n"));
        assert!(c.contains(
            ",0x2222222222222222222222222222222222222222,0.003000 USDC,paid,0xabc,agent,paid\n"
        ));
        // docs/60 P-177: the owner's passkey payment of the week is a paid line,
        // signed with the rest, in the CSV with who paid and why.
        let owner = [OwnerPaid {
            at: from + 40,
            payee: "0x3333333333333333333333333333333333333333".into(),
            amount: usdc("0.05"),
            tx: "0xowner".into(),
        }];
        let st2 = build_with(&log_dir, &scope, &owner).unwrap();
        assert_eq!(st2["totals"]["paid"]["count"], 2);
        assert_eq!(st2["totals"]["paid"]["amount"], "0.053000 USDC");
        let line = st2["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["tx"] == "0xowner")
            .unwrap()
            .clone();
        assert_eq!(line["paidBy"], "owner");
        assert_eq!(line["detail"], "paid by the owner with the passkey");
        assert_ne!(
            st2["record"]["linesHash"], st["record"]["linesHash"],
            "the signature covers it"
        );
        assert!(csv(&st2).contains(",0x3333333333333333333333333333333333333333,0.050000 USDC,paid,0xowner,owner,paid by the owner with the passkey\n"));
        assert!(c.contains(&format!(
            "# signature eip712 signer={}",
            st["signature"]["signer"].as_str().unwrap()
        )));
        let _ = fs::remove_dir_all(dir);
    }
}
