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
}

/// Where a client writes its usage, and for which caller.
#[derive(Debug, Clone)]
pub struct Meter {
    pub path: PathBuf,
    pub purpose: &'static str,
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
    ) {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let Some(line) =
            line_from_reply(raw, requested_model, key_hint, request_id, self.purpose, at)
        else {
            return;
        };
        if let Err(e) = append(&self.path, &line) {
            eprintln!("meter: {}: {e}", self.path.display());
        }
    }
}

/// A reply's `usage` (and the model it names) as a line; `None` when the reply carries no
/// `usage` (a stub or a proxy), so nothing is invented.
pub fn line_from_reply(
    raw: &str,
    requested_model: &str,
    key_hint: String,
    request_id: Option<String>,
    purpose: &str,
    at: i64,
) -> Option<Line> {
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
        key_hint,
        uncached_in: u.input_tokens,
        cache_read: u.cache_read_input_tokens.unwrap_or(0),
        cache_create: u.cache_creation_input_tokens.unwrap_or(0),
        out: u.output_tokens,
        request_id,
        purpose: purpose.to_string(),
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
#[derive(Debug, Clone, Deserialize)]
pub struct Bucket {
    pub starting_at: String,
    pub ending_at: String,
    #[serde(default)]
    pub results: Vec<Row>,
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

/// One bucket and model where the two sides disagree, or the summary of a run: the fields
/// of the log's `MeterCheck`. `*_in` counts every input token (uncached, cache read, cache
/// write); the note names the class and the split.
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
}

pub const OURS_MISSING: &str = "ours missing";
pub const THEIRS_MISSING: &str = "theirs missing";
pub const CACHE_CLASS: &str = "cache class mismatch";
pub const CLOCK_SKEW: &str = "clock skew ±1 min";
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
/// model where anything differs (the totals or the cache split), then one summary. A report
/// that agrees with us everywhere gives the summary alone.
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
        if o == t {
            continue;
        }
        let (from, to) = edges[*i];
        let skewed = [i.checked_sub(1), Some(i + 1)]
            .into_iter()
            .flatten()
            .any(|j| {
                pairs.get(&(j, model.clone())).is_some_and(|(o2, t2)| {
                    o2 != t2
                        && o.input() + o2.input() == t.input() + t2.input()
                        && o.out + o2.out == t.out + t2.out
                })
            });
        let class = if skewed && to - from == 60 {
            CLOCK_SKEW
        } else if skewed {
            "clock skew ±1 bucket"
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
        checks.push(Check {
            from,
            to,
            model: model.clone(),
            ours_in: o.input(),
            ours_out: o.out,
            theirs_in: t.input(),
            theirs_out: t.out,
            note: format!("{class}. ours: {o}; theirs: {t}; scope: {scope}"),
        });
    }
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
            "summary: {} of {} bucket-model pairs differ{listed}; {outside} of our {} lines \
             outside the report's {} buckets; scope: {scope}",
            checks.len(),
            pairs.len(),
            ours.len(),
            buckets.len()
        ),
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

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = include_str!("../tests/fixtures/anthropic-usage-report.json");
    const KEYS: &str = include_str!("../tests/fixtures/anthropic-api-keys.json");
    const SYNTHETIC: &str = include_str!("../tests/fixtures/meter-synthetic.jsonl");
    /// The docs' example key id, whose hint is `sk-ant-api03-R2D...igAA`.
    const DOCS_KEY: &str = "apikey_01Rj2N8SVvo6BePZj99NhmiT";
    const HINT: &str = "sk-ant-api03-R2D";
    /// 2025-08-01T10:00:05Z, inside the docs' one bucket.
    const T: i64 = 1_754_042_405;

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
        }
    }

    fn only_check(checks: &[Check]) -> &Check {
        assert_eq!(checks.len(), 2, "one check and the summary: {checks:#?}");
        &checks[0]
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
    fn the_synthetic_meter_matches_opus_and_flags_the_rest() {
        // Two opus lines that sum to the docs' row, a sonnet line the report does not have,
        // and a line after the bucket.
        let ours: Vec<Line> = SYNTHETIC
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let checks = reconcile(&ours, &report(), &keys()).unwrap();
        let c = only_check(&checks);
        assert_eq!(c.model, "claude-sonnet-5-5");
        assert_eq!(
            (c.ours_in, c.ours_out, c.theirs_in, c.theirs_out),
            (900, 100, 0, 0)
        );
        assert!(c.note.starts_with(THEIRS_MISSING), "{}", c.note);
        assert!(
            c.note.contains(&format!("scope: key {DOCS_KEY}")),
            "{}",
            c.note
        );
        let s = &checks[1];
        assert_eq!(s.model, "*");
        assert_eq!((s.from, s.to), (1_754_006_400, 1_754_092_800));
        assert_eq!(
            (s.ours_in, s.ours_out, s.theirs_in, s.theirs_out),
            (2600, 600, 1700, 500)
        );
        assert!(
            s.note.starts_with("summary: 1 of 2 bucket-model pairs differ (theirs missing 1); 1 of our 4 lines outside"),
            "{}",
            s.note
        );
    }

    #[test]
    fn a_meter_that_agrees_writes_the_summary_alone() {
        let ours = [
            line(T, "claude-opus-5-5", 1000, 200, 0, 300),
            line(T + 35, "claude-opus-5-5", 500, 0, 0, 200),
        ];
        let checks = reconcile(&ours, &report(), &keys()).unwrap();
        assert_eq!(checks.len(), 1, "{checks:#?}");
        assert!(
            checks[0].note.starts_with("summary: 0 of 1"),
            "{}",
            checks[0].note
        );
    }

    #[test]
    fn each_class_is_named() {
        let class = |ours: &[Line]| reconcile(ours, &report(), &keys()).unwrap()[0].note.clone();
        // Nothing of ours: capture off, or another host on the key.
        assert!(class(&[]).starts_with(OURS_MISSING));
        // Same 1,700 in and 500 out, but nothing read from the cache on our side.
        let n = class(&[line(T, "claude-opus-5-5", 1700, 0, 0, 500)]);
        assert!(n.starts_with(CACHE_CLASS), "{n}");
        // One of the two calls missing.
        let n = class(&[line(T, "claude-opus-5-5", 1000, 200, 0, 300)]);
        assert!(n.starts_with(COUNT_DIFFERS), "{n}");
        assert!(
            n.contains("ours: 1000 uncached + 200 cache read + 0 cache write in, 300 out")
                && n.contains("theirs: 1500 uncached + 200 cache read + 0 cache write in, 500 out"),
            "{n}"
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
        assert_eq!(checks.len(), 3, "{checks:#?}");
        for c in &checks[..2] {
            assert!(c.note.starts_with(CLOCK_SKEW), "{}", c.note);
        }
        assert!(
            checks[2].note.contains("(clock skew ±1 min 2)"),
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
        assert_eq!(reconcile(&ours, &buckets, &keys()).unwrap().len(), 1);
        // No key list: organisation-wide, and the 999 shows up on their side.
        let checks = reconcile(&ours, &buckets, &[]).unwrap();
        let c = only_check(&checks);
        assert_eq!((c.ours_in, c.theirs_in), (1700, 2699));
        assert!(c.note.contains("scope: organisation-wide"), "{}", c.note);
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
        let l = line_from_reply(
            &raw,
            "claude-sonnet-5-5",
            HINT.into(),
            Some("req_1".into()),
            "order",
            7,
        )
        .unwrap();
        assert_eq!(
            (
                l.model.as_str(),
                l.uncached_in,
                l.cache_read,
                l.cache_create,
                l.out
            ),
            ("claude-opus-5-5", 2095, 2051, 2051, 503)
        );
        assert_eq!(
            (l.request_id.as_deref(), l.purpose.as_str(), l.at),
            (Some("req_1"), "order", 7)
        );
        assert!(
            !serde_json::to_string(&l).unwrap().contains("Claude."),
            "words never enter"
        );
        // Null cache counts are zero; a reply without usage writes nothing.
        let nulls = r#"{"usage":{"input_tokens":5,"output_tokens":1,"cache_creation_input_tokens":null,"cache_read_input_tokens":null}}"#;
        let l = line_from_reply(nulls, "m", String::new(), None, "decide", 0).unwrap();
        assert_eq!(
            (l.model.as_str(), l.cache_read, l.cache_create),
            ("m", 0, 0)
        );
        assert!(
            line_from_reply(r#"{"content":[]}"#, "m", String::new(), None, "decide", 0).is_none()
        );
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
        let a = line(T, "claude-opus-5-5", 1, 2, 3, 4);
        append(&path, &a).unwrap();
        append(&path, &a).unwrap();
        assert_eq!(read(&path).unwrap(), vec![a.clone(), a]);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
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
