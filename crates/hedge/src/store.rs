//! The books on disk: an append-only journal and the marks it was last
//! booked against. Lives next to the `HALT` file in `runtime/hedge/`.
//!
//! Format is one JSON object per line, one line per [`Entry`], written once
//! and never rewritten. Loading re-posts every line through [`Entry::post`],
//! so a line that does not balance -- edited, truncated, corrupted -- is
//! refused with its line number rather than skipped. Amounts are integer
//! minor units carried as strings; nothing here goes through a float.

use crate::book;
use crate::engine::Books;
use ledger::{
    AccountId, Credit, Currency, Debit, DocumentKind, DocumentRef, Entry, Journal, Money,
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::str::FromStr;

pub const JOURNAL_FILE: &str = "journal.jsonl";
pub const STATE_FILE: &str = "state.json";
/// One line per engine pass (docs/106): when it ended, how long it took and
/// every order it sent with the position read back after it.
pub const PASSES_FILE: &str = "passes.jsonl";

/// Append one pass to `<halt_dir>/<account>/passes.jsonl`, next to the books.
/// Written once, never rewritten: the live feed's last orders, read-backs and
/// pass latency come from here, not from memory.
pub fn append_pass(
    halt_dir: &Path,
    user: hypersdk::Address,
    pass: &Value,
) -> Result<(), StoreError> {
    let dir = crate::driver::books_dir(halt_dir, user);
    fs::create_dir_all(&dir)?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(PASSES_FILE))?;
    writeln!(f, "{pass}")?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// A journal line that could not be read back as a balanced entry.
    #[error("journal line {line}: {what}")]
    Corrupt { line: usize, what: String },
    #[error("state: {0}")]
    State(String),
}

/// Load the books from `dir`, or `None` when nothing has been saved there.
///
/// The chart covers every coin the journal has ever booked plus `coins`, so
/// a balance on a coin hedged last week is still answerable this week.
pub fn load_or_new(dir: &Path, coins: &[&str]) -> Result<Option<Books>, StoreError> {
    let journal_path = dir.join(JOURNAL_FILE);
    if !journal_path.exists() {
        return Ok(None);
    }
    let file = fs::File::open(&journal_path)?;
    let mut journal = Journal::new();
    let mut seen_coins: Vec<String> = coins.iter().map(|c| c.to_string()).collect();
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).map_err(|e| StoreError::Corrupt {
            line: i + 1,
            what: e.to_string(),
        })?;
        let entry = entry_from_json(&v, i + 1)?;
        for leg in entry
            .debits()
            .iter()
            .map(|d| d.account().as_str())
            .chain(entry.credits().iter().map(|c| c.account().as_str()))
        {
            if let Some(coin) = coin_of_account(leg)
                && !seen_coins.iter().any(|c| c == coin)
            {
                seen_coins.push(coin.to_string());
            }
        }
        journal.append(entry);
    }

    let state_path = dir.join(STATE_FILE);
    let (last_spot_value, last_hedge_upnl, funding_cursor_ms) = if state_path.exists() {
        let v: Value = serde_json::from_str(&fs::read_to_string(&state_path)?)?;
        (
            dec_map(&v["lastSpotValue"])?,
            dec_map(&v["lastHedgeUpnl"])?,
            v["fundingCursorMs"]
                .as_u64()
                .ok_or_else(|| StoreError::State("fundingCursorMs missing".into()))?,
        )
    } else {
        (HashMap::new(), HashMap::new(), 0)
    };

    let coin_refs: Vec<&str> = seen_coins.iter().map(String::as_str).collect();
    let persisted = journal.len();
    Ok(Some(Books {
        chart: book::chart(&coin_refs),
        journal,
        last_spot_value,
        last_hedge_upnl,
        funding_cursor_ms,
        persisted,
    }))
}

/// Append the entries not yet on disk and rewrite the small state file.
/// Returns how many entries were appended.
pub fn save(dir: &Path, books: &mut Books) -> Result<usize, StoreError> {
    fs::create_dir_all(dir)?;
    let new_entries = &books.journal.entries()[books.persisted..];
    if !new_entries.is_empty() {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(JOURNAL_FILE))?;
        for e in new_entries {
            writeln!(f, "{}", entry_to_json(e))?;
        }
        f.flush()?;
    }
    let appended = new_entries.len();
    books.persisted = books.journal.len();

    let state = json!({
        "lastSpotValue": books.last_spot_value.iter().map(|(k, v)| (k.clone(), Value::String(v.normalize().to_string()))).collect::<serde_json::Map<_, _>>(),
        "lastHedgeUpnl": books.last_hedge_upnl.iter().map(|(k, v)| (k.clone(), Value::String(v.normalize().to_string()))).collect::<serde_json::Map<_, _>>(),
        "fundingCursorMs": books.funding_cursor_ms,
        "entries": books.persisted,
    });
    // Write whole, then rename: a crash mid-write leaves the old state, not
    // half of the new one.
    let tmp = dir.join(format!("{STATE_FILE}.tmp"));
    fs::write(&tmp, serde_json::to_string_pretty(&state)?)?;
    fs::rename(&tmp, dir.join(STATE_FILE))?;
    Ok(appended)
}

fn entry_to_json(e: &Entry) -> Value {
    let leg = |account: &AccountId, m: Money| json!({ "account": account.as_str(), "minor": m.to_minor().to_string(), "ccy": m.currency().code() });
    json!({
        "doc": { "kind": e.document().kind().to_string(), "id": e.document().id() },
        "debits": e.debits().iter().map(|d| leg(d.account(), d.amount())).collect::<Vec<_>>(),
        "credits": e.credits().iter().map(|c| leg(c.account(), c.amount())).collect::<Vec<_>>(),
    })
}

fn entry_from_json(v: &Value, line: usize) -> Result<Entry, StoreError> {
    let corrupt = |what: String| StoreError::Corrupt { line, what };
    let kind = kind_from_str(
        v["doc"]["kind"]
            .as_str()
            .ok_or_else(|| corrupt("doc.kind missing".into()))?,
    )
    .ok_or_else(|| corrupt(format!("unknown doc.kind {:?}", v["doc"]["kind"])))?;
    let id = v["doc"]["id"]
        .as_str()
        .ok_or_else(|| corrupt("doc.id missing".into()))?;
    let legs = |key: &str| -> Result<Vec<(AccountId, Money)>, StoreError> {
        v[key]
            .as_array()
            .ok_or_else(|| corrupt(format!("{key} missing")))?
            .iter()
            .map(|l| {
                let account = l["account"]
                    .as_str()
                    .ok_or_else(|| corrupt(format!("{key}[].account missing")))?;
                let minor: i128 = l["minor"]
                    .as_str()
                    .ok_or_else(|| corrupt(format!("{key}[].minor missing")))?
                    .parse()
                    .map_err(|e| corrupt(format!("{key}[].minor: {e}")))?;
                let ccy = currency_from_code(l["ccy"].as_str().unwrap_or("USDC"))
                    .ok_or_else(|| corrupt(format!("{key}[].ccy unknown")))?;
                Ok((AccountId::new(account), Money::minor(minor, ccy)))
            })
            .collect()
    };
    let debits = legs("debits")?
        .into_iter()
        .map(|(a, m)| Debit::new(a, m).map_err(|e| corrupt(e.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    let credits = legs("credits")?
        .into_iter()
        .map(|(a, m)| Credit::new(a, m).map_err(|e| corrupt(e.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    Entry::post(DocumentRef::new(kind, id), debits, credits).map_err(|e| corrupt(e.to_string()))
}

fn dec_map(v: &Value) -> Result<HashMap<String, Decimal>, StoreError> {
    let Some(obj) = v.as_object() else {
        return Ok(HashMap::new());
    };
    obj.iter()
        .map(|(k, x)| {
            let s = x
                .as_str()
                .ok_or_else(|| StoreError::State(format!("{k}: not a string")))?;
            Decimal::from_str(s)
                .map(|d| (k.clone(), d))
                .map_err(|e| StoreError::State(format!("{k}: {e}")))
        })
        .collect()
}

/// `asset:treasury:spot:ETH` / `asset:hedge:mtm:ETH` → `ETH`.
fn coin_of_account(id: &str) -> Option<&str> {
    id.strip_prefix("asset:treasury:spot:")
        .or_else(|| id.strip_prefix("asset:hedge:mtm:"))
}

pub fn kind_from_str(s: &str) -> Option<DocumentKind> {
    Some(match s {
        "obligation" => DocumentKind::Obligation,
        "settlement" => DocumentKind::Settlement,
        "invoice" => DocumentKind::Invoice,
        "receipt" => DocumentKind::Receipt,
        "adjustment" => DocumentKind::Adjustment,
        "hedge" => DocumentKind::Hedge,
        "funding" => DocumentKind::Funding,
        "mark" => DocumentKind::Mark,
        _ => return None,
    })
}

pub fn currency_from_code(s: &str) -> Option<Currency> {
    Some(match s {
        "USDC" => Currency::Usdc,
        "EURC" => Currency::Eurc,
        "USD" => Currency::Usd,
        "EUR" => Currency::Eur,
        _ => return None,
    })
}
