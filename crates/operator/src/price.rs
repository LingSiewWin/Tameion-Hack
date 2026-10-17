//! The price table: what one token of each model this service calls costs in
//! USD, from two public tables (docs/117 §7, docs/121).
//!
//! The only place USD per token enters the service. Prices are parsed from
//! JSON into plain decimal strings; text a model wrote never reaches one. A
//! table is pinned by the keccak256 of its rows, so a reservation priced at
//! one table can be re-priced by anyone later from the saved rows, and a
//! later price change never re-prices it. For reserves the higher of the two
//! tables wins (a reservation is never priced under either source); a capture
//! is priced at the actual.
//!
//! `TAMEION_PRICES`: unset or `off` pins nothing; `fixture` pins the offline
//! table below; anything else (`live`) fetches LiteLLM and OpenRouter at most
//! hourly, cached under `<app_dir>/price/`.

use alloy::primitives::keccak256;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";

/// Per fetch, per source (docs/117 §7).
const TIMEOUT: Duration = Duration::from_secs(12);

/// How long a source's rows, or its failure, stand before the next fetch.
// ponytail: one hour for both sources; per-source cadence if one moves faster.
const TTL_SECS: i64 = 3_600;

/// USD per single token, keyed `cache_read`, `cache_write`, `tok_in`, `tok_out`.
pub type Row = BTreeMap<String, String>;

/// The models this service calls: the ordering reasoner and the decider.
pub fn models() -> [&'static str; 2] {
    [crate::claude::MODEL, crate::decide::MODEL]
}

#[derive(Debug, Clone, PartialEq)]
pub struct PriceTable {
    /// `litellm`, `openrouter`, `litellm+openrouter` or `fixture`.
    pub source: String,
    /// Unix seconds; for a merged table, its oldest source's fetch.
    pub fetched_at: i64,
    pub rows: BTreeMap<String, Row>,
    /// `0x…` keccak256 of the rows (see [`hash_rows`]).
    pub hash: String,
}

impl PriceTable {
    pub fn new(source: &str, fetched_at: i64, rows: BTreeMap<String, Row>) -> Self {
        let hash = hash_rows(&rows);
        PriceTable {
            source: source.to_string(),
            fetched_at,
            rows,
            hash,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "source": self.source, "fetchedAt": self.fetched_at, "rows": self.rows, "hash": self.hash })
    }

    /// The record's event for this table (`PRICETABLE`, tag 11).
    pub fn event(&self) -> ledger::EventBody {
        ledger::EventBody::PriceTable {
            source: self.source.clone(),
            fetched_at: self.fetched_at,
            table_hash: self.hash.clone(),
            rows: u32::try_from(self.rows.len()).unwrap_or(u32::MAX),
            note: self.note(),
        }
    }

    /// The sentence the record carries: the models and their prices, no hash.
    pub fn note(&self) -> String {
        let rows: Vec<String> = self
            .rows
            .iter()
            .map(|(m, r)| {
                let p = |k: &str| r.get(k).map_or("?", String::as_str);
                format!(
                    "{m} in {} / out {} USD per token",
                    p("tok_in"),
                    p("tok_out")
                )
            })
            .collect();
        format!(
            "Prices pinned from {} for {} models: {}. Reservations are priced at this table until it changes.",
            self.source,
            self.rows.len(),
            rows.join("; ")
        )
    }
}

/// keccak256 of the rows as canonical JSON: keys sorted, no whitespace, each
/// price the decimal string it is. Rows only, so two fetches of the same
/// prices give the same hash. `scripts/price_hash.py` recomputes it.
pub fn hash_rows(rows: &BTreeMap<String, Row>) -> String {
    let canonical = serde_json::to_string(rows).expect("string maps serialise");
    format!("{:#x}", keccak256(canonical.as_bytes()))
}

/// A price as a plain decimal string ("2e-7" and "0.0000002" are one price
/// and must hash alike; `f64`'s `Display` never writes an exponent). `None`
/// for anything not a finite number at or above zero: OpenRouter's "-1"
/// marks a router with no fixed price.
pub fn dec(v: &Value) -> Option<String> {
    let f = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (f.is_finite() && f >= 0.0).then(|| format!("{f}"))
}

fn row(prices: &Value, keys: [(&str, &str); 4]) -> Option<Row> {
    let r: Row = keys
        .iter()
        .filter_map(|(field, ours)| Some((ours.to_string(), dec(&prices[*field])?)))
        .collect();
    (!r.is_empty()).then_some(r)
}

/// LiteLLM's rows for `models`: the model's own key (the provider's price,
/// not a reseller's), chat models only.
pub fn from_litellm(json: &Value, models: &[&str]) -> BTreeMap<String, Row> {
    models
        .iter()
        .filter(|m| json[**m]["mode"] == "chat")
        .filter_map(|m| {
            let keys = [
                ("input_cost_per_token", "tok_in"),
                ("output_cost_per_token", "tok_out"),
                ("cache_read_input_token_cost", "cache_read"),
                ("cache_creation_input_token_cost", "cache_write"),
            ];
            Some((m.to_string(), row(&json[*m], keys)?))
        })
        .collect()
}

/// OpenRouter's rows for `models`: `anthropic/claude-sonnet-5.5` is
/// `claude-sonnet-5-5`; variants after a `:` (`:batch`, `:free`) are other
/// prices and are skipped.
pub fn from_openrouter(json: &Value, models: &[&str]) -> BTreeMap<String, Row> {
    let norm = |id: &str| {
        let id = id.to_ascii_lowercase();
        id.strip_prefix("anthropic/")
            .unwrap_or(&id)
            .replace(['.', '_'], "-")
    };
    let mut out = BTreeMap::new();
    for m in json["data"].as_array().into_iter().flatten() {
        let Some(id) = m["id"].as_str().filter(|id| !id.contains(':')) else {
            continue;
        };
        let keys = [
            ("prompt", "tok_in"),
            ("completion", "tok_out"),
            ("input_cache_read", "cache_read"),
            ("input_cache_write", "cache_write"),
        ];
        if let Some(ours) = models.iter().find(|w| norm(id) == w.to_ascii_lowercase())
            && let Some(r) = row(&m["pricing"], keys)
        {
            out.insert(ours.to_string(), r);
        }
    }
    out
}

/// For reserves: each price at the higher of the tables. `None` for no tables.
pub fn merge(tables: &[PriceTable]) -> Option<PriceTable> {
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    for t in tables {
        for (model, r) in &t.rows {
            let cur = rows.entry(model.clone()).or_default();
            for (k, v) in r {
                let higher = |old: &String| {
                    v.parse::<f64>().unwrap_or(0.0) > old.parse::<f64>().unwrap_or(0.0)
                };
                if cur.get(k).is_none_or(higher) {
                    cur.insert(k.clone(), v.clone());
                }
            }
        }
    }
    let source: Vec<&str> = tables.iter().map(|t| t.source.as_str()).collect();
    let fetched_at = tables.iter().map(|t| t.fetched_at).min()?;
    Some(PriceTable::new(&source.join("+"), fetched_at, rows))
}

/// Offline, for tests and `TAMEION_PRICES=fixture`. Illustrative, not quotes.
pub fn fixture() -> PriceTable {
    let r =
        |i: &str, o: &str| Row::from([("tok_in".into(), i.into()), ("tok_out".into(), o.into())]);
    let [opus, sonnet] = models();
    PriceTable::new(
        "fixture",
        0,
        BTreeMap::from([
            (opus.to_string(), r("0.00001", "0.00005")),
            (sonnet.to_string(), r("0.000003", "0.000015")),
        ]),
    )
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn save(path: &Path, v: &Value) {
    let tmp = path.with_extension("json.tmp");
    let ok = path
        .parent()
        .is_some_and(|d| std::fs::create_dir_all(d).is_ok())
        && std::fs::write(&tmp, format!("{v:#}\n")).is_ok();
    if ok {
        let _ = std::fs::rename(&tmp, path);
    }
}

async fn fetch(url: &str) -> Result<Value, String> {
    let c = reqwest::Client::builder()
        .user_agent(concat!("tameion/", env!("CARGO_PKG_VERSION")))
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = c.get(url).send().await.map_err(|e| e.to_string())?;
    let resp = resp.error_for_status().map_err(|e| e.to_string())?;
    resp.json::<Value>().await.map_err(|e| e.to_string())
}

/// One source's rows: from `<dir>/<name>.json` while it is under an hour old,
/// else fetched; a failed fetch keeps the last rows and waits an hour too.
async fn source(dir: &Path, name: &str, now: i64) -> Option<PriceTable> {
    let path = dir.join(format!("{name}.json"));
    let mut cached = read(&path);
    let tried = cached.as_ref().and_then(|c| c["triedAt"].as_i64());
    if tried.is_none_or(|t| now - t >= TTL_SECS) {
        let url = if name == "litellm" {
            LITELLM_URL
        } else {
            OPENROUTER_URL
        };
        let fresh = fetch(url).await.map(|v| match name {
            "litellm" => from_litellm(&v, &models()),
            _ => from_openrouter(&v, &models()),
        });
        let c = match fresh {
            Ok(rows) if !rows.is_empty() => {
                json!({ "source": name, "fetchedAt": now, "triedAt": now, "rows": rows })
            }
            other => {
                let why = other
                    .err()
                    .unwrap_or_else(|| "no row for our models".into());
                println!("price table: {name} not read ({why}); keeping its last rows");
                let mut c = cached.unwrap_or_else(|| json!({ "source": name, "rows": {} }));
                c["triedAt"] = json!(now);
                c
            }
        };
        save(&path, &c);
        cached = Some(c);
    }
    let c = cached?;
    let rows: BTreeMap<String, Row> = serde_json::from_value(c["rows"].clone()).ok()?;
    let fetched_at = c["fetchedAt"].as_i64()?;
    (!rows.is_empty()).then(|| PriceTable::new(name, fetched_at, rows))
}

/// The table to pin now (see the module docs for `TAMEION_PRICES`). Saved as
/// `<app_dir>/price/<hash>.json`, so a pinned hash can always be checked again.
pub async fn current(app_dir: &Path, now: i64) -> Option<PriceTable> {
    let dir = app_dir.join("price");
    let table = match std::env::var("TAMEION_PRICES").unwrap_or_default().as_str() {
        "" | "off" | "0" => return None,
        "fixture" => fixture(),
        _ => {
            let (l, o) = tokio::join!(
                source(&dir, "litellm", now),
                source(&dir, "openrouter", now)
            );
            merge(&[l, o].into_iter().flatten().collect::<Vec<_>>())?
        }
    };
    save(&dir.join(format!("{}.json", table.hash)), &table.to_json());
    Some(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_are_plain_decimals_and_refusals_are_none() {
        assert_eq!(dec(&json!(2e-7)).as_deref(), Some("0.0000002"));
        assert_eq!(dec(&json!("2e-7")).as_deref(), Some("0.0000002"));
        assert_eq!(dec(&json!("0.0000002")).as_deref(), Some("0.0000002"));
        assert_eq!(dec(&json!(6.25e-6)).as_deref(), Some("0.00000625"));
        assert_eq!(dec(&json!(0)).as_deref(), Some("0"));
        assert_eq!(dec(&json!("-1")), None);
        assert_eq!(dec(&json!("free")), None);
        assert_eq!(dec(&Value::Null), None);
    }

    #[test]
    fn both_sources_parse_merge_high_and_hash_the_rows_only() {
        let models = ["claude-opus-5", "claude-sonnet-5-5"];
        let litellm = json!({
            "claude-opus-5": { "mode": "chat", "input_cost_per_token": 5e-06, "output_cost_per_token": 2.5e-05,
                               "cache_read_input_token_cost": 5e-07, "cache_creation_input_token_cost": 6.25e-06 },
            // A reseller's key for the same model is not ours.
            "us.anthropic.claude-sonnet-5-5": { "mode": "chat", "input_cost_per_token": 9e-06 },
            "claude-sonnet-5-5": { "mode": "embedding", "input_cost_per_token": 1e-06 },
        });
        let openrouter = json!({ "data": [
            { "id": "anthropic/claude-opus-5", "pricing": { "prompt": "0.000006", "completion": "0.000025" } },
            { "id": "anthropic/claude-sonnet-5.5", "pricing": { "prompt": "0.000002", "completion": "0.00001",
              "input_cache_read": "0.0000001", "input_cache_write": "0.0000025" } },
            { "id": "anthropic/claude-sonnet-5.5:batch", "pricing": { "prompt": "0.000009" } },
            { "id": "openrouter/auto", "pricing": { "prompt": "-1", "completion": "-1" } },
        ]});
        let l = PriceTable::new("litellm", 100, from_litellm(&litellm, &models));
        let o = PriceTable::new("openrouter", 90, from_openrouter(&openrouter, &models));
        assert_eq!(l.rows.len(), 1, "{l:?}");
        assert_eq!(o.rows["claude-sonnet-5-5"]["tok_in"], "0.000002");
        let m = merge(&[l.clone(), o]).unwrap();
        assert_eq!(
            (m.source.as_str(), m.fetched_at),
            ("litellm+openrouter", 90)
        );
        // The higher of the two: OpenRouter's input, LiteLLM's cache prices.
        assert_eq!(m.rows["claude-opus-5"]["tok_in"], "0.000006");
        assert_eq!(m.rows["claude-opus-5"]["cache_write"], "0.00000625");
        assert_eq!(m.rows["claude-sonnet-5-5"]["tok_out"], "0.00001");
        // Rows only: the same prices fetched later hash alike.
        assert_eq!(PriceTable::new("x", 1, l.rows.clone()).hash, l.hash);
        assert_eq!(merge(&[]), None);
    }

    /// The same bytes `scripts/price_hash.py` hashes (recomputed there).
    #[test]
    fn the_fixture_hash_is_pinned() {
        let f = fixture();
        assert_eq!(
            serde_json::to_string(&f.rows).unwrap(),
            r#"{"claude-opus-5":{"tok_in":"0.00001","tok_out":"0.00005"},"claude-sonnet-5-5":{"tok_in":"0.000003","tok_out":"0.000015"}}"#
        );
        assert_eq!(
            f.hash,
            "0x73a6e3924487807e9a17b6b0842ae31acc6d058e0f7a37701ec36b5c3a7947d1"
        );
    }
}
