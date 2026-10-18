//! Hardening (docs/114 rows 4, 8–11): idempotent replies that outlive a restart, one cap on every request
//! body, key-file modes checked at start, every printed or returned string scrubbed of the secrets this
//! service holds, a per-key stop that needs no chain, and how long job files are kept (docs/116 §1).

use alloy::primitives::{Address, B256, keccak256};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `bytes` to `path` through a temporary file and a rename, mode 600: a reader sees the old file or the new
/// one, never half of one, and nobody else on the machine reads it.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(bytes))
        .and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

// ---- The request reader and its body cap (docs/60 P-248) ----

/// The most a request body may be: `TAMEION_MAX_BODY_BYTES`, default 1 MiB, read once.
pub fn max_body_bytes() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| {
        std::env::var("TAMEION_MAX_BODY_BYTES")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(1 << 20)
    })
}

/// A head longer than this is handed on as it is (the router answers it), as the one 64 KiB read did.
const HEAD_MAX: usize = 64 * 1024;

/// One request off the socket: the head, then exactly the body its `Content-Length` declares. A declared body
/// over `cap` is answered 413 from the head alone, before any of it is read, and the connection is closed:
/// `None`, as for a connection that sent nothing.
pub async fn read_request<S>(sock: &mut S, cap: usize) -> Option<Vec<u8>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let head = loop {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return (!buf.is_empty()).then_some(buf);
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() >= HEAD_MAX {
            return Some(buf);
        }
    };
    let declared = content_length(&buf[..head]);
    if declared > cap {
        let words =
            format!("the request body is {declared} bytes; this service takes at most {cap}");
        let refusal = crate::refusal::refuse(
            "body_too_large",
            crate::refusal::Executed::No,
            "",
            "",
            words,
            None,
        );
        // A page reads `error` as the sentence, as `shaped` gives it.
        let page = crate::refusal::from_page(&String::from_utf8_lossy(&buf[..head]));
        let body = page
            .then(|| crate::refusal::for_page(&refusal))
            .flatten()
            .unwrap_or(refusal)
            .to_string();
        let reply = format!(
            "HTTP/1.1 413 Payload Too Large\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = sock.write_all(reply.as_bytes()).await;
        let _ = sock.shutdown().await;
        // Take what the client still sends, for a bounded while: closing with unread bytes resets the
        // connection, and the client may lose the 413 before it reads it.
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            let mut left = declared.min(16 << 20);
            while left > 0 {
                match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => left = left.saturating_sub(n),
                }
            }
        })
        .await;
        return None;
    }
    while buf.len() < head + declared {
        match sock.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    Some(buf)
}

/// The head's `Content-Length`: 0 when absent, `usize::MAX` (refused) when it is not a number.
fn content_length(head: &[u8]) -> usize {
    String::from_utf8_lossy(head)
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().unwrap_or(usize::MAX))
        })
        .unwrap_or(0)
}

// ---- Idempotency-Key replies on disk (docs/60 P-249) ----

/// When an idempotent POST was first seen, its body's hash, and its reply (the service's table entry).
pub type IdemEntry = (u64, B256, Vec<u8>);

/// How long a key's reply is kept for replay.
const IDEM_SECS: u64 = 86_400;

fn idem_path(dir: &Path, slot: &str) -> PathBuf {
    dir.join("idem").join(format!(
        "{}.json",
        alloy::hex::encode(keccak256(slot.as_bytes()))
    ))
}

/// The replay slot of an idempotent POST: the route, the key, and who sent it — a hash of the agent key and
/// the device token presented, empty for neither — so a stored reply goes back only to the caller that made
/// the first request (docs/60 P-273: a replay without the agent key returned the paid reply).
pub fn idem_slot(path: &str, key: &str, bearer: Option<&str>, device: Option<&str>) -> String {
    let (b, d) = (bearer.map(str::trim), device.map(str::trim));
    let who = match (b, d) {
        (None, None) => String::new(),
        _ => alloy::hex::encode(keccak256(
            format!("{}\n{}", b.unwrap_or(""), d.unwrap_or("")).as_bytes(),
        )),
    };
    format!("{path}\n{key}\n{who}")
}

/// Keep a finished reply under `<dir>/idem/` so a retry after a restart gets it again. Mode 600: the reply
/// to a minted agent key carries its secret.
pub fn idem_save(dir: &Path, slot: &str, e: &IdemEntry) {
    let v = json!({ "slot": slot, "at": e.0, "hash": e.1, "reply": alloy::hex::encode(&e.2) });
    if let Err(err) = write_private(&idem_path(dir, slot), v.to_string().as_bytes()) {
        println!("idempotency: keeping a reply for replay after a restart failed: {err}");
    }
}

/// The replies kept under `<dir>/idem/` that are younger than a day, by slot; older or unreadable files
/// are deleted.
pub fn idem_load(dir: &Path) -> HashMap<String, IdemEntry> {
    let now = now_secs();
    let mut out = HashMap::new();
    for p in files_in(&dir.join("idem")).filter(|p| p.extension().is_some_and(|x| x == "json")) {
        let entry = fs::read_to_string(&p)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| {
                let hash = B256::from_str(v["hash"].as_str()?).ok()?;
                let reply = alloy::hex::decode(v["reply"].as_str()?).ok()?;
                Some((
                    v["slot"].as_str()?.to_string(),
                    (v["at"].as_u64()?, hash, reply),
                ))
            });
        match entry {
            Some((slot, e)) if now.saturating_sub(e.0) < IDEM_SECS => {
                out.insert(slot, e);
            }
            _ => {
                let _ = fs::remove_file(&p);
            }
        }
    }
    out
}

// ---- The scrub (docs/60 P-250) ----

/// Every secret this service has loaded, longest first.
static SECRETS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Shorter values are not scrubbed: replacing a three-letter "secret" would mangle every reply.
const MIN_SECRET: usize = 8;

/// Scrub `secret` from everything printed or returned from now on. A leading `0x` is dropped, so a hex
/// key comes out `0x[redacted]` with or without it.
pub fn remember(secret: &str) {
    let s = secret.trim();
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() < MIN_SECRET {
        return;
    }
    let mut w = SECRETS.write().unwrap_or_else(|e| e.into_inner());
    if !w.iter().any(|x| x == s) {
        w.push(s.to_string());
        // Longest first: a secret that contains another is replaced whole.
        w.sort_by_key(|x| std::cmp::Reverse(x.len()));
    }
}

/// `s` with every remembered secret replaced by `[redacted]`.
/// ponytail: one pass per secret; Aho-Corasick if the list grows past a few hundred keys.
pub fn scrub(s: &str) -> String {
    let w = SECRETS.read().unwrap_or_else(|e| e.into_inner());
    let mut out = s.to_string();
    for k in w.iter() {
        if out.contains(k.as_str()) {
            out = out.replace(k.as_str(), "[redacted]");
        }
    }
    out
}

fn files_in(d: &Path) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
}

/// Every key or token file this service reads: the relayer's (unless `TAMEION_RELAYER_KEY` carries the key),
/// the statement signer's, the push key, the venue key, the desks' keys and tokens on this Mac, and each
/// account's agent key under the app dir. Only those that exist.
pub fn key_files(app_dir: &Path) -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".tameion");
    let env_or = |var: &str, name: &str| {
        std::env::var(var)
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(name))
    };
    let mut out = Vec::new();
    if std::env::var("TAMEION_RELAYER_KEY")
        .ok()
        .is_none_or(|k| k.trim().is_empty())
    {
        out.push(env_or("TAMEION_RELAYER_KEY_FILE", "treasury.key"));
    }
    out.push(env_or("TAMEION_STATEMENT_KEY_FILE", "statement.key"));
    out.push(env_or("TAMEION_VAPID_KEY", "vapid.key"));
    out.push(env_or("HEDGE_AGENT_KEY_FILE", "hedge-agent.key"));
    out.extend(files_in(&env_or("TAMEION_DESKS_DIR", "desks")).filter(|p| {
        p.extension()
            .is_some_and(|x| x == "key" || x == "pending" || x == "token")
    }));
    out.extend(files_in(&app_dir.join("tenants")).map(|d| d.join("agent.key")));
    out.retain(|p| p.is_file());
    out
}

/// The files among `files` that group or others may read or write, with their permission bits.
pub fn loose(files: &[PathBuf]) -> Vec<(PathBuf, u32)> {
    use std::os::unix::fs::PermissionsExt;
    files
        .iter()
        .filter_map(|p| {
            let m = fs::metadata(p).ok()?.permissions().mode() & 0o777;
            (m & 0o077 != 0).then(|| (p.clone(), m))
        })
        .collect()
}

// ---- The per-key stop (docs/60 P-251) ----

/// Agent keys the owner stopped, by account (policy, lowercase): `<app>/tokens-stopped.json`, loaded at start.
static STOPPED: Mutex<BTreeMap<String, BTreeSet<String>>> = Mutex::new(BTreeMap::new());

fn stopped_path(dir: &Path) -> PathBuf {
    dir.join("tokens-stopped.json")
}

/// Did the owner stop agent key `id` of `policy`?
pub fn is_stopped(policy: &str, id: &str) -> bool {
    STOPPED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&policy.to_lowercase())
        .is_some_and(|s| s.contains(id))
}

/// The ids of `policy`'s stopped agent keys.
pub fn stopped_tokens(policy: &str) -> Vec<String> {
    STOPPED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&policy.to_lowercase())
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default()
}

/// Read `<dir>/tokens-stopped.json`. Err when it exists but cannot be read: starting without it would turn
/// stopped keys back on.
fn stopped_load(dir: &Path) -> Result<(), String> {
    let p = stopped_path(dir);
    let m = match fs::read_to_string(&p) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", p.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(format!("{}: {e}", p.display())),
    };
    *STOPPED.lock().unwrap_or_else(|e| e.into_inner()) = m;
    Ok(())
}

/// Stop agent key `id` of `policy` now: on disk first, so a failed write changes nothing.
pub fn stop_token(dir: &Path, policy: &str, id: &str) -> Result<(), String> {
    let mut m = STOPPED.lock().unwrap_or_else(|e| e.into_inner());
    let mut next = m.clone();
    next.entry(policy.to_lowercase())
        .or_default()
        .insert(id.to_string());
    let bytes = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
    write_private(&stopped_path(dir), &bytes).map_err(|e| e.to_string())?;
    *m = next;
    Ok(())
}

/// `POST /api/level1/tokens/stop` `{policy, id}` from the owner's device (`X-Tameion-Device`, or the body's
/// `reserveToken`): agent key `id` is refused from now on, at once, with no chain and no passkey, because a
/// stop only shrinks trust. Removing the key for good stays the passkey's (`token-revoke-signed`). An owner
/// route: refusals keep the plain `{ "error" }` shape the pages read (docs/115 §2).
pub fn stop_route(dir: &Path, device: Option<&str>, body: &str) -> (&'static str, Value) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let policy = v["policy"]
        .as_str()
        .and_then(|p| Address::from_str(p.trim()).ok());
    let id = v["id"].as_str().map(str::trim).filter(|i| !i.is_empty());
    let (Some(policy), Some(id)) = (policy, id) else {
        return (
            "400 Bad Request",
            json!({ "error": "policy (the account's address) and id (the agent key's id) are required" }),
        );
    };
    let Some(t) = crate::level1::load(dir, &format!("{policy:#x}")) else {
        return ("404 Not Found", json!({ "error": "no such account" }));
    };
    if !crate::level1::reserve_token_ok(&t, device.or(v["reserveToken"].as_str())) {
        return (
            "401 Unauthorized",
            json!({ "error": "only the owner's device can stop an agent key" }),
        );
    }
    if !t.agent_tokens.iter().any(|k| k.id == id) {
        return (
            "404 Not Found",
            json!({ "error": "this account has no agent key with that id" }),
        );
    }
    match stop_token(dir, &t.policy, id) {
        Ok(()) => (
            "200 OK",
            json!({ "policy": t.policy, "stopped": id, "stoppedTokens": stopped_tokens(&t.policy),
                    "summary": "calls with this agent key are refused from now on; revoke it with your passkey to take it off the account" }),
        ),
        Err(e) => (
            "500 Internal Server Error",
            json!({ "error": format!("recording the stop failed, nothing changed: {e}") }),
        ),
    }
}

// ---- The admin token (docs/60 P-253) ----

/// Equal, in time that does not depend on where they differ.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// The operator's admin token on `GET /api/runs` and `GET /api/level1/tenants`: `Authorization: Bearer <token>`
/// only, because a URL's query string lands in proxy and access logs. `None` lets the request through;
/// otherwise the status and the refusal, in the plain `{ "error" }` shape of an operator route (docs/115 §2).
pub fn admin_refusal(
    want: Option<&str>,
    authorization: Option<&str>,
    query: &str,
) -> Option<(&'static str, Value)> {
    if query.split('&').any(|kv| kv.starts_with("token=")) {
        return Some((
            "400 Bad Request",
            json!({ "error": "send the admin token as Authorization: Bearer <token>, never in the URL: proxies and access logs keep URLs" }),
        ));
    }
    let given = authorization
        .and_then(|v| v.trim().strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let Some(given) = given else {
        return Some((
            "401 Unauthorized",
            json!({ "error": "this route needs the admin token: Authorization: Bearer <TAMEION_APP_ADMIN_TOKEN>" }),
        ));
    };
    match want.map(str::trim).filter(|t| !t.is_empty()) {
        Some(w) if same_secret(w, given) => None,
        _ => Some(("403 Forbidden", json!({ "error": "admin token" }))),
    }
}

// ---- Retention (docs/60 P-252, docs/116 §1) ----

/// `TAMEION_RETENTION_DAYS`, default 30; 0 keeps job files for ever.
pub fn retention_days() -> u64 {
    std::env::var("TAMEION_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(30)
}

/// Did this finished pass offer the owner a payment for a bill-cycle? `level1::offered_cycles` reads those
/// rows to bind the owner's later hand payment to its cycle (docs/60 P-210), so the file stays.
fn offered(v: &Value) -> bool {
    v["report"]["pendingSignature"]
        .as_array()
        .is_some_and(|a| a.iter().any(|r| r["cycleKey"].is_string()))
}

/// Delete job files (Watch scans and passes) created more than `days` ago, except what is still read: each
/// account's newest finished pass (the inbox, the due-time pass) and every finished pass that offered the
/// owner a payment. Unreadable files are left alone. Returns how many went.
pub fn prune_jobs(dir: &Path, days: u64, now: u64) -> usize {
    if days == 0 {
        return 0;
    }
    let cutoff = now.saturating_sub(days.saturating_mul(86_400));
    let mut old = Vec::new();
    let mut newest: HashMap<String, (u64, PathBuf)> = HashMap::new();
    for p in files_in(&dir.join("jobs")) {
        let Some(v) = fs::read_to_string(&p)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        else {
            continue;
        };
        let created = v["created"].as_u64().unwrap_or(u64::MAX);
        if v["request"]["level1"] == "run" && v["status"] == "done" {
            let policy = v["request"]["policy"].as_str().unwrap_or("").to_lowercase();
            if newest.get(&policy).is_none_or(|(c, _)| created > *c) {
                newest.insert(policy, (created, p.clone()));
            }
            if offered(&v) {
                continue;
            }
        }
        if created < cutoff {
            old.push(p);
        }
    }
    let keep: HashSet<PathBuf> = newest.into_values().map(|(_, p)| p).collect();
    old.into_iter()
        .filter(|p| !keep.contains(p) && fs::remove_file(p).is_ok())
        .count()
}

/// Old job files and expired idempotent replies, gone.
fn prune(dir: &Path, days: u64) -> usize {
    idem_load(dir);
    prune_jobs(dir, days, now_secs())
}

/// At start: the secrets loaded, the key-file modes checked, the stopped keys read, old files pruned — then
/// the same prune once a day on its own thread. Err when the stopped-keys file cannot be read.
pub fn start(app_dir: &Path) -> Result<(), String> {
    for var in [
        "TAMEION_RELAYER_KEY",
        "TAMEION_APP_ADMIN_TOKEN",
        "TAMEION_TELEGRAM_BOT_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
    ] {
        if let Ok(v) = std::env::var(var) {
            remember(&v);
        }
    }
    // A keyed RPC URL carries its API key.
    for c in crate::watch::CHAINS {
        if let Ok(v) = std::env::var(c.rpc_env) {
            remember(&v);
        }
    }
    let files = key_files(app_dir);
    for (p, mode) in loose(&files) {
        println!(
            "warning: {} has mode {mode:o}; group or others can read it: chmod 600 it",
            p.display()
        );
    }
    for p in &files {
        if let Ok(s) = fs::read_to_string(p) {
            remember(&s);
        }
    }
    stopped_load(app_dir)?;
    let days = retention_days();
    let gone = prune(app_dir, days);
    println!(
        "retention: job files older than {days} days are deleted at start and daily (0 = kept); {gone} deleted now"
    );
    let dir = app_dir.to_path_buf();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(86_400));
            let gone = prune(&dir, days);
            if gone > 0 {
                println!("retention: {gone} job files older than {days} days deleted");
            }
        }
    });
    Ok(())
}

/// The read routes that may answer from their last good body when the chain does not (Arc's public RPC
/// rate-limits, docs/60 P-12, P-14).
pub const STALE_ROUTES: [&str; 3] = [
    "/api/level1/inbox",
    "/api/level1/status",
    "/api/level1/symbolon",
];
/// How old a last good body may be and still be served, marked stale.
pub const STALE_SECS: u64 = 30;
const STALE_MAX: usize = 256;

/// The last good body of each read route per request key (route, query and caller, hashed), in memory only.
#[derive(Default)]
pub struct LastGood(HashMap<String, (u64, Value)>);

impl LastGood {
    /// What to send instead of a reply (`status`, `body`) to a GET of a stale route: `Some` with the last good
    /// body of `key` and `stale: true`, `asOf` when the reply is a chain failure (502 or 504, or a status whose
    /// chain read failed: `paused` null) and that body is at most STALE_SECS old. A good reply is kept and passes.
    pub fn answer(
        &mut self,
        method: &str,
        path: &str,
        key: &str,
        now: u64,
        status: u16,
        body: &Value,
    ) -> Option<Value> {
        if method != "GET" || !STALE_ROUTES.contains(&path) {
            return None;
        }
        let status_unread =
            path == "/api/level1/status" && body["policy"].is_string() && body["paused"].is_null();
        let failed = matches!(status, 502 | 504) || (status == 200 && status_unread);
        if status == 200 && !failed {
            if self.0.len() >= STALE_MAX && !self.0.contains_key(key) {
                // ponytail: an O(n) sweep at 256 entries; drop the old, else the oldest
                self.0
                    .retain(|_, (at, _)| now.saturating_sub(*at) <= STALE_SECS);
                if self.0.len() >= STALE_MAX
                    && let Some(k) = self
                        .0
                        .iter()
                        .min_by_key(|(_, (at, _))| *at)
                        .map(|(k, _)| k.clone())
                {
                    self.0.remove(&k);
                }
            }
            self.0.insert(key.to_string(), (now, body.clone()));
            return None;
        }
        let (at, good) = self
            .0
            .get(key)
            .filter(|(at, _)| failed && now.saturating_sub(*at) <= STALE_SECS)?;
        let mut v = good.clone();
        v["stale"] = json!(true);
        v["asOf"] = json!(at);
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tameion-guard-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_log_line_with_a_token_comes_out_scrubbed() {
        let token = "9f1c2e7d4b6a8c0e1f3a5b7d9c2e4f6a8b0c1d3e5f7a9b2c4d6e8f0a1b3c5d7e";
        remember(token);
        remember("abc"); // too short to scrub
        let line = format!("watcher: relayed with Bearer {token} and 0x{token}; abc stays");
        assert_eq!(
            scrub(&line),
            "watcher: relayed with Bearer [redacted] and 0x[redacted]; abc stays"
        );
    }

    #[test]
    fn a_reply_kept_on_disk_comes_back_after_a_restart_and_expires_after_a_day() {
        let d = tmp("idem");
        let reply = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec();
        let slot = "/api/level1/run\nkey-1";
        idem_save(&d, slot, &(now_secs(), keccak256(b"{}"), reply.clone()));
        idem_save(
            &d,
            "/api/level1/run\nold",
            &(now_secs() - IDEM_SECS - 1, B256::ZERO, reply.clone()),
        );
        let m = idem_load(&d);
        assert_eq!(m.len(), 1);
        assert_eq!(m[slot].1, keccak256(b"{}"));
        assert_eq!(m[slot].2, reply);
        assert_eq!(
            files_in(&d.join("idem")).count(),
            1,
            "the expired file is gone"
        );
        use std::os::unix::fs::PermissionsExt;
        let p = idem_path(&d, slot);
        assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_replay_slot_belongs_to_the_caller_that_made_the_first_request() {
        let p = "/api/level1/buy";
        let mine = idem_slot(p, "op-1", Some("s3cret"), None);
        assert_eq!(mine, idem_slot(p, "op-1", Some(" s3cret\n"), None));
        assert_ne!(
            mine,
            idem_slot(p, "op-1", None, None),
            "no agent key: not my slot"
        );
        assert_ne!(mine, idem_slot(p, "op-1", Some("other"), None));
        assert_ne!(mine, idem_slot(p, "op-1", None, Some("s3cret")));
        assert_ne!(mine, idem_slot(p, "op-2", Some("s3cret"), None));
        assert!(
            !mine.contains("s3cret"),
            "the slot is kept on disk: a hash, never the key"
        );
    }

    #[test]
    fn the_admin_token_counts_only_in_the_authorization_header() {
        let ok = Some("t0k3n-admin");
        assert_eq!(admin_refusal(ok, Some("Bearer t0k3n-admin"), ""), None);
        assert_eq!(admin_refusal(ok, None, "").unwrap().0, "401 Unauthorized");
        assert_eq!(
            admin_refusal(ok, Some("Bearer wrong"), "").unwrap().0,
            "403 Forbidden"
        );
        assert_eq!(
            admin_refusal(ok, Some("Bearer t0k3n-admin"), "token=t0k3n-admin")
                .unwrap()
                .0,
            "400 Bad Request",
            "the URL form is refused even when the header is right"
        );
        assert_eq!(
            admin_refusal(None, Some("Bearer x"), "").unwrap().0,
            "403 Forbidden"
        );
        assert_eq!(
            admin_refusal(Some(""), Some("Bearer "), "").unwrap().0,
            "401 Unauthorized"
        );
    }

    #[test]
    fn a_key_file_others_can_read_is_named() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("modes");
        let (open, closed) = (d.join("open.key"), d.join("closed.key"));
        fs::write(&open, "k").unwrap();
        fs::write(&closed, "k").unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(loose(&[open.clone(), closed]), vec![(open, 0o644)]);
    }

    #[test]
    fn a_stopped_key_stays_stopped_across_a_restart() {
        let d = tmp("stop");
        let policy = "0x00000000000000000000000000000000000000Aa";
        stop_token(&d, policy, "tok-1").unwrap();
        assert!(is_stopped(&policy.to_lowercase(), "tok-1"));
        assert!(!is_stopped(policy, "tok-2"));
        STOPPED.lock().unwrap().remove(&policy.to_lowercase());
        assert!(!is_stopped(policy, "tok-1"));
        stopped_load(&d).unwrap();
        assert_eq!(stopped_tokens(policy), vec!["tok-1".to_string()]);
        fs::write(stopped_path(&d), "not json").unwrap();
        assert!(
            stopped_load(&d).is_err(),
            "an unreadable file refuses to start"
        );
    }

    #[test]
    fn a_stopped_key_is_refused_by_every_token_check() {
        use crate::level1::{AgentToken, Tenant, token_allows, token_hash};
        let mut t: Tenant = serde_json::from_value(json!({
            "chain": "arc-testnet", "owner": "0x1", "policy": "0x00000000000000000000000000000000000057a1", "agent": "0x3",
            "operator": "0x4", "watched": "0x5", "recovery": "0x0", "per_action": "1", "daily": "1", "floor": "0",
            "delay_secs": 60, "mode": 1, "created_block": 0, "history_from_block": 0, "created": 0
        }))
        .unwrap();
        t.agent_tokens.push(AgentToken {
            id: "tok-a".into(),
            label: "ops bot".into(),
            scopes: vec!["read".into()],
            expires: 1_000,
            issued: 1,
            hash: token_hash("s3cret-a"),
            alg: crate::level1::owner_alg(),
        });
        assert_eq!(token_allows(&t, Some("s3cret-a"), "read", 100), Ok(()));
        stop_token(&tmp("stop-allows"), &t.policy, "tok-a").unwrap();
        assert_eq!(
            token_allows(&t, Some("s3cret-a"), "read", 100),
            Err("the owner stopped this agent key")
        );
    }

    #[test]
    fn old_jobs_go_but_the_newest_pass_and_passes_that_offered_a_payment_stay() {
        let d = tmp("retention");
        let now = 100 * 86_400;
        let old = now - 31 * 86_400;
        let job = |id: &str, v: Value| {
            fs::write(d.join("jobs").join(format!("{id}.json")), v.to_string()).unwrap()
        };
        fs::create_dir_all(d.join("jobs")).unwrap();
        let pass = |created: u64, rows: Value| {
            json!({ "created": created, "status": "done", "request": { "level1": "run", "policy": "0xA" },
                    "report": { "pendingSignature": rows } })
        };
        job(
            "scan-old",
            json!({ "created": old, "status": "done", "request": { "address": "0x1" } }),
        );
        job(
            "scan-new",
            json!({ "created": now - 86_400, "status": "done", "request": {} }),
        );
        job("pass-old", pass(old, json!([])));
        job(
            "pass-offered",
            pass(
                old - 1,
                json!([{ "cycleKey": "0xb@1", "actionHash": "0x01" }]),
            ),
        );
        job("pass-newest-but-old", pass(old + 1, json!([])));
        job(
            "failed-old",
            json!({ "created": old, "status": "failed", "request": { "level1": "run", "policy": "0xA" } }),
        );
        fs::write(d.join("jobs").join("garbled.json"), "{").unwrap();
        assert_eq!(prune_jobs(&d, 30, now), 3);
        let mut left: Vec<String> = files_in(&d.join("jobs"))
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            ["garbled", "pass-newest-but-old", "pass-offered", "scan-new"]
        );
        assert_eq!(prune_jobs(&d, 0, now), 0, "0 keeps everything");
    }

    #[tokio::test]
    async fn a_body_over_the_cap_is_refused_from_the_head_and_one_under_it_is_read_whole() {
        let (mut client, mut server) = tokio::io::duplex(1 << 16);
        let head = format!(
            "POST /api/level1/run HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            2 << 20
        );
        client.write_all(head.as_bytes()).await.unwrap();
        drop(client.shutdown().await);
        assert!(read_request(&mut server, 1 << 20).await.is_none());
        let mut got = String::new();
        client.read_to_string(&mut got).await.unwrap();
        assert!(
            got.starts_with("HTTP/1.1 413 Payload Too Large\r\n"),
            "{got}"
        );
        assert!(got.contains("at most 1048576"), "{got}");

        // 200 KiB declared, sent in pieces: read whole, past the old single 64 KiB read.
        let (mut client, mut server) = tokio::io::duplex(1 << 16);
        let body = "x".repeat(200 * 1024);
        let req = format!(
            "POST /x HTTP/1.1\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        let writer = tokio::spawn(async move {
            client.write_all(req.as_bytes()).await.unwrap();
            client
        });
        let got = read_request(&mut server, 1 << 20).await.unwrap();
        assert!(got.ends_with(body.as_bytes()));
        drop(writer.await.unwrap());
        assert_eq!(content_length(b"Content-Length: nope\r\n\r\n"), usize::MAX);
    }

    #[test]
    fn a_read_route_answers_its_last_good_body_only_on_a_chain_failure_and_only_inside_thirty_seconds()
     {
        let mut c = LastGood::default();
        let inbox = "/api/level1/inbox";
        let good = json!({ "policy": "0xab", "balance": "1.000000 USDC" });
        let down = json!({ "error": "the chain did not answer" });
        assert_eq!(
            c.answer("GET", inbox, "k", 100, 502, &down),
            None,
            "nothing kept yet: the failure passes"
        );
        assert_eq!(
            c.answer("GET", inbox, "k", 100, 200, &good),
            None,
            "a good read passes and is kept"
        );
        let stale = c
            .answer("GET", inbox, "k", 130, 502, &down)
            .expect("30 s old: served");
        assert_eq!(
            (
                stale["balance"].clone(),
                stale["stale"].clone(),
                stale["asOf"].clone()
            ),
            (json!("1.000000 USDC"), json!(true), json!(100))
        );
        assert_eq!(
            c.answer("GET", inbox, "k", 131, 502, &down),
            None,
            "31 s old: the failure passes"
        );
        assert_eq!(
            c.answer("GET", inbox, "other caller", 110, 502, &down),
            None,
            "another caller's body is never served"
        );
        let fresh = json!({ "policy": "0xab", "balance": "0.900000 USDC" });
        assert_eq!(
            c.answer("GET", inbox, "k", 140, 200, &fresh),
            None,
            "a fresh read is sent as it is, never the stale one"
        );
        assert_eq!(
            c.answer("GET", inbox, "k", 150, 504, &down).unwrap()["balance"],
            "0.900000 USDC"
        );
        assert_eq!(
            c.answer("GET", inbox, "k", 150, 401, &down),
            None,
            "a refusal is not a chain failure"
        );
        assert_eq!(
            c.answer("POST", inbox, "k", 150, 502, &down),
            None,
            "never for a write"
        );
        assert_eq!(
            c.answer("GET", "/api/level1/record", "k", 150, 502, &down),
            None,
            "only the three read routes"
        );
        // The status route says a failed chain read with `paused: null`, not an error status.
        let status = "/api/level1/status";
        let read = json!({ "policy": "0xab", "paused": false, "rate": "0.001 USDC" });
        let unread = json!({ "policy": "0xab", "paused": null, "rate": null });
        assert_eq!(c.answer("GET", status, "s", 200, 200, &read), None);
        assert_eq!(
            c.answer("GET", status, "s", 210, 200, &unread).unwrap()["rate"],
            "0.001 USDC",
            "an unread status is not kept as good"
        );
        assert_eq!(
            c.answer("GET", status, "s", 250, 200, &unread),
            None,
            "too old: the unread status passes"
        );
        // Bounded: the 257th key sweeps the old ones out.
        for i in 0..STALE_MAX {
            c.answer("GET", inbox, &format!("n{i}"), 1_000, 200, &good);
        }
        c.answer("GET", inbox, "late", 1_100, 200, &good);
        assert!(c.0.len() <= STALE_MAX);
        assert!(c.0.contains_key("late"));
    }
}
