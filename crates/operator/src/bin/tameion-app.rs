//! Tameion Watch: the hosted level-0 service (docs/46 #8, docs/47).
//!
//! A treasurer pastes an address, picks a chain, and gets what the agent
//! infers from that address's own USDC transfers — obligations with evidence,
//! the next 30 days of liquidity, what Autopilot would pay versus ask about,
//! and the printed maximum loss under caps they choose. No key, no policy,
//! nothing sent. Every run is recorded so the founder can see who tried.
//!
//!   cargo run --locked -p operator --bin tameion-app
//!
//! Environment:
//!   PORT                     default 7880
//!   HOST                     default 127.0.0.1 (0.0.0.0 in a container)
//!   TAMEION_APP_DIR          default runtime/app (jobs/, runs.jsonl)
//!   TAMEION_APP_ADMIN_TOKEN  required for GET /api/runs?token=…
//!   TAMEION_CONTACT_URL      the "talk to us" link on the page
//!   TAMEION_REPO_URL         the "read the code" link (set once the repo is public)
//!   TAMEION_PRICE_MONTHLY    the founder's monthly price per protected account, USD (unset: the testnet sentence)
//!   TAMEION_RPC_ARC / TAMEION_RPC_ARBITRUM / TAMEION_RPC_BASE   keyed RPCs
//!   TAMEION_RELAYER_KEY_FILE  pays gas, is the operator (default ~/.tameion/treasury.key);
//!                             TAMEION_RELAYER_KEY (hex) wins over it, for hosts with env secrets
//!   TAMEION_LEVEL1_INVITE     word required to deploy (empty = open; local use)
//!   TAMEION_LEVEL1_EVERY_SECS one pass per tenant on this interval (unset = off)
//!   TAMEION_LEVEL1_WATCH_SECS a pass when a tenant's wallet or policy moves USDC, checked on this interval (unset = off)
//!   TAMEION_AGENT_GAS_WEI     gas the relayer gives each new tenant agent (default 5e16)
//!   TAMEION_TELEGRAM_BOT_TOKEN  with a tenant's telegramChatId (admin-set), the agent reports in Telegram after each pass
//!   TAMEION_PASS_TIMEOUT_SECS wall-clock cap on one pass (default 1200)
//!   TAMEION_ANCHOR / TAMEION_AGENT_ID / TAMEION_POLICY_DELAY   see level1.rs
//!
//! Routes: GET / (the account page, docs/53) · GET /watch (Level 0) · GET /api/chains · POST /api/watch · GET /api/watch/<id> ·
//! GET /api/level1/inbox?policy= ·
//! GET /api/runs?token= · GET /api/openapi.json · GET /healthz ·
//! Level 1 (docs/52): GET /level1 · GET /api/level1/config · POST /api/level1/deploy ·
//! POST /api/level1/challenge · POST /api/level1/execute · POST /api/level1/prepare-payees ·
//! POST /api/level1/run · POST /api/level1/mode · POST /api/level1/prepare-mode ·
//! POST /api/level1/prepare-cancel · POST /api/level1/prepare-withdraw · GET /level1/call.js ·
//! GET /api/level1/tenant/<policy> · GET /api/level1/tenants?token= ·
//! POST /api/level1/wallets · POST /api/level1/reserve-token · POST /api/level1/contacts ·
//! GET /api/level1/resolve?q=&chain=

// The inbox reply is one `json!` with sixty fields; the default limit (128)
// stops the macro at sixty-odd.
#![recursion_limit = "256"]

use alloy::primitives::Address;
use operator::level1;
use operator::watch::{CHAINS, Caps, Progress, WatchParams, chain, watch};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Semaphore};

#[derive(Clone)]
struct Job {
    id: String,
    created: u64,
    status: &'static str,
    progress: Option<Value>,
    report: Option<Value>,
    error: Option<String>,
    request: Value,
}

impl Job {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "created": self.created,
            "status": self.status,
            "progress": self.progress,
            "report": self.report,
            "error": self.error,
            "request": self.request,
        })
    }
}

/// One desk write at a time: a book is an append-only file (docs/83 W5).
static DESK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// A sale whose offset was read back wakes the daily statement task, which issues today's whole-stock file again
/// (docs/101 Y2): the lender's file stays current through the day instead of failing until the next morning.
static DESK_SOLD: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
fn desk_sold() -> &'static tokio::sync::Notify {
    DESK_SOLD.get_or_init(tokio::sync::Notify::new)
}
/// One hedge step at a time across every desk route (docs/89 H2): two signed
/// steps at once must not each size an order from the same stale position.
static DESK_TRADE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct App {
    dir: PathBuf,
    jobs: Mutex<HashMap<String, Job>>,
    /// Agent passes (they pay bills): three at a time.
    slots: Arc<Semaphore>,
    /// Watch scans (anyone may ask): their own two, so strangers' scans can
    /// never hold the slots a pass needs (audit 2026-10-15, docs/60 P-75).
    scan_slots: Arc<Semaphore>,
    /// Accounts whose agent is being registered now (ERC-8004, P-75).
    identity_busy: Mutex<std::collections::HashSet<String>>,
    counter: AtomicU64,
    admin_token: Option<String>,
    page: String,
    /// The customer page: Paid / Needs you / Protected (docs/53).
    home_page: String,
    level1_page: String,
    /// Deploys cost the relayer gas; an invite word keeps a public host from
    /// paying for strangers. Empty = open (local use).
    invite: Option<String>,
    /// Notify challenges already accepted (replay guard; ten-minute window).
    notify_used: Mutex<std::collections::HashSet<alloy::primitives::B256>>,
    /// `Idempotency-Key` on POST (PLAN 5.42, docs/62 F4): path + key → when,
    /// the body's hash, and the reply (empty while the first is in flight).
    /// ponytail: in memory, lost on restart; persist when hosted at scale.
    idempotent: Mutex<HashMap<String, IdemEntry>>,
    /// PLAN 5.49: the bill wallet's exposure, per wallet, for ten minutes
    /// (three public RPC reads and one price per refresh is too many).
    exposure_cache: Mutex<HashMap<String, (u64, Value)>>,
    /// Signed raises waiting out their delay: policy (lowercase) -> eta. The
    /// watcher applies each once due, so the owner's page need not be open
    /// (docs/66 route 2, docs/60 P-49). ponytail: in memory; the next inbox
    /// read relearns it after a restart.
    due_changes: Mutex<HashMap<String, u64>>,
    /// When a passkey-verified owner call last changed each account (policy
    /// lowercase -> unix): a run asked for after it skips the ten-minute
    /// cooldown, never the one-minute floor (docs/66 route 3).
    changed_at: Mutex<HashMap<String, u64>>,
    /// Previews being read now (docs/65 F3): one read per account at a time,
    /// whatever the file's "reading" marker says (a slow public node outlives it).
    previews: Mutex<std::collections::HashSet<String>>,
    /// docs/79 §6a #1: held from "is this name free?" to the record's save, so
    /// two reserves of one name cannot both win.
    names: Mutex<()>,
    /// What happened to which account, for `/api/level1/events` (policy in
    /// lowercase, `{event, data}`). Senders never wait; slow readers lag.
    events: tokio::sync::broadcast::Sender<(String, Value)>,
}

/// When an idempotent POST was first seen, its body's hash, and its reply.
type IdemEntry = (u64, alloy::primitives::B256, Vec<u8>);

/// The Start page's price sentence (PLAN 5.41): a flat monthly fee per
/// protected account, the read-only report (Watch) free, nothing per
/// payment; "Watch" by name lost simulated buyers (docs/64). Without a number set,
/// the truth for a testnet host.
fn price_line() -> String {
    match std::env::var("TAMEION_PRICE_MONTHLY")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'))
    {
        Some(n) => format!(
            "${n} a month per protected account. The read-only report is free. Tameion takes nothing per payment."
        ),
        None => "Nothing while Tameion runs on testnet. After that, a flat monthly fee per protected account; the read-only report stays free, and Tameion takes nothing per payment.".to_string(),
    }
}

/// One request header's value, case-insensitive name.
fn header<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    raw.lines().take_while(|l| !l.is_empty()).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// `http(s)://host` this request came to (Fly terminates TLS and says so in
/// `X-Forwarded-Proto`), for absolute links in an agent card.
fn origin_of(raw: &str) -> String {
    let host = header(raw, "host").unwrap_or("127.0.0.1");
    let proto = header(raw, "x-forwarded-proto").unwrap_or("http");
    format!("{proto}://{host}")
}

impl App {
    /// A passkey-verified owner call that succeeded marks every account of
    /// that owner as changed, so the next "Check now" reads it at once
    /// (docs/66 route 3). Keyed by the owner the assertion was checked
    /// against, never by a body field a stranger could set (review 2026-10-15:
    /// unauthenticated routes or a `policy` field let anyone lift the cooldown
    /// and spend the relayer's gas on passes).
    async fn changed_by_owner(&self, body: &str, reply: Vec<u8>) -> Vec<u8> {
        let owner = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v["owner"].as_str().map(str::to_lowercase));
        if let (true, Some(owner)) = (reply.starts_with(b"HTTP/1.1 2"), owner) {
            let now = now_secs();
            let mut changed = self.changed_at.lock().await;
            for t in level1::list(&self.dir)
                .into_iter()
                .filter(|t| t.owner.to_lowercase() == owner)
            {
                changed.insert(t.policy.to_lowercase(), now);
            }
        }
        reply
    }

    /// An invoice decision signed by an owner's passkey: `issued` (fresh, used
    /// once) and `auth` over `invoice_decide_challenge`. Without them the reply
    /// is 401 and names the challenge, so a page can compute and sign it.
    async fn owner_signed_decision(
        &self,
        tenant: &level1::Tenant,
        v: &Value,
        id: alloy::primitives::B256,
        accept: bool,
    ) -> Result<(), Vec<u8>> {
        let Some(c) = chain(&tenant.chain) else {
            return Err(json_reply(
                "400 Bad Request",
                json!({ "error": "unknown chain" }),
            ));
        };
        let Ok(policy) = Address::from_str(&tenant.policy) else {
            return Err(json_reply(
                "500 Internal Server Error",
                json!({ "error": "tenant addresses" }),
            ));
        };
        let issued = v["issued"].as_u64().unwrap_or(0);
        let ch = level1::invoice_decide_challenge(c.chain_id, policy, id, accept, issued);
        self.owner_signed(
            tenant,
            v,
            ch,
            json!({ "error": "accepting or declining an invoice needs the owner's passkey", "challenge": "keccak256(\"tameion.invoice-decide.v1\" ‖ chainId u64 ‖ policy ‖ id ‖ accept u8 ‖ issued u64)" }),
        )
        .await
    }

    /// The owner's passkey signed `ch` over the request's `issued` and `auth`,
    /// fresh and unused: the shared check behind the owner's signed decisions —
    /// an invoice (P-73) and a bill import only the owner's device opened
    /// (docs/60 P-226). `missing` is the 401 when no signature came.
    async fn owner_signed(
        &self,
        tenant: &level1::Tenant,
        v: &Value,
        ch: alloy::primitives::B256,
        missing: Value,
    ) -> Result<(), Vec<u8>> {
        let Some(c) = chain(&tenant.chain) else {
            return Err(json_reply(
                "400 Bad Request",
                json!({ "error": "unknown chain" }),
            ));
        };
        let (Some(issued), Ok(auth)) = (v["issued"].as_u64(), level1::auth_from_json(&v["auth"]))
        else {
            return Err(json_reply("401 Unauthorized", missing));
        };
        let now = now_secs();
        if issued > now + 60 || now.saturating_sub(issued) > 600 {
            return Err(json_reply(
                "400 Bad Request",
                json!({ "error": "the challenge is stale; ask for a new one" }),
            ));
        }
        let Ok(owner) = Address::from_str(&tenant.owner) else {
            return Err(json_reply(
                "500 Internal Server Error",
                json!({ "error": "tenant addresses" }),
            ));
        };
        let keys = tenant_keys(c, tenant, owner).await.map_err(|e| {
            json_reply(
                "502 Bad Gateway",
                json!({ "error": format!("reading the owner's key: {e}") }),
            )
        })?;
        verify_any_owner(&keys, ch, &auth)
            .map_err(|e| json_reply("403 Forbidden", json!({ "error": e })))?;
        // Spent only once it verified: a forged signature over the same challenge
        // must not use up the owner's own.
        if !self.notify_used.lock().await.insert(ch) {
            return Err(json_reply(
                "400 Bad Request",
                json!({ "error": "that challenge was already used" }),
            ));
        }
        Ok(())
    }

    /// The latest finished pass of an account, with when it started and its
    /// report: this run's memory first, then the files of passes from before a
    /// restart. ponytail: a directory scan when memory has none; an index when
    /// accounts have hundreds of passes.
    async fn last_done_pass(&self, policy: &str) -> Option<(u64, Value)> {
        let in_memory = {
            let jobs = self.jobs.lock().await;
            jobs.values()
                .filter(|j| j.request["level1"] == "run" && j.request["policy"] == policy)
                .filter(|j| j.status == "done")
                .max_by_key(|j| j.created)
                .and_then(|j| j.report.clone().map(|r| (j.created, r)))
        };
        if in_memory.is_some() {
            return in_memory;
        }
        let mut best: Option<(u64, Value)> = None;
        let mut rd = tokio::fs::read_dir(self.dir.join("jobs")).await.ok()?;
        while let Ok(Some(e)) = rd.next_entry().await {
            let Some(v) = tokio::fs::read_to_string(e.path())
                .await
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            else {
                continue;
            };
            if v["request"]["level1"] != "run"
                || v["request"]["policy"] != policy
                || v["status"] != "done"
            {
                continue;
            }
            let created = v["created"].as_u64().unwrap_or(0);
            if best.as_ref().is_none_or(|(c, _)| created > *c) {
                best = Some((created, v["report"].clone()));
            }
        }
        best
    }

    /// Tell the account's open event streams that something happened.
    fn emit(&self, policy: &str, event: &str, data: Value) {
        let _ = self.events.send((
            policy.to_lowercase(),
            json!({ "event": event, "data": data }),
        ));
    }
}

impl App {
    /// Start an idempotent POST: `Some(reply)` when the key was seen — the
    /// first reply again, a 422 if the body differs, a 409 while the first
    /// is still running — or `None` when this request is the first.
    async fn idem_check(&self, path: &str, key: &str, body: &str) -> Option<Vec<u8>> {
        if key.is_empty() || key.len() > 255 {
            return Some(json_reply(
                "400 Bad Request",
                json!({ "error": "Idempotency-Key must be 1 to 255 characters" }),
            ));
        }
        let now = now_secs();
        let hash = alloy::primitives::keccak256(body.as_bytes());
        let mut m = self.idempotent.lock().await;
        m.retain(|_, v| now.saturating_sub(v.0) < 86_400);
        let slot = format!("{path}\n{key}");
        match m.get(&slot) {
            // docs/115 §2: this request did nothing either way (`executed: no`).
            Some((_, h, _)) if *h != hash => Some(json_reply(
                "422 Unprocessable Entity",
                operator::refusal::refuse(
                    "idempotency_conflict",
                    operator::refusal::Executed::No,
                    "",
                    "",
                    "this Idempotency-Key was used with a different body",
                    None,
                ),
            )),
            Some((_, _, r)) if r.is_empty() => Some(json_reply(
                "409 Conflict",
                operator::refusal::refuse(
                    "idempotency_pending",
                    operator::refusal::Executed::No,
                    "",
                    "",
                    "a request with this Idempotency-Key is still running",
                    Some(2),
                ),
            )),
            Some((_, _, r)) => {
                // The first reply, marked as a replay after its status line.
                let mut out = r.clone();
                if let Some(i) = out.windows(2).position(|w| w == b"\r\n") {
                    out.splice(
                        i + 2..i + 2,
                        b"Idempotent-Replayed: true\r\n".iter().copied(),
                    );
                }
                Some(out)
            }
            None => {
                m.insert(slot, (now, hash, Vec::new()));
                None
            }
        }
    }

    /// Finish an idempotent POST: keep a 2xx reply for replay; forget
    /// anything else so the caller may retry with the same key.
    async fn idem_finish(&self, path: &str, key: &str, reply: &[u8]) {
        let slot = format!("{path}\n{key}");
        let ok = reply.starts_with(b"HTTP/1.1 2");
        let mut m = self.idempotent.lock().await;
        if ok {
            if let Some(v) = m.get_mut(&slot) {
                v.2 = reply.to_vec();
            }
        } else {
            m.remove(&slot);
        }
    }
}

/// The few lines a reserved account's page needs from a read-only report:
/// the bills, what the next 30 days of them total (the amount to fund), and
/// a first daily limit (twice the largest bill, the rule the pass uses
/// once money is there, PLAN 5.4).
fn preview_of(r: &Value, usdc: &str, ms: u64) -> Value {
    let minor = |s: &str| {
        ledger::Money::parse(s.trim_end_matches(" USDC").trim(), ledger::Currency::Usdc)
            .map(|m| m.to_minor())
            .unwrap_or(0)
    };
    let bills: Vec<Value> = r["obligations"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| {
            json!({
                "counterparty": o["counterparty"], "amount": o["amount"], "every": o["every"], "intervalSecs": o["intervalSecs"],
                "nextDue": o["nextDue"], "confidence": o["confidence"],
            })
        })
        .collect();
    let largest = bills
        .iter()
        .map(|b| minor(b["amount"].as_str().unwrap_or("")))
        .max()
        .unwrap_or(0);
    let next30 = r["liquidity30d"].as_str().unwrap_or("0.000000 USDC");
    json!({
        "status": "done",
        "readMs": ms,
        "days": r["days"],
        "transfers": r["transfers"],
        "bills": bills,
        "next30": next30,
        "fundMinor": minor(next30).to_string(),
        "suggestedLimit": (largest > 0).then(|| ledger::Money::minor(largest * 2, ledger::Currency::Usdc).to_string()),
        "usdc": usdc,
    })
}

/// What the bills of a pass report (`obligationsList`) come to over the next
/// 30 days, in USDC minor units: every due date of every bill from now (an
/// overdue one counts once, now) to now + 30 days.
fn bills_next_30(report: &Value, now: u64) -> Option<i128> {
    let list = report["obligationsList"].as_array()?;
    let end = now + 30 * 86_400;
    let mut total = 0i128;
    for o in list {
        let amount = ledger::Money::parse(
            o["amount"].as_str()?.trim_end_matches(" USDC").trim(),
            ledger::Currency::Usdc,
        )
        .ok()?
        .to_minor();
        // A bill without a cadence or a date counts nothing; it does not
        // void the total for every other bill (review 2026-10-15).
        let (Some(every), Some(next)) = (
            o["intervalSecs"].as_u64().filter(|s| *s > 0),
            o["nextDue"].as_u64(),
        ) else {
            continue;
        };
        // A full interval or more overdue: it may have ended, so once, as
        // `watch::due_within` and the Runway count it (docs/60 P-58).
        if now.saturating_sub(next) >= every {
            total += amount;
            continue;
        }
        // From its own due date, not from now: a weekly bill six days late is
        // due now and then a day later, as `watch::due_within` counts it.
        let mut due = next;
        while due <= end {
            total += amount;
            due += every;
        }
    }
    Some(total)
}

/// Every passkey that owns `owner` (the primary first, then co-owners), and
/// how many must approve a change (docs/60 P-51). An off-chain owner action is
/// accepted from any one of them; one that widens trust (a new agent key, a
/// new report address) is refused when the account needs several approvals —
/// the contract refuses one key there too (review 2026-10-15).
async fn owner_keys(
    c: &operator::watch::ChainPreset,
    owner: Address,
) -> Result<(u8, Vec<(alloy::primitives::B256, alloy::primitives::B256)>), String> {
    let client = level1::read_client(c)?;
    client.owner_signers(owner).await.map_err(|e| e.to_string())
}

/// The keys an off-chain owner action may verify against: the contract's
/// owner keys once deployed (a rotated or added key counts, a replaced one
/// does not); before that the passkey the account was reserved with.
async fn tenant_keys(
    c: &operator::watch::ChainPreset,
    tenant: &level1::Tenant,
    owner: Address,
) -> Result<Vec<(alloy::primitives::B256, alloy::primitives::B256)>, String> {
    if tenant.deployed {
        return owner_keys(c, owner).await.map(|(_, k)| k);
    }
    match (tenant.qx.as_deref(), tenant.qy.as_deref()) {
        (Some(x), Some(y)) => alloy::primitives::B256::from_str(x)
            .and_then(|x| alloy::primitives::B256::from_str(y).map(|y| vec![(x, y)]))
            .map_err(|e| e.to_string()),
        _ => Err("this account has no passkey on record".into()),
    }
}

/// The refusal for a trust-widening off-chain action on an account that needs
/// `threshold` approvals; `None` on a one-approval account.
fn needs_more_approvals(threshold: u8, what: &str) -> Option<Vec<u8>> {
    (threshold > 1).then(|| {
        json_reply(
            "403 Forbidden",
            json!({ "error": format!("this account needs {threshold} owners to approve a change; {what} can be changed only on a one-approval account for now") }),
        )
    })
}

/// `Ok` when the assertion verifies against any of `keys`; otherwise the
/// primary key's reason, which is what a one-owner account always saw.
fn verify_any_owner(
    keys: &[(alloy::primitives::B256, alloy::primitives::B256)],
    challenge: alloy::primitives::B256,
    auth: &arc::PasskeyAuth,
) -> Result<(), String> {
    let mut first = Err("this account has no passkey on record".to_string());
    for (i, &(qx, qy)) in keys.iter().enumerate() {
        match level1::verify_passkey_assertion(qx, qy, challenge, auth) {
            Ok(()) => return Ok(()),
            Err(e) if i == 0 => first = Err(e),
            Err(_) => {}
        }
    }
    first
}

/// Whether the owner settled exactly this invoice: a `SettledByPrincipal`
/// to `payee` for `units` under `id` (docs/68 F1b). Pure, so it is tested.
fn settles(
    paid: &[arc::PrincipalSettlement],
    id: alloy::primitives::B256,
    payee: Address,
    units: i128,
) -> bool {
    paid.iter()
        .any(|p| p.action_hash == id && p.to == payee && p.amount.to_minor() == units)
}

async fn invoice_settled(
    tenant: &level1::Tenant,
    id: alloy::primitives::B256,
    payee: Address,
    units: i128,
) -> Result<bool, String> {
    let c = chain(&tenant.chain).ok_or("no chain preset for this account")?;
    let client = level1::read_client(c)?;
    let policy = Address::from_str(&tenant.policy).map_err(|e| e.to_string())?;
    let head = client.block_number().await.map_err(|e| e.to_string())?;
    let paid = client
        .principal_settlements(policy, tenant.created_block.min(head), head)
        .await
        .map_err(|e| e.to_string())?;
    Ok(settles(&paid, id, payee, units))
}

/// One bill in the Runway projection: payee, minor units, interval (s),
/// next due (unix), and whether it may have ended (counted once).
type RunwayBill = (String, i128, i64, i64, bool);

/// When the owner last sent a trust-shrinking call (cancel, pause, revoke) for
/// each account (lowercase policy → unix), for `shrink_blocks_apply` (docs/60
/// P-171). ponytail: in memory; a restart forgets, and the chain's delay still holds.
fn shrinks() -> &'static std::sync::Mutex<HashMap<String, u64>> {
    static S: std::sync::OnceLock<std::sync::Mutex<HashMap<String, u64>>> =
        std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// The time of each block in `blocks` (unix seconds), read eight at a time and
/// kept: a block's time never changes, so a warm account reads none (docs/110
/// item 8). A block whose read failed is left out.
async fn block_times(client: &arc::ArcClient, chain: &str, blocks: Vec<u64>) -> HashMap<u64, u64> {
    static KEPT: std::sync::OnceLock<std::sync::Mutex<HashMap<(String, u64), u64>>> =
        std::sync::OnceLock::new();
    let kept = KEPT.get_or_init(Default::default);
    let mut out = HashMap::new();
    let mut missing = Vec::new();
    {
        let kept = kept.lock().unwrap_or_else(|e| e.into_inner());
        for b in blocks {
            match kept.get(&(chain.to_string(), b)) {
                Some(t) => {
                    out.insert(b, *t);
                }
                None if !missing.contains(&b) => missing.push(b),
                None => {}
            }
        }
    }
    let read = arc::eight_at_a_time(missing, |b| {
        let client = client.clone();
        async move { (b, client.block_timestamp(b).await) }
    })
    .await
    .unwrap_or_default();
    let mut kept = kept.lock().unwrap_or_else(|e| e.into_inner());
    for (b, t) in read {
        if let Ok(t) = t {
            let t = t.as_unix() as u64;
            kept.insert((chain.to_string(), b), t);
            out.insert(b, t);
        }
    }
    out
}

/// The key the owner's payment of a Needs-you row is bound to: the bill-cycle's
/// settlement key from the pass, stable across passes, never the proposal's
/// `actionHash`, which changes every pass (docs/60 P-175: one tap paid the same
/// 30-day bill twice under two hashes). `None` for a row without one.
/// The earliest due time of a bill the pass in `report` knew that falls after
/// `since` (when that pass ran) and at or before `now`: a bill that fell due
/// after the pass, so the pass is no answer for it (docs/60 P-229). A bill an
/// x402 receipt suggests is never paid by a pass and does not count.
fn due_after(report: &Value, since: u64, now: u64) -> Option<u64> {
    report["obligationsList"]
        .as_array()?
        .iter()
        .filter(|o| o["provenance"] != "x402 receipt")
        .filter_map(|o| o["nextDue"].as_u64())
        .filter(|d| *d > since && *d <= now)
        .min()
}

/// When the watcher should run this account's pass for a bill falling due
/// (docs/60 P-229): a few seconds after the earliest due time the last pass
/// knew that is still ahead of it, and never within a minute of that pass.
/// `None` when no bill falls due after it.
fn due_pass_at(report: &Value, last_pass: u64) -> Option<u64> {
    report["obligationsList"]
        .as_array()?
        .iter()
        .filter(|o| o["provenance"] != "x402 receipt")
        .filter_map(|o| o["nextDue"].as_u64())
        .filter(|d| *d > last_pass)
        .min()
        .map(|d| (d + 5).max(last_pass + 60))
}

/// The bills a pass found in the history (not typed, not an x402 receipt),
/// confident enough to propose, with their payees: what the owner may approve
/// before they fall due (docs/60 P-228).
fn found_bills(report: &Value) -> Vec<(Address, Value)> {
    let floor = f64::from(ledger::action::CONFIDENCE_FLOOR_PROPOSE_BP) / 10_000.0;
    report["obligationsList"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| o["samples"].as_u64().unwrap_or(0) > 0 && o["provenance"] != "x402 receipt")
        .filter(|o| o["confidence"].as_f64().unwrap_or(0.0) >= floor)
        .filter_map(|o| {
            Some((
                Address::from_str(o["counterparty"].as_str()?).ok()?,
                o.clone(),
            ))
        })
        .collect()
}

/// Why the agent asks to pay a bill it found (docs/60 P-228), in the owner's words.
fn found_bill_reason(o: &Value, delay: u64, now: u64) -> String {
    let every = o["intervalSecs"].as_u64().unwrap_or(0);
    let every = if every >= 86_400 {
        format!("{} days", every / 86_400)
    } else if every >= 3_600 {
        format!("{} hours", every / 3_600)
    } else {
        format!("{} minutes", (every / 60).max(1))
    };
    let next = o["nextDue"].as_u64().unwrap_or(0);
    let when = if next > now {
        format!(
            "next due at unix {next}. Approve it before then and the agent pays that cycle itself"
        )
    } else {
        format!(
            "it fell due at unix {next}. Approve it and the agent pays the coming cycles itself"
        )
    };
    format!(
        "it found this bill in your history: {} every {every}, paid {} times; {when}, once the payee's {delay}-second waiting time has passed",
        o["amount"].as_str().unwrap_or("?"),
        o["samples"].as_u64().unwrap_or(0),
    )
}

/// The waiting "Let the agent buy from … at … (usually …)" rows (docs/60 P-217):
/// the newest priced-out purchase per seller in the last day that no later paid
/// purchase from that seller answered. `bought` is newest first.
fn prices_to_approve(bought: &[Value], now: u64) -> Vec<Value> {
    let mut seen = std::collections::BTreeSet::new();
    let mut rows = Vec::new();
    for (i, p) in bought.iter().enumerate() {
        let seller = p["seller"].as_str().unwrap_or("").to_lowercase();
        if p["status"] != "needs-you"
            || p["band"] != true
            || now.saturating_sub(p["at"].as_u64().unwrap_or(0)) > 86_400
            || !seen.insert(seller.clone())
        {
            continue;
        }
        let answered = bought[..i].iter().any(|q| {
            q["status"] == "paid"
                && q["seller"]
                    .as_str()
                    .is_some_and(|x| x.eq_ignore_ascii_case(&seller))
        });
        if !answered {
            rows.push(json!({
                "seller": seller, "host": p["host"], "amount": p["amount"], "minor": p["minor"], "usual": p["usual"],
                "reason": p["reason"], "url": p["url"], "at": p["at"], "why": p["why"],
            }));
        }
    }
    rows
}

/// The agent key a request carries: `Authorization: Bearer`, else `?token=`.
fn bearer_of(raw: &str, query: &str) -> Option<String> {
    raw.lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.eq_ignore_ascii_case("authorization") {
                v.trim()
                    .strip_prefix("Bearer ")
                    .map(|t| t.trim().to_string())
            } else {
                None
            }
        })
        .or_else(|| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix("token="))
                .map(String::from)
        })
}

/// An events file as an outsider gets it on a locked account (docs/60 P-212):
/// per line only `seq` and the leaf hash of the event's canonical bytes, so the
/// root can be rebuilt and nothing the event says is read. A line that does
/// not parse is left out, which fails the check rather than passing it.
fn leaves_only(jsonl: &str) -> String {
    jsonl
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| {
            let canonical = alloy::hex::decode(v["canonical"].as_str()?).ok()?;
            let leaf = ledger::log::leaf_from_canonical(&canonical).to_hex();
            Some(json!({ "seq": v["seq"], "leaf": leaf }).to_string() + "\n")
        })
        .collect()
}

/// The reference a waiting row's bill-cycle was paid under, if any: the
/// bill-cycle's key (the owner's tap since docs/60 P-175), the row's own hash
/// (before it), or any reference a pass offered for this bill-cycle (P-210:
/// one cycle paid under two passes' hashes was offered again under a third).
fn paid_reference(
    row: &Value,
    by_cycle: &std::collections::BTreeMap<String, Vec<alloy::primitives::B256>>,
    executed: &std::collections::BTreeSet<alloy::primitives::B256>,
) -> Option<alloy::primitives::B256> {
    let offered = row["cycleKey"]
        .as_str()
        .and_then(|c| by_cycle.get(&c.to_lowercase()))
        .cloned()
        .unwrap_or_default();
    ["settlementKey", "actionHash"]
        .into_iter()
        .filter_map(|k| {
            row[k]
                .as_str()
                .and_then(|h| alloy::primitives::B256::from_str(h).ok())
        })
        .chain(offered)
        .find(|h| executed.contains(h))
}

/// A bill's next unpaid cycle from `due`, stepped past every cycle paid under a
/// reference a pass offered for it (docs/60 P-211: "Make it standing" started
/// an order at a typed date whose cycle was paid twice). At most a year of
/// steps; the pass and `prepare-schedule` check the chain as well.
fn next_unpaid_due(
    payee: &str,
    mut due: i64,
    every: i64,
    by_cycle: &std::collections::BTreeMap<String, Vec<alloy::primitives::B256>>,
    executed: &std::collections::BTreeSet<alloy::primitives::B256>,
) -> i64 {
    for _ in 0..366 {
        let paid = by_cycle
            .get(&format!("{}@{due}", payee.to_lowercase()))
            .is_some_and(|refs| refs.iter().any(|h| executed.contains(h)));
        if !paid || every <= 0 {
            break;
        }
        due += every;
    }
    due
}

/// Why an import may not move this payee's bill to start at `wanted`, if it may
/// not (docs/60 P-227): the bill typed for `typed` is paid up to `next_unpaid`,
/// and a start before that re-opens a paid cycle. A bill with nothing paid yet
/// may move freely.
fn reopens_paid_cycle(
    payee: &str,
    typed: u64,
    next_unpaid: Option<u64>,
    wanted: u64,
) -> Option<String> {
    let n = next_unpaid?;
    (n > typed && wanted < n).then(|| {
        format!(
            "the bill to {payee} is paid up to its cycle due {n} (unix); a schedule starting {wanted} would ask for a paid period again. Start it at or after {n}, nothing was saved"
        )
    })
}

/// Why a standing order may not start at `first_due`, if it may not: a date
/// before the bill's next unpaid cycle pays a cycle already paid (docs/60 P-211).
fn first_due_refusal(first_due: u64, next_unpaid: Option<i64>) -> Option<String> {
    let n = next_unpaid?;
    ((first_due as i64) < n).then(|| {
        format!(
            "the bill-cycle due {first_due} (unix) is already paid; this bill's next unpaid cycle is due {n}. Start the order then, or it pays a cycle twice"
        )
    })
}

fn settle_key(row: &Value) -> Option<alloy::primitives::B256> {
    row["settlementKey"]
        .as_str()
        .and_then(|h| alloy::primitives::B256::from_str(h).ok())
}

/// Whether a spent action hash paid the row's payee its amount: by the agent
/// (`Paid`) or the owner (`SettledByPrincipal`). An early-payment discount
/// pays less under the same hash, never more than `MAX_DISCOUNT_BPS` off
/// (invoice.rs refuses a larger one), so the payment must reach the bill less
/// that discount; an amount that did not parse (0) matches nothing. The next
/// contract keys `executed` by (hash, to, amount) and this check goes (docs/68 F1).
fn paid_as_named(
    named: &[(alloy::primitives::B256, Address, i128)],
    hash: alloy::primitives::B256,
    to: Option<Address>,
    want: i128,
) -> bool {
    let floor = 10_000 - i128::from(operator::MAX_DISCOUNT_BPS);
    want > 0
        && named
            .iter()
            .any(|&(h, t, units)| h == hash && Some(t) == to && units * 10_000 >= want * floor)
}

/// Funding booked by the hedge engine (`hedge::book::funding`) since `from_ms`:
/// what the margin account received net (negative: it paid) and how many
/// funding lines. Read from `runtime/hedge/<wallet>/journal.jsonl`.
fn funding_since(journal: &str, from_ms: u64) -> (i128, usize) {
    let (mut net, mut n) = (0i128, 0usize);
    for e in journal
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        if e["doc"]["kind"] != "funding" {
            continue;
        }
        let at = e["doc"]["id"]
            .as_str()
            .and_then(|id| id.rsplit('@').next()?.parse::<u64>().ok())
            .unwrap_or(0);
        if at < from_ms {
            continue;
        }
        let margin = |side: &str| -> i128 {
            e[side]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|l| l["account"] == "asset:hedge:margin")
                .filter_map(|l| l["minor"].as_str()?.parse::<i128>().ok())
                .sum()
        };
        net += margin("debits") - margin("credits");
        n += 1;
    }
    (net, n)
}

/// A proof file, read-only (docs/66 route 5): the console links the team's
/// evidence instead of naming it. Flat names only (letters, digits, `.`, `-`,
/// `_`; no path, no dot first), `.txt`/`.png`/`.json`, from `TAMEION_PROOF_DIR`
/// or `proof/` beside the service.
async fn proof_file(name: &str) -> Vec<u8> {
    let flat = !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    let ctype = match name.rsplit_once('.').map(|(_, e)| e) {
        Some("txt") => "text/plain; charset=utf-8",
        Some("png") => "image/png",
        Some("json") => "application/json",
        _ => "",
    };
    if !flat || ctype.is_empty() {
        return json_reply("404 Not Found", json!({ "error": "no such proof file" }));
    }
    let dir = std::env::var("TAMEION_PROOF_DIR").unwrap_or_else(|_| "proof".into());
    // Whatever the name resolves to must stay inside the folder (a symlink in
    // proof/ pointing out is refused), as the console's files are.
    let (Ok(root), Ok(real)) = (
        tokio::fs::canonicalize(&dir).await,
        tokio::fs::canonicalize(std::path::Path::new(&dir).join(name)).await,
    ) else {
        return json_reply("404 Not Found", json!({ "error": "no such proof file" }));
    };
    if !real.starts_with(&root) {
        return json_reply("404 Not Found", json!({ "error": "no such proof file" }));
    }
    match tokio::fs::read(&real).await {
        Ok(bytes) => respond_with("200 OK", ctype, "public, max-age=300", &bytes),
        Err(_) => json_reply("404 Not Found", json!({ "error": "no such proof file" })),
    }
}

/// The status line and headers of a reply, without its body (a HEAD answer
/// keeps the GET's `Content-Length`, as RFC 9110 allows).
fn head_of(mut reply: Vec<u8>) -> Vec<u8> {
    if let Some(i) = reply.windows(4).position(|w| w == b"\r\n\r\n") {
        reply.truncate(i + 4);
    }
    reply
}

/// A text reply gzipped when the client accepts it (the console's 1.18 MB of
/// JavaScript is 380 KB gzipped; Lighthouse LCP 8.2 s on a phone). Images,
/// small bodies and already-encoded replies pass unchanged. ponytail:
/// compressed per request; cache the immutable `/_next/static` files when
/// traffic makes the CPU matter.
fn gzip_reply(reply: Vec<u8>, accept: Option<&str>) -> Vec<u8> {
    use std::io::Write;
    // `gzip` or `gzip;q=…` with a weight above zero (`gzip;q=0` refuses it).
    let wants = accept.is_some_and(|a| {
        a.split(',').any(|e| {
            let mut parts = e.split(';').map(str::trim);
            parts.next() == Some("gzip")
                && parts.all(|p| {
                    p.strip_prefix("q=")
                        .is_none_or(|q| q.parse::<f32>().is_ok_and(|q| q > 0.0))
                })
        })
    });
    let Some(i) = reply.windows(4).position(|w| w == b"\r\n\r\n") else {
        return reply;
    };
    let head = String::from_utf8_lossy(&reply[..i]).to_string();
    let lower = head.to_ascii_lowercase();
    let text = [
        "content-type: text/",
        "application/json",
        "+json",
        "image/svg+xml",
    ]
    .iter()
    .any(|t| lower.contains(t));
    let body = &reply[i + 4..];
    if !wants || !text || body.len() < 1024 || lower.contains("content-encoding:") {
        return reply;
    }
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let Ok(gz) = enc.write_all(body).and_then(|()| enc.finish()) else {
        return reply;
    };
    let mut out = String::with_capacity(head.len() + 64);
    for line in head.split("\r\n") {
        if line.to_ascii_lowercase().starts_with("content-length:") {
            out.push_str(&format!("Content-Length: {}", gz.len()));
        } else {
            out.push_str(line);
        }
        out.push_str("\r\n");
    }
    out.push_str("Content-Encoding: gzip\r\nVary: Accept-Encoding\r\n\r\n");
    let mut v = out.into_bytes();
    v.extend_from_slice(&gz);
    v
}

/// A job id as `level1_run` / `start` make it: hex seconds, `-`, four hex digits.
fn is_job_id(id: &str) -> bool {
    id.split_once('-').is_some_and(|(t, n)| {
        !t.is_empty()
            && t.len() <= 16
            && n.len() == 4
            && t.bytes().chain(n.bytes()).all(|b| b.is_ascii_hexdigit())
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Why a bill typed at Start cannot be kept on a reserved account (docs/60
/// P-77): the same shape checks `level1::set_bills` makes, minus the
/// attestation read, which needs a policy on chain.
fn reserved_bill_problem(b: &level1::SeededBill) -> Option<&'static str> {
    if Address::from_str(b.payee.trim()).is_err() {
        return Some("payee is not an address");
    }
    if !ledger::Money::parse(&b.amount, ledger::Currency::Usdc).is_ok_and(|m| m.to_minor() > 0) {
        return Some("amount is not a USDC amount");
    }
    if b.every_days == 0 || b.every_days > 366 {
        return Some("every_days must be 1..=366");
    }
    if b.discount_bps > operator::MAX_DISCOUNT_BPS
        || (b.discount_bps > 0 && (b.early_days == 0 || b.early_days >= b.every_days))
    {
        return Some(
            "early-payment terms must be at most 10% off, and the early window shorter than the billing interval",
        );
    }
    None
}

fn respond(status: &str, ctype: &str, body: &[u8]) -> Vec<u8> {
    respond_with(status, ctype, "no-store", body)
}

fn respond_with(status: &str, ctype: &str, cache: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: {cache}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

/// The console (`apps/console`, a Next.js static export): `TAMEION_CONSOLE_DIR`,
/// default `apps/console/out` (or the pre-move `web/console/out`) under the working directory. `None` until it has
/// been built, and then `/` stays the classic page (`/classic` always is).
fn console_dir() -> Option<PathBuf> {
    // `apps/console` since the move of 2026-10-15; `web/console` for an older checkout.
    let dirs = match std::env::var("TAMEION_CONSOLE_DIR") {
        Ok(d) => vec![PathBuf::from(d)],
        Err(_) => vec!["apps/console/out".into(), "apps/console/out".into()],
    };
    dirs.into_iter().find(|d| d.join("index.html").is_file())
}

/// A request path as the segments of a file under the console, or `None`
/// for anything that could leave it: `..`, `.`, a backslash, NUL, or an
/// encoded slash. Percent-escapes are decoded (Next names chunks of a
/// `[param]` route with brackets, which browsers send as `%5B…%5D`).
fn console_segments(path: &str) -> Option<Vec<String>> {
    let mut segs = Vec::new();
    for raw in path.trim_start_matches('/').split('/') {
        if raw.is_empty() {
            continue;
        }
        let b = raw.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(h, 16).ok()?);
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        let seg = String::from_utf8(out).ok()?;
        if seg == "." || seg == ".." || seg.contains(['/', '\\', '\0']) {
            return None;
        }
        segs.push(seg);
    }
    Some(segs)
}

fn content_type(p: &std::path::Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// A console file for `path`: the file itself, `<path>.html` or
/// `<path>/index.html` (Next's export, with or without trailing slashes),
/// else the export's `404.html` for a page-like path. Content-hashed
/// `/_next/static/*` files are cached for a year; pages are revalidated.
async fn console_response(path: &str) -> Option<Vec<u8>> {
    let dir = console_dir()?;
    let segs = console_segments(path)?;
    let base = segs.iter().fold(dir.clone(), |d, s| d.join(s));
    let candidates = if segs.is_empty() {
        vec![dir.join("index.html")]
    } else {
        vec![
            base.clone(),
            PathBuf::from(format!("{}.html", base.display())),
            base.join("index.html"),
        ]
    };
    // Belt and braces: whatever resolved must still be inside the export
    // (a symlink inside it pointing out is refused too).
    let root = tokio::fs::canonicalize(&dir).await.ok()?;
    for c in candidates {
        let Ok(real) = tokio::fs::canonicalize(&c).await else {
            continue;
        };
        if !real.starts_with(&root) || !real.is_file() {
            continue;
        }
        let body = tokio::fs::read(&real).await.ok()?;
        let cache = if path.starts_with("/_next/static/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return Some(respond_with("200 OK", content_type(&real), cache, &body));
    }
    let page_like = segs.last().is_none_or(|s| !s.contains('.'));
    let nf = root.join("404.html");
    if page_like && nf.is_file() {
        let body = tokio::fs::read(&nf).await.ok()?;
        return Some(respond_with(
            "404 Not Found",
            "text/html; charset=utf-8",
            "no-cache",
            &body,
        ));
    }
    None
}

/// The ERC-8004 Identity Registry: the testnets' shared CREATE2 address, with
/// code on Arc testnet and Arbitrum Sepolia (`cast code`, 2026-10-15; `name()`
/// = "AgentIdentity"). The service's own identity (agent 896918 on Arc,
/// `level1::agent_id`) predates per-account agents; this one is the account's.
const IDENTITY_REGISTRY: &str = "0x8004A818BFB912233c491871b3d84c89A494BD9e";

fn identity_path(dir: &std::path::Path, policy: &str) -> PathBuf {
    level1::tenants_dir(dir).join(format!("{}.identity.json", policy.to_lowercase()))
}

/// An account's agent described the ERC-8004 way (registration-v1): who it
/// pays for, the hard limits a contract holds it to, and where to check.
/// `live` adds what changes (the caps read from chain now, the registration
/// once it exists); without it the card is what goes on chain at
/// registration, so nothing in it can go stale. `origin` is where its links
/// point: the request's own host for the live card; for the on-chain one
/// `TAMEION_PUBLIC_URL`, and no links at all when that is unset (a
/// `localhost` link on chain helps nobody).
async fn agent_card(
    dir: &std::path::Path,
    t: &level1::Tenant,
    origin: Option<&str>,
    live: bool,
) -> Value {
    let c = chain(&t.chain);
    let id = c.map_or(0, |c| c.chain_id);
    let explorer = c.map_or("", |c| c.explorer);
    let anchor = c
        .and_then(|c| level1::anchor_for(c).ok())
        .map(|a| format!("{a:#x}"));
    let mut card = json!({
        "type": "https://eips.ethereum.org/EIPS/eip-8004#registration-v1",
        "name": format!("Tameion agent for account {}…{}", &t.policy[..6], &t.policy[t.policy.len() - 4..]),
        "description": "Pays one Tameion account's bills: only payees its owner approved, under caps a smart contract enforces. It cannot add a payee, raise its own limit or withdraw; the owner's passkey does those.",
        "services": [
            { "name": "wallet", "endpoint": format!("eip155:{id}:{}", t.agent) },
            { "name": "account", "endpoint": format!("eip155:{id}:{}", t.policy) },
        ],
        "x402Support": true,
        "active": !t.keyless,
        "supportedTrust": [],
        "tameion": {
            "chainId": id, "account": t.policy, "agent": t.agent, "owner": t.owner,
            "anchor": anchor, "kind": t.kind,
            "explorer": { "account": format!("{explorer}/address/{}", t.policy), "agent": format!("{explorer}/address/{}", t.agent) },
            "verify": "scripts/verify_tenant.sh (two independent RPCs)",
        },
    });
    if let Some(o) = origin {
        card["image"] = json!(format!("{o}/icon.svg"));
        if let Some(sv) = card["services"].as_array_mut() {
            sv.push(json!({ "name": "agent-card", "endpoint": format!("{o}/agents/{}.json", t.policy) }));
            sv.push(json!({ "name": "MCP", "endpoint": format!("{o}/llms.txt"), "version": "tameion-mcp (stdio)" }));
        }
    }
    if !live {
        return card;
    }
    if let Some(client) = c.and_then(|c| level1::read_client(c).ok())
        && let Ok(policy) = Address::from_str(&t.policy)
        && let Ok((per, day, floor)) = client.caps(policy).await
    {
        card["tameion"]["caps"] = json!({ "perAction": per.to_string(), "daily": day.to_string(), "reserveFloor": floor.to_string(), "readFrom": "chain" });
    }
    if let Some(reg) = tokio::fs::read_to_string(identity_path(dir, &t.policy))
        .await
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
    {
        card["registrations"] = json!([{ "agentId": reg["agentId"], "agentRegistry": format!("eip155:{id}:{IDENTITY_REGISTRY}") }]);
        card["tameion"]["registrationTx"] = reg["tx"].clone();
    }
    card
}

alloy::sol! {
    function register(string agentURI) external returns (uint256 agentId);
}

/// `GET /api/level1/events?policy=<addr>`: Server-Sent Events for one
/// account, so the console redraws when something happens instead of
/// polling the inbox every 15 s. Nothing here scans: the events are sent by
/// the code that changes state (`App::emit`) — a pass queued, running (with
/// progress), done or failed; any successful owner or agent call on the
/// account (signed calls, invoices, purchases, bills, funding) as `inbox`;
/// the preview finished. On `inbox` the client re-reads the inbox, which
/// stays the one source of truth. A comment line every 15 s keeps proxies
/// from closing the stream; a client that fell behind gets one `inbox`.
async fn serve_events(app: Arc<App>, sock: &mut tokio::net::TcpStream, query: &str) {
    let tenant = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("policy="))
        .and_then(|p| level1::load(&app.dir, p));
    let Some(t) = tenant else {
        let _ = sock
            .write_all(&json_reply(
                "404 Not Found",
                json!({ "error": "no such account" }),
            ))
            .await;
        return;
    };
    let mine = t.policy.to_lowercase();
    let mut rx = app.events.subscribe();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\
         Connection: keep-alive\r\nX-Accel-Buffering: no\r\n\r\nretry: 3000\n\n\
         event: hello\ndata: {}\n\n",
        json!({ "policy": mine, "at": now_secs() })
    );
    if sock.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let mut beat = tokio::time::interval(std::time::Duration::from_secs(15));
    beat.tick().await;
    // ponytail: one task per open stream, no cap; cap it when hosted streams
    // number in the thousands.
    loop {
        let chunk = tokio::select! {
            m = rx.recv() => match m {
                Ok((p, ev)) if p == mine => Some(format!(
                    "event: {}\ndata: {}\n\n",
                    ev["event"].as_str().unwrap_or("inbox"),
                    ev["data"]
                )),
                Ok(_) => None,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    Some("event: inbox\ndata: {\"cause\":\"resync\"}\n\n".to_string())
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            _ = beat.tick() => Some(": ping\n\n".to_string()),
        };
        let Some(chunk) = chunk else { continue };
        if sock.write_all(chunk.as_bytes()).await.is_err() || sock.flush().await.is_err() {
            break;
        }
    }
}

fn json_reply(status: &str, v: Value) -> Vec<u8> {
    respond(status, "application/json", v.to_string().as_bytes())
}

/// A refusal before anything was done (`executed: no`), with the catalogue's
/// meaning and next step (docs/115 §2).
fn refused(code: &str, words: impl Into<String>) -> Value {
    operator::refusal::refuse(code, operator::refusal::Executed::No, "", "", words, None)
}

/// docs/115 §2: an agent route's reply in the agent envelope
/// (`operator::refusal::envelope`: `ok`, `contractVersion`, a refusal as one
/// object with its code), headers and status kept. A page — a browser says
/// `Sec-Fetch-Mode` on every request — keeps `error` as the sentence it shows,
/// so the console and the classic pages read what they always read.
/// ponytail: Safari before 16.4 sends no `Sec-Fetch-Mode` and gets the agent's
/// shape; a header from the console if those browsers matter.
fn shaped(reply: Vec<u8>, method: &str, path: &str, raw: &str) -> Vec<u8> {
    if !operator::refusal::agent_route(method, path) {
        return reply;
    }
    let Some(i) = reply.windows(4).position(|w| w == b"\r\n\r\n") else {
        return reply;
    };
    let head = String::from_utf8_lossy(&reply[..i]).to_string();
    if !head
        .to_ascii_lowercase()
        .contains("content-type: application/json")
    {
        return reply;
    }
    let Ok(v) = serde_json::from_slice::<Value>(&reply[i + 4..]) else {
        return reply;
    };
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let out = if header(raw, "sec-fetch-mode").is_some() {
        operator::refusal::for_page(&v)
    } else {
        operator::refusal::envelope(status, &v, operator::refusal::moves_money(path))
    };
    let Some(v) = out else {
        return reply;
    };
    let body = v.to_string();
    let mut out = String::with_capacity(head.len() + 2);
    for line in head.split("\r\n") {
        if line.to_ascii_lowercase().starts_with("content-length:") {
            out.push_str(&format!("Content-Length: {}", body.len()));
        } else {
            out.push_str(line);
        }
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.push_str(&body);
    out.into_bytes()
}

/// `GET /api/level1/status[?policy=0x…]` (docs/115 §2): the contract's date,
/// what changed, and with a policy the kill switches as they stand — `paused`
/// (the contract on Watch, read from the chain; null when that read fails)
/// and `halt` (a read-back mismatch only a human clears). Open: the mode is
/// on chain and the HALT file says nothing about money.
async fn agent_status(dir: &std::path::Path, query: &str) -> Vec<u8> {
    let mut v = json!({ "recentUpdates": operator::refusal::recent_updates(), "codes": "/api/level1/codes" });
    if let Some(p) = query.split('&').find_map(|kv| kv.strip_prefix("policy=")) {
        let Some((a, t)) = Address::from_str(p.trim())
            .ok()
            .and_then(|a| level1::load(dir, &format!("{a:#x}")).map(|t| (a, t)))
        else {
            return json_reply(
                "404 Not Found",
                json!({ "error": "no such account on this service" }),
            );
        };
        let mode = level1::mode_action(&t, "status")
            .await
            .ok()
            .map(|s| s["mode"].clone());
        let halt = dir
            .join("tenants")
            .join(format!("{a:#x}"))
            .join("log")
            .join("HALT");
        v["policy"] = json!(t.policy);
        v["paused"] = mode.as_ref().map_or(Value::Null, |m| json!(m == "Watch"));
        v["mode"] = mode.unwrap_or(Value::Null);
        v["halt"] = json!(halt.exists());
    }
    json_reply("200 OK", operator::refusal::ok(v))
}

/// The agent tells the treasurer what it did (docs/53 5.3): one JSON POST per
/// finished pass to the tenant's URL, best effort, five seconds, never
/// blocking the pass. Paid, waiting for a tap, the root, and errors.
async fn notify(tenant: &level1::Tenant, job: &Job) {
    let Some(url) = tenant.notify_url.as_deref() else {
        return;
    };
    let r = job.report.as_ref();
    let body = json!({
        "kind": "tameion.pass",
        "policy": tenant.policy,
        "chain": tenant.chain,
        "status": job.status,
        "error": job.error,
        "at": now_secs(),
        "mode": r.and_then(|r| r["mode"].as_str()),
        "reasoner": r.and_then(|r| r["reasoner"].as_str()),
        "paid": r.map(|r| r["settled"].clone()).unwrap_or(json!([])),
        "needsYou": r.and_then(|r| r["pendingSignature"].as_array().map(|a| a.iter().map(|p| json!({ "counterparty": p["counterparty"], "amount": p["amount"] })).collect::<Vec<_>>())).unwrap_or_default(),
        "escalated": r.map(|r| r["escalated"].clone()).unwrap_or(json!([])),
        "root": r.and_then(|r| r["root"].as_str()),
        "anchorTx": r.and_then(|r| r["anchorTx"].as_str()),
    });
    // The owner's URL, but never the host's own network: the same pinned,
    // redirect-free client as purchases and Web Push (docs/60 P-69).
    let client =
        operator::x402::public_client(url, std::time::Duration::from_secs(5), "tameion-webhook/1")
            .await;
    if let Err(e) = &client {
        println!("notify {} refused: {e}", tenant.policy);
    }
    if let Ok((u, c)) = client {
        let ts = now_secs();
        let raw = body.to_string();
        let mut req = c
            .post(u)
            .header("content-type", "application/json")
            .header("x-tameion-timestamp", ts.to_string());
        if let Some(secret) = tenant.notify_secret.as_deref() {
            req = req.header(
                "x-tameion-signature",
                level1::webhook_signature(secret, ts, raw.as_bytes()),
            );
        }
        match req.body(raw).send().await {
            Ok(resp) => println!("notify {} -> {}", tenant.policy, resp.status()),
            Err(e) => println!("notify {} failed: {e}", tenant.policy),
        }
    }
}

/// The same report in the team's Telegram (PLAN 5.10): a few lines a
/// treasurer reads on a phone — paid, needs you, and why — sent with the
/// host's bot token to the tenant's chat. Best effort, never blocking.
async fn notify_telegram(tenant: &level1::Tenant, job: &Job) {
    let (Some(chat), Ok(token)) = (
        tenant.telegram_chat_id.as_deref(),
        std::env::var("TAMEION_TELEGRAM_BOT_TOKEN"),
    ) else {
        return;
    };
    let r = job.report.as_ref();
    let paid: Vec<String> = r
        .and_then(|r| r["settled"].as_array())
        .into_iter()
        .flatten()
        .map(|s| {
            format!(
                "paid {} to {}",
                s["amount"].as_str().unwrap_or(""),
                short(s["counterparty"].as_str().unwrap_or(""))
            )
        })
        .collect();
    let needs: Vec<String> = r
        .and_then(|r| r["pendingSignature"].as_array())
        .into_iter()
        .flatten()
        .map(|p| {
            format!(
                "needs you: {} to {}",
                p["amount"].as_str().unwrap_or(""),
                short(p["counterparty"].as_str().unwrap_or(""))
            )
        })
        .collect();
    let mut lines = vec![format!(
        "Tameion · policy {} · {}",
        short(&tenant.policy),
        if job.status == "done" {
            "checked"
        } else {
            "pass failed"
        }
    )];
    if let Some(e) = &job.error {
        lines.push(format!("error: {e}"));
    }
    if paid.is_empty() && needs.is_empty() && job.status == "done" {
        lines.push("nothing due, nothing to approve".into());
    }
    lines.extend(paid);
    lines.extend(needs);
    if r.and_then(|r| r["reasoner"].as_str())
        .is_some_and(|x| x.contains("fallback"))
    {
        lines.push(
            "ordered by rule this time: the model was unavailable, the limits were not".into(),
        );
    }
    let text = lines.join("\n");
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build();
    if let Ok(c) = client {
        match c
            .post(&url)
            .json(&json!({ "chat_id": chat, "text": text }))
            .send()
            .await
        {
            Ok(resp) => println!("telegram {} -> {}", tenant.policy, resp.status()),
            Err(e) => println!("telegram {} failed: {e}", tenant.policy),
        }
    }
}

fn short(a: &str) -> String {
    if a.len() > 12 {
        format!("{}…{}", &a[..6], &a[a.len() - 4..])
    } else {
        a.to_string()
    }
}

fn parse_money(v: &Value) -> Option<ledger::Money> {
    v.as_str()
        .and_then(|s| ledger::Money::parse(s.trim(), ledger::Currency::Usdc).ok())
}

/// The owner contract's recovery address for a new account (docs/60 P-76,
/// founder 2026-10-15). The rule:
///
/// * body has no `recovery` (or `null`): the **watched** wallet — the one
///   whose history was read, the buyer's Safe or wallet, which for most
///   buyers is also the funding wallet. It goes into `PasskeyPrincipal`'s
///   constructor at deployment, so an account made from a pasted wallet can
///   always be recovered by that wallet with nothing but the chain
///   (`scripts/exit.sh`); Settings' `prepare-recovery` is the tap to change it.
/// * body names an address: that address wins. The zero address means
///   "no recovery path" and is honoured **only** when the body says it.
/// * body names something that is not an address: refused, never silently
///   turned into "none".
fn recovery_for(v: &Value, watched: Address) -> Result<Address, String> {
    match &v["recovery"] {
        Value::Null => Ok(watched),
        Value::String(s) => Address::from_str(s.trim()).map_err(|_| {
            "recovery must be an address (0x0 for none); omit it to use the watched wallet"
                .to_string()
        }),
        _ => Err("recovery must be a string address".to_string()),
    }
}

impl App {
    async fn persist(&self, job: &Job) {
        let p = self.dir.join("jobs").join(format!("{}.json", job.id));
        let _ = tokio::fs::create_dir_all(p.parent().unwrap()).await;
        let _ = tokio::fs::write(&p, job.json().to_string()).await;
    }

    async fn record_run(&self, job: &Job) {
        let summary = json!({
            "id": job.id,
            "created": job.created,
            "finished": now_secs(),
            "status": job.status,
            "chain": job.request["chain"],
            "address": job.request["address"],
            "days": job.request["days"],
            "obligations": job.report.as_ref().and_then(|r| r["obligations"].as_array().map(|a| a.len())),
            "transfers": job.report.as_ref().and_then(|r| r["transfers"].as_u64()),
            "liquidity30d": job.report.as_ref().and_then(|r| r["liquidity30d"].as_str().map(String::from)),
            "error": job.error,
        });
        let p = self.dir.join("runs.jsonl");
        let _ = tokio::fs::create_dir_all(&self.dir).await;
        if let Ok(mut f) = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .await
        {
            let _ = f.write_all(format!("{summary}\n").as_bytes()).await;
        }
    }

    async fn start(self: &Arc<Self>, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => {
                return json_reply(
                    "400 Bad Request",
                    json!({ "error": format!("body is not JSON: {e}") }),
                );
            }
        };
        let Some(preset) = v["chain"].as_str().and_then(chain) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "chain must be one of the keys from GET /api/chains" }),
            );
        };
        // Never 0x0: every mint is a Transfer from it (docs/79 §6a #3).
        let Some(address) = v["address"]
            .as_str()
            .and_then(|s| Address::from_str(s.trim()).ok())
            .filter(|a| !a.is_zero())
        else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "address must be a 0x… EVM address (not 0x0)" }),
            );
        };
        let days = v["days"].as_u64().unwrap_or(90).clamp(7, 365) as u32;
        let also_history: Vec<Address> = v["alsoHistory"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().and_then(|s| Address::from_str(s.trim()).ok()))
                    .filter(|a| !a.is_zero())
                    // five wallets at most: each one is another full scan
                    .take(5)
                    .collect()
            })
            .unwrap_or_default();
        let caps = match (
            parse_money(&v["caps"]["perAction"]),
            parse_money(&v["caps"]["daily"]),
            parse_money(&v["caps"]["floor"]),
        ) {
            (Some(per_action), Some(daily), Some(floor)) => Some(Caps {
                per_action,
                daily,
                floor,
            }),
            _ => None,
        };
        let caps_json = caps.as_ref().map(|c| json!({ "perAction": c.per_action.to_string(), "daily": c.daily.to_string(), "floor": c.floor.to_string() }));
        // A finished run of the same address, chain, window and caps within
        // the last hour answers at once: a scan on a public RPC takes minutes
        // and the chain has not changed much. `fresh: true` forces a rescan.
        if !v["fresh"].as_bool().unwrap_or(false) {
            let cutoff = now_secs().saturating_sub(3600);
            let wanted = format!("{address:#x}");
            let jobs = self.jobs.lock().await;
            let hit = jobs
                .values()
                .filter(|j| {
                    j.status == "done" && j.created >= cutoff && j.request["level1"].is_null()
                })
                .filter(|j| j.request["chain"] == preset.key && j.request["days"] == days)
                .filter(|j| {
                    j.request["address"]
                        .as_str()
                        .is_some_and(|a| a.eq_ignore_ascii_case(&wanted))
                })
                .filter(|j| j.request["caps"] == json!(caps_json))
                .max_by_key(|j| j.created);
            if let Some(j) = hit {
                return json_reply(
                    "200 OK",
                    json!({ "id": j.id, "status": "done", "cached": true }),
                );
            }
        }
        // The slot is taken here, before the job exists: a burst of requests
        // all passed a "free?" check and then queued without limit.
        let Ok(permit) = Arc::clone(&self.scan_slots).try_acquire_owned() else {
            return json_reply(
                "429 Too Many Requests",
                json!({ "error": "all scan slots are busy; try again in a minute" }),
            );
        };
        let id = format!(
            "{:x}-{:04x}",
            now_secs(),
            self.counter.fetch_add(1, Ordering::Relaxed) & 0xffff
        );
        let job = Job {
            id: id.clone(),
            created: now_secs(),
            status: "queued",
            progress: None,
            report: None,
            error: None,
            request: json!({
                "chain": preset.key,
                "address": format!("{address:#x}"),
                "days": days,
                "alsoHistory": also_history.iter().map(|a| format!("{a:#x}")).collect::<Vec<_>>(),
                "caps": caps_json,
            }),
        };
        self.jobs.lock().await.insert(id.clone(), job.clone());
        self.persist(&job).await;

        let app = Arc::clone(self);
        let params = WatchParams {
            chain: preset,
            address,
            also_history,
            days,
            caps,
        };
        let job_id = id.clone();
        tokio::spawn(async move {
            let id = job_id;
            let _permit = permit;
            {
                let mut jobs = app.jobs.lock().await;
                if let Some(j) = jobs.get_mut(&id) {
                    j.status = "running";
                }
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
            let app_p = Arc::clone(&app);
            let id_p = id.clone();
            let pump = tokio::spawn(async move {
                let mut n = 0u32;
                while let Some(p) = rx.recv().await {
                    let mut jobs = app_p.jobs.lock().await;
                    if let Some(j) = jobs.get_mut(&id_p) {
                        let elapsed = now_secs().saturating_sub(j.created);
                        let eta = (p.scanned_blocks > 0 && p.total_blocks > p.scanned_blocks).then(
                            || {
                                let rate = elapsed as f64 / p.scanned_blocks as f64;
                                (rate * (p.total_blocks - p.scanned_blocks) as f64).round() as u64
                            },
                        );
                        let eta = eta.map(|e| p.eta_cap_secs.map_or(e, |c| e.min(c)));
                        j.progress = Some(json!({
                            "phase": p.phase,
                            "scannedBlocks": p.scanned_blocks,
                            "totalBlocks": p.total_blocks,
                            "transfersSoFar": p.transfers_so_far,
                            "elapsedSecs": elapsed,
                            "etaSecs": eta,
                        }));
                        n += 1;
                        if n.is_multiple_of(5) {
                            let snapshot = j.clone();
                            drop(jobs);
                            app_p.persist(&snapshot).await;
                        }
                    }
                }
            });
            // A scan ends like a pass does: one wall-clock cap (P-75).
            let cap = std::env::var("TAMEION_PASS_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1200u64);
            let result = match tokio::time::timeout(
                std::time::Duration::from_secs(cap),
                watch(&params, |p| {
                    let _ = tx.send(p);
                }),
            )
            .await
            {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => Err(format!("the scan took longer than {cap} s and was stopped")),
            };
            drop(tx);
            let _ = pump.await;
            let done = {
                let mut jobs = app.jobs.lock().await;
                let j = jobs.get_mut(&id).expect("job exists");
                match result {
                    Ok(report) => {
                        j.status = "done";
                        j.report = Some(report);
                    }
                    Err(e) => {
                        j.status = "failed";
                        j.error = Some(e.to_string());
                    }
                }
                j.clone()
            };
            app.persist(&done).await;
            app.record_run(&done).await;
        });
        json_reply("202 Accepted", json!({ "id": id, "status": "queued" }))
    }

    fn invited(&self, v: &Value, query: &str) -> bool {
        match &self.invite {
            None => true,
            Some(w) if w.is_empty() => true,
            Some(w) => {
                let given = v["invite"].as_str().map(String::from).or_else(|| {
                    query
                        .split('&')
                        .find_map(|kv| kv.strip_prefix("invite="))
                        .map(String::from)
                });
                given.as_deref() == Some(w.as_str())
            }
        }
    }

    async fn level1_deploy(self: &Arc<Self>, body: &str, query: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => {
                return json_reply(
                    "400 Bad Request",
                    json!({ "error": format!("body is not JSON: {e}") }),
                );
            }
        };
        if !self.invited(&v, query) {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "an invite word is needed to deploy; ask us" }),
            );
        }
        let Some(preset) = v["chain"].as_str().and_then(chain) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "chain must be one of the keys from GET /api/chains" }),
            );
        };
        let b256 = |k: &str| {
            v[k].as_str()
                .and_then(|x| x.parse::<alloy::primitives::B256>().ok())
        };
        let (Some(qx), Some(qy)) = (b256("qx"), b256("qy")) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "qx and qy (32-byte hex, the passkey's P-256 public key) are required" }),
            );
        };
        if qx.is_zero() && qy.is_zero() {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "the passkey public key is zero" }),
            );
        }
        // docs/79 §6a #1-2: an account may open by name, with no wallet yet.
        let name = match v["name"].as_str().map(|n| n.trim().to_lowercase()) {
            None => None,
            Some(n) if level1::valid_name(&n) => Some(n),
            Some(_) => {
                return json_reply(
                    "400 Bad Request",
                    json!({ "error": "name must be 3 to 32 lowercase letters, numbers or dashes, not starting or ending with a dash" }),
                );
            }
        };
        let watched = match v["watched"].as_str() {
            Some(x) => match Address::from_str(x.trim()) {
                Ok(a) => a,
                Err(_) => {
                    return json_reply(
                        "400 Bad Request",
                        json!({ "error": "watched must be an address" }),
                    );
                }
            },
            None => Address::ZERO,
        };
        if watched.is_zero()
            && (v["pull"].as_bool() == Some(true) || v["wallet7702"].as_bool() == Some(true))
        {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "a pull or wallet account needs watched: the wallet it draws from" }),
            );
        }
        // The zero address in the salt is frozen there (§6a "why watched
        // cannot change"); only a named account may take it.
        if watched.is_zero() && name.is_none() {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "name the account, or give watched: the treasury address whose history the agent reads" }),
            );
        }
        let mut recovery = match recovery_for(&v, watched) {
            Ok(r) => r,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e })),
        };
        // The recovery address alone can replace the passkey: never one of our keys (docs/102 D10).
        // Named in the body: refused. Taken from a watched wallet that is ours (a test wallet): none.
        if level1::our_addresses(&self.dir).contains(&recovery) {
            if v["recovery"].is_string() {
                return json_reply(
                    "400 Bad Request",
                    json!({ "error": "the recovery address is Tameion's own key; name a wallet you hold, or none" }),
                );
            }
            recovery = Address::ZERO;
        }
        // A named account may leave the caps to us (§6a #5): a small default
        // that follows the bills until deploy.
        let caps = (
            parse_money(&v["caps"]["perAction"]),
            parse_money(&v["caps"]["daily"]),
            parse_money(&v["caps"]["floor"]),
        );
        let caps_provisional = name.is_some() && v["caps"].is_null();
        let (per_action, daily, floor) = match caps {
            (Some(p), Some(d), Some(f)) => (p, d, f),
            _ if caps_provisional => (
                ledger::Money::minor(level1::PROVISIONAL_PER_ACTION_MINOR, ledger::Currency::Usdc),
                ledger::Money::minor(level1::PROVISIONAL_DAILY_MINOR, ledger::Currency::Usdc),
                ledger::Money::zero(ledger::Currency::Usdc),
            ),
            _ => {
                return json_reply(
                    "400 Bad Request",
                    json!({ "error": "caps.perAction, caps.daily and caps.floor are required (USDC)" }),
                );
            }
        };
        if per_action.to_minor() > daily.to_minor() {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "perAction must not exceed daily; the contract refuses that too" }),
            );
        }
        let days = v["days"].as_u64().unwrap_or(90).clamp(7, 365) as u32;
        let req = level1::DeployRequest {
            chain: preset,
            // Pull mode (PLAN 5.26): the money stays in the bill wallet.
            pull: v["pull"].as_bool().unwrap_or(false),
            qx,
            qy,
            recovery,
            watched,
            per_action,
            daily,
            floor,
            days,
            notify_url: v["notifyUrl"]
                .as_str()
                .map(str::trim)
                .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
                .map(String::from),
            name: name.clone(),
            recovery_explicit: v["recovery"].is_string(),
            caps_provisional,
            // PLAN 5.38: no agent key at all; the account stays in Propose.
            keyless: v["keyless"].as_bool().unwrap_or(false),
            // PLAN 5.28: the bill wallet itself becomes the account (EIP-7702).
            wallet: v["wallet7702"].as_bool().unwrap_or(false),
        };
        // docs/79 §6a #1: a name is the first passkey's. The same passkey
        // asking again on the same chain gets its account back; anyone else 409.
        let _names = match &name {
            Some(_) => Some(self.names.lock().await),
            None => None,
        };
        if let Some(n) = &name {
            let held = level1::by_name(&self.dir, n);
            let mine = |t: &level1::Tenant| {
                let same = |a: &Option<String>, b: alloy::primitives::B256| {
                    a.as_deref()
                        .and_then(|x| alloy::primitives::B256::from_str(x).ok())
                        == Some(b)
                };
                same(&t.qx, qx) && same(&t.qy, qy)
            };
            // qx/qy are public (the tenant route serves them), so matching
            // them proves nothing (docs/60 P-87): the same key on the same
            // chain gets the public record back, never a reserve token; a held
            // name on another chain would let anyone who read the record open
            // it there with their own spare key, so that is refused too.
            if let Some(t) = held.iter().find(|t| t.chain == preset.key && mine(t)) {
                return json_reply(
                    "200 OK",
                    json!({ "tenant": t.public(), "explorer": preset.explorer, "reserveToken": null }),
                );
            }
            if !held.is_empty() {
                return json_reply(
                    "409 Conflict",
                    json!({ "error": format!("the name {n} is taken"), "name": n }),
                );
            }
        }
        // `lazy` (PLAN 5.32): reserve the addresses now, send nothing; the
        // first pass or the first passkey action puts the contracts on chain.
        let lazy = v["lazy"].as_bool().unwrap_or(false);
        // A new reserve comes with its reserve token (docs/60 P-87); a deployed
        // account needs none (its token gate takes over).
        let outcome = if lazy {
            level1::reserve(&self.dir, req).await
        } else {
            level1::deploy(&self.dir, req).await.map(|t| (t, None))
        };
        // PLAN 5.45: an account started from a partner's link is listed for
        // that partner (an attribution, not a permission).
        let partner = v["partner"]
            .as_str()
            .filter(|id| level1::partners(&self.dir).iter().any(|p| p.id == *id))
            .map(String::from);
        // Only a new account takes the attribution: a re-claim (anyone with
        // the public qx/qy) must not rewrite an existing record.
        let outcome = match (outcome, partner) {
            (Ok((mut t, Some(tok))), Some(id)) => {
                t.partner = Some(id);
                level1::save(&self.dir, &t).map(|_| (t, Some(tok)))
            }
            (o, _) => o,
        };
        match outcome {
            Ok((t, tok)) => json_reply(
                "200 OK",
                // Shown once; the record keeps only its sha256.
                json!({ "tenant": t.public(), "explorer": preset.explorer, "reserveToken": tok }),
            ),
            Err(e) => json_reply("500 Internal Server Error", json!({ "error": e })),
        }
    }

    async fn level1_challenge(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let (Some(preset), Some(owner), Some(target), Some(data)) = (
            v["chain"].as_str().and_then(chain),
            v["owner"]
                .as_str()
                .and_then(|x| Address::from_str(x.trim()).ok()),
            v["target"]
                .as_str()
                .and_then(|x| Address::from_str(x.trim()).ok()),
            v["data"].as_str(),
        ) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "chain, owner, target, data are required" }),
            );
        };
        // A reserved tenant's owner contract must exist before it can be
        // asked for a challenge (PLAN 5.32): materialise on the first tap --
        // the account the tap is for, or (when the owner contract itself is
        // missing) one of its reserved accounts. The owner's other reserved
        // accounts stay lazy: deploying them here cost gas nobody asked for
        // and a failure there blocked this tap (docs/60 P-29).
        let reserved: Vec<level1::Tenant> = level1::list(&self.dir)
            .into_iter()
            // A wallet account (PLAN 5.28) waits for the wallet, not for the
            // passkey: it must not block the owner's other accounts.
            .filter(|t| {
                !t.deployed && t.kind != "wallet" && Address::from_str(&t.owner).ok() == Some(owner)
            })
            .collect();
        let owner_live = match level1::read_client(preset) {
            Ok(c) => !c.code_at(owner).await.unwrap_or_default().is_empty(),
            Err(_) => false,
        };
        let pick = reserved
            .iter()
            .find(|t| Address::from_str(&t.policy).ok() == Some(target))
            .or_else(|| if owner_live { None } else { reserved.first() })
            .cloned();
        if let Some(mut t) = pick
            && let Err(e) = level1::ensure_deployed(&self.dir, &mut t).await
        {
            return json_reply(
                "502 Bad Gateway",
                json!({ "error": format!("putting your account on chain: {e}") }),
            );
        }
        match level1::challenge(preset, owner, target, data).await {
            Ok((nonce, ch)) => json_reply(
                "200 OK",
                json!({ "nonce": nonce.to_string(), "challenge": format!("{ch:#x}"), "chainId": preset.chain_id }),
            ),
            Err(e) => json_reply("500 Internal Server Error", json!({ "error": e })),
        }
    }

    async fn level1_execute(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let (Some(preset), Some(owner), Some(target), Some(data)) = (
            v["chain"].as_str().and_then(chain),
            v["owner"]
                .as_str()
                .and_then(|x| Address::from_str(x.trim()).ok()),
            v["target"]
                .as_str()
                .and_then(|x| Address::from_str(x.trim()).ok()),
            v["data"].as_str(),
        ) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "chain, owner, target, data, auth are required" }),
            );
        };
        if let Err(e) = level1::auth_from_json(&v["auth"]) {
            return json_reply("400 Bad Request", json!({ "error": e }));
        }
        // PLAN 5.7: which of the owner's keys signed (0 = the primary passkey).
        // Checked here against that key before anything is stored or relayed;
        // an owner needing several approvals collects them first.
        let key_index = v["keyIndex"].as_u64().unwrap_or(0);
        // docs/60 P-171: a cancel, a pause or a revoke only shrinks trust. From
        // the moment one is on its way, no automatic apply of a raise asked
        // before it goes ahead (`shrink_blocks_apply`): the owner's brake wins.
        let shrink = operator::agent_api::shrinks_trust(data);
        if shrink.is_some() {
            shrinks()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(format!("{target:#x}"), now_secs());
        }
        match level1::execute_by_key(
            preset, &self.dir, owner, target, data, key_index, &v["auth"],
        )
        .await
        {
            Ok(r) if r["pending"] == true => json_reply("202 Accepted", r),
            Ok(r) => {
                self.recovery_landed(owner, target, data);
                json_reply("200 OK", r)
            }
            // A cancel that found nothing pending came after the change applied.
            Err(e)
                if shrink == Some("cancel")
                    && (e.contains("NothingPending") || e.contains("175c1aea")) =>
            {
                json_reply(
                    "409 Conflict",
                    json!({ "error": e, "words": "Your cancel arrived after the change had already applied, so nothing was cancelled: the change is in effect now. A cut is instant: pause the agent or lower the limit and it applies at once." }),
                )
            }
            Err(e) => json_reply("500 Internal Server Error", json!({ "error": e })),
        }
    }

    /// docs/60 P-171: whether the owner sent a trust-shrinking call (cancel,
    /// pause, revoke) for `policy` at or after the moment its pending raise was
    /// asked, so that raise must not be applied automatically. A failed read
    /// of the pending change blocks the apply: waiting is the safe side.
    async fn shrink_blocks_apply(&self, tenant: &level1::Tenant) -> bool {
        let Some(at) = shrinks()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&tenant.policy.to_lowercase())
            .copied()
        else {
            return false;
        };
        match level1::mode_action(tenant, "status").await {
            Ok(s) => match (s["pending"]["eta"].as_u64(), s["delaySecs"].as_u64()) {
                (Some(eta), Some(delay)) => at >= eta.saturating_sub(delay),
                _ => false,
            },
            Err(_) => true,
        }
    }

    /// The owner's passkey just ran `setRecovery(next)` on its own contract:
    /// the records of that owner take it as `recovery`, and a spare key that
    /// was only proposed is settled (docs/79 §6a #4, docs/60 P-88).
    fn recovery_landed(&self, owner: Address, target: Address, data: &str) {
        let Ok(bytes) = alloy::hex::decode(data.trim()) else {
            return;
        };
        let selector = &alloy::primitives::keccak256("setRecovery(address)")[..4];
        if target != owner || bytes.len() != 36 || &bytes[..4] != selector {
            return;
        }
        let next = Address::from_slice(&bytes[16..36]);
        for mut t in level1::list(&self.dir)
            .into_iter()
            .filter(|t| Address::from_str(&t.owner).ok() == Some(owner))
        {
            t.recovery = format!("{next:#x}");
            t.spare_key = None;
            let _ = level1::save(&self.dir, &t);
        }
    }

    /// PLAN 5.7: a device asks to become a co-owner (its passkey's public
    /// key); the current owners see it on the account page and add it with
    /// the approvals they need today. Open route: the device holds no key yet.
    async fn level1_join_request(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let b256 = |k: &str| {
            v[k].as_str()
                .and_then(|x| alloy::primitives::B256::from_str(x).ok())
        };
        let (Some(policy), Some(qx), Some(qy)) = (v["policy"].as_str(), b256("qx"), b256("qy"))
        else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "policy, qx and qy are required" }),
            );
        };
        let Some(mut t) = level1::load(&self.dir, policy) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let label: String = v["label"]
            .as_str()
            .unwrap_or("another device")
            .chars()
            .take(40)
            .collect();
        let (x, y) = (format!("{qx:#x}"), format!("{qy:#x}"));
        if !t.join_requests.iter().any(|r| r.0 == x && r.1 == y) {
            // Pairing happens while the owner watches: a request older than a
            // day has expired, and the newest eight are kept — `truncate` kept
            // the oldest, so eight strangers' requests silently dropped the real
            // device's (audit 2026-10-15, docs/60 P-71).
            let now = now_secs();
            t.join_requests.retain(|r| now.saturating_sub(r.3) < 86_400);
            t.join_requests.push((x, y, label, now));
            let extra = t.join_requests.len().saturating_sub(8);
            t.join_requests.drain(..extra);
            if let Err(e) = level1::save(&self.dir, &t) {
                return json_reply("500 Internal Server Error", json!({ "error": e }));
            }
        }
        json_reply(
            "200 OK",
            json!({ "asked": true, "owners": "the account's owners see it on the account page and add it" }),
        )
    }

    /// PLAN 5.7: calldata for the owner contract to set its co-owners and how
    /// many must approve. Signed like any other change, by the approvals the
    /// account needs today.
    async fn level1_prepare_coowners(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(t) = v["policy"]
            .as_str()
            .and_then(|p| level1::load(&self.dir, p))
        else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let keys: Vec<(alloy::primitives::B256, alloy::primitives::B256)> = v["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|k| {
                Some((
                    alloy::primitives::B256::from_str(k["qx"].as_str()?).ok()?,
                    alloy::primitives::B256::from_str(k["qy"].as_str()?).ok()?,
                ))
            })
            .collect();
        let threshold = v["threshold"].as_u64().unwrap_or(1).clamp(1, 9) as u8;
        if threshold as usize > keys.len() + 1 {
            return json_reply(
                "400 Bad Request",
                json!({ "error": format!("{threshold} approvals cannot be met by {} keys", keys.len() + 1) }),
            );
        }
        json_reply(
            "200 OK",
            json!({
                "to": t.owner,
                "data": arc::set_co_owners_calldata(&keys, threshold),
                "summary": format!("owners: your first passkey and {} more; {threshold} must approve every change", keys.len()),
            }),
        )
    }

    /// docs/60 P-182 (UAT U3): a prepared call says who signs it and when it
    /// takes effect, by the contract's rule; when an agent key asked for it,
    /// the request is kept (`agent_api::record_ask`) and the inbox shows it as
    /// one row on the account page with the agent's reason and one tap.
    async fn prepared(
        &self,
        policy: Address,
        kind: &str,
        raw: &str,
        mut out: Value,
        mut ask: Value,
        applies: impl FnOnce(&Value, u64) -> String,
    ) -> Vec<u8> {
        let tenant = level1::load(&self.dir, &format!("{policy:#x}"));
        let status = match &tenant {
            Some(t) => tokio::time::timeout(
                std::time::Duration::from_secs(5),
                level1::mode_action(t, "status"),
            )
            .await
            .ok()
            .and_then(Result::ok),
            None => None,
        };
        out["signer"] = json!(operator::agent_api::SIGNER);
        out["applies"] = json!(match &status {
            Some(s) => applies(s, s["delaySecs"].as_u64().unwrap_or(0)),
            None => "at once if it lowers what the agent may do; if it raises it, after the account's delay, and the owner can cancel until then".to_string(),
        });
        let asker = tenant.as_ref().and_then(|t| {
            let b = header(raw, "authorization")?
                .strip_prefix("Bearer ")?
                .trim();
            let h = level1::token_hash(b);
            t.agent_tokens.iter().find(|k| k.hash == h).map(|k| {
                if k.label.is_empty() {
                    k.id.clone()
                } else {
                    k.label.clone()
                }
            })
        });
        out["ownerSees"] = json!(match asker {
            Some(by) => {
                ask["by"] = json!(by);
                ask["at"] = json!(now_secs());
                match operator::agent_api::record_ask(&self.dir, &format!("{policy:#x}"), kind, ask)
                {
                    Ok(()) => {
                        "one row on the account page with your reason; the owner's one tap there signs it. Tell your user to open the account page."
                    }
                    Err(_) => "the owner signs it on the account page",
                }
            }
            None => "sign it on this page",
        });
        json_reply("200 OK", out)
    }

    async fn level1_prepare_payees(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let payees: Vec<Address> = v["payees"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().and_then(|s| Address::from_str(s.trim()).ok()))
                    .collect()
            })
            .unwrap_or_default();
        if payees.is_empty() {
            return json_reply("400 Bad Request", json!({ "error": "payees is empty" }));
        }
        // `allow: false` revokes (instant on the contract); default attests.
        let allow = v["allow"].as_bool().unwrap_or(true);
        // F10 (PLAN 5.47): never attest an address the issuer has frozen.
        let tenant = level1::load(&self.dir, &format!("{policy:#x}")).filter(|_| allow);
        if let Some(t) = tenant {
            // P-184: a stalled RPC read held this answer for 60 s. Five seconds,
            // then open: USDC itself refuses a transfer to a frozen address.
            let frozen = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                level1::issuer_frozen(&t, &payees),
            )
            .await
            .unwrap_or_default();
            if !frozen.is_empty() {
                return json_reply(
                    "422 Unprocessable Entity",
                    operator::refusal::with(
                        refused("payee_frozen", level1::frozen_words(&frozen)),
                        &[("frozen", json!(frozen))],
                    ),
                );
            }
        }
        let call = level1::prepare_payees_with(policy, &payees, allow);
        self.prepared(
            policy,
            "payees",
            raw,
            json!({ "to": format!("{:#x}", call.to), "data": call.data, "summary": call.summary }),
            json!({ "payees": payees.iter().map(|a| format!("{a:#x}")).collect::<Vec<_>>(), "allow": allow, "reason": v["reason"] }),
            move |_, delay| operator::agent_api::payees_apply(allow, delay),
        )
        .await
    }

    fn level1_prepare_cancel(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let call = level1::prepare_cancel(policy);
        json_reply(
            "200 OK",
            json!({ "to": format!("{:#x}", call.to), "data": call.data, "summary": call.summary }),
        )
    }

    /// The events behind one anchored root of one tenant, as the service
    /// wrote them before anchoring: what an auditor needs to recompute the
    /// root on their own machine (`operator --example audit`, docs/46 #1).
    async fn level1_record(&self, query: &str, raw: &str) -> Vec<u8> {
        let get = |k: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
                .map(str::trim)
        };
        let (Some(policy), Some(root)) = (
            get("policy").and_then(|x| Address::from_str(x).ok()),
            get("root").and_then(|x| alloy::primitives::B256::from_str(x).ok()),
        ) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "policy and root are required" }),
            );
        };
        let path = self
            .dir
            .join("tenants")
            .join(format!("{policy:#x}"))
            .join("log")
            .join(format!("{root:#x}.events.jsonl"));
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(_) => {
                return json_reply(
                    "404 Not Found",
                    json!({ "error": "no record of that root for that policy" }),
                );
            }
        };
        // On a locked account an outsider gets what checking the root needs —
        // each event's sequence number and leaf hash — and not the events: the
        // balance, payees, amounts, purchase URLs and purposes stay with a read
        // key or the owner's device (docs/60 P-212). The root rebuilt from the
        // leaves is the same root, so "Check it yourself" still matches.
        // A tenant that cannot be read is treated as locked: leaves are safe
        // for anyone, events are not (docs/60 P-221).
        let open = match level1::load(&self.dir, &format!("{policy:#x}")) {
            Some(t) => t.agent_tokens.is_empty() || self.reads_account(&t, raw, query),
            None => false,
        };
        if open {
            return respond("200 OK", "application/x-ndjson; charset=utf-8", &bytes);
        }
        respond(
            "200 OK",
            "application/x-ndjson; charset=utf-8",
            leaves_only(&String::from_utf8_lossy(&bytes)).as_bytes(),
        )
    }

    /// Whether this request may read a locked account: the owner's device, or
    /// an agent key with the `read` scope (the inbox's own rule, `token_gate`).
    fn reads_account(&self, t: &level1::Tenant, raw: &str, query: &str) -> bool {
        level1::reserve_token_ok(t, header(raw, "x-tameion-device"))
            || level1::token_allows(t, bearer_of(raw, query).as_deref(), "read", now_secs()).is_ok()
    }

    /// The EIP-712 burn intent for the bill wallet to sign: move `amount` of
    /// its Circle Gateway USDC on `sourceDomain` to this tenant's policy
    /// (PLAN 5.15). Fee and expiry come from Gateway's estimate; the data
    /// names the policy as the only recipient. Nothing here signs.
    async fn level1_prepare_fund(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let depositor = v["from"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
            // The account's first wallet, never 0x0 (docs/79 §6a #3).
            .or_else(|| level1::history_sources(&tenant).first().copied());
        let Some(depositor) = depositor else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "from (the bill wallet) is required" }),
            );
        };
        let Some(source) = v["sourceDomain"].as_u64() else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "sourceDomain (Gateway domain id) is required" }),
            );
        };
        // `all`: everything on that domain, less Circle's fee cap; else `amount`.
        let all = v["all"].as_bool().unwrap_or(false);
        let amount = if all {
            let h = operator::watch::gateway_holdings(c, depositor).await;
            let on_domain = h["chains"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|x| x["domain"].as_u64() == Some(source))
                .and_then(|x| x["balance"].as_str())
                .and_then(|b| {
                    ledger::Money::parse(b.trim_end_matches(" USDC"), ledger::Currency::Usdc).ok()
                })
                .map(|m| m.to_minor());
            match on_domain {
                Some(m) => m,
                None => {
                    return json_reply(
                        "400 Bad Request",
                        json!({ "error": format!("nothing in Gateway on domain {source} for {depositor:#x}"), "gateway": h }),
                    );
                }
            }
        } else {
            match ledger::Money::parse(v["amount"].as_str().unwrap_or(""), ledger::Currency::Usdc) {
                Ok(m) => m.to_minor(),
                Err(e) => {
                    return json_reply(
                        "400 Bad Request",
                        json!({ "error": format!("amount: {e}") }),
                    );
                }
            }
        };
        match operator::watch::gateway_fund_typed_data(c, source, depositor, policy, amount, all)
            .await
        {
            Ok(v) => json_reply("200 OK", v),
            Err(e) => json_reply("502 Bad Gateway", json!({ "error": e })),
        }
    }

    /// The signed burn intent goes to Gateway; the relayer pays gas for the
    /// mint on the policy's chain. The intent must name this tenant's policy
    /// as recipient on this chain, so the relayer never mints anywhere else.
    async fn level1_fund(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let intent = &v["burnIntent"];
        let Some(sig) = v["signature"].as_str() else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "burnIntent and signature are required" }),
            );
        };
        let spec = &intent["spec"];
        let want_recipient = operator::watch::bytes32_of(policy);
        if spec["destinationRecipient"].as_str().map(str::to_lowercase)
            != Some(want_recipient.to_lowercase())
            || spec["destinationDomain"].as_u64() != operator::watch::gateway_domain_of(c)
        {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "this intent does not pay this tenant's policy on its chain; the relayer only mints there" }),
            );
        }
        let t = match operator::watch::gateway_transfer(c, intent, sig).await {
            Ok(t) => t,
            Err(e) => return json_reply("502 Bad Gateway", json!({ "error": e })),
        };
        let (Some(att), Some(osig)) = (t["attestation"].as_str(), t["signature"].as_str()) else {
            return json_reply(
                "502 Bad Gateway",
                json!({ "error": "gateway answered without an attestation", "gateway": t }),
            );
        };
        let (Ok(att), Ok(osig)) = (alloy::hex::decode(att), alloy::hex::decode(osig)) else {
            return json_reply(
                "502 Bad Gateway",
                json!({ "error": "gateway attestation is not hex", "gateway": t }),
            );
        };
        let key = match level1::relayer_key() {
            Ok(k) => k,
            Err(e) => return json_reply("500 Internal Server Error", json!({ "error": e })),
        };
        let client = match arc::ArcClient::with_signer(&operator::watch::rpc_for(c), &key) {
            Ok(c) => c,
            Err(e) => {
                return json_reply(
                    "500 Internal Server Error",
                    json!({ "error": e.to_string() }),
                );
            }
        };
        let minter =
            Address::from_str(operator::watch::GATEWAY_MINTER_TESTNET).expect("minter address");
        match client
            .send_call(minter, operator::watch::gateway_mint_calldata(&att, &osig))
            .await
        {
            Ok(tx) => json_reply(
                "200 OK",
                json!({
                    "transferId": t["transferId"], "fees": t["fees"], "expirationBlock": t["expirationBlock"],
                    "mintTx": format!("{tx:#x}"), "explorer": format!("{}/tx/{tx:#x}", c.explorer),
                    "policy": format!("{policy:#x}"),
                }),
            ),
            Err(e) => json_reply(
                "502 Bad Gateway",
                json!({ "error": format!("mint on {}: {e}", c.name), "transferId": t["transferId"], "attestation": t["attestation"], "signature": t["signature"] }),
            ),
        }
    }

    fn level1_prepare_recovery(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let addr = |k: &str| v[k].as_str().and_then(|x| Address::from_str(x.trim()).ok());
        let (Some(owner), Some(recovery)) = (addr("owner"), addr("recovery")) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "owner and recovery are required (recovery may be the zero address to clear)" }),
            );
        };
        let call = level1::prepare_recovery(owner, recovery);
        json_reply(
            "200 OK",
            json!({ "to": format!("{:#x}", call.to), "data": call.data, "summary": call.summary }),
        )
    }

    async fn level1_prepare_limits(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let (Some(per), Some(day), Some(floor)) = (
            parse_money(&v["perAction"]),
            parse_money(&v["daily"]),
            parse_money(&v["floor"]),
        ) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "perAction, daily and floor (USDC) are required" }),
            );
        };
        match level1::prepare_limits(policy, per, day, floor) {
            Ok(call) => {
                let asked = [per.to_minor(), day.to_minor(), floor.to_minor()];
                self.prepared(
                    policy,
                    "limits",
                    raw,
                    json!({ "to": format!("{:#x}", call.to), "data": call.data, "summary": call.summary }),
                    json!({ "perAction": per.to_string(), "daily": day.to_string(), "floor": floor.to_string(), "reason": v["reason"] }),
                    move |s, delay| {
                        let m = |k: &str| {
                            ledger::Money::parse(
                                s["caps"][k].as_str().unwrap_or("0").trim_end_matches(" USDC").trim(),
                                ledger::Currency::Usdc,
                            )
                            .map_or(0, |x| x.to_minor())
                        };
                        operator::agent_api::limits_apply(
                            [m("perAction"), m("daily"), m("floor")],
                            asked,
                            delay,
                        )
                    },
                )
                .await
            }
            Err(e) => json_reply("400 Bad Request", json!({ "error": e })),
        }
    }

    /// What a treasurer needs on one screen: what was paid, what needs them,
    /// the most the agent can pay out in a day, and whether a pass is
    /// running. Aggregated from this service's finished passes for the
    /// policy and one read of the contract.
    /// What the agent can already say about a reserved account's bill
    /// wallet before any money arrives (docs/65 F3, F4): the read-only
    /// report (`/watch`) over 90 days under the caps the owner chose, kept in
    /// `tenants/<policy>.preview.json` for an hour. Started by the first
    /// inbox read of an account with no pass yet; the page polls the inbox.
    /// Reading moves nothing and costs no gas.
    async fn preview(self: &Arc<Self>, tenant: &level1::Tenant) -> Value {
        let path = level1::preview_path(&self.dir, &tenant.policy);
        let now = now_secs();
        // The wallets read are the account's history sources, never 0x0
        // (docs/79 §6a #3); a read of another set of wallets is not this one.
        let sources = level1::history_sources(tenant);
        let key = level1::sources_key(tenant);
        let cached: Option<Value> = tokio::fs::read_to_string(&path)
            .await
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .filter(|v| v["sources"] == key);
        let fresh = cached.as_ref().is_some_and(|v| {
            let at = v["at"].as_u64().unwrap_or(0);
            // A read that died with the process is retried after five minutes.
            let ttl = if v["status"] == "reading" { 300 } else { 3600 };
            at + ttl > now
        });
        if fresh {
            return cached.unwrap_or(Value::Null);
        }
        if !self.previews.lock().await.insert(tenant.policy.clone()) {
            return cached.unwrap_or(json!({ "status": "reading", "at": now }));
        }
        let (Some(c), Some(&w)) = (chain(&tenant.chain), sources.first()) else {
            self.previews.lock().await.remove(&tenant.policy);
            return Value::Null;
        };
        let money = |s: &str| {
            ledger::Money::parse(s.trim_end_matches(" USDC").trim(), ledger::Currency::Usdc)
        };
        let caps = match (
            money(&tenant.per_action),
            money(&tenant.daily),
            money(&tenant.floor),
        ) {
            (Ok(per_action), Ok(daily), Ok(floor)) => Some(Caps {
                per_action,
                daily,
                floor,
            }),
            _ => None,
        };
        let reading = json!({ "status": "reading", "at": now, "sources": key });
        let _ = tokio::fs::write(&path, reading.to_string()).await;
        let params = WatchParams {
            chain: c,
            address: w,
            also_history: sources[1..].to_vec(),
            days: 90,
            caps,
        };
        let usdc = c
            .usdc
            .map(String::from)
            .unwrap_or_else(|| format!("{:#x}", arc::USDC_ADDRESS));
        let (app, key, sources_key) = (Arc::clone(self), tenant.policy.clone(), key);
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let out = match watch(&params, |_| {}).await {
                Ok(r) => preview_of(&r, &usdc, started.elapsed().as_millis() as u64),
                Err(e) => json!({ "status": "failed", "error": e.to_string() }),
            };
            let mut out = out;
            out["at"] = json!(now_secs());
            out["sources"] = sources_key;
            let _ = tokio::fs::write(&path, out.to_string()).await;
            // docs/79 §6a #5: untyped caps follow the bills just found.
            if let Some(mut t) = level1::load(&app.dir, &key)
                && t.caps_provisional
                && !t.deployed
            {
                level1::refresh_provisional_caps(&app.dir, &mut t);
                let _ = level1::save(&app.dir, &t);
            }
            app.previews.lock().await.remove(&key);
            app.emit(&key, "inbox", json!({ "cause": "preview" }));
        });
        reading
    }

    /// `POST /api/level1/identity {policy}`: the account's own agent registers
    /// itself in the ERC-8004 Identity Registry on the account's chain — it
    /// signs and pays its own gas, so it owns its identity (an ERC-721) — with
    /// the agent card as an on-chain `data:` URI. Once per account: a second
    /// call returns the first registration. A keyless account has no agent.
    async fn level1_identity(&self, body: &str) -> Vec<u8> {
        use alloy::sol_types::SolCall;
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let Some(t) = v["policy"]
            .as_str()
            .and_then(|p| level1::load(&self.dir, p))
        else {
            return json_reply("404 Not Found", json!({ "error": "no such account" }));
        };
        if let Ok(s) = tokio::fs::read_to_string(identity_path(&self.dir, &t.policy)).await {
            return respond("200 OK", "application/json", s.as_bytes());
        }
        let Some(c) = chain(&t.chain) else {
            return json_reply("404 Not Found", json!({ "error": "unknown chain" }));
        };
        let Ok(owner) = Address::from_str(&t.owner) else {
            return json_reply(
                "500 Internal Server Error",
                json!({ "error": "owner is not an address" }),
            );
        };
        let Some(key) = level1::agent_key(&self.dir, owner).filter(|_| !t.keyless) else {
            return json_reply(
                "409 Conflict",
                refused(
                    "account_keyless",
                    "this account has no agent key: nothing to register",
                ),
            );
        };
        let public = std::env::var("TAMEION_PUBLIC_URL").ok();
        let card = agent_card(&self.dir, &t, public.as_deref(), false).await;
        use base64::Engine;
        let uri = format!(
            "data:application/json;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(card.to_string())
        );
        let registry = Address::from_str(IDENTITY_REGISTRY).expect("registry address");
        let rpc = operator::watch::rpc_for(c);
        // One registration per account (audit 2026-10-15, P-75): parallel
        // calls each sent `register` from the agent's gas, and a lagging
        // receipt read left no file so the next call registered again. The
        // transaction is written down the moment it is sent.
        let pending = identity_path(&self.dir, &t.policy).with_extension("pending");
        if let Ok(tx) = tokio::fs::read_to_string(&pending).await {
            return json_reply(
                "409 Conflict",
                json!({ "error": format!("a registration was already sent in {}; check it before asking again", tx.trim()) }),
            );
        }
        if !self
            .identity_busy
            .lock()
            .await
            .insert(t.policy.to_lowercase())
        {
            return json_reply(
                "409 Conflict",
                json!({ "error": "this account's agent is being registered now" }),
            );
        }
        let sent = async {
            let client = arc::ArcClient::with_signer(&rpc, &alloy::hex::encode(key.to_bytes()))
                .map_err(|e| e.to_string())?;
            let tx = client
                .send_call(registry, registerCall { agentURI: uri }.abi_encode())
                .await
                .map_err(|e| e.to_string())?;
            let _ = tokio::fs::write(&pending, format!("{tx:#x}")).await;
            // The new agent's id: the registry's mint (Transfer from zero).
            let provider = alloy::providers::ProviderBuilder::new()
                .connect_http(rpc.parse().map_err(|e| format!("{e}"))?);
            use alloy::providers::Provider;
            let receipt = provider
                .get_transaction_receipt(tx)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("no receipt")?;
            let minted = alloy::primitives::keccak256("Transfer(address,address,uint256)");
            let id = receipt
                .inner
                .logs()
                .iter()
                .find(|l| {
                    l.address() == registry && l.topics().len() == 4 && l.topics()[0] == minted
                })
                .map(|l| alloy::primitives::U256::from_be_bytes(l.topics()[3].0))
                .ok_or("registered, but no mint in the receipt")?;
            Ok::<_, String>((tx, id))
        }
        .await;
        self.identity_busy
            .lock()
            .await
            .remove(&t.policy.to_lowercase());
        match sent {
            Ok((tx, id)) => {
                let _ = tokio::fs::remove_file(&pending).await;
                let reg = json!({
                    "agentId": id.to_string(), "agentRegistry": format!("eip155:{}:{IDENTITY_REGISTRY}", c.chain_id),
                    "tx": format!("{tx:#x}"), "explorerTx": format!("{}/tx/{tx:#x}", c.explorer),
                    "owner": t.agent, "at": now_secs(),
                });
                let _ =
                    tokio::fs::write(identity_path(&self.dir, &t.policy), reg.to_string()).await;
                json_reply("201 Created", reg)
            }
            Err(e) => json_reply("502 Bad Gateway", json!({ "error": e })),
        }
    }

    /// `GET /api/level1/purchases.jsonl?policy=` — the account's x402
    /// purchase lines, oldest first, one JSON object per line (receipt v1,
    /// docs/76 §6): url, seller, amount, network, tx, id (the x402
    /// payment identifier or the settlement transaction), the seller's
    /// receipt when it sent one, sha256 of the bytes returned, the reason.
    /// Gated like the inbox (`read` scope once the owner minted a key).
    fn level1_purchases(&self, query: &str) -> Vec<u8> {
        let Some(policy) = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("policy="))
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        respond(
            "200 OK",
            "application/x-ndjson",
            operator::x402::jsonl(&self.dir, &tenant.policy).as_bytes(),
        )
    }

    async fn level1_inbox(self: &Arc<Self>, query: &str, raw: &str) -> Vec<u8> {
        let Some(policy) = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("policy="))
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        // `TAMEION_INBOX_TIMING=1`: where the inbox's time goes, on stderr (docs/110 item 8).
        let t0 = std::time::Instant::now();
        let timing = std::env::var_os("TAMEION_INBOX_TIMING").is_some();
        let tm = |what: &str| {
            if timing {
                eprintln!(
                    "inbox {policy:#x}: {what} at {} ms",
                    t0.elapsed().as_millis()
                );
            }
        };
        // The chain and network reads below are independent: start them all
        // at once (on a public node they were ~7 s one after another).
        // A person waits on this page: reads that stall fail in seconds (docs/110 item 8).
        let rc = operator::watch::chain(&tenant.chain).and_then(|c| level1::quick_client(c).ok());
        let owner_addr = Address::from_str(&tenant.owner).ok();
        let (
            status,
            mine_pre,
            terms_pre,
            elsewhere_pre,
            signers_pre,
            schedules_pre,
            expiry_pre,
            guardians_pre,
            agent_paid_pre,
        ) = tokio::join!(
            level1::mode_action(&tenant, "status"),
            async {
                let client = rc.as_ref()?;
                let head = client.block_number().await.ok()?;
                client
                    .principal_settlements(policy, tenant.created_block.min(head), head)
                    .await
                    .ok()
            },
            async {
                match &rc {
                    Some(client) => client.terms(policy).await,
                    None => None,
                }
            },
            async {
                match (
                    operator::watch::chain(&tenant.chain),
                    level1::history_sources(&tenant).first(),
                ) {
                    (Some(c), Some(&w)) => operator::watch::gateway_holdings(c, w).await,
                    _ => {
                        json!({ "source": "Circle Gateway", "error": "no chain preset or bill wallet for this tenant" })
                    }
                }
            },
            async {
                match (&rc, owner_addr) {
                    (Some(client), Some(o)) if tenant.deployed => {
                        client.owner_signers(o).await.ok()
                    }
                    _ => None,
                }
            },
            async {
                match &rc {
                    // A failed read is not "no standing orders": that would offer
                    // "Make it standing" for a bill an order already pays, and a
                    // second order pays it twice (audit 2026-10-15, docs/60 P-74).
                    Some(client) => client.schedules(policy).await.ok(),
                    None => None,
                }
            },
            async {
                match &rc {
                    Some(client) => client.agent_expiry(policy).await,
                    None => None,
                }
            },
            async {
                match (&rc, owner_addr) {
                    (Some(client), Some(o)) => client.guardians(o).await,
                    _ => None,
                }
            },
            async {
                let client = rc.as_ref()?;
                let head = client.block_number().await.ok()?;
                client
                    .paid_events(policy, tenant.created_block.min(head), head)
                    .await
                    .ok()
            },
        );
        tm("first chain reads (joined)");
        let status = match status {
            Ok(v) => v,
            Err(e) => return json_reply("502 Bad Gateway", json!({ "error": e })),
        };
        if let Some(eta) = status["pending"]["eta"].as_u64() {
            self.due_changes
                .lock()
                .await
                .insert(tenant.policy.to_lowercase(), eta);
        }
        // Every level-1 pass of this policy this service knows: memory first,
        // then the files (passes from before a restart).
        // ponytail: a directory scan per request; an index when tenants have
        // hundreds of passes.
        let mut passes: Vec<Job> = {
            let jobs = self.jobs.lock().await;
            jobs.values()
                .filter(|j| j.request["level1"] == "run" && j.request["policy"] == tenant.policy)
                .cloned()
                .collect()
        };
        if let Ok(mut rd) = tokio::fs::read_dir(self.dir.join("jobs")).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                let Ok(text) = tokio::fs::read_to_string(e.path()).await else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if v["request"]["level1"] != "run" || v["request"]["policy"] != tenant.policy {
                    continue;
                }
                let id = v["id"].as_str().unwrap_or("").to_string();
                if passes.iter().any(|j| j.id == id) {
                    continue;
                }
                passes.push(Job {
                    id,
                    created: v["created"].as_u64().unwrap_or(0),
                    status: match v["status"].as_str() {
                        Some("done") => "done",
                        Some("failed") => "failed",
                        Some("running") => "running",
                        _ => "queued",
                    },
                    progress: v.get("progress").cloned().filter(|p| !p.is_null()),
                    report: v.get("report").cloned().filter(|r| !r.is_null()),
                    error: v["error"].as_str().map(String::from),
                    request: v["request"].clone(),
                });
            }
        }
        passes.sort_by_key(|j| j.created);
        let explorer = operator::watch::chain(&tenant.chain)
            .map(|c| c.explorer)
            .unwrap_or("");
        let running = passes
            .iter()
            .rev()
            .find(|j| j.status == "queued" || j.status == "running")
            .map(|j| json!({ "id": j.id, "progress": j.progress }));
        let last = passes
            .iter()
            .rev()
            .find(|j| j.status == "done" && j.report.is_some());
        tm("passes loaded");
        let mut paid: Vec<Value> = Vec::new();
        let mut paid_minor: i128 = 0;
        for j in passes.iter().filter(|j| j.status == "done") {
            let Some(r) = &j.report else { continue };
            for s in r["settled"].as_array().into_iter().flatten() {
                let amount = s["amount"].as_str().unwrap_or("");
                paid_minor += ledger::Money::parse(
                    amount.trim_end_matches(" USDC").trim(),
                    ledger::Currency::Usdc,
                )
                .map(|m| m.to_minor())
                .unwrap_or(0);
                paid.push(json!({
                    "counterparty": s["counterparty"], "amount": amount, "tx": s["tx"], "at": j.created,
                    "explorer": format!("{explorer}/tx/{}", s["tx"].as_str().unwrap_or("")),
                    // PLAN 5.56: paid early for the invoice's discount.
                    "early": s["early"], "saved": s["saved"], "dueAt": s["dueAt"],
                }));
            }
        }
        paid.reverse();
        // PLAN 5.54: what the agent bought (x402), each with what and why.
        let bought = operator::x402::purchases(&self.dir, &tenant.policy);
        for p in bought.iter().filter(|p| p["status"] == "paid") {
            paid_minor += p["minor"]
                .as_str()
                .and_then(|m| m.parse::<i128>().ok())
                .unwrap_or(0);
            paid.push(json!({
                "counterparty": p["seller"], "amount": p["amount"], "tx": p["tx"], "at": p["at"],
                "explorer": p["explorer"], "bought": p["host"], "reason": p["reason"],
                "actionHash": p["actionHash"],
            }));
        }
        // Every payment on chain under its action hash, agent's and owner's:
        // a row is settled only if its hash paid its payee (docs/68 F1c).
        // Both reads cover the policy's whole life; if either failed, the
        // check is skipped (a missing read must not cry "misused").
        let named_complete = agent_paid_pre.is_some() && mine_pre.is_some();
        let named: Vec<(alloy::primitives::B256, Address, i128)> = agent_paid_pre
            .iter()
            .flatten()
            .chain(mine_pre.iter().flatten())
            .map(|e| (e.action_hash, e.to, e.amount.to_minor()))
            .collect();
        // Block times never change: read once, eight at a time, then kept
        // (docs/110 item 8: one read per payment, in sequence, on every call).
        let times = match rc.as_ref() {
            Some(client) => {
                let blocks: Vec<u64> = agent_paid_pre
                    .iter()
                    .flatten()
                    .chain(mine_pre.iter().flatten())
                    .map(|e| e.block)
                    .collect();
                block_times(client, &tenant.chain, blocks).await
            }
            None => HashMap::new(),
        };
        // The chain is the record (docs/60 P-35): an agent payment whose pass
        // failed afterwards left no report, but the policy's `Paid` event is
        // there. Anything on chain and not yet listed is added, by tx.
        if let (Some(_), Some(events)) = (rc.as_ref(), agent_paid_pre) {
            for ev in events {
                let tx = format!("{:#x}", ev.tx);
                let action = format!("{:#x}", ev.action_hash);
                // Listed already: by its transaction, or -- for a purchase whose
                // seller never reported the transaction -- by its action hash.
                if paid.iter().any(|p| {
                    p["tx"]
                        .as_str()
                        .is_some_and(|t| t.eq_ignore_ascii_case(&tx))
                        || p["actionHash"]
                            .as_str()
                            .is_some_and(|a| a.eq_ignore_ascii_case(&action))
                }) {
                    continue;
                }
                let at = times.get(&ev.block).copied().unwrap_or(0);
                paid_minor += ev.amount.to_minor();
                paid.push(json!({
                    "counterparty": format!("{:#x}", ev.to), "amount": ev.amount.to_string(),
                    "tx": tx, "at": at, "explorer": format!("{explorer}/tx/{:#x}", ev.tx),
                }));
            }
        }
        paid.sort_by_key(|p| std::cmp::Reverse(p["at"].as_u64().unwrap_or(0)));
        // The owner's own settlements (Approve & pay, Pay) are payments too:
        // read from the policy's SettledByPrincipal events since its deploy
        // block, each with its block's time, and counted under Paid as
        // "by you". Without this the tile undercounted (seen 2026-10-14).
        let by_agent = paid.len();
        let mine = match (rc.as_ref(), mine_pre) {
            (Some(_), Some(m)) => Some(m),
            _ => None,
        };
        if let Some(mine) = mine {
            let mut own: Vec<Value> = Vec::new();
            for m in mine {
                let at = times.get(&m.block).copied().unwrap_or(0);
                paid_minor += m.amount.to_minor();
                own.push(json!({
                    "counterparty": format!("{:#x}", m.to), "amount": m.amount.to_string(),
                    "tx": format!("{:#x}", m.tx), "at": at, "by": "you",
                    "explorer": format!("{explorer}/tx/{:#x}", m.tx),
                }));
            }
            own.reverse();
            paid.extend(own);
            paid.sort_by_key(|p| std::cmp::Reverse(p["at"].as_u64().unwrap_or(0)));
        }
        tm("paid rows (block times)");
        let paid_by_you = paid.len().saturating_sub(by_agent);
        let needs_you: Vec<Value> = last
            .and_then(|j| j.report.as_ref())
            .map(|r| {
                let why: std::collections::HashMap<String, String> = r["escalated"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|e| {
                        let who = e["counterparty"].as_str().unwrap_or("").to_lowercase();
                        let problems: Vec<String> = e["problems"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))).collect();
                        (who, problems.join(" · "))
                    })
                    .collect();
                r["pendingSignature"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|p| {
                        let who = p["counterparty"].as_str().unwrap_or("").to_string();
                        json!({
                            "counterparty": who, "amount": p["amount"], "summary": p["summary"], "to": p["to"], "data": p["data"],
                            // `actionHash` names this pass's proposal (it changes every
                            // pass); `settlementKey` names the bill-cycle and is what any
                            // payment of it is keyed to (docs/60 P-175).
                            "actionHash": p["actionHash"], "settlementKey": p["settlementKey"], "cycleKey": p["cycleKey"],
                            "attest": p["attest"], "why": why.get(&who.to_lowercase()).cloned().unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        // PLAN 5.23: bind every prepared approval to the terms the owner is
        // looking at right now, for ten minutes. The page recomputes the same
        // hash from `terms` before the passkey signs; the contract refuses an
        // approval whose terms moved in between. A policy from before
        // fingerprints keeps its plain calls.
        let terms = terms_pre;
        let terms_json = terms.as_ref().map(|t| {
            json!({
                "perActionCap": t.per_action_cap.to_string(), "dailyCap": t.daily_cap.to_string(), "reserveFloor": t.reserve_floor.to_string(),
                "mode": t.mode, "agent": format!("{:#x}", t.agent), "operator": format!("{:#x}", t.operator), "delay": t.delay.to_string(),
                "agentExpiry": t.agent_expiry, "pendingEta": t.pending_eta, "pendingKinds": t.pending_kinds,
                "fingerprint": format!("{:#x}", t.fingerprint),
                "recomputed": format!("{:#x}", t.recompute()),
                "consistent": t.recompute() == t.fingerprint,
            })
        });
        let needs_you: Vec<Value> = match terms.as_ref().filter(|t| t.recompute() == t.fingerprint)
        {
            Some(t) => {
                let deadline = now_secs() + 600;
                needs_you
                    .into_iter()
                    .map(|mut n| {
                        let to = n["counterparty"].as_str().and_then(|x| Address::from_str(x).ok());
                        let amount = n["amount"].as_str().and_then(|a| ledger::Money::parse(a.trim_end_matches(" USDC"), ledger::Currency::Usdc).ok());
                        // The owner's payment is keyed to the bill-cycle, never to this
                        // pass's proposal hash: a second check or a second tab then makes
                        // the same call, which the contract refuses (`AlreadyExecuted`),
                        // and the next pass sees the cycle paid (docs/60 P-175). A row
                        // without a settlement key keeps the pass's own call (keyed alike).
                        let key = settle_key(&n);
                        if let (Some(to), Some(amount), Some(key)) = (to, amount, key) {
                            if let Ok(c) = arc::prepare_principal_settlement_bound(policy, to, amount, key, t.fingerprint, deadline) {
                                n["to"] = json!(format!("{:#x}", c.to));
                                n["data"] = json!(c.data);
                                n["summary"] = json!(c.summary);
                                n["boundUntil"] = json!(deadline);
                            }
                            if n["attest"].is_object() {
                                let c = arc::prepare_payee_batch_bound(policy, &[to], true, t.fingerprint, deadline);
                                n["attest"] = json!({ "to": format!("{:#x}", c.to), "data": c.data, "summary": c.summary });
                            }
                        }
                        n
                    })
                    .collect()
            }
            None => needs_you,
        };
        // PLAN 5.54: a seller the agent wanted to buy from and nobody has
        // approved. One tap approves it (bound to these terms, ten minutes);
        // the agent then buys from it inside the limits.
        let mut sellers: Vec<Value> = Vec::new();
        let wanted: Vec<Address> = bought
            .iter()
            .filter(|p| p["status"] == "needs-you")
            .filter_map(|p| p["seller"].as_str().and_then(|a| Address::from_str(a).ok()))
            .fold(Vec::new(), |mut v, a| {
                if !v.contains(&a) {
                    v.push(a);
                }
                v
            });
        if let (false, Some(t), Some(c)) = (
            wanted.is_empty(),
            terms.as_ref().filter(|t| t.recompute() == t.fingerprint),
            operator::watch::chain(&tenant.chain),
        ) {
            let approved_now = match level1::quick_client(c) {
                Ok(client) => client.attested_among(policy, &wanted).await.ok().flatten(),
                Err(_) => None,
            };
            if let Some(approved_now) = approved_now {
                let deadline = now_secs() + 600;
                for a in wanted
                    .iter()
                    .filter(|a| !approved_now.contains(&format!("{a:#x}")))
                {
                    let first = bought
                        .iter()
                        .find(|p| {
                            p["status"] == "needs-you"
                                && p["seller"]
                                    .as_str()
                                    .is_some_and(|x| x.eq_ignore_ascii_case(&format!("{a:#x}")))
                        })
                        .cloned()
                        .unwrap_or_default();
                    let call = arc::prepare_payee_batch_bound(
                        policy,
                        &[*a],
                        true,
                        t.fingerprint,
                        deadline,
                    );
                    sellers.push(json!({
                        "seller": format!("{a:#x}"), "host": first["host"], "amount": first["amount"],
                        "reason": first["reason"], "url": first["url"],
                        "to": format!("{:#x}", call.to), "data": call.data, "summary": call.summary, "boundUntil": deadline,
                    }));
                }
            }
        }
        tm("sellers checked");
        // Protected per day: min(daily cap, balance - floor), never below zero.
        let money = |s: &str| {
            ledger::Money::parse(s.trim_end_matches(" USDC").trim(), ledger::Currency::Usdc)
                .map(|m| m.to_minor())
                .unwrap_or(0)
        };
        let daily = money(status["caps"]["daily"].as_str().unwrap_or("0"));
        let balance = money(status["balance"].as_str().unwrap_or("0"));
        let floor = money(status["caps"]["floor"].as_str().unwrap_or("0"));
        // The bill wallet's USDC in Circle Gateway on other chains (PLAN
        // 5.14): one line beside the balance, never added to it — the
        // depositor's own signature moves it, not the agent's.
        let elsewhere = elsewhere_pre;
        let protected = daily.min((balance - floor).max(0)).max(0);
        // The agent proposes a limit from what it has seen (docs/53 5.4):
        // twice the largest single bill it has prepared or paid, so one
        // month's largest payment and one surprise both fit in a day, and
        // never more than the balance. Shown only when it differs from the
        // cap by more than a fifth; the human taps, the contract applies.
        let largest = needs_you
            .iter()
            .filter_map(|n| n["amount"].as_str())
            .chain(paid.iter().filter_map(|p| p["amount"].as_str()))
            .map(money)
            .max()
            .unwrap_or(0);
        let suggested = if largest > 0 {
            (largest * 2).min(balance.max(largest))
        } else {
            0
        };
        let suggest = if suggested > 0 && (suggested * 5 < daily * 4 || suggested * 4 > daily * 5) {
            Some(json!({
                "daily": ledger::Money::minor(suggested, ledger::Currency::Usdc).to_string(),
                "perAction": ledger::Money::minor(suggested, ledger::Currency::Usdc).to_string(),
                "basis": format!("twice your largest bill so far ({}), within what the account holds", ledger::Money::minor(largest, ledger::Currency::Usdc)),
            }))
        } else {
            None
        };
        // Recommend Autopilot with evidence (docs/53 5.5): proposals the owner
        // settled, counted on chain. The agent recommends; the human taps;
        // the contract still waits the delay. ponytail: up to 20 reads per
        // inbox call; cache when tenants have long histories.
        let mut hashes: Vec<alloy::primitives::B256> = Vec::new();
        // Every reference any pass offered for each bill-cycle: a cycle paid
        // under one of them is paid, whichever pass's row is showing (docs/60
        // P-210: the owner paid one cycle under two passes' hashes and the next
        // pass's row, under a third, was offered again).
        let mut by_cycle: std::collections::BTreeMap<String, Vec<alloy::primitives::B256>> =
            std::collections::BTreeMap::new();
        // Newest pass first, so a fresh cycle's key is not the one cut (docs/103 S6c-6).
        for j in passes.iter().rev().filter(|j| j.status == "done") {
            if let Some(r) = &j.report {
                for p in r["pendingSignature"].as_array().into_iter().flatten() {
                    // The bill-cycle's settlement key first: the owner's tap is
                    // bound to it since docs/60 P-175; older taps to the hash.
                    for key in ["settlementKey", "actionHash"] {
                        let Some(h) = p[key]
                            .as_str()
                            .and_then(|h| alloy::primitives::B256::from_str(h).ok())
                        else {
                            continue;
                        };
                        if let Some(c) = p["cycleKey"].as_str() {
                            let list = by_cycle.entry(c.to_lowercase()).or_default();
                            if !list.contains(&h) {
                                list.push(h);
                            }
                        }
                        if !hashes.contains(&h) {
                            hashes.push(h);
                        }
                    }
                }
            }
        }
        // The waiting rows' cycles are read first: those are what a tap would pay.
        // ponytail: 40 reads at most; a cycle offered under more references than that
        // (a pass a minute before P-175) is checked on its newest 40.
        let waiting: Vec<alloy::primitives::B256> = needs_you
            .iter()
            .filter_map(|n| n["cycleKey"].as_str())
            .filter_map(|c| by_cycle.get(&c.to_lowercase()))
            .flatten()
            .copied()
            .collect();
        hashes.retain(|h| !waiting.contains(h));
        let mut first: Vec<alloy::primitives::B256> = Vec::new();
        for h in waiting {
            if !first.contains(&h) {
                first.push(h);
            }
        }
        first.extend(hashes);
        let mut hashes = first;
        hashes.truncate(40);
        let executed = level1::executed_hashes(&tenant, &hashes)
            .await
            .unwrap_or_default();
        tm("executed hashes");
        let approved = executed.len();
        // What the owner already settled is not waiting any more. A row whose
        // hash is spent but paid someone else (or a token amount) was not
        // settled: a stolen agent key can spend any hash (docs/68 F1b/F1c),
        // so it is said in words instead of vanishing from Needs you.
        let mut misused: Vec<Value> = Vec::new();
        let needs_you: Vec<Value> = needs_you
            .into_iter()
            .filter(|n| {
                let Some(h) = paid_reference(n, &by_cycle, &executed) else {
                    return true;
                };
                let to = n["counterparty"].as_str().and_then(|x| Address::from_str(x).ok());
                let want = n["amount"].as_str().map(money).unwrap_or(0);
                if named_complete && !paid_as_named(&named, h, to, want) {
                    misused.push(json!({
                        // The hash that was spent (the settlement key or the row's own), re-checked below.
                        "counterparty": n["counterparty"], "amount": n["amount"], "actionHash": format!("{h:#x}"),
                        "why": "this bill's reference was used on chain without paying this payee its amount, so it was not paid; pause the agent and pay this payee from your wallet",
                    }));
                }
                false
            })
            .collect();
        // PLAN 5.39 (docs/62 F1): every row still waiting, as one bound call
        // the owner reviews and approves with one tap. Offered only when the
        // terms read consistently and the policy carries `settleRunIf`
        // (deployed after 2026-10-14); older policies keep one tap per row.
        let run = {
            let rows: Vec<(Address, ledger::Money, alloy::primitives::B256)> = needs_you
                .iter()
                .filter_map(|n| {
                    Some((
                        n["counterparty"]
                            .as_str()
                            .and_then(|x| Address::from_str(x).ok())?,
                        n["amount"].as_str().and_then(|a| {
                            ledger::Money::parse(
                                a.trim_end_matches(" USDC"),
                                ledger::Currency::Usdc,
                            )
                            .ok()
                        })?,
                        // One tap pays each bill-cycle once (docs/60 P-175).
                        settle_key(n)?,
                    ))
                })
                .take(arc::MAX_RUN)
                .collect();
            let consistent = terms.as_ref().filter(|t| t.recompute() == t.fingerprint);
            match (consistent, operator::watch::chain(&tenant.chain)) {
                (Some(t), Some(c)) if rows.len() >= 2 => {
                    let supported = match level1::quick_client(c) {
                        Ok(client) => {
                            client
                                .code_has_selector(policy, arc::settle_run_selector())
                                .await
                        }
                        Err(_) => false,
                    };
                    let deadline = now_secs() + 600;
                    match (
                        supported,
                        arc::prepare_principal_run_bound(policy, &rows, t.fingerprint, deadline),
                    ) {
                        (true, Ok(call)) => {
                            let total: i128 = rows.iter().map(|r| r.1.to_minor()).sum();
                            json!({
                                "to": format!("{:#x}", call.to), "data": call.data, "summary": call.summary,
                                "count": rows.len(), "total": ledger::Money::minor(total, ledger::Currency::Usdc).to_string(),
                                "boundUntil": deadline,
                            })
                        }
                        _ => Value::Null,
                    }
                }
                _ => Value::Null,
            }
        };
        // What the checks refused on the last pass, with the reason (docs/62
        // §3.8: "the page shows why"). Nothing for the owner to do: a frozen
        // payee (5.47), a duplicate, an amount past the cap.
        let mut refused: Vec<Value> = last
            .and_then(|j| j.report.as_ref())
            .and_then(|r| r["blocked"].as_array())
            .map(|a| {
                a.iter()
                    .map(|b| {
                        json!({
                            "counterparty": b["counterparty"], "amount": b["amount"],
                            "why": b["problems"].as_array().into_iter().flatten().filter_map(|p| p.as_str()).map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(" · "),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        // The events were read before `executed` (review 2026-10-15): a payment
        // landing in between would read as misuse and invite a second payment.
        // Read them again, after, and say "misused" only if it still holds.
        if !misused.is_empty()
            && let Some(client) = rc.as_ref()
            && let Ok(head) = client.block_number().await
        {
            let from = tenant.created_block.min(head);
            let (agent, mine) = tokio::join!(
                client.paid_events(policy, from, head),
                client.principal_settlements(policy, from, head),
            );
            match (agent, mine) {
                (Ok(agent), Ok(mine)) => {
                    let fresh: Vec<(alloy::primitives::B256, Address, i128)> = agent
                        .iter()
                        .chain(mine.iter())
                        .map(|e| (e.action_hash, e.to, e.amount.to_minor()))
                        .collect();
                    misused.retain(|m| {
                        let h = m["actionHash"]
                            .as_str()
                            .and_then(|h| alloy::primitives::B256::from_str(h).ok());
                        let to = m["counterparty"]
                            .as_str()
                            .and_then(|x| Address::from_str(x).ok());
                        let want = m["amount"].as_str().map(money).unwrap_or(0);
                        h.is_some_and(|h| !paid_as_named(&fresh, h, to, want))
                    });
                }
                // Cannot re-check: say nothing rather than something false.
                _ => misused.clear(),
            }
        }
        tm("misused re-check");
        refused.extend(misused);
        // docs/60 P-174: a purchase the account refused in the last day (most
        // often "the agent buys only on Autopilot") is shown where the page shows
        // refusals, with why; before, it sat in `purchases` and nowhere on the page.
        let day_ago = now_secs().saturating_sub(86_400);
        refused.extend(
            bought
                .iter()
                .filter(|p| p["status"] == "refused" && p["at"].as_u64().is_some_and(|t| t >= day_ago))
                .map(|p| {
                    json!({
                        "counterparty": p["seller"], "amount": p["amount"], "purchase": true, "at": p["at"],
                        "why": format!("a purchase was refused: {}", p["why"].as_str().unwrap_or("the account refused it")),
                    })
                }),
        );
        // PLAN 5.7: the owner's keys and threshold, what waits for more of
        // them, and devices asking to join (not yet owners).
        let (owners, approvals, join_requests) = match (
            operator::watch::chain(&tenant.chain),
            Address::from_str(&tenant.owner),
        ) {
            (Some(c), Ok(owner_addr)) if tenant.deployed => {
                let signers = signers_pre;
                let pend = level1::pending_approvals(c, &self.dir, owner_addr).await;
                let owners = signers.as_ref().map(|(th, keys)| {
                    json!({
                        "threshold": th,
                        "keys": keys.iter().enumerate().map(|(i, k)| json!({ "index": i, "qx": format!("{:#x}", k.0), "qy": format!("{:#x}", k.1) })).collect::<Vec<_>>(),
                    })
                });
                let known: Vec<String> = signers
                    .as_ref()
                    .map(|(_, keys)| keys.iter().map(|k| format!("{:#x}", k.0)).collect())
                    .unwrap_or_default();
                let joins: Vec<Value> = tenant
                    .join_requests
                    .iter()
                    .filter(|r| !known.contains(&r.0))
                    .map(|r| json!({ "qx": r.0, "qy": r.1, "label": r.2, "at": r.3 }))
                    .collect();
                let need = signers.as_ref().map(|(th, _)| *th).unwrap_or(1);
                let approvals: Vec<Value> = pend
                    .iter()
                    .map(|a| json!({ "challenge": a.challenge, "target": a.target, "data": a.data, "have": a.approvals.len(), "need": need, "keys": a.approvals.iter().map(|(k, _)| k).collect::<Vec<_>>() }))
                    .collect();
                (owners.unwrap_or(Value::Null), approvals, joins)
            }
            _ => (Value::Null, Vec::new(), Vec::new()),
        };
        tm("owners");
        let recommend_autopilot = !tenant.keyless
            && status["mode"] == "Propose"
            && approved >= 3
            && status["pending"].is_null();
        // PLAN 5.25: standing orders on the contract, and the confirmed bills
        // the agent would turn into one (confident or imported, attested is
        // checked when the order is prepared, cadence at least an hour, not
        // covered by an order yet).
        let (standing_orders, standing_candidates) = match operator::watch::chain(&tenant.chain) {
            Some(_) => match schedules_pre.ok_or(()) {
                Ok(list) => {
                    let covered: std::collections::BTreeSet<String> = list
                        .iter()
                        .filter(|s| s.active)
                        .map(|s| format!("{:#x}", s.payee))
                        .collect();
                    let orders: Vec<Value> = list
                        .iter()
                        .map(|s| json!({ "id": s.id, "payee": format!("{:#x}", s.payee), "amount": s.amount.to_string(), "everySecs": s.every_secs, "nextDue": s.next_due, "active": s.active, "due": s.active && s.next_due <= now_secs() }))
                        .collect();
                    // Imported bills are candidates at once, before any pass:
                    // the owner already typed exactly payee, amount, cadence.
                    let imported: Vec<Value> = tenant
                        .bills
                        .iter()
                        .map(|b| json!({ "counterparty": b.payee, "amount": format!("{} USDC", b.amount), "intervalSecs": b.every_days.saturating_mul(86_400), "nextDue": b.next_due, "confidence": 1.0, "samples": 0 }))
                        .collect();
                    let mut seen: std::collections::BTreeSet<String> =
                        std::collections::BTreeSet::new();
                    // The last pass's bills first: their next due date is stepped past
                    // the cycles already paid; an imported bill's typed date is not
                    // (docs/60 P-211).
                    let cands: Vec<Value> = last
                        .and_then(|j| j.report.as_ref())
                        .and_then(|r| r["obligationsList"].as_array().cloned())
                        .unwrap_or_default()
                        .into_iter()
                        .chain(imported)
                        .filter(|o| seen.insert(o["counterparty"].as_str().unwrap_or("").to_lowercase()))
                        .filter(|o| {
                            let conf = o["confidence"].as_f64().or_else(|| o["confidence"].as_str().and_then(|c| c.parse().ok())).unwrap_or(0.0);
                            let imported = o["samples"].as_u64() == Some(0);
                            let every = o["intervalSecs"].as_u64().unwrap_or(0);
                            // A standing-order candidate is a proposal the owner taps,
                            // so the Propose floor applies (docs/76 §7 row 7).
                            (conf >= f64::from(ledger::action::CONFIDENCE_FLOOR_PROPOSE_BP) / 10_000.0 || imported)
                                && every >= 3600
                                // An API bought per call (x402 receipt) is a bill to
                                // forecast, never a standing order: paying it without
                                // a call buys nothing.
                                && o["provenance"] != "x402 receipt"
                                && !covered.contains(&o["counterparty"].as_str().unwrap_or("").to_lowercase())
                        })
                        .map(|o| {
                            // And past a cycle paid since that pass, under any reference offered for it.
                            let next = next_unpaid_due(
                                o["counterparty"].as_str().unwrap_or(""),
                                o["nextDue"].as_i64().unwrap_or(0),
                                o["intervalSecs"].as_i64().unwrap_or(0),
                                &by_cycle,
                                &executed,
                            );
                            json!({ "payee": o["counterparty"], "amount": o["amount"], "everySecs": o["intervalSecs"], "nextDue": next, "confidence": o["confidence"], "basis": if o["samples"].as_u64() == Some(0) { "you imported this bill" } else { "seen repeatedly in your history" } })
                        })
                        .collect();
                    (json!(orders), json!(cands))
                }
                Err(_) => (Value::Null, Value::Null),
            },
            None => (Value::Null, Value::Null),
        };
        tm("standing orders");
        // Runway (PLAN 5.17, docs/57 §2): a day-by-day projection of the
        // bills the last pass recovered against what the policy holds — how
        // many days until the first bill the policy could not cover, and
        // which one. Deterministic, from our own data; no model.
        // The bills the last pass recovered (none before the first pass), and
        // whether any standing order exists: runway needs at least one of them.
        // The fifth field: the bill may have ended (a full interval or more
        // overdue and unpaid) — counted once, as `watch::due_within` does,
        // not at its cadence (docs/70 C8: 15,084.40 "needed" for 0.35 of bills).
        let report_bills: Option<Vec<RunwayBill>> = last
            .and_then(|j| j.report.as_ref())
            .filter(|r| r["obligationsList"].is_array())
            .map(|r| {
                r["obligationsList"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| {
                        let amount = money(o["amount"].as_str()?);
                        let every = o["intervalSecs"].as_i64()?.max(60);
                        let next = o["nextDue"].as_i64()?;
                        let ended = now_secs() as i64 - next >= every;
                        Some((
                            o["counterparty"].as_str()?.to_string(),
                            amount,
                            every,
                            next,
                            ended,
                        ))
                    })
                    .collect()
            });
        let has_orders = standing_orders
            .as_array()
            .is_some_and(|a| a.iter().any(|o| o["active"].as_bool().unwrap_or(false)));
        let runway = if report_bills.is_none() && !has_orders {
            None
        } else {
            let mut bills: Vec<RunwayBill> = report_bills.unwrap_or_default();
            // Standing orders the owner set are bills too (PLAN 5.25): the
            // contract pays them on their day whoever triggers; one entry per
            // payee, the order winning over the inferred bill.
            if let Some(orders) = standing_orders.as_array() {
                for o in orders
                    .iter()
                    .filter(|o| o["active"].as_bool().unwrap_or(false))
                {
                    let (Some(who), Some(amount), Some(every), Some(next)) = (
                        o["payee"].as_str(),
                        o["amount"].as_str(),
                        o["everySecs"].as_i64(),
                        o["nextDue"].as_i64(),
                    ) else {
                        continue;
                    };
                    let who = who.to_lowercase();
                    bills.retain(|b| b.0.to_lowercase() != who);
                    bills.push((who, money(amount), every.max(60), next, false));
                }
            }
            let horizon_days: i64 = 90;
            let now = now_secs() as i64;
            let end = now + horizon_days * 86_400;
            let mut left = balance - floor;
            let mut spent_total: i128 = 0;
            let mut first_at_risk: Option<Value> = None;
            // Walk every due date inside the horizon in time order.
            let mut events: Vec<(i64, usize)> = Vec::new();
            for (i, b) in bills.iter_mut().enumerate() {
                // From the bill's own due date (a late bill is due now, then on
                // its cadence), as `bills_next_30` and `watch::due_within` count.
                let mut t = b.3;
                if b.4 {
                    events.push((t, i));
                    continue;
                }
                while t <= end {
                    events.push((t, i));
                    t += b.2;
                }
            }
            events.sort();
            let mut days_ok = horizon_days;
            // Every bill in the horizon, covered or not: what it would take to
            // cover all of them (docs/62 §3.4 — the owner's problem is timing,
            // so say exactly how much is missing and offer the tap that adds it).
            let horizon_total: i128 = events.iter().map(|(_, i)| bills[*i].1).sum();
            for (t, i) in events {
                let (who, amount, _, _, _) = &bills[i];
                if left < *amount {
                    days_ok = ((t - now) / 86_400).max(0);
                    first_at_risk = Some(
                        json!({ "counterparty": who, "amount": ledger::Money::minor(*amount, ledger::Currency::Usdc).to_string(), "onUnix": t, "inHours": ((t - now) / 3_600).max(0) }),
                    );
                    break;
                }
                left -= amount;
                spent_total += amount;
            }
            let shortfall = (horizon_total - (balance - floor).max(0)).max(0);
            // PLAN 5.36 (docs/62 §4.6): the part of what the policy holds
            // that no bill in the horizon needs — a number, not a product.
            // Whether it may earn is the founder's decision (docs/47).
            let idle = if first_at_risk.is_none() {
                left.max(0)
            } else {
                0
            };
            // RFB 04 "move funds between operating and reserve" (PLAN 5.57):
            // what the agent proposes sending back to the company's own wallet
            // -- the idle part beyond one more horizon of bills, so the account
            // never runs on exactly what it needs. Only for an account that
            // holds money (pull and wallet accounts keep it in the wallet
            // already); the owner's passkey signs the withdrawal.
            let returnable = if tenant.kind == "hold" {
                (idle - spent_total).max(0)
            } else {
                0
            };
            Some(json!({
                "returnable": ledger::Money::minor(returnable, ledger::Currency::Usdc).to_string(),
                // The company's own wallet; none connected yet: nowhere proposed.
                "returnTo": level1::history_sources(&tenant).first().map(|a| format!("{a:#x}")),
                "days": days_ok,
                "horizonDays": horizon_days,
                "firstAtRisk": first_at_risk,
                "billsInHorizon": ledger::Money::minor(spent_total, ledger::Currency::Usdc).to_string(),
                "idleCover": ledger::Money::minor(idle, ledger::Currency::Usdc).to_string(),
                "horizonTotal": ledger::Money::minor(horizon_total, ledger::Currency::Usdc).to_string(),
                "shortfall": ledger::Money::minor(shortfall, ledger::Currency::Usdc).to_string(),
                "basis": "your bills and standing orders, repeated on their schedule, paid from what the account can spend above the reserve you keep; prices do not move it (it holds only USDC)",
            }))
        };
        tm("runway");
        // PLAN 5.49: what the bill wallet holds outside dollars, what a 30%
        // fall would cost in days of bills, and whether a hedge covers it.
        let bill_wallet = level1::history_sources(&tenant).first().copied();
        let exposure = {
            // The first wallet; 0x0 (a name-first account) holds every burnt
            // token and is nobody's exposure (docs/79 §6a #3).
            let key = bill_wallet.map(|a| format!("{a:#x}")).unwrap_or_default();
            let cached = self
                .exposure_cache
                .lock()
                .await
                .get(&key)
                .filter(|(at, _)| now_secs().saturating_sub(*at) < 600)
                .map(|(_, v)| v.clone());
            let mut e = match cached {
                Some(v) => v,
                None => {
                    let v = match bill_wallet {
                        Some(w) => operator::watch::wallet_exposure(w).await,
                        None => Value::Null,
                    };
                    self.exposure_cache
                        .lock()
                        .await
                        .insert(key.clone(), (now_secs(), v.clone()));
                    v
                }
            };
            let notional = e["notionalUsd"]
                .as_str()
                .and_then(|x| x.parse::<f64>().ok())
                .unwrap_or(0.0);
            let per_day = runway
                .as_ref()
                .and_then(|r| {
                    let total = r["horizonTotal"]
                        .as_str()?
                        .trim_end_matches(" USDC")
                        .parse::<f64>()
                        .ok()?;
                    let days = r["horizonDays"].as_f64()?;
                    (days > 0.0).then_some(total / days)
                })
                .unwrap_or(0.0);
            let fall = notional * 0.3;
            if e.is_object() {
                e["fall30Usd"] = json!(format!("{fall:.2}"));
                e["fall30Days"] = if per_day > 0.0 {
                    json!((fall / per_day).floor() as u64)
                } else {
                    Value::Null
                };
                // The hedge engine keeps its books per wallet under runtime/hedge/<wallet>.
                let state = std::path::Path::new("runtime/hedge")
                    .join(&key)
                    .join("state.json");
                e["hedge"] = match tokio::fs::read_to_string(&state).await {
                    Ok(t) => {
                        json!({ "on": true, "state": serde_json::from_str::<Value>(&t).unwrap_or(Value::Null) })
                    }
                    Err(_) => json!({ "on": false }),
                };
            }
            e
        };
        tm("exposure");
        // docs/73 T2/T3 (docs/66 route change 4): how much of the next 30
        // days of bills a price fall cannot touch -- the USDC the account holds
        // for them, plus a short held against the bill wallet's coins -- and
        // what that short was paid or cost in the last 30 days, from the
        // hedge books. Past and measured; never a forecast.
        let protection = {
            let bills30 = last
                .and_then(|j| j.report.as_ref())
                .and_then(|r| bills_next_30(r, now_secs()))
                .unwrap_or(0);
            // ponytail: no account runs its own hedge yet (a per-customer
            // trade-only key, docs/68 D4), so the short counted here is 0;
            // read the venue position when one does.
            let hedged = 0i128;
            let protected = ((balance - floor).max(0) + hedged).min(bills30.max(0));
            let usdc = |m: i128| ledger::Money::minor(m, ledger::Currency::Usdc).to_string();
            // No wallet: no books (an empty name would read the root's).
            let journal = bill_wallet.map(|a| {
                std::path::Path::new("runtime/hedge")
                    .join(format!("{a:#x}"))
                    .join("journal.jsonl")
            });
            let since = now_secs().saturating_sub(30 * 86_400);
            let read = match &journal {
                Some(j) => tokio::fs::read_to_string(j).await,
                None => Err(std::io::ErrorKind::NotFound.into()),
            };
            let funding = match read {
                Ok(t) => match funding_since(&t, since * 1000) {
                    (_, 0) => Value::Null,
                    (net, n) => {
                        json!({ "usdc": usdc(-net), "entries": n, "from": since, "to": now_secs() })
                    }
                },
                Err(_) => Value::Null,
            };
            json!({
                "bills30d": usdc(bills30),
                "protectedUsd": usdc(protected),
                "coveragePct": (bills30 > 0).then(|| (protected * 100 / bills30).min(100)),
                "funding30d": funding,
                "basis": "the next 30 days of bills, against the USDC this account holds for them plus any short held against your wallet's coins; a price fall does not move either",
            })
        };
        tm("protection");
        // PLAN 5.21: a read-back mismatch leaves HALT beside the tenant's log;
        // the page says so instead of looking idle.
        let halt_path = self
            .dir
            .join("tenants")
            .join(format!("{policy:#x}"))
            .join("log")
            .join("HALT");
        let halted = tokio::fs::read_to_string(&halt_path).await.ok().map(|r| {
            json!({ "reason": r.trim(), "clear": format!("a human removes {}", halt_path.display()) })
        });
        // Pull mode (PLAN 5.26): what the wallet allows and holds, and the
        // exact approve call for 30 days at the daily cap, for the page or
        // any wallet software.
        let pull = if tenant.kind == "pull" {
            match (
                operator::watch::chain(&tenant.chain),
                Address::from_str(&tenant.watched),
            ) {
                (Some(c), Ok(w)) => match level1::quick_client(c) {
                    Ok(client) => {
                        let allowance = client.usdc_allowance(w, policy).await;
                        let held = client.usdc_balance(w).await;
                        // docs/70 C2: ask for what the bills use — the next 30
                        // days of them and a fifth more, at least one day's
                        // limit — not thirty days at the limit (12 USDC asked
                        // for 0.35 of bills read as 12 of exposure). The
                        // contract's daily limit binds either way.
                        let month = daily.saturating_mul(30).max(0);
                        let bills = match last.and_then(|j| j.report.as_ref()) {
                            Some(r) => bills_next_30(r, now_secs()),
                            None => tokio::fs::read_to_string(
                                level1::tenants_dir(&self.dir)
                                    .join(format!("{}.preview.json", tenant.policy.to_lowercase())),
                            )
                            .await
                            .ok()
                            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                            .and_then(|v| v["fundMinor"].as_str()?.parse::<i128>().ok()),
                        };
                        let usdc = |m: i128| ledger::Money::minor(m, ledger::Currency::Usdc);
                        let (suggested, basis) = match bills.filter(|b| *b > 0) {
                            Some(b) if b * 6 / 5 > month => (
                                month.max(0) as u128,
                                format!(
                                    "thirty days at your daily limit; the next 30 days of bills come to {}, so the limit is what binds",
                                    usdc(b)
                                ),
                            ),
                            Some(b) => (
                                (b * 6 / 5).max(daily).max(0) as u128,
                                format!("the next 30 days of bills ({}) and a fifth more", usdc(b)),
                            ),
                            None => (
                                month.max(0) as u128,
                                "thirty days at your daily limit".to_string(),
                            ),
                        };
                        json!({
                            "suggestedBasis": basis,
                            "wallet": format!("{w:#x}"),
                            "usdc": format!("{:#x}", client.usdc_address()),
                            "allowance": allowance.as_ref().map(|m| m.to_string()).unwrap_or_default(),
                            "walletBalance": held.as_ref().map(|m| m.to_string()).unwrap_or_default(),
                            "suggestedAllowance": ledger::Money::minor(suggested as i128, ledger::Currency::Usdc).to_string(),
                            "approveData": alloy::hex::encode_prefixed(arc::approve_calldata(policy, suggested)),
                            "revokeData": alloy::hex::encode_prefixed(arc::approve_calldata(policy, 0)),
                            "error": allowance.err().or(held.err()).map(|e| e.to_string()),
                        })
                    }
                    Err(e) => json!({ "error": e.to_string() }),
                },
                _ => Value::Null,
            }
        } else {
            Value::Null
        };
        tm("pull");
        // PLAN 5.24: when the agent's authority lapses; the page offers a
        // one-tap renewal inside the last fourteen days, and says so when it
        // has lapsed (the agent then proposes and does not pay).
        let agent_expiry = expiry_pre;
        let agent_authority = agent_expiry.map(|e| {
            let now = now_secs();
            let left = e as i64 - now as i64;
            let renew_suggested = left < 14 * 86_400;
            let renew_until = now + 90 * 86_400;
            json!({
                "lapsesAt": e,
                "lapsed": now >= e,
                "daysLeft": left / 86_400,
                "renewSuggested": renew_suggested,
                "renewUntil": renew_until,
            })
        });
        // PLAN 5.30: guardians on the owner's passkey contract.
        let guardians = guardians_pre.map(|(list, threshold, eta)| {
            json!({ "list": list.iter().map(|a| format!("{a:#x}")).collect::<Vec<_>>(), "threshold": threshold, "pendingEta": eta })
        });
        // docs/65 §5.4: invoices vendors sent, waiting for the owner. A
        // one-off one carries the owner's payment, bound to the terms like
        // every prepared approval and keyed by the invoice's own reference,
        // so the contract pays it once whoever taps twice.
        tm("guardians");
        let mut pending_invoices = operator::invoice::pending(&self.dir, &tenant.policy);
        // Approved since it arrived? Read now, not at receipt: a second
        // invoice from a payee approved for the first must not ask for the
        // passkey again (seen live 2026-10-14, docs/60 P-41).
        if let Some(client) = rc.as_ref().filter(|_| tenant.deployed) {
            for l in pending_invoices
                .iter_mut()
                .filter(|l| l["approvedPayee"] != true)
            {
                if let Some(payee) = l["payee"].as_str().and_then(|x| Address::from_str(x).ok())
                    && client.is_attested(policy, payee).await.unwrap_or(false)
                {
                    l["approvedPayee"] = json!(true);
                }
            }
        }
        // docs/112: the agent's decision on each waiting invoice (ask, with its
        // why; a refused one is declined and listed under `refused`).
        let agent_said: std::collections::HashMap<String, Value> =
            level1::invoice_decisions(&self.dir, &tenant.policy)
                .into_iter()
                .filter_map(|r| Some((r["id"].as_str()?.to_string(), r["decision"].clone())))
                .collect();
        let invoices: Vec<Value> = pending_invoices
            .into_iter()
            .map(|mut l| {
                if let Some(d) = l["id"].as_str().and_then(|i| agent_said.get(i)) {
                    l["why"] = d["why"].clone();
                    l["agent"] = d.clone();
                }
                let t = terms.as_ref().filter(|t| t.recompute() == t.fingerprint);
                let payee = l["payee"].as_str().and_then(|x| Address::from_str(x).ok());
                let amount = l["amount"]
                    .as_str()
                    .and_then(|a| a.parse::<i128>().ok())
                    .map(|m| ledger::Money::minor(m, ledger::Currency::Usdc));
                // The id (payee + ref), never the vendor's bare ref (P-46).
                let reference = operator::invoice::line_id(&l);
                if let (Some(t), Some(payee), Some(amount), Some(reference)) =
                    (t, payee, amount, reference)
                    && l["everyDays"].as_u64() == Some(0)
                    && let Ok(c) = arc::prepare_principal_settlement_bound(
                        policy,
                        payee,
                        amount,
                        reference,
                        t.fingerprint,
                        now_secs() + 600,
                    )
                {
                    l["pay"] = json!({ "to": format!("{:#x}", c.to), "data": c.data, "summary": c.summary });
                }
                l
            })
            .collect();
        // docs/65 F3: before the first pass, what the agent can already read.
        tm("before preview");
        let preview = if last.is_none() && running.is_none() {
            self.preview(&tenant).await
        } else {
            Value::Null
        };
        let every: u64 = std::env::var("TAMEION_LEVEL1_EVERY_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut reply = json!({
            "policy": tenant.policy,
            "mode": status["mode"], "caps": status["caps"], "balance": status["balance"], "pending": status["pending"],
            "protectedPerDay": ledger::Money::minor(protected, ledger::Currency::Usdc).to_string(),
            "lastPass": last.map(|j| json!({ "id": j.id, "at": j.created, "reasoner": j.report.as_ref().and_then(|r| r["reasoner"].as_str()).unwrap_or(""), "modelUnavailable": j.report.as_ref().and_then(|r| r["reasoner"].as_str()).is_some_and(|x| x.contains("fallback")), "root": j.report.as_ref().map(|r| r["root"].clone()), "anchorTx": j.report.as_ref().map(|r| r["anchorTx"].clone()), "obligations": j.report.as_ref().and_then(|r| r["obligations"].as_u64()).unwrap_or(0), "transfers": j.report.as_ref().and_then(|r| r["transfers"].as_u64()).unwrap_or(0), "historyNote": j.report.as_ref().map(|r| r["historyNote"].clone()).unwrap_or(Value::Null) })),
            "running": running,
            "halted": halted,
            "kind": tenant.kind,
            "deployed": tenant.deployed,
            // docs/65 F1: the limit the owner chose, before the contract
            // holds it (the chain reads zero until it exists).
            "chosenDaily": tenant.daily,
            "preview": preview,
            "pull": pull,
            "agentAuthority": agent_authority,
            "guardians": guardians,
            "agentTokens": tenant.agent_tokens.iter().map(|t| json!({ "id": t.id, "label": t.label, "scopes": t.scopes, "expires": t.expires, "issued": t.issued })).collect::<Vec<_>>(),
            "gated": !tenant.agent_tokens.is_empty(),
            "terms": terms_json,
            "standingOrders": standing_orders,
            "standingOrderCandidates": standing_candidates,
            "needsYou": needs_you,
            "invoices": invoices,
            "sellersToApprove": sellers,
            // docs/60 P-217: purchases the agent did not make alone because the price
            // was more than a quarter above that seller's usual; one tap lets one through.
            "pricesToApprove": prices_to_approve(&bought, now_secs()),
            "purchases": bought.iter().take(20).collect::<Vec<_>>(),
            "exposure": exposure,
            "keyless": tenant.keyless,
            "owners": owners,
            "approvals": approvals,
            "joinRequests": join_requests,
            // PLAN 5.28: what the wallet signs to become the account.
            "delegation": if tenant.kind == "wallet" {
                json!({
                    "implementation": level1::delegate_address().ok().map(|a| format!("{a:#x}")),
                    "wallet": tenant.policy,
                    "initializeData": level1::delegate_initialize_calldata(&tenant).ok(),
                    "active": tenant.deployed,
                })
            } else {
                Value::Null
            },
            "run": run,
            "refused": refused,
            "paid": paid,
            "paidTotal": ledger::Money::minor(paid_minor, ledger::Currency::Usdc).to_string(),
            "paidByAgent": by_agent,
            "paidByYou": paid_by_you,
            "nextPassSecs": if every > 0 { Some(every) } else { None },
            "suggestedLimit": suggest,
            "approvedByYou": approved,
            "runway": runway,
            "protection": protection,
            "holdingsElsewhere": elsewhere,
            "recommendAutopilot": recommend_autopilot,
            // docs/81: the bills the owner typed, shown (and named) before the
            // first pass turns them into candidates; same audience as `invoices`.
            "typedBills": tenant.bills.iter().map(|b| json!({
                "payee": b.payee,
                "amount": if b.amount.ends_with("USDC") { b.amount.clone() } else { format!("{} USDC", b.amount) },
                "everySecs": u64::from(b.every_days) * 86_400,
                "nextDue": b.next_due,
            })).collect::<Vec<_>>(),
        });
        tm("reply built");
        // UAT U3 (docs/60 P-180…P-182): what an outside agent is told — its own
        // requests as one-tap rows for the owner, what it may still spend and
        // whom it may pay (read from the contract), and one list of everything
        // that waits for the owner: the same items the page counts.
        {
            let now = now_secs();
            let usdc = |s: &str| {
                ledger::Money::parse(s.trim_end_matches(" USDC").trim(), ledger::Currency::Usdc)
                    .ok()
            };
            let asks = operator::agent_api::asks(&self.dir, &tenant.policy, now);
            let delay = status["delaySecs"].as_u64().unwrap_or(0);
            // P-182: an agent's limits request, shown where the page already puts
            // the agent's limit suggestion (one tap, "Set it").
            if let Some(a) = asks.get("limits") {
                let asked = ["perAction", "daily", "floor"].map(|k| a[k].as_str().and_then(usdc));
                let cur = ["perAction", "daily", "floor"]
                    .map(|k| status["caps"][k].as_str().and_then(usdc));
                if asked.iter().all(Option::is_some) && asked != cur {
                    let change = |i: usize, name: &str| match (&cur[i], &asked[i]) {
                        (Some(c), Some(n)) if c != n => Some(format!("{name} {c} → {n}")),
                        _ => None,
                    };
                    let changes: Vec<String> = [
                        change(0, "per payment"),
                        change(1, "per day"),
                        change(2, "floor"),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    let minor =
                        |x: [Option<ledger::Money>; 3]| x.map(|m| m.map_or(0, |m| m.to_minor()));
                    let applies =
                        operator::agent_api::limits_apply(minor(cur), minor(asked), delay);
                    let [p, d, f] = asked.map(|m| m.map(|m| m.to_string()).unwrap_or_default());
                    reply["suggestedLimit"] = json!({
                        "perAction": p, "daily": d, "floor": f, "byAgent": true, "askedAt": a["at"],
                        "basis": format!(
                            "your agent ({}) asked: {}{}; it applies {applies}",
                            a["by"].as_str().unwrap_or("an agent key"),
                            changes.join(", "),
                            a["reason"].as_str().filter(|r| !r.is_empty()).map(|r| format!(" (\"{r}\")")).unwrap_or_default()
                        ),
                    });
                }
            }
            // P-182: an agent's request for Autopilot, where the page already asks
            // for it ("Turn autopilot on").
            if asks["mode"]["mode"].as_u64() == Some(2)
                && status["mode"] != "Autopilot"
                && status["pending"].is_null()
                && !tenant.keyless
            {
                reply["recommendAutopilot"] = json!(true);
                reply["autopilotAsk"] = json!({ "by": asks["mode"]["by"], "reason": asks["mode"]["reason"], "at": asks["mode"]["at"] });
            }
            // P-182: payees an agent asked for, as one-tap approval rows beside the
            // sellers it wanted (bound to these terms for ten minutes, as those are).
            if asks["payees"]["allow"] == true
                && let (Some(t), Some(client)) = (
                    terms.as_ref().filter(|t| t.recompute() == t.fingerprint),
                    rc.as_ref(),
                )
            {
                let wanted: Vec<Address> = asks["payees"]["payees"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().and_then(|s| Address::from_str(s).ok()))
                    .collect();
                let listed: Vec<String> = reply["sellersToApprove"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["seller"].as_str().map(str::to_lowercase))
                    .collect();
                if let Ok(Some(approved)) = client.attested_among(policy, &wanted).await {
                    let deadline = now + 600;
                    for a in wanted.iter().filter(|a| {
                        let a = format!("{a:#x}");
                        !approved.contains(&a) && !listed.contains(&a)
                    }) {
                        let call = arc::prepare_payee_batch_bound(
                            policy,
                            &[*a],
                            true,
                            t.fingerprint,
                            deadline,
                        );
                        if let Some(rows) = reply["sellersToApprove"].as_array_mut() {
                            rows.push(json!({
                                "seller": format!("{a:#x}"), "host": "a payee it asked for", "amount": status["caps"]["perAction"],
                                "reason": asks["payees"]["reason"], "askedBy": asks["payees"]["by"],
                                "to": format!("{:#x}", call.to), "data": call.data, "summary": call.summary, "boundUntil": deadline,
                            }));
                        }
                    }
                }
            }
            // docs/60 P-228: a bill the pass found in the history whose payee the
            // agent may not pay yet, offered BEFORE it falls due: one tap approves
            // the payee, its waiting time runs out before the due date, and the
            // agent pays that cycle itself (until now the first offer came at the
            // due date, as "Approve & pay", and the owner paid that cycle).
            if let (Some(t), Some(client), Some(found)) = (
                terms.as_ref().filter(|t| t.recompute() == t.fingerprint),
                rc.as_ref(),
                last.and_then(|j| j.report.as_ref()).map(found_bills),
            ) && !found.is_empty()
            {
                let listed: Vec<String> = reply["sellersToApprove"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["seller"].as_str().map(str::to_lowercase))
                    .collect();
                let addrs: Vec<Address> = found.iter().map(|(a, _)| *a).collect();
                if let Ok(Some(approved)) = client.attested_among(policy, &addrs).await {
                    let deadline = now + 600;
                    let delay = status["delaySecs"].as_u64().unwrap_or(0);
                    for (a, o) in found.iter().filter(|(a, _)| {
                        let a = format!("{a:#x}");
                        !approved.contains(&a) && !listed.contains(&a)
                    }) {
                        let call = arc::prepare_payee_batch_bound(
                            policy,
                            &[*a],
                            true,
                            t.fingerprint,
                            deadline,
                        );
                        if let Some(rows) = reply["sellersToApprove"].as_array_mut() {
                            rows.push(json!({
                                "seller": format!("{a:#x}"), "host": "a bill it found", "amount": o["amount"],
                                "askedBy": "Your agent", "reason": found_bill_reason(o, delay, now),
                                "bill": { "everySecs": o["intervalSecs"], "nextDue": o["nextDue"], "samples": o["samples"] },
                                "to": format!("{:#x}", call.to), "data": call.data, "summary": call.summary, "boundUntil": deadline,
                            }));
                        }
                    }
                }
            }
            // P-181: spent and left in the trailing 24 hours, and whom the agent may
            // pay and from when, read from the contract (not learned from refusals).
            if let (Some(client), true) = (rc.as_ref(), tenant.deployed) {
                let named = operator::parties::named_in(&reply);
                let (auth, left, since) = tokio::join!(
                    client.authority(policy),
                    client.remaining_allowance(policy),
                    client.attested_since_among(policy, &named),
                );
                if let Ok(a) = auth {
                    reply["caps"]["spent24h"] = json!(a.spent_today.to_string());
                }
                if let Ok(l) = left {
                    reply["caps"]["leftNow"] = json!(l.to_string());
                }
                if let Ok(list) = since {
                    reply["payees"] = json!(list
                        .into_iter()
                        .filter(|(_, at)| *at > 0)
                        .map(|(a, at)| json!({ "address": format!("{a:#x}"), "approvedAt": at, "payableFrom": at + delay, "payableNow": now >= at + delay }))
                        .collect::<Vec<_>>());
                }
            }
            // P-177: every payment row and every Needs-you row says who asked and
            // why, from the newest pass that knows the bill (a purchase carries its own).
            let known: Vec<Value> = last
                .and_then(|j| j.report.as_ref())
                .and_then(|r| r["obligationsList"].as_array().cloned())
                .unwrap_or_default();
            for row in reply["needsYou"].as_array_mut().into_iter().flatten() {
                let who = row["counterparty"].as_str().unwrap_or("").to_string();
                row["askedBy"] = json!("the agent");
                row["reason"] = json!(
                    operator::agent_api::reason_for(&known, &who)
                        .unwrap_or_else(|| "a payment the agent prepared".to_string())
                );
            }
            for row in reply["paid"].as_array_mut().into_iter().flatten() {
                if row["reason"].is_string() {
                    continue;
                }
                let who = row["counterparty"].as_str().unwrap_or("").to_string();
                let why = operator::agent_api::reason_for(&known, &who);
                row["reason"] = json!(match (row["by"] == "you", why) {
                    (true, Some(w)) => format!("paid by the owner; the agent asked: {w}"),
                    (true, None) => "paid by the owner with the passkey".to_string(),
                    (false, Some(w)) => format!("paid by the agent: {w}"),
                    (false, None) => "paid by the agent inside the limits".to_string(),
                });
            }
            // P-180: everything that waits for the owner, in one list.
            let list = |k: &str| reply[k].as_array().cloned().unwrap_or_default();
            let waiting = operator::agent_api::waiting(
                &list("needsYou"),
                &list("sellersToApprove"),
                &list("invoices"),
            );
            reply["waitingCount"] = json!(waiting.len());
            reply["waiting"] = json!(waiting);
            reply["agentAsks"] = asks;
        }
        // docs/81: every party in this reply, named for a person. The owner's
        // names go to the owner's device (`X-Tameion-Device`), or to an agent
        // key — on an account with keys the gate has already checked one.
        let private_ok = !tenant.agent_tokens.is_empty()
            || level1::reserve_token_ok(&tenant, header(raw, "x-tameion-device"));
        let invoices = operator::invoice::list(&self.dir, &tenant.policy);
        let claimed = operator::parties::claims(&invoices);
        let mut named = operator::parties::named_in(&reply);
        named.extend(claimed.keys().copied());
        let chain_id = operator::watch::chain(&tenant.chain).map_or(0, |c| c.chain_id);
        // Names proved outside the account (Tameion handle, domain, ENS):
        // cached, so only a first sight of an address waits, and never long.
        let proved = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            operator::parties::proved(&self.dir, chain_id, &named, &claimed),
        )
        .await
        .unwrap_or_default();
        tm("agent view and names");
        reply["parties"] =
            operator::parties::parties(&tenant, &reply, &invoices, &proved, private_ok, now_secs());
        // docs/115 §2: the agent's status read — the kill switches as they stand
        // (Watch is the contract's pause; HALT a human clears) and what changed.
        reply["paused"] = json!(reply["mode"] == "Watch");
        reply["halt"] = json!(!reply["halted"].is_null());
        reply["recentUpdates"] = operator::refusal::recent_updates();
        json_reply("200 OK", reply)
    }

    /// `GET /api/level1/resolve?q=<what a person typed>&chain=<key>` (docs/81):
    /// a Tameion handle, an ENS name, a web domain that lists one address for
    /// the chain (ERC-7529 TXT), or an address — as `{address, label, kind}`.
    /// Read-only and public: everything it reads is public.
    async fn level1_resolve(&self, query: &str) -> Vec<u8> {
        let get = |k: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
                .map(|v| {
                    v.replace('+', " ")
                        .replace("%40", "@")
                        .replace("%2E", ".")
                        .replace("%2e", ".")
                })
                .unwrap_or_default()
        };
        let q = get("q");
        let Some(c) = operator::watch::chain(&get("chain")) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "chain must be one of the service's chains" }),
            );
        };
        if q.trim().is_empty() || q.len() > 255 {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "q is what the person typed" }),
            );
        }
        match operator::parties::resolve_typed(&self.dir, c.chain_id, &q).await {
            Ok((a, label, kind)) => json_reply(
                "200 OK",
                json!({ "address": format!("{a:#x}"), "label": label, "kind": kind, "tail": operator::parties::tail(a) }),
            ),
            Err(e) => json_reply("404 Not Found", json!({ "error": e })),
        }
    }

    /// `POST /api/level1/contacts {policy, address, name, reserveToken}`
    /// (docs/81): the owner names a party ("Alice, design"), or forgets its
    /// name with an empty `name`. Only the device that opened the account (its
    /// device token): a name is what the owner trusts when approving, so an
    /// agent key cannot write one, and neither can anyone who knows the
    /// account (the reason names were device-only before the token existed).
    async fn level1_contacts(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(mut t) = v["policy"]
            .as_str()
            .and_then(|p| level1::load(&self.dir, p))
        else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        if !level1::reserve_token_ok(&t, v["reserveToken"].as_str()) {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "only the device that opened this account can name its payees; on another device, confirm with your passkey first" }),
            );
        }
        let Some(a) = v["address"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
            .filter(|a| !a.is_zero())
        else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "address must be a wallet address" }),
            );
        };
        if let Err(e) =
            operator::parties::set_contact(&mut t, a, v["name"].as_str().unwrap_or(""), now_secs())
        {
            return json_reply("400 Bad Request", json!({ "error": e }));
        }
        if let Err(e) = level1::save(&self.dir, &t) {
            return json_reply("500 Internal Server Error", json!({ "error": e }));
        }
        json_reply(
            "200 OK",
            json!({ "policy": t.policy, "contacts": t.contacts }),
        )
    }

    /// The desk's owner check (docs/83 W5): the device that opened the
    /// account (its token, as a header or in the body), or an agent key that
    /// carries `scope`. Positions and prices are the trader's own business.
    fn desk_owner(&self, t: &level1::Tenant, raw: &str, v: Option<&Value>, scope: &str) -> bool {
        let device =
            header(raw, "x-tameion-device").or_else(|| v.and_then(|v| v["reserveToken"].as_str()));
        if level1::reserve_token_ok(t, device) {
            return true;
        }
        let bearer = header(raw, "authorization")
            .and_then(|h| h.trim().strip_prefix("Bearer "))
            .map(str::trim);
        // Strict (docs/89 C1): an account with no agent keys is open to the
        // treasury's read routes, but the desk is the owner's — a bearer
        // counts only when it is a real key on this account with `scope`.
        bearer.is_some_and(|b| !b.is_empty())
            && !t.agent_tokens.is_empty()
            && level1::token_allows(t, bearer, scope, now_secs()).is_ok()
    }

    fn desk_tenant(&self, policy: Option<&str>) -> Result<level1::Tenant, Vec<u8>> {
        policy
            .and_then(|p| Address::from_str(p.trim()).ok())
            .and_then(|a| level1::load(&self.dir, &format!("{a:#x}")))
            .ok_or_else(|| json_reply("404 Not Found", json!({ "error": "no such account" })))
    }

    fn desk_book(
        &self,
        t: &level1::Tenant,
    ) -> Result<(operator::desk::Book, &'static operator::watch::ChainPreset), Vec<u8>> {
        let c = chain(&t.chain).ok_or_else(|| {
            json_reply(
                "500 Internal Server Error",
                json!({ "error": "unknown chain" }),
            )
        })?;
        let desk_id = Address::from_str(&t.policy).map_err(|e| {
            json_reply(
                "500 Internal Server Error",
                json!({ "error": e.to_string() }),
            )
        })?;
        let book = operator::desk::Book::open(
            &operator::desk::book_path(&self.dir, &t.policy),
            c.chain_id,
            desk_id,
        )
        .map_err(|e| {
            json_reply(
                "500 Internal Server Error",
                json!({ "error": format!("the desk's book does not replay: {e}") }),
            )
        })?;
        Ok((book, c))
    }

    /// `GET /api/desk?policy=` (docs/83 W5): the book — deals, where the goods
    /// are, who vouched for each step, the exposure and the short it implies.
    async fn desk_get(&self, query: &str, raw: &str) -> Vec<u8> {
        let policy = query.split('&').find_map(|kv| kv.strip_prefix("policy="));
        let t = match self.desk_tenant(policy) {
            Ok(t) => t,
            Err(r) => return r,
        };
        if !self.desk_owner(&t, raw, None, "read") {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "the desk is the owner's: open it on the device that opened the account, or with an agent key that can read" }),
            );
        }
        let book = {
            let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            match self.desk_book(&t) {
                Ok((b, _)) => b,
                Err(r) => return r,
            }
        };
        let mut v = match operator::desk::view(&book, &t.policy) {
            Ok(v) => v,
            Err(e) => return json_reply("500 Internal Server Error", json!({ "error": e })),
        };
        // The venue as it is now, read (never traded) so the page shows the
        // real offset when it opens, not only the target.
        if let Some(venue) = operator::desk::venue_account(&t.policy) {
            let market = ::desk::Goods::Gold.market();
            let (pos, mark) = tokio::join!(
                operator::desk::venue_position(venue, market),
                operator::desk::venue_mark(market)
            );
            v["venue"] = json!({
                "account": format!("{venue:#x}"),
                "market": market,
                "position": pos.as_ref().map(|x| x.to_string()).ok(),
                "mark": mark.as_ref().map(|x| x.to_string()).ok(),
                "error": pos.err().or(mark.err()),
                "readAt": now_secs(),
            });
        }
        json_reply("200 OK", v)
    }

    /// `GET /api/desk/statements?policy=`: the lender kits this desk issued,
    /// newest first — what each one said and its file, so the owner's other
    /// devices show the last maximum loss and can download it again.
    async fn desk_statements(&self, query: &str, raw: &str) -> Vec<u8> {
        let policy = query.split('&').find_map(|kv| kv.strip_prefix("policy="));
        let t = match self.desk_tenant(policy) {
            Ok(t) => t,
            Err(r) => return r,
        };
        if !self.desk_owner(&t, raw, None, "read") {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "statements are the owner's" }),
            );
        }
        let dir = operator::desk::book_path(&self.dir, &t.policy).with_file_name("statements");
        let want = query.split('&').find_map(|kv| kv.strip_prefix("file="));
        let mut kits: Vec<(i64, String, Value)> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        if !name.ends_with(".lender.json") {
                            return None;
                        }
                        let kit: Value =
                            serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
                        let at = kit
                            .pointer("/signed/statement/at")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        Some((at, name, kit))
                    })
                    .collect()
            })
            .unwrap_or_default();
        kits.sort_by_key(|k| std::cmp::Reverse(k.0));
        // The whole-stock files (`book-<at>.json`), newest first; one by name, as written.
        let mut books: Vec<(i64, String)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                let at = n
                    .strip_prefix("book-")?
                    .strip_suffix(".json")?
                    .parse()
                    .ok()?;
                Some((at, n))
            })
            .collect();
        books.sort_by_key(|b| std::cmp::Reverse(b.0));
        if let Some(b) = query.split('&').find_map(|kv| kv.strip_prefix("book=")) {
            return match books.iter().find(|(_, n)| n == b) {
                Some((_, n)) => match std::fs::read_to_string(dir.join(n)) {
                    Ok(text) => respond("200 OK", "application/json", text.as_bytes()),
                    Err(e) => json_reply(
                        "500 Internal Server Error",
                        json!({ "error": e.to_string() }),
                    ),
                },
                None => json_reply("404 Not Found", json!({ "error": "no such statement" })),
            };
        }
        // One file by name, byte for byte as issued (names come from the listing only).
        if let Some(f) = want {
            return match kits.into_iter().find(|(_, n, _)| n == f) {
                Some((_, _, kit)) => json_reply("200 OK", kit),
                None => json_reply("404 Not Found", json!({ "error": "no such statement" })),
            };
        }
        let list: Vec<Value> = kits
            .iter()
            .map(|(at, name, kit)| {
                let s = &kit["signed"]["statement"];
                json!({
                    "file": name, "at": at, "deal": s["deal"], "held": s["held"], "stage": s["stage"],
                    "offsetShareBps": s["offsetShareBps"], "maxLossUsdc": s["maxLossUsdc"], "eligibleUsdc": s["eligibleUsdc"],
                    "fundingUsdc": s["fundingUsdc"], "stressBps": s["stressBps"], "basisBps": s["basisBps"], "anchorTx": kit["anchor"]["tx"],
                    // What the owner's page needs to tell a statement still current (docs/96 V4, V7).
                    "offsetContracts": s["offsetContracts"], "steps": s["milestones"].as_array().map_or(0, Vec::len),
                })
            })
            .collect();
        json_reply(
            "200 OK",
            json!({ "statements": list, "books": books.iter().map(|(at, n)| json!({ "file": n, "at": at })).collect::<Vec<_>>() }),
        )
    }

    /// The owner's passkey over one desk challenge (docs/89 M5): `issued`
    /// within ten minutes, the assertion verified against the account's owner
    /// keys, the challenge spent once. Returns what the book keeps with the
    /// line: the assertion, the challenge and when it was issued.
    async fn desk_owner_passkey(
        &self,
        t: &level1::Tenant,
        v: &Value,
        what: &str,
        challenge: impl FnOnce(u64, Address, u64) -> alloy::primitives::B256,
    ) -> Result<Value, Vec<u8>> {
        let (Some(issued), Ok(auth)) = (v["issued"].as_u64(), level1::auth_from_json(&v["auth"]))
        else {
            return Err(json_reply(
                "401 Unauthorized",
                json!({ "error": format!("{what} needs the owner's passkey: send issued and auth (a passkey assertion over the challenge)") }),
            ));
        };
        let now = now_secs();
        if issued > now + 60 || now.saturating_sub(issued) > 600 {
            return Err(json_reply(
                "400 Bad Request",
                json!({ "error": "the challenge is stale; ask for a new one" }),
            ));
        }
        let c = chain(&t.chain).ok_or_else(|| {
            json_reply(
                "500 Internal Server Error",
                json!({ "error": "unknown chain" }),
            )
        })?;
        let (Ok(policy), Ok(owner)) = (Address::from_str(&t.policy), Address::from_str(&t.owner))
        else {
            return Err(json_reply(
                "500 Internal Server Error",
                json!({ "error": "the account record is malformed" }),
            ));
        };
        // Every desk decision here widens trust, so an account that needs
        // several owners refuses one co-owner alone (docs/91 R5), as the
        // agent-key and report routes do.
        let (threshold, keys) = if t.deployed {
            owner_keys(c, owner).await
        } else {
            tenant_keys(c, t, owner).await.map(|k| (1, k))
        }
        .map_err(|e| {
            json_reply(
                "502 Bad Gateway",
                json!({ "error": format!("reading the owner's key: {e}") }),
            )
        })?;
        let ch = challenge(c.chain_id, policy, issued);
        verify_any_owner(&keys, ch, &auth)
            .map_err(|e| json_reply("403 Forbidden", json!({ "error": e })))?;
        if let Some(refused) = needs_more_approvals(threshold, "the desk") {
            return Err(refused);
        }
        if !self.notify_used.lock().await.insert(ch) {
            return Err(json_reply(
                "400 Bad Request",
                json!({ "error": "that challenge was already used" }),
            ));
        }
        Ok(json!({ "issued": issued, "challenge": format!("{ch:#x}"), "auth": v["auth"] }))
    }

    /// `POST /api/desk/party {policy, name, address, issued, auth}`: the owner
    /// approves a counterparty — a trust decision, like adding a payee, so it
    /// needs the owner's passkey over `desk::party_challenge` (docs/89 M5), not
    /// only the device token: a stolen token must not be able to add a signer
    /// whose steps move the hedge. The assertion is kept with the line.
    async fn desk_party(&self, body: &str, _raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        let name = v["name"].as_str().unwrap_or("").trim().to_string();
        let Some(address) = v["address"]
            .as_str()
            .and_then(|a| Address::from_str(a.trim()).ok())
        else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "address must be a wallet address" }),
            );
        };
        // The kinds of step it may sign (docs/91 R7); absent = any kind, the v1 challenge.
        let kinds: Option<Vec<::desk::MilestoneKind>> = match v.get("kinds") {
            None | Some(Value::Null) => None,
            Some(k) => match serde_json::from_value(k.clone()) {
                Ok(k) => Some(k),
                Err(e) => {
                    return json_reply(
                        "400 Bad Request",
                        json!({ "error": format!("kinds: {e}") }),
                    );
                }
            },
        };
        let approval = match self
            .desk_owner_passkey(
                &t,
                &v,
                "approving who may sign",
                |chain_id, policy, issued| match &kinds {
                    Some(k) => operator::desk::party_challenge_v2(
                        chain_id, policy, &name, address, k, issued,
                    ),
                    None => {
                        operator::desk::party_challenge(chain_id, policy, &name, address, issued)
                    }
                },
            )
            .await
        {
            Ok(a) => a,
            Err(r) => return r,
        };
        let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut book, _) = match self.desk_book(&t) {
            Ok(b) => b,
            Err(r) => return r,
        };
        match book.approve_for(&name, &format!("{address:#x}"), kinds, Some(approval)) {
            Ok(()) => json_reply("200 OK", json!({ "parties": book.parties })),
            Err(e) => json_reply("400 Bad Request", json!({ "error": e })),
        }
    }

    /// `GET /api/desk/venue?policy=`: this Mac's trade-only key, the one
    /// exchange account that approved it (Hyperliquid's own `userRole`), and
    /// whether this desk trades on it. The owner's.
    async fn desk_venue_get(&self, query: &str, raw: &str) -> Vec<u8> {
        let policy = query.split('&').find_map(|kv| kv.strip_prefix("policy="));
        let t = match self.desk_tenant(policy) {
            Ok(t) => t,
            Err(r) => return r,
        };
        if !self.desk_owner(&t, raw, None, "read") {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "the desk is the owner's" }),
            );
        }
        let connected = operator::desk::venue_account(&t.policy).map(|a| format!("{a:#x}"));
        let by_env = std::env::var("TAMEION_DESK_POLICY").is_ok();
        // S3 (docs/103 S3-1): the key this desk trades with — its own after an `ApproveAgent` connect.
        match operator::desk::desk_agent(&t.policy).await {
            Ok((agent, account)) => json_reply(
                "200 OK",
                json!({ "agent": format!("{agent:#x}"), "account": account.map(|a| format!("{a:#x}")), "connected": connected, "byEnvironment": by_env }),
            ),
            Err(e) => json_reply(
                "200 OK",
                json!({ "error": e, "connected": connected, "byEnvironment": by_env }),
            ),
        }
    }

    /// `POST /api/desk/venue {policy, account, issued, auth}`: the owner lets
    /// the desk trade on the exchange account that approved this Mac's
    /// trade-only key — a trust decision, so the owner's passkey signs
    /// `desk::venue_challenge`. Refused unless Hyperliquid says that account
    /// approved the key, and while another desk holds it.
    async fn desk_venue_connect(&self, body: &str, _raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        if std::env::var("TAMEION_DESK_POLICY").is_ok() {
            return json_reply(
                "409 Conflict",
                json!({ "error": "this service was started for one desk by its environment (TAMEION_DESK_POLICY); restart it without that to connect from the page" }),
            );
        }
        let Some(account) = v["account"]
            .as_str()
            .and_then(|a| Address::from_str(a.trim()).ok())
        else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "account must be an address" }),
            );
        };
        match operator::desk::this_macs_agent().await {
            Ok((_, Some(a))) if a == account => {}
            Ok((agent, other)) => {
                return json_reply(
                    "400 Bad Request",
                    json!({ "error": match other {
                        Some(o) => format!("this Mac's trade-only key {agent:#x} was approved by {o:#x}, not {account:#x}"),
                        None => format!("no exchange account has approved this Mac's trade-only key {agent:#x} yet"),
                    } }),
                );
            }
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e })),
        }
        let approval = match self
            .desk_owner_passkey(
                &t,
                &v,
                "letting the desk trade on an exchange account",
                |c, p, i| operator::desk::venue_challenge(c, p, account, i),
            )
            .await
        {
            Ok(a) => a,
            Err(r) => return r,
        };
        match operator::desk::connect_venue(
            &t.policy,
            account,
            approval,
            now_secs(),
            &operator::desk::desks_on_this_mac(),
        ) {
            Ok(()) => json_reply("200 OK", json!({ "connected": format!("{account:#x}") })),
            Err(e) => json_reply("409 Conflict", json!({ "error": e })),
        }
    }

    /// `POST /api/desk/venue/disconnect {policy}`: the desk stops trading at
    /// once (trust shrinks: the device or a run key is enough).
    async fn desk_venue_disconnect(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        if !self.desk_owner(&t, raw, Some(&v), "run") {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "only the owner's device or a run key disconnects the desk" }),
            );
        }
        match operator::desk::disconnect_venue(&t.policy, now_secs()) {
            Ok(was) => json_reply("200 OK", json!({ "disconnected": was })),
            Err(e) => json_reply("500 Internal Server Error", json!({ "error": e })),
        }
    }

    /// `POST /api/desk/party/revoke {policy, name}`: the owner stops a
    /// counterparty (docs/91 R4). Trust shrinks at once, like lowering a cap:
    /// the owner's device or a run key is enough, no passkey and no wait.
    /// Steps it signed before stay booked; a later one is refused.
    async fn desk_party_revoke(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        if !self.desk_owner(&t, raw, Some(&v), "run") {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "only the owner's device or a run key stops a counterparty" }),
            );
        }
        let name = v["name"].as_str().unwrap_or("").trim().to_string();
        let done = {
            let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            match self.desk_book(&t) {
                Ok((mut book, _)) => book.revoke(&name, now_secs()),
                Err(r) => return r,
            }
        };
        match done {
            Ok(()) => {
                let (dir, policy) = (self.dir.clone(), t.policy.clone());
                let text = format!(
                    "{name} was stopped on your desk: its signature books nothing until you approve it again"
                );
                tokio::spawn(async move { operator::push::tell(&dir, &policy, &text).await });
                json_reply("200 OK", json!({ "stopped": name }))
            }
            Err(e) => json_reply("400 Bad Request", json!({ "error": e })),
        }
    }

    /// `POST /api/desk/explain {policy, deal}`: the deal's latest statement in
    /// words, written by the model on this Mac (`TAMEION_LOCAL_MODEL`, Ollama),
    /// kept only when every number in it is the statement's; otherwise the
    /// plain template. Reads a saved statement: nothing trades, nothing anchors.
    async fn desk_explain(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        if !self.desk_owner(&t, raw, Some(&v), "read") {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "statements are the owner's" }),
            );
        }
        let deal = v["deal"].as_str().unwrap_or("");
        let dir = operator::desk::book_path(&self.dir, &t.policy).with_file_name("statements");
        let latest = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let kit: Value =
                    serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
                (kit.pointer("/signed/statement/deal")?.as_str()? == deal).then_some(kit)
            })
            .max_by_key(|k| {
                k.pointer("/signed/statement/at")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            });
        let Some(kit) = latest else {
            return json_reply(
                "404 Not Found",
                json!({ "error": "no statement for this deal yet; issue one first" }),
            );
        };
        let st: ::desk::Statement = match serde_json::from_value(kit["signed"]["statement"].clone())
        {
            Ok(s) => s,
            Err(e) => {
                return json_reply(
                    "500 Internal Server Error",
                    json!({ "error": e.to_string() }),
                );
            }
        };
        let name = format!(
            "ollama:{}",
            std::env::var("TAMEION_LOCAL_MODEL")
                .unwrap_or_else(|_| operator::local::DEFAULT_MODEL.into())
        );
        let model = operator::local::LocalReasoner::from_env();
        let mut out = operator::desk::explain(&st, &model, &name).await;
        out["deal"] = json!(deal);
        out["statementAt"] = json!(st.at);
        json_reply("200 OK", out)
    }

    /// `POST /api/desk/control {policy, text}`: the owner states who holds the
    /// venue's master key ("held by <lender>", docs/87 option A); printed on
    /// every statement. The owner's device only.
    async fn desk_control(&self, body: &str, _raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        let text = v["text"].as_str().unwrap_or("").trim().to_string();
        let approval = match self
            .desk_owner_passkey(
                &t,
                &v,
                "saying who controls the offset",
                |chain_id, policy, issued| {
                    operator::desk::control_challenge(chain_id, policy, &text, issued)
                },
            )
            .await
        {
            Ok(a) => a,
            Err(r) => return r,
        };
        let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut book, _) = match self.desk_book(&t) {
            Ok(b) => b,
            Err(r) => return r,
        };
        match book.set_control(&text, Some(approval)) {
            Ok(()) => json_reply("200 OK", json!({ "control": book.control })),
            Err(e) => json_reply("400 Bad Request", json!({ "error": e })),
        }
    }

    /// `POST /api/desk/base {policy, goods, grams}`: the owner sets the stock
    /// kept unhedged because it is replaced every day anyway (docs/84 §1) —
    /// a trust decision, the owner's device only.
    async fn desk_base(&self, body: &str, _raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        let Some(goods) = v["goods"].as_str().and_then(::desk::Goods::parse) else {
            return json_reply("400 Bad Request", json!({ "error": "goods must be gold" }));
        };
        let grams = v["grams"].as_str().unwrap_or("").trim().to_string();
        let approval = match self
            .desk_owner_passkey(
                &t,
                &v,
                "setting what stays unhedged",
                |chain_id, policy, issued| {
                    operator::desk::base_challenge(chain_id, policy, goods.name(), &grams, issued)
                },
            )
            .await
        {
            Ok(a) => a,
            Err(r) => return r,
        };
        let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut book, _) = match self.desk_book(&t) {
            Ok(b) => b,
            Err(r) => return r,
        };
        match book.set_base(goods, &grams, Some(approval)) {
            Ok(()) => json_reply(
                "200 OK",
                json!({
                    "unhedgedBase": book.desk.base(goods).to_string(),
                    "offsetTarget": book.desk.offset_target(goods).map(|x| x.round_dp(6).to_string()).ok(),
                }),
            ),
            Err(e) => json_reply("400 Bad Request", json!({ "error": e })),
        }
    }

    /// `POST /api/desk/milestone {policy, milestone, signature?}`: a supplier's
    /// purchase confirmation, a carrier's bill of lading, a buyer's sale —
    /// signed by the approved counterparty it names, booked with no tap. An
    /// unsigned milestone is the owner's own entry and needs the owner.
    async fn desk_milestone(&self, body: &str, _raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        let m: ::desk::Milestone = match serde_json::from_value(v["milestone"].clone()) {
            Ok(m) => m,
            Err(e) => {
                return json_reply(
                    "400 Bad Request",
                    json!({ "error": format!("milestone: {e}") }),
                );
            }
        };
        let sig = v["signature"].as_str().filter(|s| !s.trim().is_empty());
        // An unsigned step is the owner's own word, and a purchase or sale moves
        // the hedge: the owner's passkey signs it (docs/89 M5), over the step's
        // EIP-712 digest for this desk, so a stolen device token cannot invent one.
        let owner = if sig.is_none() {
            let mut unsigned = m.clone();
            unsigned.signer = String::new();
            match self
                .desk_owner_passkey(&t, &v, "a step of your own", |chain_id, policy, issued| {
                    operator::desk::step_challenge(
                        chain_id,
                        policy,
                        operator::desk::event_digest(chain_id, policy, &unsigned),
                        issued,
                    )
                })
                .await
            {
                Ok(a) => Some(a),
                Err(r) => return r,
            }
        } else {
            None
        };
        // The venue's mark now, kept beside a priced step so the deal card can
        // still name a price far from the market later (read before the lock:
        // it is a network call).
        let mark = match m.usdc {
            Some(_) => operator::desk::venue_mark(m.goods.market()).await.ok(),
            None => None,
        };
        let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut book, _) = match self.desk_book(&t) {
            Ok(b) => b,
            Err(r) => return r,
        };
        let m = match book.record_with(m, sig, owner, mark) {
            Ok(m) => m,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e })),
        };
        drop(_g);
        // The surface rule (docs/53): a purchase or a sale changes what the
        // desk holds, so the offset follows it now, with no tap — inside every
        // rule of the hedge engine, on evidence from a counterparty the owner
        // approved. Movements (shipped, into stock, fabrication) change only
        // where the goods are, not how much is held.
        // A signed step above the owner's limit is booked, but the offset
        // waits for the owner's tap: a stolen counterparty key must not be
        // able to move a large hedge by itself (TAMEION_DESK_AUTO_GRAMS,
        // default 1,000 g; docs/60 P-114).
        let auto_limit = operator::desk::auto_limit(m.goods);
        let limit_g = rust_decimal::Decimal::from_i128_with_scale(
            auto_limit.to_minor(),
            u32::from(auto_limit.currency().scale()),
        );
        let offset = match (m.kind, operator::desk::venue_account(&t.policy)) {
            (::desk::MilestoneKind::Bought | ::desk::MilestoneKind::Sold, Some(venue)) => {
                // One step at a time (docs/89 H2), sized from the book as it is
                // now — another step may have landed since this one was booked.
                let _trade = DESK_TRADE.lock().await;
                let latest = {
                    let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                    self.desk_book(&t).map(|(b, _)| b)
                };
                match latest {
                    Err(_) => {
                        json!({ "error": "the book could not be read again; nothing was traded" })
                    }
                    Ok(mut latest) => {
                        let market = m.goods.market();
                        let target = latest.desk.offset_target(m.goods).unwrap_or_default();
                        match operator::desk::read_back(venue, market).await {
                            Err(e) => json!({
                                "waiting": format!("the venue did not answer ({e}); the offset waits for your tap"),
                            }),
                            Ok(pos) => {
                                // The limit is on what this step would trade, not
                                // on the step's own grams (docs/89 H1): a 50 kg
                                // purchase that waited is not traded by the next 1 g.
                                // And on the total moved without the owner since the
                                // owner last confirmed one (docs/91 R3): a stolen key
                                // signing step after step cannot walk the offset.
                                let trade_g = operator::desk::trade_grams(m.goods, target, pos);
                                if trade_g + latest.auto_moved > limit_g {
                                    json!({
                                        "waiting": format!(
                                            "this would trade {} g of {} on the venue{}, above the {} you let the desk move by itself; it waits for your tap",
                                            trade_g.round_dp(3), m.goods.name(),
                                            if latest.auto_moved.is_zero() { String::new() } else { format!(", after {} g it moved by itself since you last confirmed", latest.auto_moved.round_dp(3)) },
                                            auto_limit
                                        ),
                                    })
                                } else {
                                    match operator::desk::offset(&latest.desk, m.goods, venue).await
                                    {
                                        Ok(r) => {
                                            let rb = operator::desk::read_back(venue, market).await;
                                            let moved = rb.as_ref().map_or(trade_g, |x| {
                                                (*x - pos).abs() * m.goods.units_per_contract()
                                            });
                                            let kept = {
                                                let _g = DESK_LOCK
                                                    .lock()
                                                    .unwrap_or_else(|e| e.into_inner());
                                                latest.moved(moved, true, now_secs(), None)
                                            };
                                            json!({
                                                "movedError": kept.err(),
                                                "live": std::env::var("HEDGE_LIVE").as_deref() == Ok("1"),
                                                "coins": r.coins.iter().map(|c| json!({ "coin": c.coin, "before": c.hedge_size_before.to_string(), "target": c.target_size.round_dp(6).to_string(), "action": format!("{:?}", c.action) })).collect::<Vec<_>>(),
                                                "readBack": rb.as_ref().ok().map(|x| x.to_string()),
                                                "readBackError": rb.err(),
                                            })
                                        }
                                        Err(e) => json!({ "error": e }),
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => Value::Null,
        };
        // What the offset did for this step, in the service's log: a step that
        // did not trade says why (docs/60 P-129).
        eprintln!(
            "desk {} {} {:?}: offset {}",
            t.policy,
            m.deal,
            m.kind,
            offset
                .get("waiting")
                .or_else(|| offset.get("error"))
                .or_else(|| offset.get("readBack"))
                .map_or_else(
                    || "none (a movement, or no exchange)".to_string(),
                    |x| x.to_string()
                )
        );
        if m.kind == ::desk::MilestoneKind::Sold
            && offset.get("readBack").is_some_and(|x| !x.is_null())
        {
            desk_sold().notify_one();
        }
        // A price far from the market is named, not refused: the signature
        // says the counterparty means it, and the owner decides what to do.
        let price_note =
            mark.and_then(|k| operator::desk::price_note(&m, k, operator::desk::price_band_bps()));
        // Tell the owner's phone what arrived and what the offset did (docs/53 5.3).
        let text = match offset.get("readBack").and_then(Value::as_str) {
            _ if offset.get("waiting").is_some() => format!(
                "{} · the offset waits for you: open the desk",
                operator::desk::words(&m)
            ),
            Some(r) => format!(
                "{} · offset now {r} {}",
                operator::desk::words(&m),
                m.goods.market()
            ),
            None => operator::desk::words(&m),
        };
        let text = match &price_note {
            Some(n) => format!("{text} · {n}"),
            None => text,
        };
        let (dir, policy) = (self.dir.clone(), t.policy.clone());
        tokio::spawn(async move { operator::push::tell(&dir, &policy, &text).await });
        json_reply(
            "200 OK",
            json!({ "booked": operator::desk::words(&m), "signer": m.signer, "root": book.desk.root().map(|r| r.to_hex()).ok(), "offset": offset, "priceNote": price_note }),
        )
    }

    /// `POST /api/desk/offset {policy}`: one hedge step sized from the book,
    /// read back from the venue. Live only when the service runs with
    /// `HEDGE_LIVE=1` and a trade-only key; otherwise a dry run that says so.
    async fn desk_offset(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        if !self.desk_owner(&t, raw, Some(&v), "run") {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "only the owner's device or a run key moves the offset" }),
            );
        }
        let Some(venue) = operator::desk::venue_account(&t.policy) else {
            return json_reply(
                "409 Conflict",
                json!({ "error": "no venue account is connected to this desk (TAMEION_DESK_VENUE)" }),
            );
        };
        let _trade = DESK_TRADE.lock().await;
        let mut book = {
            let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            match self.desk_book(&t) {
                Ok((b, _)) => b,
                Err(r) => return r,
            }
        };
        let goods = ::desk::Goods::Gold;
        let target = book.desk.offset_target(goods).unwrap_or_default();
        let pos = match operator::desk::read_back(venue, goods.market()).await {
            Ok(p) => p,
            Err(e) => {
                return json_reply(
                    "502 Bad Gateway",
                    json!({ "error": format!("the venue did not answer ({e}); nothing was traded") }),
                );
            }
        };
        // Above what the desk may move by itself, only the owner's passkey
        // releases the trade (docs/91 R6): a device token or a run key moves
        // the offset inside the limit, never past it. The passkey signs the
        // target the page showed; if a step moved it since, the page asks again.
        let auto_limit = operator::desk::auto_limit(goods);
        let limit_g = rust_decimal::Decimal::from_i128_with_scale(
            auto_limit.to_minor(),
            u32::from(auto_limit.currency().scale()),
        );
        let trade_g = operator::desk::trade_grams(goods, target, pos);
        let by_owner = !v["auth"].is_null() || trade_g + book.auto_moved > limit_g;
        let approval = if by_owner {
            let shown = target.round_dp(6).to_string();
            match self
                .desk_owner_passkey(&t, &v, "moving the offset past your limit", |c, p, i| {
                    operator::desk::offset_challenge(c, p, &shown, i)
                })
                .await
            {
                // An offset approval is good once, also across restarts (docs/94 U8).
                Ok(a) if book.is_spent(&a) => {
                    return json_reply(
                        "400 Bad Request",
                        json!({ "error": "that approval was already used; sign again" }),
                    );
                }
                Ok(a) => Some(a),
                Err(r) => return r,
            }
        } else {
            None
        };
        match operator::desk::offset(&book.desk, goods, venue).await {
            Ok(r) => {
                let rb = operator::desk::read_back(venue, "PAXG").await;
                let read = rb.as_ref().ok().copied();
                let moved = read.map_or(trade_g, |x| (x - pos).abs() * goods.units_per_contract());
                let kept = {
                    let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                    book.moved(moved, !by_owner, now_secs(), approval)
                };
                json_reply(
                    "200 OK",
                    json!({
                        "byOwner": by_owner,
                        "movedError": kept.err(),
                        "live": std::env::var("HEDGE_LIVE").as_deref() == Ok("1"),
                        "target": book.desk.offset_target(::desk::Goods::Gold).map(|x| x.round_dp(6).to_string()).ok(),
                        "coins": r.coins.iter().map(|c| json!({ "coin": c.coin, "mark": c.mark_px.to_string(), "before": c.hedge_size_before.to_string(), "action": format!("{:?}", c.action) })).collect::<Vec<_>>(),
                        "readBack": read.map(|x| x.to_string()),
                        "readBackError": rb.err(),
                        "accountValue": r.account.account_value.to_string(),
                        "venueAccount": format!("{venue:#x}"),
                    }),
                )
            }
            Err(e) => json_reply("502 Bad Gateway", json!({ "error": e })),
        }
    }

    /// `POST /api/desk/statement {policy, deal}`: the lender's kit for one
    /// deal — signed statement, disclosure, counterparty signatures, and the
    /// root anchored now on the account's chain (`scripts/verify_desk.py`).
    async fn desk_statement(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        if !self.desk_owner(&t, raw, Some(&v), "run") {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "only the owner's device or a run key issues a statement" }),
            );
        }
        let Some(venue) = operator::desk::venue_account(&t.policy) else {
            return json_reply(
                "409 Conflict",
                json!({ "error": "no venue account is connected to this desk (TAMEION_DESK_VENUE)" }),
            );
        };
        let (book, c) = {
            let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            match self.desk_book(&t) {
                Ok(b) => b,
                Err(r) => return r,
            }
        };
        let deal = v["deal"].as_str().unwrap_or("");
        let dir = operator::desk::book_path(&self.dir, &t.policy).with_file_name("statements");
        let reuse = book
            .desk
            .root()
            .ok()
            .and_then(|r| operator::desk::anchored_before(&dir, &r.to_hex(), c.chain_id));
        let earlier = operator::desk::earlier_roots(&dir);
        match operator::desk::lender_kit(&book, deal, c, venue, now_secs() as i64, reuse, &earlier)
            .await
        {
            Ok(kit) => {
                let _ = std::fs::create_dir_all(&dir);
                let at = kit
                    .pointer("/signed/statement/at")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let _ = std::fs::write(
                    dir.join(format!("{deal}-{at}.lender.json")),
                    kit.to_string(),
                );
                json_reply("200 OK", kit)
            }
            Err(e) => json_reply("400 Bad Request", json!({ "error": e })),
        }
    }

    /// `POST /api/desk/statements/all {policy}`: every open deal's statement at
    /// one moment, over one root and one anchor — what a lender financing the
    /// dealer's whole stock checks together, so no two deals can each claim the
    /// same short (docs/94 U1, check w). One file: `{"kits": [...], "at"}`.
    async fn desk_statement_all(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let t = match self.desk_tenant(v["policy"].as_str()) {
            Ok(t) => t,
            Err(r) => return r,
        };
        if !self.desk_owner(&t, raw, Some(&v), "run") {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "only the owner's device or a run key issues statements" }),
            );
        }
        match self.statements_all(&t).await {
            Ok(file) => json_reply("200 OK", file),
            Err((code, e)) => json_reply(code, json!({ "error": e })),
        }
    }

    /// Every open deal's statement at one moment, saved per deal and as
    /// `statements/book-<at>.json`; the route and the morning task share it.
    async fn statements_all(&self, t: &level1::Tenant) -> Result<Value, (&'static str, String)> {
        let venue = operator::desk::venue_account(&t.policy).ok_or((
            "409 Conflict",
            "no exchange account is connected to this desk".to_string(),
        ))?;
        let (book, c) = {
            let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            self.desk_book(t).map_err(|_| {
                (
                    "500 Internal Server Error",
                    "the book could not be read".to_string(),
                )
            })?
        };
        let open: Vec<String> = book
            .desk
            .deals()
            .filter(|d| {
                book.desk
                    .holding(&d.id)
                    .ok()
                    .and_then(|h| h.total().ok())
                    .is_some_and(|m| m.to_minor() > 0)
            })
            .map(|d| d.id.clone())
            .collect();
        if open.is_empty() {
            return Err(("409 Conflict", "no open deal: nothing to finance".into()));
        }
        let dir = operator::desk::book_path(&self.dir, &t.policy).with_file_name("statements");
        let earlier = operator::desk::earlier_roots(&dir);
        let mut reuse = book
            .desk
            .root()
            .ok()
            .and_then(|r| operator::desk::anchored_before(&dir, &r.to_hex(), c.chain_id));
        let now = now_secs() as i64;
        let _ = std::fs::create_dir_all(&dir);
        let mut kits = Vec::new();
        for deal in &open {
            let kit =
                operator::desk::lender_kit(&book, deal, c, venue, now, reuse.clone(), &earlier)
                    .await
                    .map_err(|e| ("400 Bad Request", format!("{deal}: {e}")))?;
            // One root, one anchor for them all; kept on disk at once so a retry after a later deal fails reuses
            // it (`anchored_before`) instead of anchoring again.
            if reuse.is_none() {
                let root = kit["signed"]["statement"]["root"]
                    .as_str()
                    .unwrap_or_default();
                let _ = std::fs::write(
                    dir.join(format!("anchor-{root}.json")),
                    json!({ "signed": { "statement": { "root": root } }, "anchor": kit["anchor"] })
                        .to_string(),
                );
            }
            reuse = Some(kit["anchor"].clone());
            kits.push(kit);
        }
        // The per-deal files only once every deal succeeded (docs/96 V8): a half book leaves no statements behind.
        for (deal, kit) in open.iter().zip(&kits) {
            let _ = std::fs::write(
                dir.join(format!("{deal}-{now}.lender.json")),
                kit.to_string(),
            );
        }
        let file = json!({ "kits": kits, "at": now });
        let _ = std::fs::write(dir.join(format!("book-{now}.json")), file.to_string());
        Ok(file)
    }

    /// Every root this tenant's agent has anchored, newest last: the
    /// `anchors.jsonl` index the service keeps (root, anchorTx, at, events,
    /// agent, anchor). With `/api/level1/record` this is enough to verify a
    /// hosted tenant from another machine (`scripts/verify_tenant.sh`).
    async fn level1_records(&self, query: &str) -> Vec<u8> {
        let policy = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("policy="))
            .and_then(|x| Address::from_str(x.trim()).ok());
        let Some(policy) = policy else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let path = self
            .dir
            .join("tenants")
            .join(format!("{policy:#x}"))
            .join("log")
            .join("anchors.jsonl");
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => {
                let records: Vec<Value> = text
                    .lines()
                    .filter_map(|l| serde_json::from_str(l).ok())
                    .collect();
                // The chain the roots live on, so a verifier asks that chain's
                // nodes (a wrong answer here only makes verification fail).
                let chain =
                    operator::level1::load(&self.dir, &format!("{policy:#x}")).map(|t| t.chain);
                json_reply(
                    "200 OK",
                    json!({ "policy": format!("{policy:#x}"), "chain": chain, "records": records }),
                )
            }
            Err(_) => json_reply(
                "404 Not Found",
                json!({ "error": "no anchored records for that policy" }),
            ),
        }
    }

    /// `GET /api/level1/statement?policy=&week=YYYY-Www[&format=csv]` — the
    /// week's paid, refused and needs-you lines from the anchored record,
    /// signed EIP-712 by the statement key (docs/76 §1d, row 8). The week
    /// defaults to the current ISO week. Read gate as the inbox; the check
    /// is `scripts/verify_statement.sh`.
    async fn level1_statement(&self, query: &str) -> Vec<u8> {
        let get = |k: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
                .map(str::trim)
        };
        let Some(policy) = get("policy").and_then(|x| Address::from_str(x).ok()) else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let week = get("week")
            .map(str::to_string)
            .unwrap_or_else(|| operator::statement::week_of(now_secs() as i64));
        let Some((from, to)) = operator::statement::week_bounds(&week) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "week must be an ISO week like 2026-W42" }),
            );
        };
        let log_dir = level1::tenants_dir(&self.dir)
            .join(format!("{policy:#x}"))
            .join("log");
        let scope = operator::statement::Scope {
            policy,
            chain_key: c.key,
            chain_id: c.chain_id,
            explorer: c.explorer,
            week: &week,
            from,
            to,
        };
        // docs/60 P-177: the owner's own passkey payments are in the week too,
        // read from the policy's `SettledByPrincipal` events (a failed read
        // leaves the agent's record alone, as before).
        let owner_paid: Vec<operator::statement::OwnerPaid> = match level1::read_client(c) {
            Ok(client) => match client.block_number().await {
                Ok(head) => {
                    let list = client
                        .principal_settlements(policy, tenant.created_block.min(head), head)
                        .await
                        .unwrap_or_default();
                    let mut out = Vec::new();
                    for m in list {
                        if let Ok(t) = client.block_timestamp(m.block).await {
                            out.push(operator::statement::OwnerPaid {
                                at: t.as_unix(),
                                payee: format!("{:#x}", m.to),
                                amount: m.amount,
                                tx: format!("{:#x}", m.tx),
                            });
                        }
                    }
                    out
                }
                Err(_) => Vec::new(),
            },
            Err(_) => Vec::new(),
        };
        match operator::statement::build_with(&log_dir, &scope, &owner_paid) {
            Ok(st) if get("format") == Some("csv") => respond(
                "200 OK",
                "text/csv; charset=utf-8",
                operator::statement::csv(&st).as_bytes(),
            ),
            Ok(st) => json_reply("200 OK", st),
            Err(e) => json_reply("500 Internal Server Error", json!({ "error": e })),
        }
    }

    /// Set or clear where the agent reports after each pass. Admin token: a
    /// stranger must not be able to point a tenant's summaries at themselves.
    async fn level1_notify(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let given = v["token"].as_str().unwrap_or("");
        if !matches!(&self.admin_token, Some(t) if !t.is_empty() && t == given) {
            return json_reply("403 Forbidden", json!({ "error": "admin token" }));
        }
        let Some(policy) = v["policy"].as_str() else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(mut tenant) = level1::load(&self.dir, policy) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let chat = v
            .get("telegramChatId")
            .map(|c| c.as_str().map(String::from));
        let minted = match level1::apply_notify(
            &mut tenant,
            v["url"].as_str(),
            chat,
            v["rotateSecret"].as_bool().unwrap_or(false),
        ) {
            Ok(m) => m,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e })),
        };
        if let Err(e) = level1::save(&self.dir, &tenant) {
            return json_reply("500 Internal Server Error", json!({ "error": e }));
        }
        json_reply(
            "200 OK",
            json!({ "policy": tenant.policy, "notifyUrl": tenant.notify_url, "telegramChatId": tenant.telegram_chat_id, "notifySecret": minted, "signature": "X-Tameion-Signature: sha256=hex(HMAC-SHA256(key = the 32 bytes the hex secret encodes, msg = timestamp + '.' + raw body)); X-Tameion-Timestamp carries the timestamp" }),
        )
    }

    /// The challenge the owner's passkey signs to set where the agent reports
    /// (PLAN 5.3, no admin): over chain, policy, url, chat id and `issued`.
    /// This browser gets the agent's report as a notification (docs/65, Web
    /// Push, `operator::push`). `{policy, subscription}` answers the
    /// challenge the owner's passkey signs; the same body plus `issued` and
    /// `auth` adds the browser, once the assertion checks against the key
    /// the contract holds (the service's record of it before the account
    /// exists on chain). A stranger cannot add their own browser.
    async fn level1_push_subscribe(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let (endpoint, p256dh, auth_key) = match operator::push::subscription(&v["subscription"]) {
            Ok(s) => s,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e })),
        };
        let now = now_secs();
        if v["auth"].is_null() {
            let ch =
                operator::push::challenge(c.chain_id, policy, &endpoint, &p256dh, &auth_key, now);
            return json_reply(
                "200 OK",
                json!({ "challenge": format!("{ch:#x}"), "issued": now, "summary": "Tell this browser what the agent did after each check." }),
            );
        }
        let Some(issued) = v["issued"].as_u64() else {
            return json_reply("400 Bad Request", json!({ "error": "issued is required" }));
        };
        if issued > now + 60 || now.saturating_sub(issued) > 600 {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "the challenge is stale; ask for a new one" }),
            );
        }
        let auth = match level1::auth_from_json(&v["auth"]) {
            Ok(a) => a,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e })),
        };
        let ch =
            operator::push::challenge(c.chain_id, policy, &endpoint, &p256dh, &auth_key, issued);
        if !self.notify_used.lock().await.insert(ch) {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "that challenge was already used" }),
            );
        }
        let keys = if tenant.deployed {
            match Address::from_str(&tenant.owner) {
                Ok(o) => owner_keys(c, o).await.map(|(_, k)| k),
                Err(e) => Err(e.to_string()),
            }
        } else {
            match (tenant.qx.as_deref(), tenant.qy.as_deref()) {
                (Some(x), Some(y)) => alloy::primitives::B256::from_str(x)
                    .and_then(|x| alloy::primitives::B256::from_str(y).map(|y| vec![(x, y)]))
                    .map_err(|e| e.to_string()),
                _ => Err("this account has no passkey on record".into()),
            }
        };
        let keys = match keys {
            Ok(k) => k,
            Err(e) => {
                return json_reply(
                    "502 Bad Gateway",
                    json!({ "error": format!("reading the owner's key: {e}") }),
                );
            }
        };
        if let Err(e) = verify_any_owner(&keys, ch, &auth) {
            return json_reply("403 Forbidden", json!({ "error": e }));
        }
        match operator::push::add(&self.dir, &tenant.policy, &endpoint, &p256dh, &auth_key) {
            Ok(n) => json_reply("200 OK", json!({ "policy": tenant.policy, "devices": n })),
            Err(e) => json_reply("500 Internal Server Error", json!({ "error": e })),
        }
    }

    async fn level1_prepare_notify(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let url = v["url"].as_str().map(str::trim).unwrap_or("");
        let chat = v["telegramChatId"].as_str().map(str::trim).unwrap_or("");
        if !(url.is_empty() || url.starts_with("https://") || url.starts_with("http://")) {
            return json_reply("400 Bad Request", json!({ "error": "url must be http(s)" }));
        }
        let issued = now_secs();
        let ch = level1::notify_challenge(c.chain_id, policy, url, chat, issued);
        let summary = match (url.is_empty(), chat.is_empty()) {
            (true, true) => format!("Stop the agent's reports for policy {policy:#x}."),
            (false, true) => format!("Send the agent's reports for policy {policy:#x} to {url}."),
            (true, false) => format!(
                "Send the agent's reports for policy {policy:#x} to Telegram chat {chat}; no webhook."
            ),
            (false, false) => format!(
                "Send the agent's reports for policy {policy:#x} to {url} and Telegram chat {chat}."
            ),
        };
        json_reply(
            "200 OK",
            json!({ "challenge": format!("{ch:#x}"), "issued": issued, "summary": summary }),
        )
    }

    /// Apply notify settings carried by a passkey assertion over the
    /// challenge from `prepare-notify`: fresh, unused, and verified against
    /// the key the owner's contract holds. Anyone may call; only the owner's
    /// key passes.
    /// The challenge the owner's passkey signs to mint a capability token for
    /// another agent (PLAN 5.31): scopes, label, expiry.
    async fn level1_prepare_token(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let scopes: Vec<String> = v["scopes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        if scopes.is_empty()
            || !scopes
                .iter()
                .all(|s| level1::TOKEN_SCOPES.iter().any(|(k, _)| k == s))
        {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "scopes must be one or more of read, run, prepare, import, buy" }),
            );
        }
        let label = v["label"].as_str().unwrap_or("").trim().to_string();
        let days = v["days"].as_u64().unwrap_or(90).clamp(1, 365);
        let issued = now_secs();
        let expires = issued + days * 86_400;
        let ch = level1::token_challenge(c.chain_id, policy, &scopes, &label, expires, issued);
        json_reply(
            "200 OK",
            json!({ "challenge": format!("{ch:#x}"), "issued": issued, "expires": expires, "scopes": scopes,
                    "summary": format!("Give an agent{} a key to this account that may: {} — for {days} days. It can never sign; your passkey stays the only key that moves authority.", if label.is_empty() { String::new() } else { format!(" ({label})") }, scopes.join(", ")) }),
        )
    }

    /// Mint the token the passkey approved: verified against the owner's key
    /// on chain, fresh and unused; the secret is returned once, its hash kept.
    async fn level1_token_signed(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(mut tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let scopes: Vec<String> = v["scopes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        // The same scopes `prepare-token` allows (audit 2026-10-15, P-74 note):
        // the passkey signs whatever list is sent, so the route checks it too.
        if scopes.is_empty()
            || !scopes
                .iter()
                .all(|s| level1::TOKEN_SCOPES.iter().any(|(k, _)| k == s))
        {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "scopes must be one or more of read, run, prepare, import, buy" }),
            );
        }
        let label = v["label"].as_str().unwrap_or("").trim().to_string();
        let (Some(issued), Some(expires)) = (v["issued"].as_u64(), v["expires"].as_u64()) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "issued and expires are required" }),
            );
        };
        let auth = match level1::auth_from_json(&v["auth"]) {
            Ok(a) => a,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e })),
        };
        let now = now_secs();
        if issued > now + 60 || now.saturating_sub(issued) > 600 {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "the challenge is stale; ask for a new one" }),
            );
        }
        let ch = level1::token_challenge(c.chain_id, policy, &scopes, &label, expires, issued);
        if !self.notify_used.lock().await.insert(ch) {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "that challenge was already used" }),
            );
        }
        let Ok(owner) = Address::from_str(&tenant.owner) else {
            return json_reply(
                "500 Internal Server Error",
                json!({ "error": "tenant owner is not an address" }),
            );
        };
        let (threshold, keys) = match owner_keys(c, owner).await {
            Ok(k) => k,
            Err(e) => {
                return json_reply(
                    "502 Bad Gateway",
                    json!({ "error": format!("reading the owner's key: {e}") }),
                );
            }
        };
        if let Err(e) = verify_any_owner(&keys, ch, &auth) {
            return json_reply("403 Forbidden", json!({ "error": e }));
        }
        if let Some(refused) = needs_more_approvals(threshold, "agent keys") {
            return refused;
        }
        let secret = level1::new_secret();
        let id = format!(
            "tok-{}",
            &level1::token_hash(&format!("{secret}{now}"))[..8]
        );
        tenant.agent_tokens.push(level1::AgentToken {
            id: id.clone(),
            label: label.clone(),
            scopes: scopes.clone(),
            expires,
            issued: now,
            hash: level1::token_hash(&secret),
        });
        if let Err(e) = level1::save(&self.dir, &tenant) {
            return json_reply("500 Internal Server Error", json!({ "error": e }));
        }
        json_reply(
            "200 OK",
            json!({ "id": id, "token": secret, "scopes": scopes, "label": label, "expires": expires,
                    "use": "set TAMEION_TOKEN=<token> for tameion-mcp, or send Authorization: Bearer <token>; shown once",
                    "gated": "from now on this account's read / run / prepare / import routes need an agent key; buying (x402) always did" }),
        )
    }

    /// The challenge for revoking token `id` (the page asks, then signs).
    fn level1_prepare_token_revoke(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let id = v["id"].as_str().unwrap_or("");
        let issued = v["issued"].as_u64().unwrap_or_else(now_secs);
        let ch = level1::token_revoke_challenge(c.chain_id, policy, id, issued);
        json_reply(
            "200 OK",
            json!({ "challenge": format!("{ch:#x}"), "issued": issued, "summary": format!("revoke agent key {id}") }),
        )
    }

    /// Revoke a token by id, on a passkey assertion over the revoke challenge.
    async fn level1_token_revoke_signed(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(mut tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let id = v["id"].as_str().unwrap_or("").to_string();
        let Some(issued) = v["issued"].as_u64() else {
            return json_reply("400 Bad Request", json!({ "error": "issued is required" }));
        };
        let auth = match level1::auth_from_json(&v["auth"]) {
            Ok(a) => a,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e })),
        };
        let now = now_secs();
        if issued > now + 60 || now.saturating_sub(issued) > 600 {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "the challenge is stale; ask for a new one" }),
            );
        }
        let ch = level1::token_revoke_challenge(c.chain_id, policy, &id, issued);
        if !self.notify_used.lock().await.insert(ch) {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "that challenge was already used" }),
            );
        }
        let Ok(owner) = Address::from_str(&tenant.owner) else {
            return json_reply(
                "500 Internal Server Error",
                json!({ "error": "tenant owner is not an address" }),
            );
        };
        let (_threshold, keys) = match owner_keys(c, owner).await {
            Ok(k) => k,
            Err(e) => {
                return json_reply(
                    "502 Bad Gateway",
                    json!({ "error": format!("reading the owner's key: {e}") }),
                );
            }
        };
        if let Err(e) = verify_any_owner(&keys, ch, &auth) {
            return json_reply("403 Forbidden", json!({ "error": e }));
        }
        let before = tenant.agent_tokens.len();
        tenant.agent_tokens.retain(|t| t.id != id);
        if tenant.agent_tokens.len() == before {
            return json_reply("404 Not Found", json!({ "error": "no such token" }));
        }
        if let Err(e) = level1::save(&self.dir, &tenant) {
            return json_reply("500 Internal Server Error", json!({ "error": e }));
        }
        json_reply(
            "200 OK",
            json!({ "revoked": id, "remaining": tenant.agent_tokens.len(), "gated": !tenant.agent_tokens.is_empty() }),
        )
    }

    /// The gate (PLAN 5.31): for a route that needs a scope, on a tenant
    /// whose owner minted at least one agent key, the bearer must carry it.
    /// `None` = proceed.
    async fn token_gate(
        &self,
        method: &str,
        path: &str,
        query: &str,
        raw: &str,
        body: &str,
    ) -> Option<Vec<u8>> {
        // The event stream is read like the inbox (same "read" scope).
        let scope = level1::scope_for(method, path)
            .or((method == "GET" && path == "/api/level1/events").then_some("read"))
            .or((method == "POST" && path == "/api/level1/identity").then_some("run"))?;
        let in_body = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v["policy"].as_str().map(|p| p.trim().to_lowercase()));
        let in_query = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("policy="))
            .map(|p| p.trim().to_lowercase());
        // The gate must judge the account the handler will serve: a GET reads
        // the query, a POST the body. Two different accounts in one request are
        // refused (audit 2026-10-15: a GET carrying a body that named an open
        // account read a locked account's inbox without a key).
        let named = match (in_body, in_query) {
            (Some(b), Some(q)) if b != q => {
                return Some(json_reply(
                    "400 Bad Request",
                    json!({ "error": "the request names two different accounts" }),
                ));
            }
            (b, q) if method == "GET" => q.or(b),
            (b, q) => b.or(q),
        };
        // A gated route with a policy that is not an address is refused here,
        // not waved through to a handler that might resolve it (docs/60 P-45).
        let policy = match named.as_deref().map(|p| Address::from_str(p.trim())) {
            Some(Ok(a)) => a,
            Some(Err(_)) => {
                return Some(json_reply(
                    "400 Bad Request",
                    json!({ "error": "policy must be the account's address" }),
                ));
            }
            None => return None,
        };
        // A tenant file that exists but cannot be read refuses, never opens
        // (docs/60 P-221); no file at all is the handler's 404.
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            if level1::tenant_file_exists(&self.dir, &format!("{policy:#x}")) {
                return Some(json_reply(
                    "503 Service Unavailable",
                    refused(
                        "service_unavailable",
                        "this account could not be read just now; nothing was done, try again",
                    ),
                ));
            }
            return None;
        };
        if tenant.agent_tokens.is_empty() && scope != "buy" {
            return None;
        }
        // The owner's own device may read what the account's agent key may read (docs/103 S4-1): the desk's routes already
        // open to it (`desk_owner`), and an account is not locked against its owner. And it may `prepare` (docs/60 P-170: the
        // first agent key locked the owner out of every change on their own page): a prepare only builds calldata that
        // the owner's passkey must still sign, so it moves no authority. A run, an import or a buy still needs a key.
        // And the owner's own check, bills and invoice decisions (docs/60 P-225: with an agent key, "Check now", "Save bill"
        // and accept/decline answered "requires an agent key"): what the owner does on their own page. Never `buy`: buying is
        // the agent's, inside the caps. The device token comes as a header, or as the `reserveToken` a bills import carries.
        let device = header(raw, "x-tameion-device")
            .map(String::from)
            .or_else(|| {
                serde_json::from_str::<Value>(body)
                    .ok()
                    .and_then(|v| v["reserveToken"].as_str().map(String::from))
            });
        if matches!(scope, "read" | "prepare" | "run" | "import")
            && level1::reserve_token_ok(&tenant, device.as_deref())
        {
            return None;
        }
        let bearer = bearer_of(raw, query);
        match level1::token_allows(&tenant, bearer.as_deref(), scope, now_secs()) {
            Ok(()) => None,
            Err(e) => Some(json_reply(
                if bearer.is_none() {
                    "401 Unauthorized"
                } else {
                    "403 Forbidden"
                },
                operator::refusal::with(
                    operator::refusal::refuse(
                        operator::refusal::token_code(e),
                        operator::refusal::Executed::No,
                        "",
                        "",
                        e,
                        None,
                    ),
                    &[("scope", json!(scope))],
                ),
            )),
        }
    }

    async fn level1_notify_signed(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(mut tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let url = v["url"].as_str().map(str::trim).unwrap_or("");
        let chat = v["telegramChatId"].as_str().map(str::trim).unwrap_or("");
        let Some(issued) = v["issued"].as_u64() else {
            return json_reply("400 Bad Request", json!({ "error": "issued is required" }));
        };
        let auth = match level1::auth_from_json(&v["auth"]) {
            Ok(a) => a,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e })),
        };
        let now = now_secs();
        if issued > now + 60 || now.saturating_sub(issued) > 600 {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "the challenge is stale; ask for a new one" }),
            );
        }
        let ch = level1::notify_challenge(c.chain_id, policy, url, chat, issued);
        if !self.notify_used.lock().await.insert(ch) {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "that challenge was already used" }),
            );
        }
        let Ok(owner) = Address::from_str(&tenant.owner) else {
            return json_reply(
                "500 Internal Server Error",
                json!({ "error": "tenant owner is not an address" }),
            );
        };
        let (threshold, keys) = match owner_keys(c, owner).await {
            Ok(k) => k,
            Err(e) => {
                return json_reply(
                    "502 Bad Gateway",
                    json!({ "error": format!("reading the owner's key: {e}") }),
                );
            }
        };
        if let Err(e) = verify_any_owner(&keys, ch, &auth) {
            return json_reply("403 Forbidden", json!({ "error": e }));
        }
        if let Some(refused) = needs_more_approvals(threshold, "where the agent reports") {
            return refused;
        }
        let minted =
            match level1::apply_notify(&mut tenant, Some(url), Some(Some(chat.to_string())), false)
            {
                Ok(m) => m,
                Err(e) => return json_reply("400 Bad Request", json!({ "error": e })),
            };
        if let Err(e) = level1::save(&self.dir, &tenant) {
            return json_reply("500 Internal Server Error", json!({ "error": e }));
        }
        json_reply(
            "200 OK",
            json!({ "policy": tenant.policy, "notifyUrl": tenant.notify_url, "telegramChatId": tenant.telegram_chat_id, "notifySecret": minted, "signedBy": "the owner's passkey, verified against the key the contract holds", "signature": "X-Tameion-Signature: sha256=hex(HMAC-SHA256(key = the 32 bytes the hex secret encodes, msg = timestamp + '.' + raw body)); X-Tameion-Timestamp carries the timestamp" }),
        )
    }

    /// Import bills (PLAN 5.8). Open to anyone who knows the policy, because
    /// the service keeps only payees the passkey attested on chain; the
    /// worst a stranger can do is distort an amount the owner then sees
    /// under "Needs you" with the agent's reason.
    /// PLAN 5.54: buy one call of a paid API (x402) for the account, inside
    /// its limits. Needs an agent key with the `buy` scope (the gate says so
    /// before this runs); the reason is kept with the purchase.
    async fn level1_buy(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(tenant) = v["policy"]
            .as_str()
            .and_then(|p| level1::load(&self.dir, p))
        else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(url) = v["url"].as_str().filter(|u| !u.trim().is_empty()) else {
            return json_reply("400 Bad Request", json!({ "error": "url is required" }));
        };
        let reason = v["reason"].as_str().unwrap_or("").trim();
        if reason.is_empty() {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "reason is required: every purchase is written down with why it was made" }),
            );
        }
        let max = v["maxUsdc"]
            .as_str()
            .map(String::from)
            .or_else(|| v["maxUsdc"].as_f64().map(|f| f.to_string()))
            .and_then(|m| ledger::Money::parse(m.trim(), ledger::Currency::Usdc).ok())
            .map(|m| m.to_minor())
            .filter(|m| *m > 0);
        let Some(max) = max else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "maxUsdc is required: the most this call may cost, e.g. \"0.01\"" }),
            );
        };
        let body = match &v["body"] {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        };
        let req = operator::x402::BuyRequest {
            url,
            method: v["method"].as_str().unwrap_or("GET"),
            body,
            max_minor: max as u128,
            reason,
        };
        match operator::x402::buy(&self.dir, &tenant, req).await {
            Ok(v) => json_reply("200 OK", v),
            Err((status, v)) => json_reply(status, v),
        }
    }

    /// The newest finished pass's report for `policy`, from the job files.
    async fn last_report(&self, policy: &str) -> Option<Value> {
        let mut best: Option<(u64, Value)> = None;
        let mut rd = tokio::fs::read_dir(self.dir.join("jobs")).await.ok()?;
        while let Ok(Some(e)) = rd.next_entry().await {
            let Ok(text) = tokio::fs::read_to_string(e.path()).await else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if v["request"]["level1"] != "run"
                || !v["request"]["policy"]
                    .as_str()
                    .is_some_and(|p| p.eq_ignore_ascii_case(policy))
                || v["status"] != "done"
            {
                continue;
            }
            let at = v["created"].as_u64().unwrap_or(0);
            if best.as_ref().is_none_or(|(b, _)| at > *b) {
                best = Some((at, v["report"].clone()));
            }
        }
        best.map(|(_, r)| r)
    }

    /// A vendor, or a vendor's agent, sends an invoice signed by the address
    /// that will be paid (docs/65 §5.4, `operator::invoice`). Open: the
    /// signature is the authentication, and receiving pays nothing — the
    /// agent checks it against the account's history and approved payees
    /// and puts it in Needs you with what it found, in words. Twenty an hour
    /// per account; 4 KiB a body.
    async fn level1_invoice(&self, body: &str) -> Vec<u8> {
        if body.len() > 4096 {
            return json_reply(
                "413 Payload Too Large",
                json!({ "error": "an invoice is under 4 KiB" }),
            );
        }
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(tenant) = v["policy"]
            .as_str()
            .and_then(|p| level1::load(&self.dir, p))
        else {
            return json_reply("404 Not Found", json!({ "error": "no such account" }));
        };
        let now = now_secs();
        // The signature first: an unsigned or mis-signed body costs no file
        // read and no chain read (review, P-46).
        let (Some(c), Ok(addr)) = (chain(&tenant.chain), Address::from_str(&tenant.policy)) else {
            return json_reply("404 Not Found", json!({ "error": "no such account" }));
        };
        let inv = match operator::invoice::verify(c.chain_id, addr, &v, now) {
            Ok(i) => i,
            Err(e) => return json_reply("422 Unprocessable Entity", json!({ "error": e })),
        };
        let all = operator::invoice::list(&self.dir, &tenant.policy);
        // One payee cannot fill Needs you: five waiting at most.
        let payee = format!("{:#x}", inv.payee);
        if all
            .iter()
            .filter(|l| l["status"] == "received" && l["payee"].as_str() == Some(payee.as_str()))
            .count()
            >= 5
        {
            return json_reply(
                "429 Too Many Requests",
                operator::refusal::refuse(
                    "rate_limited",
                    operator::refusal::Executed::No,
                    "",
                    "Send it after the owner answers the invoices that wait.",
                    "five invoices from this payee are already waiting for the owner",
                    None,
                ),
            );
        }
        // What history says this payee usually costs: the last pass's bills.
        let known: Vec<(String, ledger::Money)> = self
            .last_report(&tenant.policy)
            .await
            .and_then(|r| r["obligationsList"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|o| {
                let who = o["counterparty"].as_str()?.to_string();
                let m = ledger::Money::parse(
                    o["amount"].as_str()?.trim_end_matches(" USDC").trim(),
                    ledger::Currency::Usdc,
                )
                .ok()?;
                Some((who, m))
            })
            .collect();
        // Twenty an hour from payees the account does not know yet (not among
        // its bills, never accepted before); a known vendor is never blocked by
        // strangers' throwaway keys using up the hour (audit 2026-10-15, P-73).
        let is_known = |p: &str| {
            known.iter().any(|(w, _)| w.eq_ignore_ascii_case(p))
                || all.iter().any(|l| {
                    l["status"] == "accepted"
                        && l["payee"]
                            .as_str()
                            .is_some_and(|q| q.eq_ignore_ascii_case(p))
                })
        };
        if !is_known(&payee) {
            let recent = all
                .iter()
                .filter(|l| l["at"].as_u64().unwrap_or(0) + 3600 > now)
                .filter(|l| l["payee"].as_str().is_none_or(|q| !is_known(q)))
                .count();
            if recent >= 20 {
                return json_reply(
                    "429 Too Many Requests",
                    operator::refusal::refuse(
                        "rate_limited",
                        operator::refusal::Executed::No,
                        "",
                        "Send it again in an hour.",
                        "twenty invoices an hour from new payees per account; try later",
                        None,
                    ),
                );
            }
        }
        match operator::invoice::receive(&self.dir, &tenant, &v, &known, now).await {
            Ok(line) => {
                // The owner hears of it now, not at the next check (PLAN 5.61);
                // no address in the text, and the vendor's reply does not wait.
                let text = format!(
                    "An invoice is waiting: {}{}",
                    operator::push::human(line["amountText"].as_str().unwrap_or("")),
                    if line["everyDays"].as_u64().unwrap_or(0) > 0 {
                        format!(", every {} days", line["everyDays"])
                    } else {
                        String::new()
                    }
                );
                let (dir, policy) = (self.dir.clone(), tenant.policy.clone());
                tokio::spawn(async move { operator::push::tell(&dir, &policy, &text).await });
                json_reply("201 Created", line)
            }
            Err(e) => json_reply("422 Unprocessable Entity", json!({ "error": e })),
        }
    }

    /// The owner's answer to an invoice: `{policy, ref, accept}`. Accepting a
    /// recurring invoice adds it as a bill (the payee must already be
    /// approved on chain — the page asks the passkey for that first, in the
    /// same tap); a one-off one is paid by the owner's tap on its row.
    async fn level1_invoice_decide(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(tenant) = v["policy"]
            .as_str()
            .and_then(|p| level1::load(&self.dir, p))
        else {
            return json_reply("404 Not Found", json!({ "error": "no such account" }));
        };
        let Some(key) = v["id"]
            .as_str()
            .and_then(|r| alloy::primitives::B256::from_str(r).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "id is required" }));
        };
        let accept = v["accept"].as_bool().unwrap_or(false);
        // The owner's decision (audit 2026-10-15, docs/60 P-73): an agent key
        // with `import` decides; otherwise the owner's passkey signs this exact
        // decision — on an open account, and when the owner's device passed the
        // gate (P-225: the device opens the route; the passkey still decides).
        let by_key = !tenant.agent_tokens.is_empty()
            && level1::token_allows(&tenant, bearer_of(raw, "").as_deref(), "import", now_secs())
                .is_ok();
        if !by_key && let Err(refused) = self.owner_signed_decision(&tenant, &v, key, accept).await
        {
            return refused;
        }
        let line = match operator::invoice::list(&self.dir, &tenant.policy)
            .into_iter()
            .find(|l| operator::invoice::line_id(l) == Some(key))
        {
            Some(l) => l,
            None => return json_reply("404 Not Found", json!({ "error": "no such invoice" })),
        };
        // Refused before anything is written: decided already, or a payee the
        // issuer froze (F10) — the same rule `decide` keeps.
        if line["status"] != "received" {
            return json_reply(
                "409 Conflict",
                json!({ "error": "this invoice was answered already" }),
            );
        }
        if accept && line["frozen"] == true {
            return json_reply(
                "422 Unprocessable Entity",
                refused(
                    "payee_frozen",
                    "the stablecoin issuer has frozen this payee; it cannot be paid",
                ),
            );
        }
        // A one-off invoice is "accepted" only once the chain shows the owner
        // paid THIS vendor THIS amount under its id (`SettledByPrincipal`):
        // otherwise anyone could clear it unpaid on an open account (review,
        // P-46), and `executed(id)` alone a stolen agent key can set by paying
        // one unit to anyone under the id (docs/68 F1b).
        if accept && line["everyDays"].as_u64().unwrap_or(0) == 0 {
            let payee = line["payee"]
                .as_str()
                .and_then(|p| Address::from_str(p).ok());
            let units = line["amount"].as_str().and_then(|a| a.parse::<i128>().ok());
            let (Some(payee), Some(units)) = (payee, units) else {
                return json_reply(
                    "422 Unprocessable Entity",
                    json!({ "error": "this invoice has no payee or amount to check against" }),
                );
            };
            match invoice_settled(&tenant, key, payee, units).await {
                Ok(true) => {}
                Ok(false) => {
                    return json_reply(
                        "409 Conflict",
                        json!({ "error": "no payment of this invoice is on chain yet: pay it with your key first — if you just paid, wait a few seconds and accept again (the same invoice can never be paid twice)" }),
                    );
                }
                Err(e) => return json_reply("502 Bad Gateway", json!({ "error": e })),
            }
        }
        let mut added = Value::Null;
        if accept && line["everyDays"].as_u64().unwrap_or(0) > 0 {
            let bill = match operator::invoice::to_bill(&line) {
                Ok(b) => b,
                Err(e) => return json_reply("422 Unprocessable Entity", json!({ "error": e })),
            };
            match level1::set_bills(&self.dir, &tenant, vec![bill]).await {
                Ok(r) if r["bills"].as_array().is_some_and(|b| !b.is_empty()) => added = r,
                Ok(r) => {
                    return json_reply(
                        "422 Unprocessable Entity",
                        json!({ "error": r["refused"][0]["why"].as_str().unwrap_or("refused") }),
                    );
                }
                Err(e) => return json_reply("502 Bad Gateway", json!({ "error": e })),
            }
        }
        match operator::invoice::decide(&self.dir, &tenant.policy, key, accept, now_secs()) {
            Ok(d) => json_reply("200 OK", json!({ "decision": d, "bill": added })),
            Err(e) => json_reply("422 Unprocessable Entity", json!({ "error": e })),
        }
    }

    async fn level1_bills(&self, body: &str, raw: &str) -> Vec<u8> {
        // `TAMEION_INBOX_TIMING=1`: where a bill save's time goes, on stderr (docs/60 P-231).
        let t0 = std::time::Instant::now();
        let timing = std::env::var_os("TAMEION_INBOX_TIMING").is_some();
        let tm = |what: &str| {
            if timing {
                eprintln!("bills: {what} at {} ms", t0.elapsed().as_millis());
            }
        };
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"].as_str() else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, policy) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        // `check: true` asks only whether this caller may import here, before the
        // page spends a passkey tap and a transaction approving the payee (docs/60
        // P-225: "Save bill" signed first and was refused after). The gate already
        // ran; nothing is saved.
        if v["check"] == true {
            return json_reply(
                "200 OK",
                json!({ "ok": true, "summary": "you may tell the agent about bills on this account; nothing was saved" }),
            );
        }
        let Some(items) = v["bills"].as_array() else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "bills must be a list of {payee, amount, everyDays, nextDue?, discountPercent?, earlyDays?}" }),
            );
        };
        let bills: Vec<level1::SeededBill> = items
            .iter()
            .map(|b| level1::SeededBill {
                payee: b["payee"].as_str().unwrap_or("").trim().to_string(),
                amount: b["amount"]
                    .as_str()
                    .map(String::from)
                    .or_else(|| b["amount"].as_f64().map(|f| f.to_string()))
                    .unwrap_or_default(),
                every_days: b["everyDays"].as_u64().unwrap_or(30).min(u32::MAX as u64) as u32,
                next_due: b["nextDue"].as_u64().unwrap_or_else(now_secs),
                // PLAN 5.56: "2% if paid 10 days early" = discountPercent 2,
                // earlyDays 10. Out-of-range terms are refused by set_bills.
                discount_bps: b["discountPercent"]
                    .as_f64()
                    .or_else(|| {
                        b["discountPercent"]
                            .as_str()
                            .and_then(|p| p.trim().trim_end_matches('%').parse().ok())
                    })
                    .filter(|p| p.is_finite() && *p > 0.0)
                    .map(|p| (p * 100.0).round().min(f64::from(u16::MAX)) as u16)
                    .unwrap_or(0),
                early_days: b["earlyDays"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32,
            })
            .collect();
        // docs/60 P-226: on an account with agent keys, an import only the owner's
        // device opened (no `import` agent key) carries the owner's passkey
        // signature over exactly these bills: a device token alone could make or
        // replace bills the agent then pays on Autopilot.
        let by_key =
            level1::token_allows(&tenant, bearer_of(raw, "").as_deref(), "import", now_secs())
                .is_ok();
        if !tenant.agent_tokens.is_empty() && !by_key {
            let ch = chain(&tenant.chain)
                .zip(Address::from_str(&tenant.policy).ok())
                .and_then(|(c, p)| {
                    level1::bills_challenge(
                        c.chain_id,
                        p,
                        &bills,
                        v["issued"].as_u64().unwrap_or(0),
                    )
                });
            let Some(ch) = ch else {
                return json_reply(
                    "400 Bad Request",
                    json!({ "error": "a bill's payee is not an address or its amount is not USDC" }),
                );
            };
            if let Err(refused) = self
                .owner_signed(
                    &tenant,
                    &v,
                    ch,
                    json!({ "error": "saving bills from this device needs the owner's passkey: sign exactly these bills", "challenge": "keccak256(\"tameion.bills.v1\" ‖ chainId u64 ‖ policy ‖ per bill: payee ‖ amount minor u256 ‖ everyDays u32 ‖ nextDue u64 ‖ discountBps u16 ‖ earlyDays u32 ‖ issued u64)" }),
                )
                .await
            {
                return refused;
            }
        }
        tm("signature checked");
        // docs/60 P-227: no import, whoever sends it, moves a bill whose current
        // cycle is paid to a start that re-opens a paid cycle (its cycle keys
        // would change, and a paid period would be asked or paid again).
        let retyped: Vec<(String, u64, u64, u64)> = bills
            .iter()
            .filter_map(|b| {
                let old = tenant
                    .bills
                    .iter()
                    .find(|o| o.payee.eq_ignore_ascii_case(&b.payee))?;
                Some((
                    b.payee.to_lowercase(),
                    old.next_due,
                    u64::from(old.every_days) * 86_400,
                    b.next_due,
                ))
            })
            .collect();
        if tenant.deployed {
            for (payee, typed, every, wanted) in &retyped {
                let next_unpaid = match level1::next_unpaid_typed(
                    &self.dir,
                    &tenant,
                    payee,
                    *typed,
                    *every,
                    now_secs(),
                )
                .await
                {
                    Ok(n) => n,
                    Err(e) => {
                        return json_reply(
                            "503 Service Unavailable",
                            json!({ "error": format!("could not read which cycles of this bill are paid ({e}); nothing was saved, try again") }),
                        );
                    }
                };
                if let Some(w) = reopens_paid_cycle(payee, *typed, Some(next_unpaid), *wanted) {
                    return json_reply(
                        "409 Conflict",
                        json!({ "error": w, "nextUnpaid": next_unpaid }),
                    );
                }
            }
        }
        tm("paid cycles read");
        // F10 (PLAN 5.47): a frozen payee never becomes a bill.
        let addrs: Vec<Address> = bills
            .iter()
            .filter_map(|b| Address::from_str(&b.payee).ok())
            .collect();
        let frozen = level1::issuer_frozen(&tenant, &addrs).await;
        tm("issuer freeze read");
        if !frozen.is_empty() {
            return json_reply(
                "422 Unprocessable Entity",
                operator::refusal::with(
                    refused("payee_frozen", level1::frozen_words(&frozen)),
                    &[("frozen", json!(frozen))],
                ),
            );
        }
        // docs/60 P-77: a reserved account has no contract to attest against
        // yet (`set_bills` would read a policy that is not on chain). Its first
        // bill is kept un-attested in the record, only for the browser that
        // opened the account: it holds the reserve token (body `reserveToken`,
        // docs/60 P-87; qx/qy are public and gate nothing). Nothing is payable
        // from it: the first pass puts the payee in Needs you with the attest
        // tap, and the policy refuses an unattested payee anyway.
        if !tenant.deployed {
            // The device token in the body, or as the header the page now sends (docs/60 P-225).
            if !level1::reserve_token_ok(&tenant, v["reserveToken"].as_str())
                && !level1::reserve_token_ok(&tenant, header(raw, "x-tameion-device"))
            {
                return json_reply(
                    "403 Forbidden",
                    json!({ "error": "this account is not on chain yet; only the device that opened it can add its first bill" }),
                );
            }
            let (mut kept, mut refused) = (Vec::new(), Vec::new());
            for b in bills {
                match reserved_bill_problem(&b) {
                    None => kept.push(level1::SeededBill {
                        payee: format!(
                            "{:#x}",
                            Address::from_str(b.payee.trim()).unwrap_or_default()
                        ),
                        ..b
                    }),
                    Some(why) => refused.push(json!({ "bill": b, "why": why })),
                }
            }
            let mut t = tenant;
            t.bills.retain(|old| {
                !kept
                    .iter()
                    .any(|k| k.payee.eq_ignore_ascii_case(&old.payee))
            });
            t.bills.extend(kept.clone());
            // docs/79 §6a #5: untyped caps follow the bills.
            level1::refresh_provisional_caps(&self.dir, &mut t);
            return match level1::save(&self.dir, &t) {
                Ok(()) => json_reply(
                    "200 OK",
                    json!({ "policy": t.policy, "bills": kept, "refused": refused }),
                ),
                Err(e) => json_reply("500 Internal Server Error", json!({ "error": e })),
            };
        }
        let saved = level1::set_bills(&self.dir, &tenant, bills).await;
        tm("approvals read and saved");
        match saved {
            Ok(r) => json_reply("200 OK", r),
            Err(e) => json_reply("502 Bad Gateway", json!({ "error": e })),
        }
    }

    /// `POST /api/level1/wallets {policy, add?, remove?, reserveToken?}` (docs/79
    /// §6a #4): the wallets whose history is the account's, at most five.
    /// Never open, reserved or deployed (docs/60 P-87, P-93): the device token
    /// minted when the account was opened (body `reserveToken`), or an agent
    /// key with the `import` scope (`Authorization: Bearer`). The first wallet of an account reserved without a
    /// spare key is proposed as one (`spareKey`): it goes on chain only by the
    /// owner's passkey (`prepare-recovery`), never by this call (docs/60 P-88).
    async fn level1_wallets(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(mut t) = v["policy"]
            .as_str()
            .and_then(|p| level1::load(&self.dir, p))
        else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        // An empty key list would make `token_allows` say yes: checked first.
        let agent_ok = !t.agent_tokens.is_empty()
            && header(raw, "authorization")
                .and_then(|h| h.strip_prefix("Bearer "))
                .is_some_and(|b| level1::token_allows(&t, Some(b), "import", now_secs()).is_ok());
        if !(agent_ok || level1::reserve_token_ok(&t, v["reserveToken"].as_str())) {
            return json_reply(
                "403 Forbidden",
                json!({ "error": "only the device that opened this account can change its wallets; on another device, confirm with your passkey first" }),
            );
        }
        let addr = |k: &str| -> Result<Option<Address>, Vec<u8>> {
            match v[k].as_str().map(|x| Address::from_str(x.trim())) {
                None => Ok(None),
                Some(Ok(a)) if !a.is_zero() => Ok(Some(a)),
                _ => Err(json_reply(
                    "400 Bad Request",
                    json!({ "error": format!("{k} must be a wallet address (not 0x0)") }),
                )),
            }
        };
        let (add, remove) = match (addr("add"), addr("remove")) {
            (Ok(a), Ok(r)) => (a, r),
            (Err(e), _) | (_, Err(e)) => return e,
        };
        let has = |t: &level1::Tenant, a: Address| {
            t.wallets
                .iter()
                .any(|w| Address::from_str(w).ok() == Some(a))
        };
        if let Some(r) = remove {
            t.wallets.retain(|w| Address::from_str(w).ok() != Some(r));
            // A disconnected wallet is no longer proposed as the spare key.
            if t.spare_key
                .as_deref()
                .and_then(|k| Address::from_str(k).ok())
                == Some(r)
            {
                t.spare_key = None;
            }
        }
        if let Some(a) = add
            && !has(&t, a)
        {
            if t.wallets.len() >= 5 {
                return json_reply(
                    "400 Bad Request",
                    json!({ "error": "five wallets at most; remove one first", "wallets": t.wallets }),
                );
            }
            t.wallets.push(format!("{a:#x}"));
            let no_recovery = Address::from_str(&t.recovery).is_ok_and(|r| r.is_zero());
            if !t.deployed && no_recovery && !t.recovery_explicit && t.spare_key.is_none() {
                t.spare_key = Some(format!("{a:#x}"));
            }
        }
        level1::refresh_provisional_caps(&self.dir, &mut t);
        if let Err(e) = level1::save(&self.dir, &t) {
            return json_reply("500 Internal Server Error", json!({ "error": e }));
        }
        json_reply(
            "200 OK",
            json!({
                "policy": t.policy,
                "wallets": t.wallets,
                // What the owner contract holds (or will hold when it is created).
                "recovery": t.recovery,
                // Proposed, not on chain: the owner's passkey names it (one tap).
                "spareKey": t.spare_key,
                "spareKeyCall": t.spare_key.as_deref().and_then(|k| Some(level1::prepare_recovery(Address::from_str(&t.owner).ok()?, Address::from_str(k).ok()?))).map(|c| json!({ "to": format!("{:#x}", c.to), "data": c.data, "summary": c.summary })),
                "deployed": t.deployed,
                "caps": (!t.deployed).then(|| json!({ "perAction": t.per_action, "daily": t.daily, "floor": t.floor, "provisional": t.caps_provisional })),
            }),
        )
    }

    /// `POST /api/level1/reserve-token {policy, issued, auth}` (docs/60 P-87,
    /// P-93): a new device token, for a device that holds the passkey but not
    /// the token (another browser, cleared storage, an account opened before
    /// tokens existed). The page computes `level1::reserve_token_challenge`
    /// itself; the assertion must verify against an owner key (the contract's
    /// once deployed, the reserved passkey before), be fresh (ten minutes) and
    /// unused. The previous token stops working.
    async fn level1_reserve_token(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(mut t) = v["policy"]
            .as_str()
            .and_then(|p| level1::load(&self.dir, p))
        else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let (Some(c), Ok(policy), Ok(owner)) = (
            chain(&t.chain),
            Address::from_str(&t.policy),
            Address::from_str(&t.owner),
        ) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let (Some(issued), Ok(auth)) = (v["issued"].as_u64(), level1::auth_from_json(&v["auth"]))
        else {
            return json_reply(
                "401 Unauthorized",
                json!({ "error": "a reserve token needs the account's passkey", "challenge": "keccak256(\"tameion.reserve-token.v1\" ‖ chainId u64 ‖ policy ‖ issued u64)" }),
            );
        };
        let now = now_secs();
        if issued > now + 60 || now.saturating_sub(issued) > 600 {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "the challenge is stale; ask for a new one" }),
            );
        }
        let keys = match tenant_keys(c, &t, owner).await {
            Ok(k) => k,
            Err(e) => {
                return json_reply(
                    "502 Bad Gateway",
                    json!({ "error": format!("reading the owner's key: {e}") }),
                );
            }
        };
        let ch = level1::reserve_token_challenge(c.chain_id, policy, issued);
        if let Err(e) = verify_any_owner(&keys, ch, &auth) {
            return json_reply("403 Forbidden", json!({ "error": e }));
        }
        // Spent only once verified, so nobody can burn the owner's challenge.
        if !self.notify_used.lock().await.insert(ch) {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "that challenge was already used" }),
            );
        }
        let tok = level1::mint_reserve_token(&mut t);
        if let Err(e) = level1::save(&self.dir, &t) {
            return json_reply("500 Internal Server Error", json!({ "error": e }));
        }
        json_reply("200 OK", json!({ "policy": t.policy, "reserveToken": tok }))
    }

    /// Calldata for the owner's passkey to set a standing order (PLAN 5.25):
    /// payee, USDC amount, cadence, first due date. Refused in words when the
    /// payee is not attested yet (the contract would refuse too) or the
    /// cadence is under an hour.
    async fn level1_prepare_schedule(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let addr = |k: &str| v[k].as_str().and_then(|x| Address::from_str(x.trim()).ok());
        let (Some(policy), Some(payee)) = (addr("policy"), addr("payee")) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "policy and payee are required" }),
            );
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let amount = match ledger::Money::parse(
            v["amount"].as_str().unwrap_or(""),
            ledger::Currency::Usdc,
        ) {
            Ok(m) if m.to_minor() > 0 => m,
            _ => {
                return json_reply(
                    "400 Bad Request",
                    json!({ "error": "amount (USDC) is required" }),
                );
            }
        };
        let every_secs = v["everySecs"]
            .as_u64()
            .or_else(|| v["everyDays"].as_f64().map(|d| (d * 86_400.0) as u64))
            .unwrap_or(0);
        if every_secs < 3600 {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "every must be at least one hour (everyDays or everySecs)" }),
            );
        }
        let first_due = v["firstDue"].as_u64().unwrap_or_else(now_secs);
        let attested =
            match operator::watch::chain(&tenant.chain).and_then(|c| level1::read_client(c).ok()) {
                // An unreadable chain does not block preparing; the contract
                // refuses an unattested payee anyway.
                Some(client) => client.is_attested(policy, payee).await.unwrap_or(true),
                None => true,
            };
        if !attested {
            return json_reply(
                "400 Bad Request",
                json!({ "error": format!("{payee:#x} is not an approved payee yet; approve it first (one tap), then set the order") }),
            );
        }
        // The order starts at this payee's bill's next unpaid cycle, never before:
        // `payDue` pays past dates under its own key and the contract refuses
        // nothing (docs/60 P-211). Read as the next pass would read it.
        if tenant.deployed {
            let next_unpaid = match level1::plan(&self.dir, &tenant).await {
                Ok(p) => p["obligations"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|o| {
                        o["cycleKey"]
                            .as_str()
                            .is_some_and(|k| k.starts_with(&format!("{payee:#x}@")))
                    })
                    .and_then(|o| o["nextDue"].as_i64()),
                Err(e) => {
                    return json_reply(
                        "503 Service Unavailable",
                        json!({ "error": format!("could not read which cycles of this bill are paid ({e}); nothing was prepared, try again") }),
                    );
                }
            };
            if let Some(w) = first_due_refusal(first_due, next_unpaid) {
                return json_reply(
                    "409 Conflict",
                    json!({ "error": w, "nextUnpaid": next_unpaid }),
                );
            }
        }
        let days = every_secs as f64 / 86_400.0;
        json_reply(
            "200 OK",
            json!({
                "to": format!("{policy:#x}"),
                "data": alloy::hex::encode_prefixed(arc::add_schedule_calldata(payee, amount, every_secs, first_due)),
                "summary": format!("standing order: {amount} to {payee:#x} every {days:.1} days, first on unix {first_due}; anyone may trigger it on its due date inside your caps; ending it is instant"),
            }),
        )
    }

    /// Calldata for the owner's passkey to name guardians on its own
    /// PasskeyPrincipal (PLAN 5.30): `setGuardians(list, threshold)`.
    fn level1_prepare_guardians(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(owner) = v["owner"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "owner (the passkey contract) is required" }),
            );
        };
        let list: Vec<Address> = v["guardians"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| g.as_str().and_then(|x| Address::from_str(x.trim()).ok()))
            .collect();
        let threshold = v["threshold"].as_u64().unwrap_or(0);
        if list.len() > 16
            || (list.is_empty() && threshold != 0)
            || (!list.is_empty() && (threshold == 0 || threshold as usize > list.len()))
        {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "threshold must be between 1 and the number of guardians (at most 16); an empty list with threshold 0 removes them" }),
            );
        }
        let mut seen = std::collections::BTreeSet::new();
        if !list.iter().all(|a| seen.insert(*a)) {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "a guardian is listed twice" }),
            );
        }
        // Never one of our keys, never the account itself (docs/102 D10, docs/103 S1-4/S1-5): the same
        // check `execute_by_key` makes on every relay, so skipping this route does not skip it.
        let data = arc::set_guardians_calldata(&list, threshold as u8);
        if let Some(e) = level1::guardian_refusal(&self.dir, owner, owner, &data) {
            return json_reply("400 Bad Request", json!({ "error": e }));
        }
        json_reply(
            "200 OK",
            json!({
                "to": format!("{owner:#x}"),
                "data": alloy::hex::encode_prefixed(data),
                "summary": format!("name {} guardian(s), {threshold} of whom may together propose a replacement passkey after the recovery delay; you can cancel until then", list.len()),
            }),
        )
    }

    /// Calldata for the owner's passkey to renew (or shorten) the agent's
    /// authority to pay (PLAN 5.24): `until` unix seconds, or `days` from now.
    fn level1_prepare_renew(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let until = v["until"]
            .as_u64()
            .or_else(|| v["days"].as_u64().map(|d| now_secs() + d * 86_400))
            .unwrap_or_else(|| now_secs() + 90 * 86_400);
        if until <= now_secs() || until > now_secs() + 365 * 86_400 {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "until must be in the future and at most a year ahead" }),
            );
        }
        json_reply(
            "200 OK",
            json!({ "to": format!("{policy:#x}"), "data": alloy::hex::encode_prefixed(arc::renew_agent_calldata(until)), "summary": format!("let the agent keep paying until unix {until}; earlier than today's expiry is your brake") }),
        )
    }

    /// Calldata for the owner's passkey to end a standing order.
    fn level1_prepare_unschedule(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(id) = v["id"].as_u64() else {
            return json_reply("400 Bad Request", json!({ "error": "id is required" }));
        };
        json_reply(
            "200 OK",
            json!({ "to": format!("{policy:#x}"), "data": alloy::hex::encode_prefixed(arc::remove_schedule_calldata(id)), "summary": format!("end standing order #{id}; instant") }),
        )
    }

    /// Pay one due cycle of a standing order with the relayer's gas. Anyone
    /// may: the contract checks the due date, the payee and the caps; the
    /// agent does this on its passes, this route is for a tap on the page.
    async fn level1_pay_due(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(id) = v["id"].as_u64() else {
            return json_reply("400 Bad Request", json!({ "error": "id is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let Some(c) = operator::watch::chain(&tenant.chain) else {
            return json_reply("400 Bad Request", json!({ "error": "unknown chain" }));
        };
        let key = match level1::relayer_key() {
            Ok(k) => k,
            Err(e) => return json_reply("500 Internal Server Error", json!({ "error": e })),
        };
        let client = match arc::ArcClient::with_signer(&operator::watch::rpc_for(c), &key) {
            Ok(c) => c,
            Err(e) => {
                return json_reply(
                    "500 Internal Server Error",
                    json!({ "error": e.to_string() }),
                );
            }
        };
        // Never a bill-cycle some path already paid (docs/60 P-211): the order's
        // oldest date is what payDue pays, under a key no bill payment carries.
        let order = match client.schedules(policy).await {
            Ok(list) => list.into_iter().find(|s| s.id == id),
            Err(e) => {
                return json_reply(
                    "503 Service Unavailable",
                    json!({ "error": format!("could not read the standing order ({e}); nothing was paid") }),
                );
            }
        };
        if let Some(sc) = order {
            let cycle = format!("{:#x}@{}", sc.payee, sc.next_due);
            match level1::cycle_paid(&self.dir, &tenant, &cycle).await {
                Ok(false) => {}
                Ok(true) => {
                    return json_reply(
                        "409 Conflict",
                        json!({ "error": format!("standing order #{id}'s due date {} is a bill-cycle already paid; paying it would pay it twice, so nothing was paid. End the order, then make it standing again from the next unpaid cycle", sc.next_due) }),
                    );
                }
                Err(e) => {
                    return json_reply(
                        "503 Service Unavailable",
                        json!({ "error": format!("could not read whether this cycle is paid ({e}); nothing was paid") }),
                    );
                }
            }
        }
        match client.pay_due(policy, id).await {
            Ok(tx) => json_reply(
                "200 OK",
                json!({ "tx": format!("{tx:#x}"), "explorer": format!("{}/tx/{tx:#x}", c.explorer) }),
            ),
            Err(e) => json_reply("502 Bad Gateway", json!({ "error": e.to_string() })),
        }
    }

    /// The owner's tap on a "Let the agent buy from … at …" row (docs/60
    /// P-217): the next purchase from that seller at up to that price passes the
    /// price band once, within ten minutes. The owner's device only: this lets
    /// the agent pay more than usual, so an agent key cannot grant it.
    fn level1_purchase_approve(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let addr = |k: &str| v[k].as_str().and_then(|x| Address::from_str(x.trim()).ok());
        let (Some(policy), Some(seller)) = (addr("policy"), addr("seller")) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "policy and seller are required" }),
            );
        };
        let Some(amount) = parse_money(&v["amount"]).filter(|m| m.to_minor() > 0) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "amount (USDC) is required" }),
            );
        };
        let Some(tenant) = level1::load(&self.dir, &format!("{policy:#x}")) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        if !level1::reserve_token_ok(&tenant, header(raw, "x-tameion-device")) {
            return json_reply(
                "401 Unauthorized",
                json!({ "error": "only the owner's device can let the agent pay more than usual" }),
            );
        }
        let until = now_secs() + 600;
        operator::x402::approve_price(
            &self.dir,
            &tenant.policy,
            &format!("{seller:#x}"),
            amount.to_minor() as u128,
            until,
        );
        json_reply(
            "200 OK",
            json!({ "seller": format!("{seller:#x}"), "max": amount.to_string(), "until": until,
                "summary": format!("the agent may buy once from {seller:#x} at up to {amount} until unix {until}; your limits still bound it") }),
        )
    }

    fn level1_prepare_withdraw(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let addr = |k: &str| v[k].as_str().and_then(|x| Address::from_str(x.trim()).ok());
        let (Some(policy), Some(to)) = (addr("policy"), addr("to")) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "policy and to are required" }),
            );
        };
        let Some(amount) = parse_money(&v["amount"]) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "amount (USDC) is required" }),
            );
        };
        match level1::prepare_withdraw(policy, to, amount) {
            Ok(call) => json_reply(
                "200 OK",
                json!({ "to": format!("{:#x}", call.to), "data": call.data, "summary": call.summary }),
            ),
            Err(e) => json_reply("400 Bad Request", json!({ "error": e })),
        }
    }

    async fn level1_prepare_mode(&self, body: &str, raw: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"]
            .as_str()
            .and_then(|x| Address::from_str(x.trim()).ok())
        else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(mode) = v["mode"].as_u64().and_then(|m| u8::try_from(m).ok()) else {
            return json_reply(
                "400 Bad Request",
                json!({ "error": "mode must be 0, 1 or 2" }),
            );
        };
        // A keyless account (PLAN 5.38) has no agent to hand autonomy to.
        if mode == 2 && level1::load(&self.dir, &format!("{policy:#x}")).is_some_and(|t| t.keyless)
        {
            return json_reply(
                "409 Conflict",
                refused(
                    "account_keyless",
                    "this account has no agent key, so there is nothing to put on autopilot: you approve every payment (one tap pays all that wait), and standing orders pay themselves",
                ),
            );
        }
        match level1::prepare_mode(policy, mode) {
            Ok(call) => {
                self.prepared(
                    policy,
                    "mode",
                    raw,
                    json!({ "to": format!("{:#x}", call.to), "data": call.data, "summary": call.summary }),
                    json!({ "mode": mode, "reason": v["reason"] }),
                    move |s, delay| {
                        operator::agent_api::mode_apply(
                            operator::agent_api::mode_number(s["mode"].as_str().unwrap_or("")),
                            mode,
                            delay,
                        )
                    },
                )
                .await
            }
            Err(e) => json_reply("400 Bad Request", json!({ "error": e })),
        }
    }

    async fn level1_run(self: &Arc<Self>, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"].as_str() else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, policy) else {
            return json_reply(
                "404 Not Found",
                json!({ "error": "no such tenant on this service" }),
            );
        };
        // Anyone may ask for a run (it moves nothing), but each one costs the
        // tenant's agent an anchor and this host a scan: one per tenant per
        // ten minutes; the latest run answers in between -- unless the owner
        // changed the account after that run finished (docs/66 route 3).
        {
            let changed = self
                .changed_at
                .lock()
                .await
                .get(&tenant.policy.to_lowercase())
                .copied()
                .unwrap_or(0);
            let jobs = self.jobs.lock().await;
            let recent = jobs
                .values()
                .filter(|j| j.request["level1"] == "run" && j.request["policy"] == tenant.policy)
                .filter(|j| j.status != "failed" && j.created + 600 > now_secs())
                // At least a minute between passes whatever changed: each pass
                // anchors with the agent's gas, which the relayer tops up.
                .filter(|j| {
                    !(j.status == "done" && changed > j.created && now_secs() >= j.created + 60)
                })
                .max_by_key(|j| j.created);
            if let Some(j) = recent {
                // docs/60 P-229: a pass from before a bill fell due is no answer
                // after it. Fresh once a minute has passed since it; before that
                // the watcher runs it at the minute (`due_pass_at`).
                match j
                    .report
                    .as_ref()
                    .filter(|_| j.status == "done")
                    .and_then(|r| due_after(r, j.created, now_secs()))
                {
                    Some(_) if now_secs() >= j.created + 60 => {}
                    Some(d) => {
                        let at = (j.created + 60).max(d + 5);
                        return json_reply(
                            "200 OK",
                            json!({ "id": j.id, "status": j.status, "cached": true, "next": at,
                                "note": format!("this check ran before a bill fell due; a fresh one runs at unix {at}") }),
                        );
                    }
                    None => {
                        return json_reply(
                            "200 OK",
                            json!({ "id": j.id, "status": j.status, "cached": true, "note": "a run for this policy started less than ten minutes ago" }),
                        );
                    }
                }
            }
        }
        let Ok(permit) = Arc::clone(&self.slots).try_acquire_owned() else {
            return json_reply(
                "429 Too Many Requests",
                json!({ "error": "all scan slots are busy; try again in a minute" }),
            );
        };
        let id = format!(
            "{:x}-{:04x}",
            now_secs(),
            self.counter.fetch_add(1, Ordering::Relaxed) & 0xffff
        );
        let job = Job {
            id: id.clone(),
            created: now_secs(),
            status: "queued",
            progress: None,
            report: None,
            error: None,
            request: json!({ "level1": "run", "policy": tenant.policy, "chain": tenant.chain, "watched": tenant.watched }),
        };
        self.jobs.lock().await.insert(id.clone(), job.clone());
        self.persist(&job).await;
        self.emit(
            &tenant.policy,
            "pass",
            json!({ "id": id, "status": "queued" }),
        );
        let app = Arc::clone(self);
        let job_id = id.clone();
        tokio::spawn(async move {
            let id = job_id;
            let _permit = permit;
            app.emit(
                &tenant.policy,
                "pass",
                json!({ "id": id, "status": "running" }),
            );
            {
                let mut jobs = app.jobs.lock().await;
                if let Some(j) = jobs.get_mut(&id) {
                    j.status = "running";
                    // No number until the scan reports one: a guess from the
                    // window said "about 78 min" to a client whose pass took
                    // seconds once read right (docs/60 P-38).
                    j.progress = Some(json!({
                        "phase": "reading your bills, then deciding",
                    }));
                }
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
            let app_p = Arc::clone(&app);
            let id_p = id.clone();
            let policy_p = tenant.policy.clone();
            let pump = tokio::spawn(async move {
                while let Some(p) = rx.recv().await {
                    let mut jobs = app_p.jobs.lock().await;
                    if let Some(j) = jobs.get_mut(&id_p) {
                        let elapsed = now_secs().saturating_sub(j.created);
                        let eta = (p.scanned_blocks > 0 && p.total_blocks > p.scanned_blocks).then(
                            || {
                                let rate = elapsed as f64 / p.scanned_blocks as f64;
                                (rate * (p.total_blocks - p.scanned_blocks) as f64).round() as u64
                            },
                        );
                        let eta = eta.map(|e| p.eta_cap_secs.map_or(e, |c| e.min(c)));
                        let progress = json!({
                            "phase": p.phase,
                            "scannedBlocks": p.scanned_blocks,
                            "totalBlocks": p.total_blocks,
                            "transfersSoFar": p.transfers_so_far,
                            "elapsedSecs": elapsed,
                            "etaSecs": eta,
                        });
                        app_p.emit(
                            &policy_p,
                            "pass",
                            json!({ "id": id_p, "status": "running", "progress": progress }),
                        );
                        j.progress = Some(progress);
                    }
                }
            });
            // A pass that cannot finish (an RPC answering slowly forever)
            // ends here; the next schedule retries (docs/54 §3, Rec 18).
            let cap = std::env::var("TAMEION_PASS_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1200u64);
            let result = match tokio::time::timeout(
                std::time::Duration::from_secs(cap),
                level1::run(&app.dir, &tenant, |p| {
                    let _ = tx.send(p);
                }),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => Err(format!(
                    "pass exceeded {cap} s and was stopped; the next pass retries"
                )),
            };
            drop(tx);
            let _ = pump.await;
            let done = {
                let mut jobs = app.jobs.lock().await;
                let j = jobs.get_mut(&id).expect("job exists");
                match result {
                    Ok(report) => {
                        j.status = "done";
                        j.report = Some(report);
                    }
                    Err(e) => {
                        j.status = "failed";
                        j.error = Some(e);
                    }
                }
                j.clone()
            };
            app.persist(&done).await;
            app.record_run(&done).await;
            {
                let r = done.report.as_ref();
                let count = |k: &str| r.and_then(|r| r[k].as_array()).map_or(0, Vec::len);
                app.emit(
                    &tenant.policy,
                    "pass",
                    json!({
                        "id": done.id, "status": done.status, "error": done.error,
                        "paid": count("settled"), "needsYou": count("pendingSignature"),
                        "anchorTx": r.map(|r| r["anchorTx"].clone()),
                    }),
                );
            }
            notify(&tenant, &done).await;
            notify_telegram(&tenant, &done).await;
            // docs/65: the same news on the owner's phone (Web Push), only
            // when it changed since the last one.
            if let Some(text) = operator::push::line(done.status, done.report.as_ref()) {
                operator::push::tell(&app.dir, &tenant.policy, &text).await;
            }
        });
        json_reply("202 Accepted", json!({ "id": id, "status": "queued" }))
    }

    /// The autonomy level as the contract holds it, and the operator's own
    /// actions on it. `status` and `apply` are anyone's (read-only, and
    /// anyone may apply a change whose delay has passed); `pause`, `propose`
    /// and `autopilot` sign with the relayer key and need the admin token:
    /// the principal's path is their passkey through `prepare-mode` +
    /// `execute`, never this route.
    async fn level1_mode(&self, body: &str) -> Vec<u8> {
        let v: Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => return json_reply("400 Bad Request", json!({ "error": e.to_string() })),
        };
        let Some(policy) = v["policy"].as_str() else {
            return json_reply("400 Bad Request", json!({ "error": "policy is required" }));
        };
        let Some(tenant) = level1::load(&self.dir, policy) else {
            return json_reply("404 Not Found", json!({ "error": "no such tenant" }));
        };
        let action = v["action"].as_str().unwrap_or("status");
        if !matches!(action, "status" | "apply") {
            let given = v["token"].as_str().unwrap_or("");
            let ok = matches!(&self.admin_token, Some(t) if !t.is_empty() && t == given);
            if !ok {
                return json_reply(
                    "403 Forbidden",
                    json!({ "error": "the operator's mode actions need the admin token; the principal uses prepare-mode + execute with the passkey" }),
                );
            }
        }
        // docs/60 P-171: the page applies a due raise by itself; never over the
        // owner's own cancel, pause or revoke sent after that raise was asked.
        if action == "apply" && self.shrink_blocks_apply(&tenant).await {
            return json_reply(
                "409 Conflict",
                json!({ "error": "the owner lowered trust after this change was asked (a cancel, a pause or a revoke), so it is not applied automatically; the owner can cancel it or ask again" }),
            );
        }
        match level1::mode_action(&tenant, action).await {
            Ok(r) => json_reply("200 OK", r),
            Err(e) if e.starts_with("unknown action") => {
                json_reply("400 Bad Request", json!({ "error": e }))
            }
            Err(e) => json_reply("502 Bad Gateway", json!({ "error": e })),
        }
    }

    async fn get(&self, id: &str) -> Vec<u8> {
        // Only the shape this service gives its jobs (`<hex>-<4 hex>`): the id
        // is joined into a path, and `../tenants/0x…` read any account's file,
        // webhook secret included (audit 2026-10-15, docs/60 P-75).
        if !is_job_id(id) {
            return json_reply("404 Not Found", json!({ "error": "no such run" }));
        }
        if let Some(j) = self.jobs.lock().await.get(id) {
            return json_reply("200 OK", j.json());
        }
        // A job from before a restart: read it back from disk.
        let p = self.dir.join("jobs").join(format!("{id}.json"));
        match tokio::fs::read_to_string(&p).await {
            Ok(s) => respond("200 OK", "application/json", s.as_bytes()),
            Err(_) => json_reply("404 Not Found", json!({ "error": "no such run" })),
        }
    }

    async fn runs(&self, query: &str) -> Vec<u8> {
        let given = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("token="))
            .unwrap_or("");
        match &self.admin_token {
            Some(t) if !t.is_empty() && t == given => {}
            _ => return json_reply("403 Forbidden", json!({ "error": "admin token" })),
        }
        let text = tokio::fs::read_to_string(self.dir.join("runs.jsonl"))
            .await
            .unwrap_or_default();
        let runs: Vec<Value> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        json_reply("200 OK", json!({ "runs": runs }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(7880);
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let dir =
        PathBuf::from(std::env::var("TAMEION_APP_DIR").unwrap_or_else(|_| "runtime/app".into()));
    operator::desk::set_app_dir(&dir);
    let contact = std::env::var("TAMEION_CONTACT_URL")
        .unwrap_or_else(|_| "https://discord.gg/rsVfYutFZg".into());
    let repo = std::env::var("TAMEION_REPO_URL")
        .unwrap_or_else(|_| "https://github.com/lingsiewwin/Tameion-Hack".into());
    let page = include_str!("../../../../apps/classic/index.html")
        .replace("{{CONTACT_URL}}", &contact)
        .replace("{{REPO_URL}}", &repo);
    // A public host without an invite word would let anyone spend the
    // relayer's gas on deploys: give it one and print it, rather than open.
    let mut invite = std::env::var("TAMEION_LEVEL1_INVITE")
        .ok()
        .filter(|w| !w.is_empty());
    if invite.is_none() && host != "127.0.0.1" && host != "localhost" && host != "::1" {
        // Random, not derived from the start time and pid (which could be
        // guessed: audit 2026-10-15, docs/60 P-75).
        let word = level1::new_secret()[..16].to_string();
        println!(
            "TAMEION_LEVEL1_INVITE is unset and HOST is {host}: Level 1 deploys on this host need the invite word {word} (set the variable to choose your own)"
        );
        invite = Some(word);
    }
    // A pass that was running when the previous process died is not running
    // now: say so in its file, or the page shows "checking" for ever and the
    // scheduler thinks a pass is in flight.
    if let Ok(rd) = std::fs::read_dir(dir.join("jobs")) {
        for e in rd.flatten() {
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            let Ok(mut v) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if matches!(v["status"].as_str(), Some("running") | Some("queued")) {
                v["status"] = json!("failed");
                v["error"] = json!("the service restarted during this pass; run it again");
                let _ = std::fs::write(e.path(), serde_json::to_vec_pretty(&v).unwrap_or_default());
            }
        }
    }
    let app = Arc::new(App {
        notify_used: Mutex::new(Default::default()),
        idempotent: Mutex::new(Default::default()),
        exposure_cache: Mutex::new(Default::default()),
        due_changes: Mutex::new(Default::default()),
        changed_at: Mutex::new(Default::default()),
        previews: Mutex::new(std::collections::HashSet::new()),
        names: Mutex::new(()),
        events: tokio::sync::broadcast::channel(256).0,
        dir: dir.clone(),
        jobs: Mutex::new(HashMap::new()),
        slots: Arc::new(Semaphore::new(3)),
        scan_slots: Arc::new(Semaphore::new(2)),
        identity_busy: Mutex::new(Default::default()),
        counter: AtomicU64::new(0),
        admin_token: std::env::var("TAMEION_APP_ADMIN_TOKEN").ok(),
        page,
        home_page: include_str!("../../../../apps/classic/home.html")
            .replace("{{CONTACT_URL}}", &contact)
            .replace("{{REPO_URL}}", &repo)
            // PLAN 5.41 (docs/62 §4.2): the price, where the buyer decides.
            // The number is the founder's (TAMEION_PRICE_MONTHLY, in USD);
            // until it is set the page says what is true today.
            .replace("{{PRICE_LINE}}", &price_line())
            // The page's illustrations (docs/61), inline so the page's
            // colour tokens and dark mode apply to them.
            .replace(
                "{{HERO_SVG}}",
                include_str!("../../../../assets/illustrations/hero.svg"),
            )
            .replace(
                "{{TRUST_SVG}}",
                include_str!("../../../../assets/illustrations/trust.svg"),
            )
            .replace(
                "{{LAZY_SVG}}",
                include_str!("../../../../assets/illustrations/lazy.svg"),
            ),
        level1_page: include_str!("../../../../apps/classic/level1.html")
            .replace("{{CONTACT_URL}}", &contact)
            .replace("{{REPO_URL}}", &repo),
        invite,
    });
    tokio::fs::create_dir_all(dir.join("jobs")).await?;
    let listener = TcpListener::bind((host.as_str(), port)).await?;
    // The tenant scheduler: every `TAMEION_LEVEL1_EVERY_SECS` (0 or unset =
    // off), one pass per tenant through the same job path a button press
    // takes, so each run is recorded and rate limited the same way.
    // ponytail: one process, one interval for every tenant; per-tenant
    // schedules when a tenant asks for one.
    let every: u64 = std::env::var("TAMEION_LEVEL1_EVERY_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if every > 0 {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(every)).await;
                for t in level1::list(&app.dir) {
                    let body = json!({ "policy": t.policy }).to_string();
                    let reply = app.level1_run(&body).await;
                    let line = String::from_utf8_lossy(&reply);
                    let status = line.split_whitespace().nth(1).unwrap_or("?");
                    println!("scheduler: run for {} → {status}", t.policy);
                }
            }
        });
        println!("scheduler: one pass per tenant every {every} s");
    }
    // The desk issues its lender's statement of the whole stock by itself each
    // day at TAMEION_DESK_STATEMENT_UTC_HOUR (0 = 08:00 in Malaysia; unset =
    // off): the surface rule — it is not a trust decision, so the owner only
    // forwards it. One anchor a day, for the desk that trades on this Mac.
    if let Some(hour) = std::env::var("TAMEION_DESK_STATEMENT_UTC_HOUR")
        .ok()
        .and_then(|h| h.parse::<u64>().ok())
        .filter(|h| *h < 24)
    {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            let mut wait = 30;
            // One line per reason per day (docs/96 V8): a failure that repeats every ten minutes is said once.
            let mut said: Option<(u64, String)> = None;
            let mut sent_today: Option<u64> = None;
            let mut sold = false;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(wait)) => {}
                    _ = desk_sold().notified() => sold = true,
                }
                wait = 600;
                let reissue = std::mem::take(&mut sold);
                let now = now_secs();
                let due = now - now % 86_400 + hour * 3600;
                if now < due || sent_today == Some(due) {
                    continue;
                }
                let Some(policy) = std::env::var("TAMEION_DESK_POLICY").ok().or_else(|| {
                    level1::list(&app.dir)
                        .into_iter()
                        .map(|t| t.policy)
                        .find(|p| operator::desk::venue_account(p).is_some())
                }) else {
                    continue;
                };
                let Some(t) = level1::load(&app.dir, &policy) else {
                    continue;
                };
                let dir =
                    operator::desk::book_path(&app.dir, &t.policy).with_file_name("statements");
                let done_today = std::fs::read_dir(&dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .any(|e| {
                        let n = e.file_name().to_string_lossy().to_string();
                        n.strip_prefix("book-")
                            .and_then(|x| x.strip_suffix(".json"))
                            .and_then(|x| x.parse::<u64>().ok())
                            .is_some_and(|at| at >= due)
                    });
                // The morning run issues when today's file is missing; a sale issues again only when it exists.
                if done_today != reissue {
                    continue;
                }
                match app.statements_all(&t).await {
                    Ok(file) => {
                        let n = file["kits"].as_array().map_or(0, Vec::len);
                        println!(
                            "desk: the {} statement of {} ({n} deals) is ready",
                            if reissue { "after-sale" } else { "morning" },
                            t.policy
                        );
                        let text = if reissue {
                            format!(
                                "A deal was sold: the statement of your whole stock was issued again ({n} deals). Send your lender the new one; the earlier one now fails their check."
                            )
                        } else {
                            format!(
                                "Today's statement of your whole stock is ready for your lender: {n} deals, signed and anchored. Open the desk to send it."
                            )
                        };
                        let (d, p) = (app.dir.clone(), t.policy.clone());
                        tokio::spawn(async move { operator::push::tell(&d, &p, &text).await });
                    }
                    // Nothing open or no exchange: nothing to finance, nothing to say.
                    Err(("409 Conflict", _)) => {}
                    Err((_, e)) => {
                        // A transaction hash makes each text differ: compare without the hex.
                        let why: String = e
                            .split("0x")
                            .map(|p| p.trim_start_matches(|c: char| c.is_ascii_hexdigit()))
                            .collect();
                        if said.as_ref() != Some(&(due, why.clone())) {
                            println!("desk: no morning statement for {}: {e}", t.policy);
                            // Once per reason per day, the owner hears it too (docs/101 Y8).
                            if level1::sent_unread(&e).is_none() {
                                let text = format!(
                                    "Today's statement of your whole stock could not be issued: {e}"
                                );
                                let (d, p) = (app.dir.clone(), t.policy.clone());
                                tokio::spawn(
                                    async move { operator::push::tell(&d, &p, &text).await },
                                );
                            }
                            said = Some((due, why));
                        }
                        // An anchor that was sent may have landed: no second one today (docs/99 W9), and the
                        // owner hears why there is no file this morning (docs/100 X10).
                        if let Some(tx) = level1::sent_unread(&e) {
                            sent_today = Some(due);
                            let text = format!(
                                "Today's statement of your whole stock is not ready: its anchor {tx} was sent but not confirmed. Look it up on the explorer before issuing it from the desk: issuing anchors the book again."
                            );
                            let (d, p) = (app.dir.clone(), t.policy.clone());
                            tokio::spawn(async move { operator::push::tell(&d, &p, &text).await });
                        }
                    }
                }
            }
        });
        println!("desk: the whole-stock statement is issued daily at {hour}:00 UTC");
    }
    // PLAN 5.6: passes on new transfers, not only on a clock. Every
    // `TAMEION_LEVEL1_WATCH_SECS` (0 or unset = off) the watcher asks each
    // chain which tenant wallets or policies moved USDC since its last tick
    // (two eth_getLogs per chain, the new blocks only) and starts a pass for
    // each one touched, through the same rate-limited job path a button uses
    // (a pass in the last ten minutes answers instead of starting another).
    let watch_every: u64 = std::env::var("TAMEION_LEVEL1_WATCH_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if watch_every > 0 {
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            let mut last: HashMap<String, u64> = HashMap::new();
            // docs/60 P-229: the pass each account gets when a known bill falls
            // due, by policy (the time it was set for, so it is set once).
            let mut due_set: HashMap<String, u64> = HashMap::new();
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(watch_every)).await;
                // docs/60 P-229: a pass a few seconds after a known bill falls due,
                // not only when USDC moves — through the same job path a button
                // uses (it keeps the minute between passes).
                for t in level1::list(&app.dir).into_iter().filter(|t| t.deployed) {
                    let Some((ran, report)) = app.last_done_pass(&t.policy).await else {
                        continue;
                    };
                    let Some(at) = due_pass_at(&report, ran) else {
                        continue;
                    };
                    if at > now_secs() + watch_every || due_set.get(&t.policy) == Some(&at) {
                        continue;
                    }
                    due_set.insert(t.policy.clone(), at);
                    let (app, policy) = (Arc::clone(&app), t.policy.clone());
                    tokio::spawn(async move {
                        let wait = at.saturating_sub(now_secs());
                        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                        let reply = app
                            .level1_run(&json!({ "policy": policy }).to_string())
                            .await;
                        let line = String::from_utf8_lossy(&reply);
                        println!(
                            "watcher: a bill of {policy} fell due → pass {}",
                            line.split_whitespace().nth(1).unwrap_or("?")
                        );
                    });
                }
                // A signed raise whose delay has passed is applied here
                // (docs/66 route 2, P-49). Anyone may apply; a second
                // application reverts `NothingPending`, harmlessly. Forgotten
                // either way: an inbox read relearns one still pending.
                let due: Vec<String> = {
                    let now = now_secs();
                    let m = app.due_changes.lock().await;
                    m.iter()
                        .filter(|(_, eta)| **eta <= now)
                        .map(|(p, _)| p.clone())
                        .collect()
                };
                for p in due {
                    app.due_changes.lock().await.remove(&p);
                    let Some(t) = level1::load(&app.dir, &p) else {
                        continue;
                    };
                    // docs/60 P-171: not over the owner's cancel, pause or revoke.
                    if app.shrink_blocks_apply(&t).await {
                        println!(
                            "watcher: not applying {p}'s waiting change: the owner lowered trust after it was asked"
                        );
                        continue;
                    }
                    match level1::mode_action(&t, "apply").await {
                        Ok(r) => {
                            println!("watcher: applied the waiting change of {p} in {}", r["tx"]);
                            app.emit(&p, "inbox", json!({ "cause": "change applied" }));
                        }
                        Err(e) => println!("watcher: applying the waiting change of {p}: {e}"),
                    }
                }
                // Reserved accounts too (PLAN 5.32): money sent to the reserved
                // address is what should put the account on chain, whether or
                // not the owner's page is open. A wallet account waits for the
                // wallet's own transaction instead.
                let tenants: Vec<level1::Tenant> = level1::list(&app.dir)
                    .into_iter()
                    .filter(|t| t.deployed || t.kind != "wallet")
                    .collect();
                let mut chains: Vec<String> = tenants.iter().map(|t| t.chain.clone()).collect();
                chains.sort();
                chains.dedup();
                for key in chains {
                    let Some(c) = operator::watch::chain(&key) else {
                        continue;
                    };
                    let Ok(client) = level1::read_client(c) else {
                        continue;
                    };
                    let Ok(head) = client.block_number().await else {
                        continue;
                    };
                    // First tick: start from now, never backfill (the passes do that).
                    let from = last
                        .get(&key)
                        .map(|b| b + 1)
                        .unwrap_or(head)
                        .max(head.saturating_sub(2_000));
                    let mine: Vec<&level1::Tenant> =
                        tenants.iter().filter(|t| t.chain == key).collect();
                    // An account's wallets and its own address, never 0x0: a
                    // name-first account's `watched` would match every mint.
                    let watched_by = |t: &level1::Tenant| -> Vec<Address> {
                        let mut a = level1::history_sources(t);
                        a.extend(Address::from_str(&t.policy).ok().filter(|p| !p.is_zero()));
                        a
                    };
                    let addrs: Vec<Address> = mine.iter().flat_map(|t| watched_by(t)).collect();
                    let touched = match client.touched_among(&addrs, from, head).await {
                        Ok(t) => t,
                        Err(e) => {
                            println!("watcher: {key} blocks {from}–{head}: {e}");
                            continue;
                        }
                    };
                    // A replacement key proposed for one of these accounts (docs/103 S1-2): the
                    // delay is the owner's only window to cancel, so the owner hears it now, page
                    // open or not. Read before `last` moves, so a failed read is read again.
                    let owners: Vec<Address> = mine
                        .iter()
                        .filter(|t| t.deployed)
                        .filter_map(|t| Address::from_str(&t.owner).ok())
                        .collect();
                    match client.recoveries_proposed(&owners, from, head).await {
                        Ok(found) => {
                            for (owner, eta, by) in found {
                                let text = level1::key_replacement_text(eta, by, now_secs());
                                for t in mine
                                    .iter()
                                    .filter(|t| Address::from_str(&t.owner).ok() == Some(owner))
                                {
                                    println!(
                                        "watcher: a new key is waiting for {} (eta {eta}, proposed by {by:#x}); telling the owner",
                                        t.policy
                                    );
                                    app.emit(
                                        &t.policy,
                                        "keys",
                                        json!({ "cause": "replacement pending", "eta": eta }),
                                    );
                                    let (d, p, text) =
                                        (app.dir.clone(), t.policy.clone(), text.clone());
                                    tokio::spawn(async move {
                                        operator::push::tell(&d, &p, &text).await
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            println!(
                                "watcher: {key} replacement keys in blocks {from}–{head}: {e}"
                            );
                            continue;
                        }
                    }
                    last.insert(key.clone(), head);
                    // ponytail: at most 3 watcher passes per chain per tick, so
                    // a wallet many accounts watch cannot start a pass storm on
                    // a public node; the rest wait for the clock scheduler.
                    let hits: Vec<&level1::Tenant> = mine
                        .into_iter()
                        .filter(|t| watched_by(t).iter().any(|a| touched.contains(a)))
                        .collect();
                    if hits.len() > 3 {
                        println!(
                            "watcher: {} accounts touched; 3 now, the rest on the clock",
                            hits.len()
                        );
                    }
                    for t in hits.into_iter().take(3) {
                        {
                            let reply = app
                                .level1_run(&json!({ "policy": t.policy }).to_string())
                                .await;
                            let line = String::from_utf8_lossy(&reply);
                            let status = line.split_whitespace().nth(1).unwrap_or("?");
                            println!(
                                "watcher: USDC moved for {} in blocks {from}–{head} → pass {status}",
                                t.policy
                            );
                        }
                    }
                }
            }
        });
        println!("watcher: a pass on new transfers, checked every {watch_every} s");
    }
    println!(
        "Tameion Watch → http://{host}:{port}  (runs in {})",
        dir.display()
    );
    for c in CHAINS {
        println!(
            "  {:<12} {}  rpc {}",
            c.key,
            c.name,
            std::env::var(c.rpc_env).unwrap_or_else(|_| format!(
                "{} (public; set {} for a keyed one)",
                c.default_rpc, c.rpc_env
            ))
        );
    }

    loop {
        let (mut sock, _) = listener.accept().await?;
        let app = Arc::clone(&app);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            let n = match sock.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => return,
            };
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let line = req.lines().next().unwrap_or("");
            let mut parts = line.split_whitespace();
            // HEAD is GET without the body (RFC 9110 §9.3.2): monitors and
            // browsers ask it, and every route answered 404 (seen by Lighthouse).
            let head_only = parts.clone().next() == Some("HEAD");
            let method = match parts.next().unwrap_or("") {
                "HEAD" => "GET",
                m => m,
            };
            let target = parts.next().unwrap_or("/");
            let (path, query) = target.split_once('?').unwrap_or((target, ""));
            let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();

            if method == "GET" && path == "/api/level1/events" {
                match app.token_gate(method, path, query, &req, &body).await {
                    Some(denied) => {
                        let _ = sock.write_all(&shaped(denied, method, path, &req)).await;
                    }
                    // HEAD: the stream's headers, never the stream (review 2026-10-15).
                    None if head_only => {
                        let _ = sock
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n")
                            .await;
                    }
                    None => serve_events(Arc::clone(&app), &mut sock, query).await,
                }
                return;
            }
            // S3 (docs/106): the agent's live feed, one event a second, the owner's only.
            // `EventSource` cannot send headers, so the device token may come as `device=`,
            // on this Mac only: a public copy's URLs land in proxy logs (docs/103 S3-7).
            if method == "GET" && path == "/api/desk/live" {
                let q = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(k));
                let on_this_mac = header(&req, "host").is_some_and(|h| {
                    ["localhost", "127.0.0.1", "[::1]"]
                        .iter()
                        .any(|l| h == *l || h.starts_with(&format!("{l}:")))
                });
                let device = json!({ "reserveToken": q("device=").filter(|_| on_this_mac) });
                let denied = match app.desk_tenant(q("policy=")) {
                    Err(r) => Some(r),
                    Ok(t) if !app.desk_owner(&t, &req, Some(&device), "read") => Some(json_reply(
                        "403 Forbidden",
                        json!({ "error": "the desk is the owner's: open it on the device that opened the account, or with an agent key that can read" }),
                    )),
                    Ok(_) if head_only => Some(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n".to_vec()),
                    Ok(t) => {
                        // Asked again every minute: a token minted again or a key withdrawn ends the stream.
                        let still_owner = || {
                            app.desk_tenant(Some(&t.policy))
                                .is_ok_and(|t| app.desk_owner(&t, &req, Some(&device), "read"))
                        };
                        operator::desk_live::stream(&mut sock, &app.dir, &t.policy, still_owner)
                            .await;
                        None
                    }
                };
                if let Some(r) = denied {
                    let _ = sock.write_all(&r).await;
                }
                return;
            }
            let idem_key = (method == "POST")
                .then(|| header(&req, "idempotency-key").map(str::to_string))
                .flatten();
            let replay = match &idem_key {
                Some(k) => app.idem_check(path, k, &body).await,
                None => None,
            };
            let first = replay.is_none();
            let reply = if let Some(r) = replay {
                r
            } else if let Some(denied) = app.token_gate(method, path, query, &req, &body).await {
                denied
            } else {
                match (method, path) {
                    // The console when it is built; the classic page until then
                    // and always at /classic.
                    ("GET", "/") | ("GET", "/index.html") | ("GET", "/home") => {
                        match console_response("/").await {
                            Some(r) => r,
                            None => respond(
                                "200 OK",
                                "text/html; charset=utf-8",
                                app.home_page.as_bytes(),
                            ),
                        }
                    }
                    ("GET", "/classic") | ("GET", "/classic/") => respond(
                        "200 OK",
                        "text/html; charset=utf-8",
                        app.home_page.as_bytes(),
                    ),
                    ("GET", "/classic/watch") => {
                        respond("200 OK", "text/html; charset=utf-8", app.page.as_bytes())
                    }
                    ("GET", "/classic/level1") => respond(
                        "200 OK",
                        "text/html; charset=utf-8",
                        app.level1_page.as_bytes(),
                    ),
                    // The console's report when it is built (like `/`); the first page's
                    // version, with its caps and days options, stays at /classic/watch.
                    ("GET", "/watch") | ("GET", "/watch.html") => match console_response("/watch")
                        .await
                    {
                        Some(r) => r,
                        None => respond("200 OK", "text/html; charset=utf-8", app.page.as_bytes()),
                    },
                    ("GET", p) if p.starts_with("/api/level1/inbox") => {
                        app.level1_inbox(query, &req).await
                    }
                    ("GET", "/api/level1/purchases.jsonl") => app.level1_purchases(query),
                    ("GET", "/healthz") => respond("200 OK", "text/plain", b"ok"),
                    ("GET", "/favicon.ico") => respond("204 No Content", "image/x-icon", b""),
                    // The account page installs like an app on a phone (manifest + icon).
                    ("GET", "/manifest.webmanifest") => respond(
                        "200 OK",
                        "application/manifest+json",
                        include_str!("../../../../apps/classic/manifest.webmanifest").as_bytes(),
                    ),
                    // Shows the agent's pushes; served from the root so its
                    // scope is the whole page (no Service-Worker-Allowed needed).
                    ("GET", "/sw.js") => respond(
                        "200 OK",
                        "text/javascript",
                        include_str!("../../../../apps/classic/sw.js").as_bytes(),
                    ),
                    ("GET", "/icon.svg") => respond(
                        "200 OK",
                        "image/svg+xml",
                        include_str!("../../../../apps/classic/icon.svg").as_bytes(),
                    ),
                    // PLAN 5.42: the agent-readable map of this service.
                    ("GET", "/llms.txt") => respond(
                        "200 OK",
                        "text/plain; charset=utf-8",
                        include_str!("../../../../apps/classic/llms.txt").as_bytes(),
                    ),
                    // docs/115 §2: the refusal codes, the retry rules and what changed.
                    ("GET", "/api/level1/codes") => {
                        json_reply("200 OK", operator::refusal::catalogue())
                    }
                    ("GET", "/api/level1/status") => agent_status(&app.dir, query).await,
                    ("GET", "/api/openapi.json") => respond(
                        "200 OK",
                        "application/json",
                        include_str!("../../../../apps/classic/openapi.json").as_bytes(),
                    ),
                    ("GET", "/api/chains") => json_reply(
                        "200 OK",
                        json!({ "chains": CHAINS.iter().map(|c| json!({ "key": c.key, "name": c.name, "chainId": c.chain_id, "explorer": c.explorer, "usdc": c.usdc, "dust": c.dust, "keyed": operator::watch::keyed(c), "note": if operator::watch::keyed(c) { "keyed RPC" } else if c.needs_key { "public RPC refuses log scans; needs a key" } else { "public RPC; slow" } })).collect::<Vec<_>>() }),
                    ),
                    ("POST", "/api/watch") => app.start(&body).await,
                    // The clear-signing routine the Level 1 page uses to describe
                    // calldata and recompute challenges itself (docs/46 #5: the
                    // same file whose hash scripts/verify.sh prints).
                    ("GET", "/level1/call.js") => respond(
                        "200 OK",
                        "application/javascript; charset=utf-8",
                        include_str!("../../../../apps/classic/passkey/call.js").as_bytes(),
                    ),
                    ("GET", "/level1") | ("GET", "/level1.html") => respond(
                        "200 OK",
                        "text/html; charset=utf-8",
                        app.level1_page.as_bytes(),
                    ),
                    ("GET", "/api/level1/config") => json_reply(
                        "200 OK",
                        json!({
                            "delaySecs": level1::delay_secs(),
                            "agentId": level1::agent_id().to_string(),
                            "operator": level1::relayer_address().map(|a| format!("{a:#x}")).ok(),
                            // The key that signs weekly statements and nothing else (docs/76 §1d).
                            "statementSigner": operator::statement::signer_address().map(|a| format!("{a:#x}")).ok(),
                            "inviteNeeded": app.invite.as_ref().is_some_and(|w| !w.is_empty()),
                            // Where pages other people open live (the lender's check, a counterparty's signing page): a public
                            // copy of this console; unset, the desk says those links only work on this Mac (docs/95 P3).
                            "publicUrl": std::env::var("TAMEION_PUBLIC_URL").ok(),
                            "chains": CHAINS.iter().map(|c| json!({ "key": c.key, "name": c.name, "chainId": c.chain_id, "explorer": c.explorer, "anchor": level1::anchor_for(c).map(|a| format!("{a:#x}")).ok() })).collect::<Vec<_>>(),
                            // PLAN 5.33: where a browser wallet can deposit into Circle Gateway from.
                            "gatewaySources": operator::watch::gateway_sources(true),
                        }),
                    ),
                    ("POST", "/api/level1/deploy") => app.level1_deploy(&body, query).await,
                    ("POST", "/api/level1/challenge") => app.level1_challenge(&body).await,
                    ("POST", "/api/level1/execute") => {
                        app.changed_by_owner(&body, app.level1_execute(&body).await)
                            .await
                    }
                    ("POST", "/api/level1/join-request") => app.level1_join_request(&body).await,
                    ("POST", "/api/level1/prepare-coowners") => {
                        app.level1_prepare_coowners(&body).await
                    }
                    ("POST", "/api/level1/prepare-payees") => {
                        app.level1_prepare_payees(&body, &req).await
                    }
                    ("POST", "/api/level1/prepare-cancel") => app.level1_prepare_cancel(&body),
                    ("POST", "/api/level1/prepare-mode") => {
                        app.level1_prepare_mode(&body, &req).await
                    }
                    ("POST", "/api/level1/prepare-withdraw") => app.level1_prepare_withdraw(&body),
                    ("POST", "/api/level1/notify") => app.level1_notify(&body).await,
                    ("POST", "/api/level1/push-subscribe") => {
                        app.level1_push_subscribe(&body).await
                    }
                    ("GET", "/api/level1/push-key") => match operator::push::vapid_public() {
                        Ok(k) => json_reply("200 OK", json!({ "key": k })),
                        Err(e) => json_reply("500 Internal Server Error", json!({ "error": e })),
                    },
                    ("POST", "/api/level1/bills") => app.level1_bills(&body, &req).await,
                    ("POST", "/api/level1/wallets") => app.level1_wallets(&body, &req).await,
                    ("POST", "/api/level1/contacts") => app.level1_contacts(&body).await,
                    // docs/83 W5: the desk.
                    ("GET", "/api/desk") => app.desk_get(query, &req).await,
                    ("GET", "/api/desk/statements") => app.desk_statements(query, &req).await,
                    ("POST", "/api/desk/party") => app.desk_party(&body, &req).await,
                    ("POST", "/api/desk/party/revoke") => app.desk_party_revoke(&body, &req).await,
                    // docs/105 (S2): a counterparty joins by a link, so the owner types no address. The
                    // owner mints a one-use invite for a role; the counterparty's key answers it, signed
                    // (`desk_invite::propose`, public: the invite and the signature are the authority); the
                    // owner's page lists the answers; approving one is the passkey-signed
                    // `POST /api/desk/party` above. Nothing in this block approves anyone.
                    ("POST", "/api/desk/party/invite")
                    | ("POST", "/api/desk/party/propose")
                    | ("GET", "/api/desk/party/proposals")
                    | ("POST", "/api/desk/party/dismiss") => {
                        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                        let named = v["policy"]
                            .as_str()
                            .or_else(|| query.split('&').find_map(|kv| kv.strip_prefix("policy=")));
                        match app.desk_tenant(named) {
                            Err(r) => r,
                            Ok(t) => {
                                // Minting an invite and turning an answer down are the owner's
                                // device only (docs/103 S2-4): an agent key may read the list.
                                let owner = if method == "GET" {
                                    app.desk_owner(&t, &req, Some(&v), "read")
                                } else {
                                    level1::reserve_token_ok(
                                        &t,
                                        header(&req, "x-tameion-device")
                                            .or_else(|| v["reserveToken"].as_str()),
                                    )
                                };
                                let book = {
                                    let _g = DESK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                                    app.desk_book(&t)
                                };
                                let refused = |e: String| {
                                    json_reply("400 Bad Request", json!({ "error": e }))
                                };
                                match (path, owner, book) {
                                    (_, _, Err(r)) => r,
                                    ("/api/desk/party/propose", _, Ok((book, c))) => {
                                        match Address::from_str(&t.policy)
                                            .map_err(|e| e.to_string())
                                            .and_then(|desk| {
                                                operator::desk_invite::propose(
                                                    &app.dir,
                                                    &t.policy,
                                                    c.chain_id,
                                                    desk,
                                                    &book.parties,
                                                    &book.revoked,
                                                    &v,
                                                    now_secs(),
                                                )
                                            }) {
                                            Ok(r) => json_reply("200 OK", r),
                                            Err(e) => refused(e),
                                        }
                                    }
                                    (_, false, _) => json_reply(
                                        "403 Forbidden",
                                        json!({ "error": "invites are the owner's: open the desk on the device that opened the account" }),
                                    ),
                                    ("/api/desk/party/invite", _, _) => {
                                        match operator::desk_invite::mint(
                                            &app.dir,
                                            &t.policy,
                                            v["role"].as_str().unwrap_or(""),
                                            now_secs(),
                                        ) {
                                            Ok((invite, role, expires)) => json_reply(
                                                "200 OK",
                                                json!({ "invite": invite, "role": role, "expires": expires }),
                                            ),
                                            Err(e) => refused(e),
                                        }
                                    }
                                    ("/api/desk/party/proposals", _, Ok((book, _))) => json_reply(
                                        "200 OK",
                                        json!({ "proposals": operator::desk_invite::pending(&app.dir, &t.policy, &book.parties, &book.revoked, now_secs()) }),
                                    ),
                                    _ => match operator::desk_invite::dismiss(
                                        &app.dir,
                                        &t.policy,
                                        v["id"].as_str().unwrap_or(""),
                                    ) {
                                        Ok(true) => {
                                            json_reply("200 OK", json!({ "dismissed": v["id"] }))
                                        }
                                        Ok(false) => json_reply(
                                            "404 Not Found",
                                            json!({ "error": "no such invite on this desk" }),
                                        ),
                                        Err(e) => refused(e),
                                    },
                                }
                            }
                        }
                    }
                    ("GET", "/api/desk/venue") => app.desk_venue_get(query, &req).await,
                    ("POST", "/api/desk/venue") => app.desk_venue_connect(&body, &req).await,
                    ("POST", "/api/desk/venue/disconnect") => {
                        app.desk_venue_disconnect(&body, &req).await
                    }
                    // S3 (docs/106): the exchange connected with one wallet signature (`ApproveAgent`).
                    ("POST", "/api/desk/venue/connect") | ("POST", "/api/desk/venue/approve") => {
                        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                        match app.desk_tenant(v["policy"].as_str()) {
                            Err(r) => r,
                            Ok(t) if !app.desk_owner(&t, &req, Some(&v), "run") => json_reply(
                                "403 Forbidden",
                                json!({ "error": "only the owner's device or a run key connects the exchange" }),
                            ),
                            Ok(t) if path.ends_with("/connect") => {
                                let (status, out) = operator::desk_live::connect(&t.policy, &v);
                                json_reply(status, out)
                            }
                            Ok(t) => {
                                // Where the offset lives is the owner's decision (docs/89): the
                                // owner's passkey over `desk::venue_challenge` for the wallet that
                                // signed, as `POST /api/desk/venue`, before the venue sees anything.
                                let master = v["master"]
                                    .as_str()
                                    .and_then(|m| Address::from_str(m.trim()).ok());
                                match master {
                                    None => json_reply(
                                        "400 Bad Request",
                                        json!({ "error": "master must be the address of the wallet that signed" }),
                                    ),
                                    Some(master) => match app
                                        .desk_owner_passkey(
                                            &t,
                                            &v,
                                            "connecting the desk to an exchange account",
                                            |c, p, i| {
                                                operator::desk::venue_challenge(c, p, master, i)
                                            },
                                        )
                                        .await
                                    {
                                        Err(r) => r,
                                        Ok(approval) => {
                                            // No offset step runs while the desk's account changes.
                                            let _trade = DESK_TRADE.lock().await;
                                            let (status, out) = operator::desk_live::approve(
                                                &t.policy, &v, master, approval,
                                            )
                                            .await;
                                            json_reply(status, out)
                                        }
                                    },
                                }
                            }
                        }
                    }
                    ("POST", "/api/desk/milestone") => app.desk_milestone(&body, &req).await,
                    ("POST", "/api/desk/base") => app.desk_base(&body, &req).await,
                    ("POST", "/api/desk/control") => app.desk_control(&body, &req).await,
                    ("POST", "/api/desk/explain") => app.desk_explain(&body, &req).await,
                    ("POST", "/api/desk/offset") => app.desk_offset(&body, &req).await,
                    ("POST", "/api/desk/statement") => app.desk_statement(&body, &req).await,
                    ("POST", "/api/desk/statements/all") => {
                        app.desk_statement_all(&body, &req).await
                    }
                    ("GET", "/api/level1/resolve") => app.level1_resolve(query).await,
                    ("POST", "/api/level1/reserve-token") => app.level1_reserve_token(&body).await,
                    // docs/79 §6a #6: a handle names an account; public, read-only.
                    ("GET", p) if p.starts_with("/api/level1/name/") => {
                        let n = p["/api/level1/name/".len()..].to_lowercase();
                        let held = level1::by_name(&app.dir, &n);
                        match held.first() {
                            _ if !level1::valid_name(&n) => json_reply(
                                "400 Bad Request",
                                json!({ "error": "a name is 3 to 32 lowercase letters, numbers or dashes" }),
                            ),
                            None => json_reply(
                                "404 Not Found",
                                json!({ "error": "no account has that name" }),
                            ),
                            Some(first) => json_reply(
                                "200 OK",
                                json!({
                                    "name": n,
                                    "account": first.policy,
                                    "owner": first.owner,
                                    "chains": held.iter().filter_map(|t| {
                                        let c = chain(&t.chain)?;
                                        Some(json!({ "key": c.key, "name": c.name, "chainId": c.chain_id, "deployed": t.deployed, "account": t.policy }))
                                    }).collect::<Vec<_>>(),
                                }),
                            ),
                        }
                    }
                    ("POST", "/api/level1/invoice") => app.level1_invoice(&body).await,
                    ("POST", "/api/level1/invoice-decide") => {
                        app.level1_invoice_decide(&body, &req).await
                    }
                    ("POST", "/api/level1/buy") => app.level1_buy(&body).await,
                    ("POST", "/api/level1/prepare-limits") => {
                        app.level1_prepare_limits(&body, &req).await
                    }
                    ("POST", "/api/level1/prepare-recovery") => app.level1_prepare_recovery(&body),
                    ("POST", "/api/level1/prepare-fund") => app.level1_prepare_fund(&body).await,
                    ("POST", "/api/level1/prepare-schedule") => {
                        app.level1_prepare_schedule(&body).await
                    }
                    ("POST", "/api/level1/prepare-unschedule") => {
                        app.level1_prepare_unschedule(&body)
                    }
                    ("POST", "/api/level1/prepare-renew") => app.level1_prepare_renew(&body),
                    ("POST", "/api/level1/prepare-token") => app.level1_prepare_token(&body).await,
                    ("POST", "/api/level1/token-signed") => app.level1_token_signed(&body).await,
                    ("POST", "/api/level1/token-revoke-signed") => {
                        app.level1_token_revoke_signed(&body).await
                    }
                    ("POST", "/api/level1/prepare-token-revoke") => {
                        app.level1_prepare_token_revoke(&body)
                    }
                    ("POST", "/api/level1/prepare-guardians") => {
                        app.level1_prepare_guardians(&body)
                    }
                    ("POST", "/api/level1/pay-due") => app.level1_pay_due(&body).await,
                    ("POST", "/api/level1/purchase-approve") => {
                        app.level1_purchase_approve(&body, &req)
                    }
                    ("POST", "/api/level1/prepare-notify") => {
                        app.level1_prepare_notify(&body).await
                    }
                    ("POST", "/api/level1/notify-signed") => app.level1_notify_signed(&body).await,
                    ("POST", "/api/level1/fund") => app.level1_fund(&body).await,
                    ("GET", "/api/level1/record") => app.level1_record(query, &req).await,
                    ("GET", "/api/level1/records") => app.level1_records(query).await,
                    ("GET", "/api/level1/statement") => app.level1_statement(query).await,
                    ("POST", "/api/level1/run") => app.level1_run(&body).await,
                    ("POST", "/api/level1/mode") => app.level1_mode(&body).await,
                    ("GET", p) if p.starts_with("/api/level1/tenant/") => {
                        match level1::load(&app.dir, &p["/api/level1/tenant/".len()..]) {
                            Some(t) => json_reply("200 OK", json!({ "tenant": t.public() })),
                            None => {
                                json_reply("404 Not Found", json!({ "error": "no such tenant" }))
                            }
                        }
                    }
                    // PLAN 5.45: a partner's public name (for the Start page) and,
                    // with its key, the accounts it brought — from files, no chain reads.
                    ("GET", "/api/partner/public") => {
                        let id = query
                            .split('&')
                            .find_map(|kv| kv.strip_prefix("id="))
                            .unwrap_or("");
                        match level1::partners(&app.dir).into_iter().find(|p| p.id == id) {
                            Some(p) => json_reply("200 OK", json!({ "id": p.id, "name": p.name })),
                            None => {
                                json_reply("404 Not Found", json!({ "error": "no such partner" }))
                            }
                        }
                    }
                    ("GET", "/api/partner/accounts") => {
                        let key = header(&req, "authorization")
                            .and_then(|v| v.strip_prefix("Bearer "))
                            .map(str::trim)
                            .unwrap_or("");
                        match level1::partner_for_key(&app.dir, key) {
                            None => json_reply(
                                "401 Unauthorized",
                                json!({ "error": "send Authorization: Bearer <partner key>" }),
                            ),
                            Some(p) => {
                                let jobs = app.jobs.lock().await;
                                let accounts: Vec<Value> = level1::list(&app.dir)
                                    .into_iter()
                                    .filter(|t| t.partner.as_deref() == Some(p.id.as_str()))
                                    .map(|t| {
                                        let last = jobs
                                            .values()
                                            .filter(|j| j.status == "done" && j.request["policy"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(&t.policy)))
                                            .max_by_key(|j| j.created);
                                        json!({
                                            "policy": t.policy, "owner": t.owner, "kind": t.kind, "keyless": t.keyless,
                                            "deployed": t.deployed, "created": t.created,
                                            "lastPass": last.map(|j| json!({
                                                "at": j.created,
                                                "obligations": j.report.as_ref().and_then(|r| r["obligations"].as_u64()),
                                                "needsYou": j.report.as_ref().and_then(|r| r["pendingSignature"].as_array().map(|a| a.len())),
                                                "settled": j.report.as_ref().and_then(|r| r["settled"].as_array().map(|a| a.len())),
                                            })),
                                        })
                                    })
                                    .collect();
                                let live =
                                    accounts.iter().filter(|a| a["deployed"] == true).count();
                                json_reply(
                                    "200 OK",
                                    json!({ "partner": { "id": p.id, "name": p.name }, "accounts": accounts, "live": live }),
                                )
                            }
                        }
                    }
                    ("GET", "/api/level1/tenants") => {
                        let given = query
                            .split('&')
                            .find_map(|kv| kv.strip_prefix("token="))
                            .unwrap_or("");
                        match &app.admin_token {
                            Some(t) if !t.is_empty() && t == given => json_reply(
                                "200 OK",
                                json!({ "tenants": level1::list(&app.dir).iter().map(level1::Tenant::public).collect::<Vec<_>>() }),
                            ),
                            _ => json_reply("403 Forbidden", json!({ "error": "admin token" })),
                        }
                    }
                    ("GET", p) if p.starts_with("/api/watch/") => {
                        app.get(&p["/api/watch/".len()..]).await
                    }
                    ("GET", "/api/runs") => app.runs(query).await,
                    ("POST", "/api/level1/identity") => app.level1_identity(&body).await,
                    ("GET", p) if p.starts_with("/proof/") => {
                        proof_file(&p["/proof/".len()..]).await
                    }
                    ("GET", p) if p.starts_with("/agents/") && p.ends_with(".json") => {
                        match level1::load(&app.dir, &p["/agents/".len()..p.len() - 5]) {
                            Some(t) => json_reply(
                                "200 OK",
                                agent_card(&app.dir, &t, Some(&origin_of(&req)), true).await,
                            ),
                            None => {
                                json_reply("404 Not Found", json!({ "error": "no such account" }))
                            }
                        }
                    }
                    // Everything else a browser asks for is the console's.
                    ("GET", p) if !p.starts_with("/api/") => console_response(p)
                        .await
                        .unwrap_or_else(|| respond("404 Not Found", "text/plain", b"not found")),
                    _ => respond("404 Not Found", "text/plain", b"not found"),
                }
            };
            let reply = shaped(reply, method, path, &req);
            if let (Some(k), true) = (&idem_key, first) {
                app.idem_finish(path, k, &reply).await;
            }
            // Any call that changed an account tells its event streams; the
            // console re-reads the inbox (the call's own route is the cause).
            // A purchase answered 409 (waits for the owner, or refused) was
            // recorded too, so it tells them as well (docs/60 P-173, P-174).
            if method == "POST"
                && first
                && (reply.starts_with(b"HTTP/1.1 2")
                    || (path == "/api/level1/buy" && reply.starts_with(b"HTTP/1.1 409")))
                && let Some(cause) = path.strip_prefix("/api/level1/")
                && !cause.starts_with("prepare")
                && let Some(p) = serde_json::from_str::<Value>(&body).ok().and_then(|v| {
                    v["policy"]
                        .as_str()
                        .or(v["target"].as_str())
                        .map(String::from)
                })
            {
                app.emit(&p, "inbox", json!({ "cause": cause }));
            }
            let reply = gzip_reply(reply, header(&req, "accept-encoding"));
            let reply = if head_only { head_of(reply) } else { reply };
            let _ = sock.write_all(&reply).await;
            let _ = sock.flush().await;
        });
    }
}

#[cfg(test)]
mod console_tests {
    use super::*;

    #[test]
    fn the_allowance_is_sized_from_the_bills_of_the_next_30_days() {
        let now = 1_000_000;
        let r = json!({ "obligationsList": [
            // monthly, due in 10 days: once in the window
            { "amount": "0.200000 USDC", "intervalSecs": 30 * 86_400, "nextDue": now + 10 * 86_400 },
            // weekly, overdue: counts now, then every 7 days -> 5 times (0, 7, 14, 21, 28)
            { "amount": "0.010000 USDC", "intervalSecs": 7 * 86_400, "nextDue": now - 86_400 },
        ]});
        assert_eq!(bills_next_30(&r, now), Some(200_000 + 5 * 10_000));
        assert_eq!(bills_next_30(&json!({}), now), None);
        // funding booked by the hedge engine: received +2, paid -0.5 -> net 1.5;
        // older than the window and other kinds are not counted
        let j = [
            r#"{"credits":[{"account":"revenue:hedge:funding","ccy":"USDC","minor":"2000000"}],"debits":[{"account":"asset:hedge:margin","ccy":"USDC","minor":"2000000"}],"doc":{"id":"ETH@5000","kind":"funding"}}"#,
            r#"{"credits":[{"account":"asset:hedge:margin","ccy":"USDC","minor":"500000"}],"debits":[{"account":"expense:hedge:funding","ccy":"USDC","minor":"500000"}],"doc":{"id":"ETH@6000","kind":"funding"}}"#,
            r#"{"credits":[{"account":"revenue:hedge:funding","ccy":"USDC","minor":"9000000"}],"debits":[{"account":"asset:hedge:margin","ccy":"USDC","minor":"9000000"}],"doc":{"id":"ETH@100","kind":"funding"}}"#,
            r#"{"credits":[{"account":"equity:hedge:contributed","ccy":"USDC","minor":"7"}],"debits":[{"account":"asset:hedge:margin","ccy":"USDC","minor":"7"}],"doc":{"id":"cash@7000","kind":"hedge"}}"#,
        ]
        .join("\n");
        assert_eq!(funding_since(&j, 1000), (1_500_000, 2));
        assert_eq!(funding_since("", 0), (0, 0));
        // weekly, six days late: due now, then +1, +8, +15, +22, +29 days -> 6 times
        // (was 5: the cadence restarted from now; review 2026-10-15)
        let late = json!({ "obligationsList": [
            { "amount": "0.010000 USDC", "intervalSecs": 7 * 86_400, "nextDue": now - 6 * 86_400 },
            // no cadence: counts nothing, voids nothing
            { "amount": "0.500000 USDC", "intervalSecs": 0, "nextDue": now },
        ]});
        assert_eq!(bills_next_30(&late, now), Some(6 * 10_000));
        // every 3 minutes, last due 12 hours ago: may have ended, counts once
        // (was 14,400 times: docs/60 P-58)
        let ended = json!({ "obligationsList": [
            { "amount": "0.200000 USDC", "intervalSecs": 180, "nextDue": now - 12 * 3_600 },
        ]});
        assert_eq!(bills_next_30(&ended, now), Some(200_000));
    }

    #[test]
    fn recovery_defaults_to_the_watched_wallet_and_an_explicit_one_is_kept() {
        // docs/60 P-76: a reserve body without `recovery` names the pasted
        // wallet; an explicit address (zero included) wins; junk is refused.
        let watched = Address::repeat_byte(0x11);
        let other = Address::repeat_byte(0x22);
        assert_eq!(recovery_for(&json!({}), watched), Ok(watched));
        assert_eq!(
            recovery_for(&json!({ "recovery": null }), watched),
            Ok(watched)
        );
        assert_eq!(
            recovery_for(&json!({ "recovery": format!(" {other:#x} ") }), watched),
            Ok(other)
        );
        assert_eq!(
            recovery_for(
                &json!({ "recovery": format!("{:#x}", Address::ZERO) }),
                watched
            ),
            Ok(Address::ZERO)
        );
        assert!(recovery_for(&json!({ "recovery": "" }), watched).is_err());
        assert!(recovery_for(&json!({ "recovery": "0x12" }), watched).is_err());
        assert!(recovery_for(&json!({ "recovery": 7 }), watched).is_err());
    }

    #[test]
    fn console_paths_never_leave_the_export() {
        // A Next chunk of a `[param]` route, as a browser asks for it.
        assert_eq!(
            console_segments("/_next/static/chunks/app/account/%5Bid%5D/page-1.js").unwrap(),
            [
                "_next",
                "static",
                "chunks",
                "app",
                "account",
                "[id]",
                "page-1.js"
            ]
        );
        assert_eq!(console_segments("/").unwrap(), Vec::<String>::new());
        for bad in [
            "/../secret",
            "/a/../../secret",
            "/%2e%2e/secret",
            "/_next/..%2F..%2Fsecret",
            "/a%5C..%5Csecret",
            "/a%00b",
            "/./x",
        ] {
            assert!(console_segments(bad).is_none(), "{bad} must be refused");
        }
    }
}

#[cfg(test)]
mod invoice_settled_tests {
    use super::*;
    use alloy::primitives::B256;
    use ledger::{Currency, Money};

    fn paid(to: Address, units: i128, id: B256) -> arc::PrincipalSettlement {
        arc::PrincipalSettlement {
            to,
            amount: Money::minor(units, Currency::Usdc),
            action_hash: id,
            tx: B256::ZERO,
            block: 1,
        }
    }

    #[test]
    fn text_is_gzipped_for_a_client_that_accepts_it_and_nothing_else_is() {
        use std::io::Read;
        let body = "x".repeat(5_000);
        let js = respond("200 OK", "text/javascript; charset=utf-8", body.as_bytes());
        let gz = gzip_reply(js.clone(), Some("br, gzip, deflate"));
        let i = gz.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&gz[..i]).to_string();
        assert!(head.contains("Content-Encoding: gzip"));
        assert!(head.contains(&format!("Content-Length: {}", gz.len() - i - 4)));
        let mut back = String::new();
        flate2::read::GzDecoder::new(&gz[i + 4..])
            .read_to_string(&mut back)
            .unwrap();
        assert_eq!(back, body);
        // no gzip asked, gzip refused with q=0, an image, a small body: unchanged
        assert_eq!(gzip_reply(js.clone(), None), js);
        assert_eq!(gzip_reply(js.clone(), Some("gzip;q=0, br")), js);
        assert_ne!(gzip_reply(js.clone(), Some("gzip;q=0.5")), js);
        let png = respond("200 OK", "image/png", &[0u8; 5_000]);
        assert_eq!(gzip_reply(png.clone(), Some("gzip")), png);
        let small = respond("200 OK", "application/json", b"{}");
        assert_eq!(gzip_reply(small.clone(), Some("gzip")), small);
    }

    #[test]
    fn only_a_job_id_reaches_the_jobs_folder() {
        assert!(is_job_id("6acfc3a5-0000"));
        assert!(is_job_id("6acfdbae-00ff"));
        for bad in [
            "../tenants/0x7ebc",
            "6acfc3a5-0000/../x",
            "6acfc3a5",
            "-0000",
            "6acfc3a5-000",
            "6acfc3a5-00000",
            "..%2f-0000",
            "6acfc3a5-00g0",
        ] {
            assert!(!is_job_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_first_bill_on_a_reserved_account_is_shape_checked_without_the_chain() {
        let ok = level1::SeededBill {
            payee: "0x1111111111111111111111111111111111111111".into(),
            amount: "12.50".into(),
            every_days: 30,
            next_due: 0,
            discount_bps: 0,
            early_days: 0,
        };
        assert_eq!(reserved_bill_problem(&ok), None);
        let bad = |f: fn(&mut level1::SeededBill)| {
            let mut b = ok.clone();
            f(&mut b);
            reserved_bill_problem(&b).is_some()
        };
        assert!(bad(|b| b.payee = "vendor".into()));
        assert!(bad(|b| b.amount = "0".into()));
        assert!(bad(|b| b.amount = "lots".into()));
        assert!(bad(|b| b.every_days = 0));
        assert!(bad(|b| b.every_days = 367));
        assert!(bad(|b| {
            b.discount_bps = 200;
            b.early_days = 30;
        }));
    }

    #[test]
    fn a_head_answer_is_the_get_answer_without_its_body() {
        let get = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".to_vec();
        assert_eq!(
            head_of(get),
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n".to_vec()
        );
        assert_eq!(head_of(b"no headers".to_vec()), b"no headers".to_vec());
    }

    #[test]
    fn half_a_bill_under_its_hash_is_not_the_bill() {
        // review 2026-10-15: the discount cap is 10 %, so 90 % counts, 50 % does not
        let h = B256::repeat_byte(4);
        let payee = Address::repeat_byte(1);
        assert!(paid_as_named(
            &[(h, payee, 315_000)],
            h,
            Some(payee),
            350_000
        ));
        assert!(!paid_as_named(
            &[(h, payee, 175_000)],
            h,
            Some(payee),
            350_000
        ));
        // an amount that did not parse matches nothing
        assert!(!paid_as_named(&[(h, payee, 1)], h, Some(payee), 0));
    }

    #[test]
    fn a_spent_hash_counts_as_paid_only_to_its_payee() {
        let h = B256::repeat_byte(3);
        let (payee, other) = (Address::repeat_byte(1), Address::repeat_byte(2));
        // paid in full, or at an early-payment discount
        assert!(paid_as_named(
            &[(h, payee, 350_000)],
            h,
            Some(payee),
            350_000
        ));
        assert!(paid_as_named(
            &[(h, payee, 343_000)],
            h,
            Some(payee),
            350_000
        ));
        // F1c: the hash burned with one unit to someone else, or to the payee
        assert!(!paid_as_named(&[(h, other, 1)], h, Some(payee), 350_000));
        assert!(!paid_as_named(&[(h, payee, 1)], h, Some(payee), 350_000));
        // no payment event at all (e.g. one older than the read window)
        assert!(!paid_as_named(&[], h, Some(payee), 350_000));
    }

    #[test]
    fn only_the_vendor_paid_in_full_under_the_id_counts() {
        let id = B256::repeat_byte(7);
        let vendor = Address::repeat_byte(1);
        let other = Address::repeat_byte(2);
        // the real payment
        assert!(settles(&[paid(vendor, 7_000, id)], id, vendor, 7_000));
        // F1b: a stolen agent key burns the id with one unit to someone else
        assert!(!settles(&[paid(other, 1, id)], id, vendor, 7_000));
        // one unit to the vendor is not the invoice either
        assert!(!settles(&[paid(vendor, 1, id)], id, vendor, 7_000));
        // the full amount to the vendor under another id is another payment
        assert!(!settles(
            &[paid(vendor, 7_000, B256::repeat_byte(8))],
            id,
            vendor,
            7_000
        ));
        assert!(!settles(&[], id, vendor, 7_000));
    }
}

#[cfg(test)]
mod settle_key_tests {
    use super::*;
    use alloy::primitives::B256;
    use ledger::{Currency, Money};

    /// docs/60 P-227: an import may not move a bill whose cycle is paid to a
    /// start before its next unpaid cycle; a bill with nothing paid moves freely.
    #[test]
    fn a_reschedule_never_reopens_a_paid_cycle() {
        let payee = "0x179dca89f69faef5765d2e7445aa5452b8159520";
        let (d0, month) = (1_792_156_029u64, 2_592_000u64);
        let w = reopens_paid_cycle(payee, d0, Some(d0 + month), d0 + 5 * 86_400).unwrap();
        assert!(
            w.contains("paid up to") && w.contains(&(d0 + month).to_string()),
            "{w}"
        );
        assert_eq!(
            reopens_paid_cycle(payee, d0, Some(d0 + month), d0 + month),
            None
        );
        assert_eq!(
            reopens_paid_cycle(payee, d0, Some(d0), d0 - 86_400),
            None,
            "nothing paid yet"
        );
        assert_eq!(
            reopens_paid_cycle(payee, d0, None, 1),
            None,
            "no bill found by the plan"
        );
    }

    /// docs/60 P-229, U1's re-check: a watcher pass at 15:02:40 knew the bill due
    /// 15:03:14 (34 s later). "Check now" after the due time is not answered from
    /// that pass, and the watcher's own pass is set for the due time + 5 s — a
    /// minute after the last pass at the earliest — instead of a cycle late.
    #[test]
    fn a_pass_runs_when_a_known_bill_falls_due() {
        let (ran, due) = (1_792_220_560u64, 1_792_220_594u64);
        let report = json!({ "obligationsList": [
            { "nextDue": due, "provenance": "history" },
            { "nextDue": due - 100, "provenance": "x402 receipt" },
        ]});
        assert_eq!(
            due_after(&report, ran, due - 1),
            None,
            "not due yet: the pass answers"
        );
        assert_eq!(
            due_after(&report, ran, due + 2),
            Some(due),
            "fell due after the pass: run again"
        );
        assert_eq!(
            due_pass_at(&report, ran),
            Some(ran + 60),
            "a minute after the last pass"
        );
        assert_eq!(due_pass_at(&report, ran - 300), Some(due + 5), "due + 5 s");
        assert_eq!(due_pass_at(&json!({ "obligationsList": [] }), ran), None);
    }

    /// docs/60 P-228, U1's re-check: the bill found in the history (5 payments,
    /// confident) is offered for approval before it falls due; a typed bill (its
    /// payee is approved when it is saved), an x402 receipt and a weak guess are not.
    #[test]
    fn a_found_bill_is_offered_for_approval_before_it_falls_due() {
        let report = json!({ "obligationsList": [
            { "counterparty": "0xf4d08abf297f57acd381f000a25442186d860e43", "amount": "0.002000 USDC", "intervalSecs": 243,
              "nextDue": 1792220350, "confidence": 0.8125, "samples": 5, "provenance": "history" },
            { "counterparty": "0xee80b9f69d80a04d4b2e803c426715aa8c1bb1f3", "amount": "0.001000 USDC", "intervalSecs": 2592000,
              "nextDue": 1792220000, "confidence": 1.0, "samples": 0, "provenance": "imported" },
            { "counterparty": "0x00000000000000000000000000000000000000a1", "amount": "0.001000 USDC", "intervalSecs": 60,
              "nextDue": 1792220000, "confidence": 0.9, "samples": 4, "provenance": "x402 receipt" },
            { "counterparty": "0x00000000000000000000000000000000000000a2", "amount": "0.5 USDC", "intervalSecs": 86400,
              "nextDue": 1792220000, "confidence": 0.4, "samples": 3, "provenance": "history" },
        ]});
        let found = found_bills(&report);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            format!("{:#x}", found[0].0),
            "0xf4d08abf297f57acd381f000a25442186d860e43"
        );
        let why = found_bill_reason(&found[0].1, 60, 1_792_220_000);
        assert!(
            why.contains("every 4 minutes")
                && why.contains("paid 5 times")
                && why.contains("1792220350"),
            "{why}"
        );
        assert!(why.contains("pays that cycle itself"), "{why}");
        let late = found_bill_reason(&found[0].1, 60, 1_792_220_400);
        assert!(
            late.contains("fell due") && late.contains("coming cycles"),
            "{late}"
        );
    }

    /// docs/60 P-217: a priced-out purchase waits as one row per seller until a
    /// later paid purchase from that seller answers it, for a day at most.
    #[test]
    fn a_priced_out_purchase_waits_until_a_later_purchase_answers_it() {
        let (a, b) = (
            "0x00000000000000000000000000000000000000a1",
            "0x00000000000000000000000000000000000000a2",
        );
        let now = 1_792_214_000u64;
        let band = |seller: &str, at: u64| json!({ "seller": seller, "status": "needs-you", "band": true, "at": at, "amount": "0.002000 USDC", "usual": "0.001000 USDC" });
        // Newest first, as `x402::purchases` returns them.
        let bought = vec![
            json!({ "seller": b, "status": "paid", "at": now - 10 }),
            band(b, now - 20),
            band(a, now - 30),
            band(a, now - 40),
            band("0x00000000000000000000000000000000000000a3", now - 90_000),
        ];
        let rows = prices_to_approve(&bought, now);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["seller"], a);
        assert_eq!(rows[0]["at"], now - 30, "the newest for that seller");
    }

    /// docs/60 P-211, uat-u2-fin: "Make it standing" offered the typed date of
    /// the cycle paid twice. The candidate starts at the next unpaid cycle, and
    /// `prepare-schedule` refuses any date before it; a first order is free.
    #[test]
    fn a_standing_order_starts_at_the_next_unpaid_cycle() {
        const MONTH: i64 = 2_592_000;
        let payee = "0x179DCA89F69FAEF5765D2E7445AA5452B8159520";
        let d0 = 1_792_156_029;
        let paid = B256::repeat_byte(0x85);
        let by_cycle = std::collections::BTreeMap::from([(
            format!("{}@{d0}", payee.to_lowercase()),
            vec![paid, B256::repeat_byte(0x8e)],
        )]);
        let executed = std::collections::BTreeSet::from([paid]);
        assert_eq!(
            next_unpaid_due(payee, d0, MONTH, &by_cycle, &executed),
            d0 + MONTH
        );
        assert_eq!(
            next_unpaid_due(payee, d0, MONTH, &by_cycle, &Default::default()),
            d0,
            "nothing paid: the typed date stands"
        );
        let w = first_due_refusal(d0 as u64, Some(d0 + MONTH)).unwrap();
        assert!(
            w.contains("already paid") && w.contains(&(d0 + MONTH).to_string()),
            "{w}"
        );
        assert_eq!(
            first_due_refusal((d0 + MONTH) as u64, Some(d0 + MONTH)),
            None
        );
        assert_eq!(
            first_due_refusal(d0 as u64, None),
            None,
            "no bill for this payee yet"
        );
    }

    /// docs/60 P-210, uat-u2-fin: the showing row (22:20 pass) is under hashes
    /// no payment used; the cycle was paid under two earlier passes' hashes.
    /// The row leaves Needs you; a row of another cycle stays.
    #[test]
    fn a_waiting_row_whose_bill_cycle_was_paid_under_another_pass_leaves() {
        let h = |s: &str| B256::from_str(s).unwrap();
        let cycle = "0x179dca89f69faef5765d2e7445aa5452b8159520@1792156029";
        let (a, b, c, k) = (
            h("0x85dcc14b8c469d30b6e7edd45f28d92267583487c27c38111089a41008861969"),
            h("0xd160efeaea984549ffaa3620f39f0aa0d3f9243a754ccab50bf7b87e7ed444f7"),
            h("0x8e14ca725749f09ad6fad8e5c2a128373f82a1e526a2dfa810e3f3cab1d789eb"),
            h("0xf39a6156a544af2b1965d16d1ce40bfb73b73ad47f192f60c2db0649d38f256f"),
        );
        let by_cycle = std::collections::BTreeMap::from([(cycle.to_string(), vec![k, a, b, c])]);
        let executed = std::collections::BTreeSet::from([a, b]);
        let row = json!({ "cycleKey": cycle, "actionHash": format!("{c:#x}"), "settlementKey": format!("{k:#x}") });
        assert_eq!(paid_reference(&row, &by_cycle, &executed), Some(a));
        let next = json!({ "cycleKey": "0x179dca89f69faef5765d2e7445aa5452b8159520@1794748029", "actionHash": format!("{:#x}", B256::repeat_byte(1)) });
        assert_eq!(paid_reference(&next, &by_cycle, &executed), None);
    }

    /// docs/60 P-175: two checks offer the same 30-day bill under two proposal
    /// hashes; the owner's payment of either is the SAME call, keyed to the
    /// bill-cycle, so the contract refuses the second (`AlreadyExecuted`) and
    /// the next pass finds the cycle executed.
    #[test]
    fn the_owners_tap_is_keyed_to_the_bill_cycle_not_the_pass() {
        let row = |hash: &str| {
            json!({
                "counterparty": "0x179dca89f69faef5765d2e7445aa5452b8159520", "amount": "0.050000 USDC",
                "actionHash": hash,
                "settlementKey": "0xf39a6156a544af2b1965d16d1ce40bfb73b73ad47f192f60c2db0649d38f256f",
            })
        };
        let (first, second) = (
            row("0x85dcc14b00000000000000000000000000000000000000000000000000001969"),
            row("0xd160efea000000000000000000000000000000000000000000000000000044f7"),
        );
        let key = settle_key(&first).unwrap();
        assert_eq!(
            settle_key(&second),
            Some(key),
            "both checks name the same bill-cycle"
        );
        let call = |k: B256| {
            arc::prepare_principal_settlement_bound(
                Address::repeat_byte(0x48),
                Address::from_str("0x179dca89f69faef5765d2e7445aa5452b8159520").unwrap(),
                Money::minor(50_000, Currency::Usdc),
                k,
                B256::repeat_byte(0x11),
                1_792_160_000,
            )
            .unwrap()
            .data
        };
        assert_eq!(
            call(key),
            call(settle_key(&second).unwrap()),
            "one call, so one payment"
        );
        assert!(
            call(key).contains("f39a6156a544af2b1965d16d1ce40bfb73b73ad47f192f60c2db0649d38f256f")
        );
        assert_eq!(
            settle_key(&json!({ "actionHash": "0x01" })),
            None,
            "no key, no rebinding"
        );
    }
}
