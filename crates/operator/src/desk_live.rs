//! What the agent is doing, live, and the exchange connected with one
//! signature (docs/102 D11, D13; docs/106).
//!
//! **The feed** (`GET /api/desk/live`): one JSON event a second. Every venue
//! number is Hyperliquid's own `clearinghouseState`, read once a second by one
//! poller per venue account that every open stream shares (the IP has 1,200
//! request weight a minute for everything, the desk's own orders included; one
//! read costs 2), and a reading more than 2 s old is never sent: its fields go
//! `null` and `venueError` says why. Funding is the venue's own `userFunding`
//! record, read when the poller starts and again at each hourly settlement —
//! the only moments a payment can appear. Orders with their read-back, HALT,
//! pass latency and the last anchor are our own records on disk.
//!
//! **The connect** (`POST /api/desk/venue/connect` then `/approve`): the desk
//! makes its own trade-only key, the exchange wallet signs Hyperliquid's
//! `ApproveAgent` for it once, the owner's passkey approves that account for
//! the desk (checked by the route before the venue sees anything), and the
//! service submits the signature and reads the key back from the venue. Nobody
//! pastes a key; the key never leaves this Mac, and like every API wallet it
//! can trade and cannot withdraw.

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use hedge::policy::HedgePolicy;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;

/// Testnet only, like everything that talks to the venue (`CLAUDE.md`).
const INFO: &str = "https://api.hyperliquid-testnet.xyz/info";
/// A venue reading older than this is not sent.
const MAX_AGE: Duration = Duration::from_secs(2);
/// The orders a stream shows, newest first.
const LAST_ORDERS: usize = 10;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// What the poller last got from the venue for one account.
#[derive(Clone, Default)]
pub struct VenueRead {
    /// `clearinghouseState` as the venue sent it, and when it was **asked**
    /// for: the venue stamped it after that, so "asked ≤ 2 s ago" bounds its
    /// real age whatever the reply took (docs/103 S3-5).
    state: Option<(Value, Instant)>,
    /// Funding payments (venue time in ms, USDC; + received, − paid) from
    /// `userFunding`, the last 25 hours; `None` until the first read.
    funding: Option<Vec<(u64, Decimal)>>,
    /// When funding was last read from the venue (unix s), for the frame.
    funding_read_at: Option<u64>,
    /// Why the last read failed, if it did.
    error: Option<String>,
}

/// One poller per venue account. An entry exists while its poller runs: the
/// poller removes it under this lock before it stops, so a stream that finds
/// an entry always has a live poller behind it.
static FEEDS: LazyLock<Mutex<HashMap<Address, watch::Receiver<VenueRead>>>> =
    LazyLock::new(Default::default);

fn subscribe(venue: Address) -> watch::Receiver<VenueRead> {
    let mut feeds = FEEDS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(rx) = feeds.get(&venue) {
        return rx.clone();
    }
    let (tx, rx) = watch::channel(VenueRead::default());
    feeds.insert(venue, rx.clone());
    tokio::spawn(poll(venue, tx));
    rx
}

/// When to read funding again after a read at `t` (unix s): 2, 10, 30 and 60 s
/// after each hour, when the venue books the hourly payment, else the next hour.
fn next_funding_read(t: u64) -> u64 {
    let hour = t - t % 3600;
    [2, 10, 30, 60, 3602]
        .into_iter()
        .map(|s| hour + s)
        .find(|&s| s > t)
        .unwrap_or(hour + 3602)
}

async fn poll(venue: Address, tx: watch::Sender<VenueRead>) {
    let http = reqwest::Client::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut funding_due = 0u64;
    // Each position's `cumFunding.sinceOpen` as last read: when the venue's own
    // running total moves, a payment was booked, so funding is read again then,
    // even if it landed after the hour's last scheduled read (docs/103 S3-6).
    let mut last_cum: Option<Vec<(String, String)>> = None;
    loop {
        tick.tick().await;
        {
            // The registry's own receiver is the one left when nobody watches.
            let mut feeds = FEEDS.lock().unwrap_or_else(|e| e.into_inner());
            if tx.receiver_count() <= 1 {
                feeds.remove(&venue);
                return;
            }
        }
        let now = now_ms() / 1000;
        let funding = if now >= funding_due {
            let r = info(
                &http,
                json!({ "type": "userFunding", "user": format!("{venue:#x}"), "startTime": now_ms().saturating_sub(25 * 3600 * 1000) }),
            )
            .await
            .and_then(|v| funding_entries(&v));
            // A failed read is tried again in 5 s; what was read before stays
            // true (a payment, once made, does not change).
            funding_due = if r.is_ok() {
                next_funding_read(now)
            } else {
                now + 5
            };
            Some(r)
        } else {
            None
        };
        let asked = Instant::now();
        let state = info(
            &http,
            json!({ "type": "clearinghouseState", "user": format!("{venue:#x}") }),
        )
        .await;
        if let Ok(v) = &state {
            let cum = cum_funding(v);
            if last_cum.as_ref().is_some_and(|c| *c != cum) {
                funding_due = 0;
            }
            last_cum = Some(cum);
        }
        tx.send_modify(|r| {
            match state {
                Ok(v) => {
                    r.state = Some((v, asked));
                    r.error = None;
                }
                Err(e) => r.error = Some(e),
            }
            match funding {
                Some(Ok(f)) => {
                    r.funding = Some(f);
                    r.funding_read_at = Some(now);
                }
                Some(Err(e)) if r.error.is_none() => r.error = Some(format!("funding: {e}")),
                _ => {}
            }
        });
    }
}

/// Each open position's `cumFunding.sinceOpen`, as the venue reports it.
fn cum_funding(chs: &Value) -> Vec<(String, String)> {
    chs["assetPositions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|ap| {
            let p = &ap["position"];
            (
                p["coin"].as_str().unwrap_or_default().to_string(),
                p["cumFunding"]["sinceOpen"].to_string(),
            )
        })
        .collect()
}

async fn info(http: &reqwest::Client, body: Value) -> Result<Value, String> {
    let r = http
        .post(INFO)
        .timeout(MAX_AGE)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("the venue did not answer: {e}"))?;
    if !r.status().is_success() {
        return Err(format!("the venue answered {}", r.status()));
    }
    r.json()
        .await
        .map_err(|e| format!("the venue's answer: {e}"))
}

fn funding_entries(v: &Value) -> Result<Vec<(u64, Decimal)>, String> {
    v.as_array()
        .ok_or("userFunding is not a list")?
        .iter()
        .map(|e| {
            let usdc = e["delta"]["usdc"]
                .as_str()
                .ok_or("userFunding usdc missing")?;
            Ok((
                e["time"].as_u64().ok_or("userFunding time missing")?,
                Decimal::from_str(usdc).map_err(|x| format!("userFunding usdc {usdc:?}: {x}"))?,
            ))
        })
        .collect()
}

/// The venue part of an event from one `clearinghouseState` reply. Sizes,
/// entries, PnL and margins are the venue's own strings; `mark` is
/// `positionValue ÷ |szi|` from the same reply (exact: a perp price has at most
/// `6 − szDecimals` decimals); `kRatio` is what the k rule enforces, equity ÷
/// maintenance margin (`hedge::safety::health` × k), `null` with nothing open.
fn venue_fields(
    chs: &Value,
    funding: Option<&[(u64, Decimal)]>,
    now_ms: u64,
) -> Result<Map<String, Value>, String> {
    let text = |p: &str| {
        chs.pointer(p)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("clearinghouseState{p} missing"))
    };
    let num = |s: &str| Decimal::from_str(s).map_err(|e| format!("{s:?}: {e}"));
    let account_value = text("/crossMarginSummary/accountValue")?;
    let maintenance = text("/crossMaintenanceMarginUsed")?;
    let mm = num(maintenance)?;
    let k_ratio = if mm.is_zero() {
        None
    } else {
        // Toward zero: the meter never shows more room than there is.
        Some(
            (num(account_value)? / mm)
                .round_dp_with_strategy(2, RoundingStrategy::ToZero)
                .to_string(),
        )
    };
    let positions = chs["assetPositions"]
        .as_array()
        .ok_or("clearinghouseState/assetPositions missing")?
        .iter()
        .map(|ap| {
            let p = &ap["position"];
            let s = |k: &str| {
                p[k].as_str()
                    .ok_or_else(|| format!("position.{k} missing"))
            };
            let size = num(s("szi")?)?;
            let mark = (!size.is_zero())
                .then(|| num(s("positionValue")?).map(|v| (v / size.abs()).normalize().to_string()))
                .transpose()?;
            Ok(json!({ "coin": s("coin")?, "size": s("szi")?, "entry": p["entryPx"], "mark": mark, "pnl": s("unrealizedPnl")? }))
        })
        .collect::<Result<Vec<Value>, String>>()?;
    let since = |ms: u64| {
        funding.map(|f| {
            f.iter()
                .filter(|(t, _)| *t >= now_ms.saturating_sub(ms))
                .map(|(_, u)| *u)
                .sum::<Decimal>()
                .normalize()
                .to_string()
        })
    };
    let Value::Object(m) = json!({
        "positions": positions,
        "marginUsed": text("/crossMarginSummary/totalMarginUsed")?,
        "maintenanceMarginUsed": maintenance,
        "accountValue": account_value,
        "kRatio": k_ratio,
        "fundingLastHour": since(3600 * 1000),
        "funding24h": since(24 * 3600 * 1000),
        "venueTime": chs["time"],
        "venueError": null,
    }) else {
        unreachable!("a json! object literal is an object")
    };
    Ok(m)
}

/// The last `max` bytes of a file as whole lines (an append-only record grows;
/// a stream reads it every second).
fn tail_lines(p: &Path, max: u64) -> Vec<String> {
    let Ok(mut f) = fs::File::open(p) else {
        return Vec::new();
    };
    let start = f.metadata().map_or(0, |m| m.len().saturating_sub(max));
    let mut text = String::new();
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    // A cut in the middle of a line leaves its tail first: drop it.
    text.lines()
        .skip(usize::from(start > 0))
        .map(str::to_string)
        .collect()
}

/// The newest anchor among the desk's statements: its transaction and when it
/// was first cited (a root that did not move is cited again, not re-anchored).
fn last_anchor(statements: &Path) -> Value {
    // ponytail: parses every kit each second; index by mtime when a desk keeps thousands.
    let kits: Vec<(i64, String)> = fs::read_dir(statements)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".lender.json"))
        .filter_map(|e| {
            let k: Value = serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok()?;
            Some((
                k.pointer("/signed/statement/at")?.as_i64()?,
                k.pointer("/anchor/tx")?.as_str()?.to_string(),
            ))
        })
        .collect();
    let Some((_, tx)) = kits.iter().max_by_key(|k| k.0) else {
        return Value::Null;
    };
    let first = kits.iter().filter(|k| &k.1 == tx).map(|k| k.0).min();
    json!({ "tx": tx, "at": first })
}

/// Our own part of an event: the engine's pass record and HALT file, and the
/// desk's statements.
fn records(halt_dir: &Path, venue: Option<Address>, statements: &Path) -> Map<String, Value> {
    let halt = fs::read_to_string(hedge::halt::path(halt_dir))
        .ok()
        .and_then(|t| {
            // The first line is what stopped the desk; later lines came after it.
            let l = t.lines().find(|l| !l.trim().is_empty())?.to_string();
            let (at, reason) = l.split_once('\t').unwrap_or(("", &l));
            Some(json!({ "reason": reason, "at": at.parse::<u64>().ok() }))
        });
    let passes: Vec<Value> = venue
        .map(|v| {
            tail_lines(
                &hedge::driver::books_dir(halt_dir, v).join(hedge::store::PASSES_FILE),
                64 * 1024,
            )
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let orders: Vec<Value> = passes
        .iter()
        .rev()
        .flat_map(|p| p["orders"].as_array().cloned().unwrap_or_default().into_iter().rev())
        .take(LAST_ORDERS)
        .map(|o| json!({ "at": o["at"], "coin": o["coin"], "side": o["side"], "size": o["size"], "oid": o["oid"], "readBack": o["readBack"] }))
        .collect();
    let last = passes.last();
    let Value::Object(m) = json!({
        "lastOrders": orders,
        "halt": halt,
        "passLatencyMs": last.map(|p| p["ms"].clone()),
        "lastPassAt": last.map(|p| p["at"].clone()),
        "lastAnchor": last_anchor(statements),
    }) else {
        unreachable!("a json! object literal is an object")
    };
    m
}

/// One event of the feed at `now_ms`.
fn event(
    now_ms: u64,
    venue: Option<Address>,
    read: Option<&VenueRead>,
    halt_dir: &Path,
    statements: &Path,
) -> Value {
    let mut ev = Map::new();
    ev.insert("at".into(), json!(now_ms / 1000));
    ev.insert("venue".into(), json!(venue.map(|a| format!("{a:#x}"))));
    ev.insert(
        "kPolicy".into(),
        json!(HedgePolicy::default().k_maintenance),
    );
    ev.insert(
        "fundingReadAt".into(),
        json!(read.and_then(|r| r.funding_read_at)),
    );
    let fresh = read
        .and_then(|r| r.state.as_ref())
        .filter(|(_, at)| at.elapsed() <= MAX_AGE);
    let venue_part = match (venue, fresh) {
        (None, _) => Err("no exchange account is connected to this desk".to_string()),
        (Some(_), Some((chs, _))) => {
            venue_fields(chs, read.and_then(|r| r.funding.as_deref()), now_ms)
        }
        (Some(_), None) => Err(read
            .and_then(|r| r.error.clone())
            .unwrap_or_else(|| "no reading from the venue in the last 2 s".to_string())),
    };
    match venue_part {
        Ok(m) => ev.extend(m),
        Err(e) => {
            for k in [
                "positions",
                "marginUsed",
                "maintenanceMarginUsed",
                "accountValue",
                "kRatio",
                "fundingLastHour",
                "funding24h",
                "venueTime",
            ] {
                ev.insert(k.into(), Value::Null);
            }
            ev.insert("venueError".into(), json!(e));
        }
    }
    ev.extend(records(halt_dir, venue, statements));
    Value::Object(ev)
}

/// Where the engine keeps `HALT`, the books and the pass record: the same
/// place `hedge::driver` uses.
fn halt_dir() -> PathBuf {
    PathBuf::from(std::env::var("HEDGE_HALT_DIR").unwrap_or_else(|_| "runtime/hedge".into()))
}

/// Serve the feed for `policy`'s desk on `sock` until the reader goes away.
/// The caller has checked that the reader is the desk's owner; `still_owner`
/// asks again every minute, and the stream ends when it says no (a device
/// token minted again, a read key withdrawn: docs/103 S3-9).
pub async fn stream<S, F>(sock: &mut S, app_dir: &Path, policy: &str, still_owner: F)
where
    S: AsyncWrite + Unpin,
    F: Fn() -> bool,
{
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\
                Connection: keep-alive\r\nX-Accel-Buffering: no\r\n\r\nretry: 3000\n\n";
    if sock.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let statements = crate::desk::book_path(app_dir, policy).with_file_name("statements");
    let halt_dir = halt_dir();
    let mut venue = None;
    let mut rx: Option<watch::Receiver<VenueRead>> = None;
    let mut checked = Instant::now();
    loop {
        if checked.elapsed() >= Duration::from_secs(60) {
            if !still_owner() {
                return;
            }
            checked = Instant::now();
        }
        // The desk may be connected or disconnected while the page is open.
        let now_venue = crate::desk::venue_account(policy);
        if now_venue != venue {
            venue = now_venue;
            rx = venue.map(subscribe);
            // The first event waits (up to 2 s) for the poller's first reading.
            if let Some(r) = rx.as_mut()
                && r.borrow().state.is_none()
            {
                let _ = tokio::time::timeout(MAX_AGE, r.changed()).await;
            }
        }
        let read = rx.as_mut().map(|r| r.borrow_and_update().clone());
        let ev = event(now_ms(), venue, read.as_ref(), &halt_dir, &statements);
        let sent = Instant::now();
        if sock
            .write_all(format!("data: {ev}\n\n").as_bytes())
            .await
            .is_err()
            || sock.flush().await.is_err()
        {
            return;
        }
        // At most one event a second (docs/103 S3-4): the next one carries the
        // poller's next reading; when none comes, an event goes only once the
        // last reading is stale, to say so (its venue fields are then null).
        tokio::time::sleep_until((sent + Duration::from_secs(1)).into()).await;
        if let Some(r) = rx.as_mut() {
            loop {
                match tokio::time::timeout(Duration::from_millis(250), r.changed()).await {
                    Ok(Ok(())) => break,
                    Ok(Err(_)) => {
                        // The poller is gone: subscribe again.
                        venue = None;
                        break;
                    }
                    Err(_) => {
                        let stale = r
                            .borrow()
                            .state
                            .as_ref()
                            .is_none_or(|(_, asked)| asked.elapsed() > MAX_AGE);
                        if stale {
                            break;
                        }
                    }
                }
            }
        }
    }
}

// ---- The exchange connected with one signature ----

/// This Mac's desk folder (`scripts/desk_init.sh`; `TAMEION_DESKS_DIR` overrides).
fn desks_dir() -> PathBuf {
    std::env::var("TAMEION_DESKS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".tameion/desks")
        })
}

/// The desk's trade-only key once the venue has confirmed it.
pub fn agent_key_path(policy: &str) -> PathBuf {
    desks_dir().join(format!("{}.agent.key", policy.trim().to_lowercase()))
}

/// The key made for a connection the master wallet has not signed yet, with
/// the nonce and name it was offered under.
fn pending_path(policy: &str) -> PathBuf {
    desks_dir().join(format!("{}.agent.pending", policy.trim().to_lowercase()))
}

/// The name the desk's key is approved under. Hyperliquid replaces an agent
/// approved under the same name, so connecting again retires the desk's old
/// key, and the account's other keys (other names) are never touched.
fn agent_name(policy: &str) -> String {
    let hex = policy.trim().trim_start_matches("0x").to_lowercase();
    format!("desk-{}", &hex[..hex.len().min(6)])
}

fn write_secret(p: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(p)
        .map_err(|e| e.to_string())?;
    // A file that existed keeps its old mode through `open`; set it again.
    f.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())
}

const ONE_DESK_BY_ENV: &str = "this service was started for one desk by its environment (TAMEION_DESK_POLICY); restart it without that to connect from the page";

/// The key made for a connection not signed yet, with its nonce and name.
fn read_pending(policy: &str) -> Option<(PrivateKeySigner, u64, String)> {
    let p: Value = serde_json::from_str(&fs::read_to_string(pending_path(policy)).ok()?).ok()?;
    Some((
        PrivateKeySigner::from_str(p["key"].as_str()?).ok()?,
        p["nonce"].as_u64()?,
        p["name"].as_str()?.to_string(),
    ))
}

/// How long a pending key is offered again instead of a new one.
const PENDING_FOR_MS: u64 = 10 * 60 * 1000;

/// `POST /api/desk/venue/connect {policy, fresh?}`: the desk's trade-only key,
/// kept pending (mode 600, never shown) until the venue confirms it, and the
/// typed data the exchange wallet signs once. A pending key younger than ten
/// minutes is offered again (a second tab or a retry must not change the key
/// under a wallet that is signing, docs/103 S3-8) unless `fresh` asks for a new
/// one. The caller has checked that the request comes from the desk's owner.
pub fn connect(policy: &str, body: &Value) -> (&'static str, Value) {
    if std::env::var("TAMEION_DESK_POLICY").is_ok() {
        return ("409 Conflict", json!({ "error": ONE_DESK_BY_ENV }));
    }
    let reuse = (body["fresh"] != true)
        .then(|| read_pending(policy))
        .flatten()
        .filter(|(_, nonce, _)| now_ms().saturating_sub(*nonce) < PENDING_FOR_MS);
    let (agent, nonce, name) = match reuse {
        Some((key, nonce, name)) => (key.address(), nonce, name),
        None => {
            let key = PrivateKeySigner::random();
            let (nonce, name) = (now_ms(), agent_name(policy));
            let pending =
                json!({ "key": key.to_bytes().to_string(), "nonce": nonce, "name": name });
            if let Err(e) = write_secret(&pending_path(policy), pending.to_string().as_bytes()) {
                return (
                    "500 Internal Server Error",
                    json!({ "error": format!("the desk's key could not be kept: {e}") }),
                );
            }
            (key.address(), nonce, name)
        }
    };
    (
        "200 OK",
        json!({
            "agent": format!("{agent:#x}"),
            "name": name,
            "nonce": nonce,
            "typedData": hedge::hl::approve_agent_typed_data(agent, &name, nonce),
            "words": "Your exchange wallet signs once. It lets this desk's own key trade on your Hyperliquid account; that key cannot withdraw, and the desk's limits still apply.",
        }),
    )
}

/// The venue's refusal in words the owner can act on.
fn refusal_words(e: &str) -> String {
    if e.contains("Must deposit") || e.contains("does not exist") {
        "Hyperliquid testnet does not know this wallet yet. An account exists there only after a first deposit, and the testnet faucet pays only wallets that already exist on Hyperliquid mainnet. Sign with a wallet that has a Hyperliquid account, or have one send it test USDC, then connect again. Nothing was approved.".into()
    } else {
        format!(
            "The exchange refused the approval ({e}). Nothing was approved; the desk trades as before."
        )
    }
}

/// Whether the venue lists `agent` among `master`'s API wallets, tried three
/// times 0.5 s and 1.5 s apart (testnet drops a read now and then, P-129).
async fn listed(venue: &hedge::hl::Venue, master: Address, agent: Address) -> Option<Value> {
    for wait in [0u64, 500, 1500] {
        tokio::time::sleep(Duration::from_millis(wait)).await;
        if let Ok(agents) = venue.api_agents(master).await
            && let Some(a) = agents.into_iter().find(|a| a.address == agent)
        {
            return Some(json!({ "name": a.name, "validUntil": a.valid_until }));
        }
    }
    None
}

/// `POST /api/desk/venue/approve {policy, signature, master, issued, auth, agent?}`:
/// connect the desk to the master's account with the desk's pending key.
/// Where the offset lives is the owner's decision, so the caller has already
/// verified the owner's passkey over `desk::venue_challenge` for `master`
/// (`approval`); the wallet's signature only lets the key trade there.
///
/// Order (docs/103 S3-2): every refusal of the claim first, writing nothing;
/// then the key file and the claim, so a desk tracks the key before the venue
/// can make it live; then the venue. If the venue does not approve, the key
/// file and the claim are put back as they were. No key is ever live at the
/// venue and on disk without a desk naming it.
pub async fn approve(
    policy: &str,
    body: &Value,
    master: Address,
    approval: Value,
) -> (&'static str, Value) {
    if std::env::var("TAMEION_DESK_POLICY").is_ok() {
        return ("409 Conflict", json!({ "error": ONE_DESK_BY_ENV }));
    }
    let Some(signature) = body["signature"].as_str() else {
        return (
            "400 Bad Request",
            json!({ "error": "signature missing: the wallet's answer to the typed data" }),
        );
    };
    let Some((key, nonce, name)) = read_pending(policy) else {
        return (
            "409 Conflict",
            json!({ "error": "nothing to approve: ask to connect the exchange first" }),
        );
    };
    let agent = key.address();
    // The page names the key it showed the wallet: another `connect` since then is a race.
    if body["agent"]
        .as_str()
        .is_some_and(|a| !a.trim().eq_ignore_ascii_case(&format!("{agent:#x}")))
    {
        return (
            "409 Conflict",
            json!({ "error": "the desk's key changed since the wallet signed", "words": "The desk's key changed since your wallet signed. Connect again and sign the new one. Nothing was sent to the exchange." }),
        );
    }
    // 1. Every refusal of the claim, writing nothing.
    let known = crate::desk::desks_on_this_mac();
    if let Err(e) = crate::desk::connect_venue_check(policy, &approval, &known) {
        return (
            "409 Conflict",
            json!({ "error": e, "words": "Nothing was sent to the exchange." }),
        );
    }
    // 2. The desk names the key before it can trade: key file and claim first.
    let key_path = agent_key_path(policy);
    let old_key = fs::read(&key_path).ok();
    let old_claim = crate::desk::venue_claim_now();
    let put_back = || -> Result<(), String> {
        match &old_key {
            Some(k) => write_secret(&key_path, k)?,
            None => match fs::remove_file(&key_path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
                _ => {}
            },
        }
        crate::desk::restore_venue_claim(old_claim.as_ref())
    };
    if let Err(e) = write_secret(&key_path, format!("{}\n", key.to_bytes()).as_bytes()) {
        let _ = put_back();
        return (
            "500 Internal Server Error",
            json!({ "error": format!("the desk's key could not be kept: {e}"), "words": "Nothing was sent to the exchange." }),
        );
    }
    let mut approval = approval;
    approval["approveAgent"] = json!({ "agent": format!("{agent:#x}"), "name": name, "nonce": nonce, "signature": signature.trim() });
    if let Err(e) = crate::desk::connect_venue(policy, master, approval, now_ms() / 1000, &known) {
        let restored = put_back();
        return (
            "409 Conflict",
            json!({ "error": e, "words": "Nothing was sent to the exchange.", "restoreError": restored.err() }),
        );
    }
    // 3. The venue.
    let venue = hedge::hl::Venue::testnet();
    let refused = match venue
        .approve_agent_signed(agent, &name, nonce, signature, Some(master))
        .await
    {
        Ok(_) => None,
        // Our own check, before the venue saw anything: not a signature, or not the wallet asked.
        Err(hedge::hl::VenueError::Shape(e)) => Some((
            "400 Bad Request",
            json!({ "error": e, "words": "The wallet's answer is not a signature for this connection. Nothing was sent to the exchange; ask to connect again." }),
        )),
        // A refusal, or no answer: the venue's own list says whether it took effect.
        Err(e) => match listed(&venue, master, agent).await {
            Some(_) => None,
            None => {
                let e = e.to_string();
                Some((
                    "409 Conflict",
                    json!({ "error": e, "words": refusal_words(&e), "agent": format!("{agent:#x}") }),
                ))
            }
        },
    };
    if let Some((status, mut out)) = refused {
        if let Err(e) = put_back() {
            out["restoreError"] = json!(e);
        }
        return (status, out);
    }
    drop(key);
    let _ = fs::remove_file(pending_path(policy));
    // 4. Read back: the venue's own list of the master's API wallets names the key.
    match listed(&venue, master, agent).await {
        Some(back) => (
            "200 OK",
            json!({ "master": format!("{master:#x}"), "agent": format!("{agent:#x}"), "readBack": back, "connected": true }),
        ),
        None => (
            "502 Bad Gateway",
            json!({
                "error": "the venue accepted the approval but did not list the key when read back; the desk is connected and keeps the key; ask again in a moment",
                "master": format!("{master:#x}"), "agent": format!("{agent:#x}"), "connected": true,
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The demo account's real `clearinghouseState` shape with a PAXG short
    /// (numbers from a testnet reply; `positionValue` = |szi| × mark).
    fn chs() -> Value {
        json!({
            "marginSummary": { "accountValue": "945.912105", "totalNtlPos": "12.4506", "totalRawUsd": "958.362705", "totalMarginUsed": "6.2253" },
            "crossMarginSummary": { "accountValue": "945.912105", "totalNtlPos": "12.4506", "totalRawUsd": "958.362705", "totalMarginUsed": "6.2253" },
            "crossMaintenanceMarginUsed": "0.62253",
            "withdrawable": "939.686805",
            "assetPositions": [{ "type": "oneWay", "position": {
                "coin": "PAXG", "szi": "-0.003", "entryPx": "4150.0", "positionValue": "12.4506",
                "unrealizedPnl": "-0.0006", "returnOnEquity": "-0.0001", "marginUsed": "6.2253" } }],
            "time": 1792140000123u64
        })
    }

    #[test]
    fn the_event_carries_the_venues_numbers_and_the_k_the_rule_enforces() {
        let funding = [
            (
                1_792_140_000_000 - 25 * 3600 * 1000,
                Decimal::from_str("-9").unwrap(),
            ),
            (
                1_792_140_000_000 - 2 * 3600 * 1000,
                Decimal::from_str("-0.005724").unwrap(),
            ),
            (
                1_792_140_000_000 - 1000,
                Decimal::from_str("-0.013675").unwrap(),
            ),
        ];
        let m = venue_fields(&chs(), Some(&funding), 1_792_140_000_000).unwrap();
        // The venue's own strings, untouched.
        assert_eq!(m["accountValue"], "945.912105");
        assert_eq!(m["marginUsed"], "6.2253");
        assert_eq!(m["maintenanceMarginUsed"], "0.62253");
        let p = &m["positions"][0];
        assert_eq!(
            (&p["coin"], &p["size"], &p["entry"], &p["pnl"]),
            (
                &json!("PAXG"),
                &json!("-0.003"),
                &json!("4150.0"),
                &json!("-0.0006")
            )
        );
        // 12.4506 / 0.003 = 4150.2, exactly.
        assert_eq!(p["mark"], "4150.2");
        // Equity ÷ maintenance (945.912105 / 0.62253 = 1519.464…), never margin ÷ maintenance (= 10 at 2x).
        assert_eq!(m["kRatio"], "1519.46");
        // Funding: the last hour holds the newest payment; 24 h leaves out the one 25 h ago.
        assert_eq!(m["fundingLastHour"], "-0.013675");
        assert_eq!(m["funding24h"], "-0.019399");
        assert_eq!(m["venueTime"], 1_792_140_000_123u64);
    }

    #[test]
    fn a_flat_account_has_no_k_and_a_stale_reading_is_not_sent() {
        let mut flat = chs();
        flat["crossMaintenanceMarginUsed"] = json!("0.0");
        flat["assetPositions"] = json!([]);
        let m = venue_fields(&flat, None, 0).unwrap();
        assert_eq!(m["kRatio"], Value::Null);
        assert_eq!(
            m["fundingLastHour"],
            Value::Null,
            "funding not read yet is unknown, not zero"
        );

        let dir = std::env::temp_dir().join(format!("desk-live-{}", std::process::id()));
        let venue = Some(Address::ZERO);
        let old = VenueRead {
            state: Some((chs(), Instant::now() - Duration::from_secs(3))),
            funding: None,
            funding_read_at: None,
            error: None,
        };
        let ev = event(0, venue, Some(&old), &dir, &dir);
        assert_eq!(
            ev["accountValue"],
            Value::Null,
            "older than 2 s is never sent"
        );
        assert!(ev["venueError"].as_str().unwrap().contains("2 s"));
        let fresh = VenueRead {
            state: Some((chs(), Instant::now())),
            ..old
        };
        let ev = event(0, venue, Some(&fresh), &dir, &dir);
        assert_eq!(ev["accountValue"], "945.912105");
        assert_eq!(ev["kPolicy"], 4);
    }

    #[test]
    fn our_records_name_the_orders_their_read_back_and_the_halt() {
        let dir = std::env::temp_dir().join(format!("desk-live-rec-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let venue = Address::ZERO;
        let passes = hedge::driver::books_dir(&dir, venue);
        fs::create_dir_all(&passes).unwrap();
        fs::write(
            passes.join(hedge::store::PASSES_FILE),
            concat!(
                r#"{"at":100,"ms":812,"mode":"live","orders":[{"at":99,"coin":"PAXG","side":"sell","size":"0.003","oid":7,"readBack":"match","expected":"-0.003","read":"-0.003"}]}"#, "\n",
                r#"{"at":200,"ms":640,"mode":"dry-run","orders":[]}"#, "\n",
            ),
        )
        .unwrap();
        hedge::halt::halt(&dir, "PAXG: read back 0, expected -0.003").unwrap();
        let r = records(&dir, Some(venue), &dir);
        assert_eq!(r["passLatencyMs"], 640);
        assert_eq!(r["lastPassAt"], 200);
        assert_eq!(r["lastOrders"][0]["oid"], 7);
        assert_eq!(r["lastOrders"][0]["readBack"], "match");
        assert_eq!(r["halt"]["reason"], "PAXG: read back 0, expected -0.003");
        assert!(r["halt"]["at"].is_u64());
        assert_eq!(r["lastAnchor"], Value::Null);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_moved_running_funding_total_is_seen() {
        // The venue's own `cumFunding.sinceOpen` per position: when it moves, a
        // payment was booked and the poller reads funding again (docs/103 S3-6).
        let mut v = chs();
        v["assetPositions"][0]["position"]["cumFunding"] =
            json!({ "allTime": "0.1", "sinceOpen": "0.000052", "sinceChange": "0.000052" });
        let before = cum_funding(&v);
        assert_eq!(
            before,
            vec![("PAXG".to_string(), "\"0.000052\"".to_string())]
        );
        v["assetPositions"][0]["position"]["cumFunding"]["sinceOpen"] = json!("0.000104");
        assert_ne!(cum_funding(&v), before);
    }

    #[test]
    fn funding_is_read_again_at_each_settlement() {
        assert_eq!(next_funding_read(7200), 7202);
        assert_eq!(next_funding_read(7202), 7210);
        assert_eq!(next_funding_read(7215), 7230);
        assert_eq!(next_funding_read(7260), 10802);
        assert_eq!(next_funding_read(9000), 10802);
    }
}
