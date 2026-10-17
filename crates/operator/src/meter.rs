//! Two-sided metering (docs/117 §6): our count of Claude tokens against Anthropic's.
//!
//! Every successful Messages reply's `usage` becomes one line of `<app_dir>/meter.jsonl`
//! ([`Meter::record`], called from `ClaudeReasoner::ask_structured` for the decider and the
//! ordering reasoner). [`reconcile`] compares those lines with Anthropic's usage report
//! (`GET /v1/organizations/usage_report/messages`, an Admin API call) in the report's own
//! buckets: a bucket where the two sides disagree is a check with a reason class, never
//! "close enough". The witness is Anthropic's report; nothing here can make it agree.
//!
//! Shapes come from the reference pages, not from memory: `api/messages/create` (the total
//! input is `input_tokens` + `cache_creation_input_tokens` + `cache_read_input_tokens`),
//! `api/errors` (the `request-id` header), `admin-api/usage-cost/get-messages-usage-report`
//! and `admin-api/apikeys/list-api-keys` (`partial_key_hint`), on platform.claude.com/docs.
//!
//! What a line never holds: the reply's words, and the credential (only its first
//! [`HINT_LEN`] characters).
//!
//! A bucket and model where both sides agree is a witness (founder's ruling, docs/117
//! addendum of 2026-10-17): [`witnesses`] gives each decision the model made in it the
//! slot `provider-report` / `anthropic:usage_report` / SHA-256 of the bucket's JSON.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How much of the API key a line keeps. Anthropic's own `partial_key_hint`
/// (`sk-ant-api03-R2D...igAA`) shows this many characters before the dots, so a line can be
/// matched to its key's id in the Admin API's key list; the first 12 would read
/// `sk-ant-api03` for every key.
pub const HINT_LEN: usize = 16;

/// One Claude reply, as the meter keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// Unix seconds when the reply arrived.
    pub at: i64,
    /// The model the reply names (the requested one when it names none).
    pub model: String,
    pub key_hint: String,
    pub uncached_in: u64,
    pub cache_read: u64,
    pub cache_create: u64,
    pub out: u64,
    pub request_id: Option<String>,
    /// Which caller spent it: `decide` or `order`.
    pub purpose: String,
    /// The pass the call was made in, `<account>@<pass time>`: the time every event of that
    /// pass carries, so a decision finds its call ([`witnesses`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<String>,
    /// What the caller asked about: for a decision, the payee's label (`decide::label`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

/// Where a client writes its usage, for which caller, and in which pass.
#[derive(Debug, Clone)]
pub struct Meter {
    pub path: PathBuf,
    pub purpose: &'static str,
    pub pass: Option<String>,
}

impl Meter {
    /// One line for one successful reply. Never fails the call it meters: a line that could
    /// not be written is what [`reconcile`] reports as "ours missing".
    pub fn record(
        &self,
        raw: &str,
        requested_model: &str,
        key_hint: String,
        request_id: Option<String>,
        subject: Option<&str>,
    ) {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let Some(usage) = line_from_reply(raw, requested_model, at) else {
            return;
        };
        let line = Line {
            key_hint,
            request_id,
            purpose: self.purpose.to_string(),
            pass: self.pass.clone(),
            subject: subject.map(str::to_string),
            ..usage
        };
        if let Err(e) = append(&self.path, &line) {
            eprintln!("meter: {}: {e}", self.path.display());
        }
    }
}

/// A reply's `usage` and the model it names, as a line whose caller fields are empty;
/// `None` when the reply carries no `usage` (a stub or a proxy), so nothing is invented.
pub fn line_from_reply(raw: &str, requested_model: &str, at: i64) -> Option<Line> {
    #[derive(Deserialize)]
    struct Reply {
        model: Option<String>,
        usage: Option<Usage>,
    }
    #[derive(Deserialize)]
    struct Usage {
        #[serde(default)]
        input_tokens: u64,
        #[serde(default)]
        output_tokens: u64,
        // "number or null" in the reference.
        cache_creation_input_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
    }
    let r: Reply = serde_json::from_str(raw).ok()?;
    let u = r.usage?;
    Some(Line {
        at,
        model: r
            .model
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| requested_model.to_string()),
        key_hint: String::new(),
        uncached_in: u.input_tokens,
        cache_read: u.cache_read_input_tokens.unwrap_or(0),
        cache_create: u.cache_creation_input_tokens.unwrap_or(0),
        out: u.output_tokens,
        request_id: None,
        purpose: String::new(),
        pass: None,
        subject: None,
    })
}

/// Append one line, mode 600 when the file is new. One `write` per line, so passes writing
/// at once (O_APPEND) never interleave inside a line.
// ponytail: one file that only grows (~250 bytes a reply); rotate by day when it matters.
pub fn append(path: &Path, line: &Line) -> io::Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let mut text = serde_json::to_string(line)?;
    text.push('\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?
        .write_all(text.as_bytes())
}

/// Every line of a meter file; none when there is no file yet. A line that is not a meter
/// line is an error naming it: a reconcile over a damaged file would blame Anthropic.
pub fn read(path: &Path) -> io::Result<Vec<Line>> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: line {}: {e}", path.display(), i + 1),
                )
            })
        })
        .collect()
}

// --- Anthropic's side --------------------------------------------------------------------

/// One page of the usage report.
#[derive(Debug, Deserialize)]
pub struct Report {
    pub data: Vec<Bucket>,
    #[serde(default)]
    pub has_more: bool,
    pub next_page: Option<String>,
}

/// One time bucket, `[starting_at, ending_at)` in RFC 3339.
#[derive(Debug, Clone)]
pub struct Bucket {
    pub starting_at: String,
    pub ending_at: String,
    pub results: Vec<Row>,
    /// SHA-256 of the bucket's JSON as Anthropic returned it, in `serde_json`'s form (keys
    /// sorted, no whitespace): what a witness slot cites. Recomputable from a fresh read of
    /// the same minute with the same grouping.
    pub sha256: [u8; 32],
}

impl<'de> Deserialize<'de> for Bucket {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        use sha2::Digest;
        #[derive(Deserialize)]
        struct Shape {
            starting_at: String,
            ending_at: String,
            #[serde(default)]
            results: Vec<Row>,
        }
        let v = serde_json::Value::deserialize(d)?;
        let bytes = serde_json::to_vec(&v).map_err(D::Error::custom)?;
        let s: Shape = serde_json::from_value(v).map_err(D::Error::custom)?;
        Ok(Bucket {
            starting_at: s.starting_at,
            ending_at: s.ending_at,
            results: s.results,
            sha256: sha2::Sha256::digest(bytes).into(),
        })
    }
}

/// One row of a bucket: one (key, model) group when grouped by both; `None` where the
/// report is not grouped by that field.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Row {
    pub api_key_id: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub uncached_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation: CacheCreation,
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct CacheCreation {
    #[serde(default)]
    pub ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    pub ephemeral_1h_input_tokens: u64,
}

/// One page of the Admin API's key list; only what matching a line to a key needs.
#[derive(Debug, Deserialize)]
pub struct Keys {
    pub data: Vec<Key>,
    #[serde(default)]
    pub has_more: bool,
    pub last_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Key {
    pub id: String,
    pub partial_key_hint: Option<String>,
}

/// The report for `[from, to)` (unix seconds) in one-minute buckets, by key and model.
pub fn report_url(
    base: &str,
    from: i64,
    to: i64,
    page: Option<&str>,
) -> Result<reqwest::Url, String> {
    let mut u = reqwest::Url::parse(&format!("{base}/v1/organizations/usage_report/messages"))
        .map_err(|e| format!("{base}: {e}"))?;
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("starting_at", &crate::statement::iso8601(from))
            .append_pair("ending_at", &crate::statement::iso8601(to))
            .append_pair("bucket_width", "1m")
            .append_pair("group_by[]", "api_key_id")
            .append_pair("group_by[]", "model")
            // The whole window on one page, up to the 1m maximum of 1,440 buckets.
            .append_pair("limit", &((to - from) / 60).clamp(1, 1440).to_string());
        if let Some(p) = page {
            q.append_pair("page", p);
        }
    }
    Ok(u)
}

/// Every bucket of `[from, to)`, following `next_page`.
pub async fn fetch_report(
    http: &reqwest::Client,
    base: &str,
    admin_key: &str,
    from: i64,
    to: i64,
) -> Result<Vec<Bucket>, String> {
    let mut out = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let r: Report = get(
            http,
            report_url(base, from, to, page.as_deref())?,
            admin_key,
        )
        .await?;
        out.extend(r.data);
        match r.next_page {
            Some(p) if r.has_more => page = Some(p),
            _ => return Ok(out),
        }
    }
    Err(format!("the usage report ran past {MAX_PAGES} pages"))
}

/// Every key of the organisation (id and hint), following `last_id`.
pub async fn fetch_keys(
    http: &reqwest::Client,
    base: &str,
    admin_key: &str,
) -> Result<Vec<Key>, String> {
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut u = reqwest::Url::parse(&format!("{base}/v1/organizations/api_keys"))
            .map_err(|e| format!("{base}: {e}"))?;
        u.query_pairs_mut().append_pair("limit", "1000");
        if let Some(a) = &after {
            u.query_pairs_mut().append_pair("after_id", a);
        }
        let k: Keys = get(http, u, admin_key).await?;
        out.extend(k.data);
        match k.last_id {
            Some(l) if k.has_more => after = Some(l),
            _ => return Ok(out),
        }
    }
    Err(format!("the key list ran past {MAX_PAGES} pages"))
}

/// A reply that keeps saying "more" cannot keep the job running.
const MAX_PAGES: usize = 100;

async fn get<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: reqwest::Url,
    admin_key: &str,
) -> Result<T, String> {
    let path = url.path().to_string();
    let resp = http
        .get(url)
        .header("x-api-key", admin_key)
        .header("anthropic-version", crate::claude::API_VERSION)
        .send()
        .await
        .map_err(|e| format!("{path}: {e}"))?;
    let status = resp.status();
    let raw = resp.text().await.map_err(|e| format!("{path}: {e}"))?;
    if !status.is_success() {
        // The API's own explanation: a Messages key is refused here (Admin API only).
        let detail: String = raw.trim().chars().take(400).collect();
        return Err(format!("{path}: HTTP {status}: {detail}"));
    }
    serde_json::from_str(&raw).map_err(|e| format!("{path}: not the documented shape: {e}"))
}

/// `2025-08-01T00:00:00Z` (a bucket edge: UTC, whole seconds; a fraction is dropped) to
/// unix seconds; `None` for anything else, so a bucket is never guessed.
pub fn unix_from_rfc3339(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z').or_else(|| s.strip_suffix("+00:00"))?;
    let (date, time) = s.split_once('T')?;
    let parts = |text: &str, sep: char| -> Option<[u32; 3]> {
        let v: Vec<u32> = text
            .split(sep)
            .map(|p| p.parse().ok())
            .collect::<Option<_>>()?;
        v.try_into().ok()
    };
    let [y, m, d] = parts(date, '-')?;
    let [h, mi, sec] = parts(time.split('.').next()?, ':')?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let days = crate::statement::days_from_civil(i64::from(y), m, d);
    Some(days * 86_400 + i64::from(h * 3_600 + mi * 60 + sec))
}

// --- the comparison ----------------------------------------------------------------------

/// One bucket and model, compared, or the summary of a run: the fields of the log's
/// `MeterCheck` plus what the witness join needs. `*_in` counts every input token
/// (uncached, cache read, cache write); the note names the class and the split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub from: i64,
    pub to: i64,
    pub model: String,
    pub ours_in: u64,
    pub ours_out: u64,
    pub theirs_in: u64,
    pub theirs_out: u64,
    pub note: String,
    /// [`MATCH`], a difference class, or [`SUMMARY`].
    pub class: &'static str,
    /// The bucket's [`Bucket::sha256`]; `None` on the summary.
    pub bucket: Option<[u8; 32]>,
}

pub const MATCH: &str = "match";
pub const SUMMARY: &str = "summary";
pub const OURS_MISSING: &str = "ours missing";
pub const THEIRS_MISSING: &str = "theirs missing";
pub const CACHE_CLASS: &str = "cache class mismatch";
pub const CLOCK_SKEW: &str = "clock skew ±1 min";
pub const CLOCK_SKEW_BUCKET: &str = "clock skew ±1 bucket";
pub const COUNT_DIFFERS: &str = "count differs";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tally {
    uncached: u64,
    read: u64,
    create: u64,
    out: u64,
}

impl Tally {
    fn input(self) -> u64 {
        self.uncached + self.read + self.create
    }

    fn add(&mut self, o: Tally) {
        self.uncached += o.uncached;
        self.read += o.read;
        self.create += o.create;
        self.out += o.out;
    }
}

impl fmt::Display for Tally {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} uncached + {} cache read + {} cache write in, {} out",
            self.uncached, self.read, self.create, self.out
        )
    }
}

/// Our lines against the report, in the report's buckets: one [`Check`] per bucket and
/// model with usage on either side, [`MATCH`] where both sides agree to the token (totals
/// and cache split), a difference class where they do not; then one summary.
///
/// Classes, first match wins: the opposite difference in the neighbouring bucket for the
/// same model (clock skew: a reply at 12:00:59 on our clock, 12:01:00 on theirs); nothing on
/// our side (capture off, another host or process on the key, a call we timed out on that
/// was still billed); nothing on theirs (report lag, or a reply not billed to this key);
/// equal totals split differently (cache class mismatch); otherwise the counts differ.
pub fn reconcile(ours: &[Line], buckets: &[Bucket], keys: &[Key]) -> Result<Vec<Check>, String> {
    let edges = buckets
        .iter()
        .map(|b| {
            match (
                unix_from_rfc3339(&b.starting_at),
                unix_from_rfc3339(&b.ending_at),
            ) {
                (Some(f), Some(t)) if f < t => Ok((f, t)),
                _ => Err(format!(
                    "bucket {}..{} is not two RFC 3339 times in order",
                    b.starting_at, b.ending_at
                )),
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    let (Some(&(start, _)), Some(&(_, end))) = (edges.first(), edges.last()) else {
        return Err("the report holds no buckets".into());
    };
    let (scope, ids) = scope(ours, keys);

    // (bucket, model) -> (ours, theirs)
    let mut pairs: BTreeMap<(usize, String), (Tally, Tally)> = BTreeMap::new();
    let mut outside = 0;
    for l in ours {
        let t = Tally {
            uncached: l.uncached_in,
            read: l.cache_read,
            create: l.cache_create,
            out: l.out,
        };
        match edges.iter().position(|&(f, to)| f <= l.at && l.at < to) {
            Some(i) => pairs.entry((i, l.model.clone())).or_default().0.add(t),
            None => outside += 1,
        }
    }
    for (i, b) in buckets.iter().enumerate() {
        for r in &b.results {
            let in_scope = ids
                .as_ref()
                .is_none_or(|ids| r.api_key_id.as_deref().is_some_and(|k| ids.contains(k)));
            if !in_scope {
                continue;
            }
            let t = Tally {
                uncached: r.uncached_input_tokens,
                read: r.cache_read_input_tokens,
                create: r.cache_creation.ephemeral_5m_input_tokens
                    + r.cache_creation.ephemeral_1h_input_tokens,
                out: r.output_tokens,
            };
            pairs
                .entry((i, r.model.clone().unwrap_or_default()))
                .or_default()
                .1
                .add(t);
        }
    }

    let mut checks = Vec::new();
    let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut ours_all, mut theirs_all) = (Tally::default(), Tally::default());
    for ((i, model), (o, t)) in &pairs {
        ours_all.add(*o);
        theirs_all.add(*t);
        if o == t && *o == Tally::default() {
            continue; // a row of zeros: nothing to compare
        }
        let (from, to) = edges[*i];
        let skewed = o != t
            && [i.checked_sub(1), Some(i + 1)]
                .into_iter()
                .flatten()
                .any(|j| {
                    pairs.get(&(j, model.clone())).is_some_and(|(o2, t2)| {
                        o2 != t2
                            && o.input() + o2.input() == t.input() + t2.input()
                            && o.out + o2.out == t.out + t2.out
                    })
                });
        let class = if o == t {
            MATCH
        } else if skewed && to - from == 60 {
            CLOCK_SKEW
        } else if skewed {
            CLOCK_SKEW_BUCKET
        } else if *o == Tally::default() {
            OURS_MISSING
        } else if *t == Tally::default() {
            THEIRS_MISSING
        } else if o.input() == t.input() && o.out == t.out {
            CACHE_CLASS
        } else {
            COUNT_DIFFERS
        };
        *classes.entry(class).or_default() += 1;
        let note = if class == MATCH {
            format!("{MATCH}. {o} on both sides; scope: {scope}")
        } else {
            format!("{class}. ours: {o}; theirs: {t}; scope: {scope}")
        };
        checks.push(Check {
            from,
            to,
            model: model.clone(),
            ours_in: o.input(),
            ours_out: o.out,
            theirs_in: t.input(),
            theirs_out: t.out,
            note,
            class,
            bucket: Some(buckets[*i].sha256),
        });
    }
    let matched = classes.remove(MATCH).unwrap_or(0);
    let differ: usize = classes.values().sum();
    let listed: Vec<String> = classes.iter().map(|(c, n)| format!("{c} {n}")).collect();
    let listed = if listed.is_empty() {
        String::new()
    } else {
        format!(" ({})", listed.join(", "))
    };
    checks.push(Check {
        from: start,
        to: end,
        model: "*".into(),
        ours_in: ours_all.input(),
        ours_out: ours_all.out,
        theirs_in: theirs_all.input(),
        theirs_out: theirs_all.out,
        note: format!(
            "{SUMMARY}: {differ} of {} bucket-model pairs differ{listed}, {matched} match; \
             {outside} of our {} lines outside the report's {} buckets; scope: {scope}",
            differ + matched,
            ours.len(),
            buckets.len()
        ),
        class: SUMMARY,
        bucket: None,
    });
    Ok(checks)
}

/// Which report rows are ours: those of the keys our lines name, when every hint they carry
/// is found exactly once in the key list. Otherwise (an OAuth token, no key list, a key the
/// list does not hold) the comparison is organisation-wide and the note says so: another
/// host or person on any key of the organisation then counts on their side.
fn scope<'k>(ours: &[Line], keys: &'k [Key]) -> (String, Option<BTreeSet<&'k str>>) {
    let hints: BTreeSet<&str> = ours.iter().map(|l| l.key_hint.as_str()).collect();
    let named_once = |h: &str| {
        let mut found = keys.iter().filter(|k| {
            h.len() >= HINT_LEN
                && k.partial_key_hint
                    .as_deref()
                    .is_some_and(|p| p.starts_with(h))
        });
        match (found.next(), found.next()) {
            (Some(k), None) => Some(k.id.as_str()),
            _ => None,
        }
    };
    let ids: Option<BTreeSet<&str>> = if hints.is_empty() {
        None
    } else {
        hints.iter().map(|h| named_once(h)).collect()
    };
    match ids {
        Some(ids) => {
            let named: Vec<&str> = ids.iter().copied().collect();
            (format!("key {}", named.join(", ")), Some(ids))
        }
        None => (
            "organisation-wide (our key is not named once in the key list)".into(),
            None,
        ),
    }
}

// --- the witness slot --------------------------------------------------------------------

/// `ledger::symbolon::SlotSig`'s fields, until that type is on main (seat S20s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Witness {
    pub alg: String,
    pub key_id: String,
    pub sig: Vec<u8>,
    pub at: i64,
}

pub const WITNESS_ALG: &str = "provider-report";
pub const WITNESS_KEY: &str = "anthropic:usage_report";

/// The witness slots one reconcile gives one account's record, as `(event seq, slot)`:
/// a decision the model made gets `provider-report` / `anthropic:usage_report` /
/// SHA-256 of the bucket's JSON, `at` = `read_at` (when the report was read), when the one
/// metered call behind it sits in a bucket and model that matched to the token.
///
/// The join, exactly. A `Decision` event whose line reads "Decided by model <m>" belongs to
/// the pass `<account>@<event time>` (every event of a pass carries the pass's time, and
/// the pass tags each of its meter lines with it, `level1::run`); its call is the one meter
/// line with that pass, purpose `decide`, model <m>, and subject = the payee's label
/// (`decide::label` of the event's counterparty, what the decider was asked about); its
/// minute is the bucket holding that line's `at` (the reply's time, not the pass's).
/// No line (answered from the decider's memory, or not metered), two lines (the same payee
/// twice in one pass), or a bucket that differs: no witness.
pub fn witnesses(
    events: &[ledger::Event],
    account: &str,
    lines: &[Line],
    checks: &[Check],
    read_at: i64,
) -> Vec<(u64, Witness)> {
    let mut out = Vec::new();
    for e in events {
        let ledger::EventBody::Decision {
            counterparty, note, ..
        } = &e.body
        else {
            continue;
        };
        let Some(model) = note
            .split("Decided by model ")
            .nth(1)
            .and_then(|rest| rest.split(':').next())
        else {
            continue;
        };
        let pass = format!("{account}@{}", e.at.as_unix());
        let subject = crate::decide::label(counterparty.as_str());
        let mut calls = lines.iter().filter(|l| {
            l.purpose == "decide"
                && l.model == model
                && l.pass.as_deref() == Some(pass.as_str())
                && l.subject.as_deref() == Some(subject.as_str())
        });
        let (Some(call), None) = (calls.next(), calls.next()) else {
            continue;
        };
        let bucket = checks
            .iter()
            .find(|c| c.class == MATCH && c.model == model && c.from <= call.at && call.at < c.to);
        if let Some(sig) = bucket.and_then(|c| c.bucket) {
            out.push((
                e.seq,
                Witness {
                    alg: WITNESS_ALG.into(),
                    key_id: WITNESS_KEY.into(),
                    sig: sig.to_vec(),
                    at: read_at,
                },
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ledger::{
        ActionHash, Confidence, Counterparty, Currency, DecisionLog, EventBody, Money, Timestamp,
    };

    const REPORT: &str = include_str!("../tests/fixtures/anthropic-usage-report.json");
    const KEYS: &str = include_str!("../tests/fixtures/anthropic-api-keys.json");
    const SYNTHETIC: &str = include_str!("../tests/fixtures/meter-synthetic.jsonl");
    /// The docs' example key id, whose hint is `sk-ant-api03-R2D...igAA`.
    const DOCS_KEY: &str = "apikey_01Rj2N8SVvo6BePZj99NhmiT";
    const HINT: &str = "sk-ant-api03-R2D";
    /// 2025-08-01T10:00:05Z, inside the docs' one bucket.
    const T: i64 = 1_754_042_405;
    /// SHA-256 of the docs' bucket, keys sorted and no whitespace, computed outside Rust:
    /// `json.dumps(bucket, sort_keys=True, separators=(',', ':'))` in Python.
    const DOCS_BUCKET_SHA256: &str =
        "7985dca6f706d55e759d0a6f92f8118cdfdab09f22e75014392f350f283c6165";

    fn report() -> Vec<Bucket> {
        serde_json::from_str::<Report>(REPORT).unwrap().data
    }

    fn keys() -> Vec<Key> {
        serde_json::from_str::<Keys>(KEYS).unwrap().data
    }

    fn line(at: i64, model: &str, uncached: u64, read: u64, create: u64, out: u64) -> Line {
        Line {
            at,
            model: model.into(),
            key_hint: HINT.into(),
            uncached_in: uncached,
            cache_read: read,
            cache_create: create,
            out,
            request_id: None,
            purpose: "decide".into(),
            pass: None,
            subject: None,
        }
    }

    fn classes(checks: &[Check]) -> Vec<&'static str> {
        checks.iter().map(|c| c.class).collect()
    }

    #[test]
    fn reads_the_documented_report_and_key_list() {
        let r: Report = serde_json::from_str(REPORT).unwrap();
        assert!(r.has_more);
        assert_eq!(
            r.next_page.as_deref(),
            Some("page_MjAyNS0wNS0xNFQwMDowMDowMFo=")
        );
        let row = &r.data[0].results[0];
        assert_eq!(row.api_key_id.as_deref(), Some(DOCS_KEY));
        assert_eq!(row.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(
            (
                row.uncached_input_tokens,
                row.cache_read_input_tokens,
                row.output_tokens
            ),
            (1500, 200, 500)
        );
        let k = keys();
        assert_eq!(k[0].id, DOCS_KEY);
        assert!(k[0].partial_key_hint.as_deref().unwrap().starts_with(HINT));
    }

    #[test]
    fn a_bucket_hash_is_its_json_with_keys_sorted_whatever_the_order_received() {
        let docs = &report()[0];
        assert_eq!(alloy::hex::encode(docs.sha256), DOCS_BUCKET_SHA256);
        let a: Bucket = serde_json::from_str(
            r#"{"starting_at":"2026-10-17T12:00:00Z","ending_at":"2026-10-17T12:01:00Z","results":[]}"#,
        )
        .unwrap();
        let b: Bucket = serde_json::from_str(
            r#"{ "results": [], "ending_at": "2026-10-17T12:01:00Z", "starting_at": "2026-10-17T12:00:00Z" }"#,
        )
        .unwrap();
        assert_eq!(a.sha256, b.sha256);
        assert_ne!(a.sha256, docs.sha256);
    }

    #[test]
    fn the_synthetic_meter_matches_opus_and_flags_the_rest() {
        // Two opus lines that sum to the docs' row, a sonnet line the report does not have,
        // and a line after the bucket.
        let ours: Vec<Line> = SYNTHETIC
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let checks = reconcile(&ours, &report(), &keys()).unwrap();
        assert_eq!(classes(&checks), [MATCH, THEIRS_MISSING, SUMMARY]);
        let m = &checks[0];
        assert_eq!(m.model, "claude-opus-5-5");
        assert_eq!(
            (m.ours_in, m.ours_out, m.theirs_in, m.theirs_out),
            (1700, 500, 1700, 500)
        );
        assert_eq!(alloy::hex::encode(m.bucket.unwrap()), DOCS_BUCKET_SHA256);
        let c = &checks[1];
        assert_eq!(c.model, "claude-sonnet-5-5");
        assert_eq!(
            (c.ours_in, c.ours_out, c.theirs_in, c.theirs_out),
            (900, 100, 0, 0)
        );
        assert!(
            c.note.contains(&format!("scope: key {DOCS_KEY}")),
            "{}",
            c.note
        );
        let s = &checks[2];
        assert_eq!((s.model.as_str(), s.bucket), ("*", None));
        assert_eq!((s.from, s.to), (1_754_006_400, 1_754_092_800));
        assert_eq!(
            (s.ours_in, s.ours_out, s.theirs_in, s.theirs_out),
            (2600, 600, 1700, 500)
        );
        assert!(
            s.note.starts_with(
                "summary: 1 of 2 bucket-model pairs differ (theirs missing 1), 1 match; 1 of our 4 lines outside"
            ),
            "{}",
            s.note
        );
    }

    #[test]
    fn each_class_is_named() {
        let first = |ours: &[Line]| reconcile(ours, &report(), &keys()).unwrap()[0].clone();
        // Agreement to the token, in two calls.
        let ours = [
            line(T, "claude-opus-5-5", 1000, 200, 0, 300),
            line(T + 35, "claude-opus-5-5", 500, 0, 0, 200),
        ];
        assert_eq!(first(&ours).class, MATCH);
        // Nothing of ours: capture off, or another host on the key.
        assert_eq!(first(&[]).class, OURS_MISSING);
        // Same 1,700 in and 500 out, but nothing read from the cache on our side.
        assert_eq!(
            first(&[line(T, "claude-opus-5-5", 1700, 0, 0, 500)]).class,
            CACHE_CLASS
        );
        // One of the two calls missing.
        let c = first(&[line(T, "claude-opus-5-5", 1000, 200, 0, 300)]);
        assert_eq!(c.class, COUNT_DIFFERS);
        assert!(
            c.note
                .contains("ours: 1000 uncached + 200 cache read + 0 cache write in, 300 out")
                && c.note
                    .contains("theirs: 1500 uncached + 200 cache read + 0 cache write in, 500 out"),
            "{}",
            c.note
        );
    }

    #[test]
    fn a_reply_across_a_minute_edge_is_clock_skew_not_two_errors() {
        // Two one-minute buckets: our clock put the reply in the first, theirs in the second.
        let buckets: Vec<Bucket> = serde_json::from_value(serde_json::json!([
            { "starting_at": "2026-10-17T12:00:00Z", "ending_at": "2026-10-17T12:01:00Z", "results": [] },
            { "starting_at": "2026-10-17T12:01:00Z", "ending_at": "2026-10-17T12:02:00Z", "results": [
                { "api_key_id": DOCS_KEY, "model": "claude-sonnet-5-5", "uncached_input_tokens": 1200,
                  "cache_read_input_tokens": 0, "cache_creation": { "ephemeral_5m_input_tokens": 0,
                  "ephemeral_1h_input_tokens": 0 }, "output_tokens": 80 } ] }
        ]))
        .unwrap();
        let at = unix_from_rfc3339("2026-10-17T12:00:59Z").unwrap();
        let checks = reconcile(
            &[line(at, "claude-sonnet-5-5", 1200, 0, 0, 80)],
            &buckets,
            &keys(),
        )
        .unwrap();
        assert_eq!(classes(&checks), [CLOCK_SKEW, CLOCK_SKEW, SUMMARY]);
        assert!(
            checks[2].note.contains("(clock skew ±1 min 2), 0 match"),
            "{}",
            checks[2].note
        );
    }

    #[test]
    fn another_keys_rows_count_only_when_ours_is_not_named() {
        let mut buckets = report();
        buckets[0].results.push(Row {
            api_key_id: Some("apikey_someone_else".into()),
            model: Some("claude-opus-5-5".into()),
            uncached_input_tokens: 999,
            ..Row::default()
        });
        let ours = [
            line(T, "claude-opus-5-5", 1000, 200, 0, 300),
            line(T + 35, "claude-opus-5-5", 500, 0, 0, 200),
        ];
        // Our hint names the docs' key: the other key's 999 is not ours.
        assert_eq!(
            classes(&reconcile(&ours, &buckets, &keys()).unwrap()),
            [MATCH, SUMMARY]
        );
        // No key list: organisation-wide, and the 999 shows up on their side.
        let checks = reconcile(&ours, &buckets, &[]).unwrap();
        assert_eq!(classes(&checks), [COUNT_DIFFERS, SUMMARY]);
        assert_eq!((checks[0].ours_in, checks[0].theirs_in), (1700, 2699));
        assert!(
            checks[0].note.contains("scope: organisation-wide"),
            "{}",
            checks[0].note
        );
    }

    #[test]
    fn a_matched_minute_witnesses_the_model_decisions_whose_call_it_holds() {
        // One pass at 12:00:30 on account 0xacc; its decider calls came back at 12:00:50
        // (payee A) and 12:01:10 (payee D). Minute 12:00 matches; minute 12:01 does not.
        let at = |s: &str| unix_from_rfc3339(s).unwrap();
        let pass_at = at("2026-10-17T12:00:30Z");
        let (a, b, c, d) = (
            "0xaaaa00000000000000000000000000000000000a",
            "0xbbbb00000000000000000000000000000000000b",
            "0xcccc00000000000000000000000000000000000c",
            "0xdddd00000000000000000000000000000000000d",
        );
        let mut log = DecisionLog::new();
        let usdc = Money::minor(2_000, Currency::Usdc);
        let decision = |who: &str, line: &str| EventBody::Decision {
            counterparty: Counterparty::new(who),
            amount: usdc,
            decision: ledger::Decision::Allow,
            action_hash: ActionHash::from_bytes([7u8; 32]),
            confidence: Confidence::new(0.9).unwrap(),
            balance_seen: usdc,
            weakest_evidence: None,
            evidence: vec![],
            note: format!("Allow for {who}. Basis: monthly. {line}"),
        };
        let now = Timestamp::from_unix(pass_at);
        log.append(
            now,
            EventBody::Skipped {
                counterparty: Counterparty::new(a),
                cycle_key: "k".into(),
                note: "not a decision".into(),
            },
        );
        // seq 1: a model decision whose one call sits in the matched minute -> witnessed.
        log.append(
            now,
            decision(a, "Decided by model claude-sonnet-5-5: pay (usual). Due."),
        );
        // seq 2: a model decision answered from the decider's memory: no call, no witness.
        log.append(
            now,
            decision(
                b,
                "Decided by model claude-sonnet-5-5: wait (usual). Later.",
            ),
        );
        // seq 3: the rule decided: nothing for the provider to witness.
        log.append(now, decision(c, "Decided by rule: pay (usual). Due."));
        // seq 4: a model decision whose call sits in the minute that differs.
        log.append(
            now,
            decision(d, "Decided by model claude-sonnet-5-5: pay (usual). Due."),
        );

        let pass = format!("0xacc@{pass_at}");
        let call = |who: &str, when: i64, out: u64| Line {
            pass: Some(pass.clone()),
            subject: Some(crate::decide::label(who)),
            ..line(when, "claude-sonnet-5-5", 1200, 0, 0, out)
        };
        let lines = [
            call(a, at("2026-10-17T12:00:50Z"), 60),
            call(d, at("2026-10-17T12:01:10Z"), 70),
        ];
        let row = |out: u64| {
            serde_json::json!({ "api_key_id": DOCS_KEY, "model": "claude-sonnet-5-5",
                "uncached_input_tokens": 1200, "output_tokens": out })
        };
        let buckets: Vec<Bucket> = serde_json::from_value(serde_json::json!([
            { "starting_at": "2026-10-17T12:00:00Z", "ending_at": "2026-10-17T12:01:00Z", "results": [row(60)] },
            { "starting_at": "2026-10-17T12:01:00Z", "ending_at": "2026-10-17T12:02:00Z", "results": [row(90)] }
        ]))
        .unwrap();
        let checks = reconcile(&lines, &buckets, &keys()).unwrap();
        assert_eq!(classes(&checks), [MATCH, COUNT_DIFFERS, SUMMARY]);

        let read_at = at("2026-10-17T12:30:00Z");
        let got = witnesses(log.events(), "0xacc", &lines, &checks, read_at);
        assert_eq!(
            got,
            vec![(
                1,
                Witness {
                    alg: "provider-report".into(),
                    key_id: "anthropic:usage_report".into(),
                    sig: buckets[0].sha256.to_vec(),
                    at: read_at,
                }
            )]
        );
        // Another account's record, or the same payee twice in the pass: no witness.
        assert!(witnesses(log.events(), "0xother", &lines, &checks, read_at).is_empty());
        let twice = [lines[0].clone(), lines[0].clone()];
        assert!(witnesses(log.events(), "0xacc", &twice, &checks, read_at).is_empty());
    }

    #[test]
    fn usage_comes_from_the_reply_and_nothing_is_invented() {
        // The usage block of the Messages reference's example reply.
        let raw = serde_json::json!({
            "id": "msg_013Zva2CMHLNnXjNJJKqJ2EF",
            "model": "claude-opus-5-5",
            "content": [{ "type": "text", "text": "Hi! My name is Claude." }],
            "stop_reason": "end_turn",
            "usage": {
                "cache_creation": { "ephemeral_1h_input_tokens": 0, "ephemeral_5m_input_tokens": 0 },
                "cache_creation_input_tokens": 2051, "cache_read_input_tokens": 2051,
                "inference_geo": "global", "input_tokens": 2095, "output_tokens": 503,
                "output_tokens_details": { "thinking_tokens": 0 },
                "server_tool_use": { "web_fetch_requests": 2, "web_search_requests": 0 },
                "service_tier": "standard", "speed": "standard"
            }
        })
        .to_string();
        let l = line_from_reply(&raw, "claude-sonnet-5-5", 7).unwrap();
        assert_eq!(
            (
                l.model.as_str(),
                l.uncached_in,
                l.cache_read,
                l.cache_create,
                l.out,
                l.at
            ),
            ("claude-opus-5-5", 2095, 2051, 2051, 503, 7)
        );
        assert!(
            !serde_json::to_string(&l).unwrap().contains("Claude."),
            "words never enter"
        );
        // Null cache counts are zero; a reply without usage writes nothing.
        let nulls = r#"{"usage":{"input_tokens":5,"output_tokens":1,"cache_creation_input_tokens":null,"cache_read_input_tokens":null}}"#;
        let l = line_from_reply(nulls, "m", 0).unwrap();
        assert_eq!(
            (l.model.as_str(), l.cache_read, l.cache_create),
            ("m", 0, 0)
        );
        assert!(line_from_reply(r#"{"content":[]}"#, "m", 0).is_none());
    }

    #[test]
    fn lines_append_privately_and_read_back() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("tameion-meter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("meter.jsonl");
        assert!(
            read(&path).unwrap().is_empty(),
            "no file yet is no usage yet"
        );
        let meter = Meter {
            path: path.clone(),
            purpose: "decide",
            pass: Some("0xacc@1".into()),
        };
        let raw = r#"{"model":"claude-sonnet-5-5","usage":{"input_tokens":3,"output_tokens":4}}"#;
        meter.record(
            raw,
            "x",
            HINT.into(),
            Some("req_1".into()),
            Some("0xaaaa…000a"),
        );
        meter.record(raw, "x", HINT.into(), None, None);
        let got = read(&path).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(
            (
                got[0].purpose.as_str(),
                got[0].pass.as_deref(),
                got[0].subject.as_deref(),
                got[0].request_id.as_deref()
            ),
            (
                "decide",
                Some("0xacc@1"),
                Some("0xaaaa…000a"),
                Some("req_1")
            )
        );
        assert_eq!((got[1].subject.as_deref(), got[1].uncached_in), (None, 3));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // A line from before `pass` and `subject` existed still reads.
        fs::write(&path, SYNTHETIC.lines().next().unwrap()).unwrap();
        assert_eq!(read(&path).unwrap()[0].pass, None);
        fs::write(&path, "{not a line}\n").unwrap();
        assert!(read(&path).unwrap_err().to_string().contains("line 1"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bucket_edges_parse_and_round_trip() {
        assert_eq!(
            unix_from_rfc3339("2025-08-01T00:00:00Z"),
            Some(1_754_006_400)
        );
        assert_eq!(
            unix_from_rfc3339("2025-08-01T00:00:00.427722Z"),
            Some(1_754_006_400)
        );
        assert_eq!(
            unix_from_rfc3339("2025-08-01T00:00:00+00:00"),
            Some(1_754_006_400)
        );
        for bad in [
            "2025-08-01 00:00:00",
            "2025-08-01T00:00:00+08:00",
            "2025-13-01T00:00:00Z",
            "",
        ] {
            assert_eq!(unix_from_rfc3339(bad), None, "{bad}");
        }
        for t in [0, T, 1_760_659_200, 4_102_444_799] {
            assert_eq!(unix_from_rfc3339(&crate::statement::iso8601(t)), Some(t));
        }
    }

    #[test]
    fn the_report_request_carries_the_documented_query() {
        let u = report_url(
            "https://api.anthropic.com",
            1_760_702_400,
            1_760_706_000,
            Some("page_x="),
        )
        .unwrap();
        assert_eq!(u.path(), "/v1/organizations/usage_report/messages");
        let q: Vec<(String, String)> = u.query_pairs().into_owned().collect();
        let want = [
            ("starting_at", "2025-10-17T12:00:00Z"),
            ("ending_at", "2025-10-17T13:00:00Z"),
            ("bucket_width", "1m"),
            ("group_by[]", "api_key_id"),
            ("group_by[]", "model"),
            ("limit", "60"),
            ("page", "page_x="),
        ];
        let want: Vec<(String, String)> = want
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(q, want);
    }
}
