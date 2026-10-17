//! Demo mode: the judge's five minutes (docs/115 §3).
//!
//! `TAMEION_DEMO=1` with `TAMEION_DEMO_VENDOR=0x…` (an address; its key never
//! enters the service) and an invite word (`TAMEION_LEVEL1_INVITE`). An account
//! opened with `demo: true` and that word waits 30 s on a trust-expanding change,
//! lands on chain at once, and the relayer sends it 0.05 USDC without a human (its
//! agent's gas comes with the deploy, as for every account). Bills on a demo host
//! may repeat every minute or more; elsewhere the product rule stays (a day).

use crate::level1::{self, Tenant};
use crate::watch::ChainPreset;
use alloy::primitives::{Address, U256};
use alloy::sol_types::SolCall;
use arc::ArcClient;
use ledger::{Currency, Money};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::str::FromStr;

/// A demo account's delay on trust-expanding changes.
pub const DELAY_SECS: u64 = 30;
/// What the relayer sends a new demo account: 0.05 USDC, in minor units.
pub const FUND_MINOR: i128 = 50_000;
/// The demo bill: 0.002 USDC every two minutes.
pub const BILL_USDC: &str = "0.002";
pub const BILL_EVERY_SECS: u64 = 120;
/// The shortest cadence a bill may have, in demo mode only.
pub const MIN_EVERY_SECS: u64 = 60;
/// A bill in seconds ends after this many cycles (30 minutes at two minutes): an
/// account a judge leaves open stops passing, paying and anchoring by itself.
pub const CYCLES: u64 = 15;
/// Below either the relayer funds no more demo accounts. Set from its balance
/// read on 2026-10-17 16:05 +08 (Arbitrum Sepolia: 24.62 USDC, 0.0889 ETH):
/// about 90 accounts' funding above the floor, the rest kept for other work.
const RELAYER_FLOOR_MINOR: i128 = 20_000_000;
const RELAYER_FLOOR_WEI: u128 = 20_000_000_000_000_000;

alloy::sol! {
    function transfer(address to, uint256 amount) external returns (bool);
}

/// Whether a bill in seconds has run its `CYCLES` cycles by `now`.
pub fn ended(b: &level1::SeededBill, now: u64) -> bool {
    b.every_secs > 0 && now >= b.next_due + CYCLES * b.every_secs
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn enabled() -> bool {
    std::env::var("TAMEION_DEMO").is_ok_and(|v| v.trim() == "1") && vendor().is_some()
}

pub fn vendor() -> Option<Address> {
    std::env::var("TAMEION_DEMO_VENDOR")
        .ok()
        .and_then(|v| Address::from_str(v.trim()).ok())
}

/// `GET /api/level1/demo`. Off without an invite word: a host that funds
/// strangers' accounts must ask them for it.
pub fn config(invite_set: bool) -> Value {
    json!({
        "enabled": enabled() && invite_set,
        "vendor": vendor().map(|a| format!("{a:#x}")),
        "bill": { "usdc": BILL_USDC, "everySecs": BILL_EVERY_SECS, "cycles": CYCLES },
        "delaySecs": DELAY_SECS,
    })
}

/// Whether a demo account may open on `c` now; the refusal is the status and
/// the reply. The deploy route has already checked the invite word itself.
pub async fn may_open(c: &ChainPreset, invite: Option<&str>) -> Result<(), (&'static str, Value)> {
    if !enabled() || invite.is_none_or(str::is_empty) {
        return Err((
            "403 Forbidden",
            json!({ "error": "demo mode is off on this service" }),
        ));
    }
    if c.usdc.is_none() {
        return Err((
            "400 Bad Request",
            json!({ "error": "demo accounts open on Arbitrum Sepolia, where USDC is a token the relayer can send" }),
        ));
    }
    match relayer_short(c).await {
        Ok(None) => Ok(()),
        // refusal: service_paused
        Ok(Some(why)) | Err(why) => Err((
            "503 Service Unavailable",
            json!({ "error": format!("the demo is paused: {why}; nothing was opened") }),
        )),
    }
}

/// What the relayer lacks to fund one more demo account, in words; `None` when
/// it holds both floors.
async fn relayer_short(c: &ChainPreset) -> Result<Option<String>, String> {
    let client = level1::quick_client(c)?;
    let relayer = level1::relayer_address()?;
    let (usdc, wei) = tokio::join!(client.usdc_balance(relayer), client.native_wei(relayer));
    let usdc = usdc.map_err(|e| format!("the relayer's USDC could not be read ({e})"))?;
    let wei = wei.map_err(|e| format!("the relayer's gas could not be read ({e})"))?;
    if usdc.to_minor() < RELAYER_FLOOR_MINOR {
        return Ok(Some(format!(
            "the relayer holds {usdc}, under its floor of {}",
            Money::minor(RELAYER_FLOOR_MINOR, Currency::Usdc)
        )));
    }
    if wei < U256::from(RELAYER_FLOOR_WEI) {
        return Ok(Some(format!(
            "the relayer holds {wei} wei of gas, under its floor of {RELAYER_FLOOR_WEI}"
        )));
    }
    Ok(None)
}

/// Reserve and put on chain at once, keeping the device token a reserve mints.
pub async fn open(
    app_dir: &Path,
    r: level1::DeployRequest,
) -> Result<(Tenant, Option<String>), String> {
    let (mut t, token) = level1::reserve(app_dir, r).await?;
    level1::ensure_deployed(app_dir, &mut t).await?;
    Ok((t, token))
}

fn log_path(app_dir: &Path) -> std::path::PathBuf {
    app_dir.join("demo-funding.jsonl")
}

/// The funding already logged for `policy`, if any.
fn funded(app_dir: &Path, policy: &str) -> Option<Value> {
    std::fs::read_to_string(log_path(app_dir))
        .ok()?
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| {
            v["policy"]
                .as_str()
                .is_some_and(|p| p.eq_ignore_ascii_case(policy))
        })
}

/// Send a demo account its 0.05 USDC once, logged with the transaction
/// (`demo-funding.jsonl` in the service's directory, and stdout). The reply is
/// the log line, or `{error}` when nothing was sent.
pub async fn fund_once(app_dir: &Path, t: &Tenant) -> Value {
    // One funding at a time: the check and the send must not interleave.
    static ONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = ONE.lock().await;
    if let Some(done) = funded(app_dir, &t.policy) {
        return done;
    }
    if !t.deployed || t.delay_secs != DELAY_SECS {
        return json!({ "error": "this account was not opened as a demo, so the relayer sends it nothing" });
    }
    match send(t).await {
        Ok(tx) => {
            let line = json!({
                "at": now(),
                "policy": t.policy,
                "usdc": Money::minor(FUND_MINOR, Currency::Usdc)
                    .to_string()
                    .trim_end_matches(" USDC")
                    .trim_end_matches('0'),
                "tx": tx,
                "agentGasTx": t.agent_gas_tx,
            });
            println!("demo: funded {} with {} in {tx}", t.policy, line["usdc"]);
            let logged = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path(app_dir))
                .and_then(|mut f| writeln!(f, "{line}"));
            if let Err(e) = logged {
                println!(
                    "demo: the funding of {} in {tx} was not logged: {e}",
                    t.policy
                );
            }
            line
        }
        Err(e) => {
            println!("demo: funding {} failed: {e}", t.policy);
            json!({ "error": format!("the relayer could not fund the account ({e}); open it again to retry") })
        }
    }
}

async fn send(t: &Tenant) -> Result<String, String> {
    let c = crate::watch::chain(&t.chain).ok_or("tenant chain unknown")?;
    let usdc = c
        .usdc
        .and_then(|u| Address::from_str(u).ok())
        .ok_or("this chain's USDC is not a token")?;
    if let Some(why) = relayer_short(c).await? {
        return Err(why);
    }
    let policy = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
    let client = ArcClient::with_signer(&crate::watch::rpc_for(c), &level1::relayer_key()?)
        .map_err(|e| e.to_string())?
        .with_token(usdc);
    let data = transferCall {
        to: policy,
        amount: U256::from(FUND_MINOR as u128),
    }
    .abi_encode();
    client
        .send_call(usdc, data)
        .await
        .map(|h| format!("{h:#x}"))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bill_repeats_in_seconds_only_from_a_minute() {
        let bill = |every_days, every_secs| level1::SeededBill {
            payee: "0x1111111111111111111111111111111111111111".into(),
            amount: BILL_USDC.into(),
            every_days,
            next_due: 0,
            discount_bps: 0,
            early_days: 0,
            every_secs,
        };
        assert_eq!(bill(0, BILL_EVERY_SECS).interval_secs(), 120);
        assert_eq!(bill(0, BILL_EVERY_SECS).cadence_problem(), None);
        assert!(bill(0, MIN_EVERY_SECS - 1).cadence_problem().is_some());
        assert_eq!(bill(30, 0).interval_secs(), 30 * 86_400);
        assert!(
            bill(0, 0).cadence_problem().is_some(),
            "the product rule: a day"
        );
        let early = level1::SeededBill {
            discount_bps: 100,
            early_days: 1,
            ..bill(0, BILL_EVERY_SECS)
        };
        assert!(
            early.cadence_problem().is_some(),
            "an early window longer than the cycle"
        );
        let demo = bill(0, BILL_EVERY_SECS);
        assert!(!ended(&demo, 15 * 120 - 1));
        assert!(ended(&demo, 15 * 120), "fifteen cycles, then it stops");
        assert!(
            !ended(&bill(30, 0), u64::MAX / 2),
            "a bill in days never ends here"
        );
    }

    #[test]
    fn the_demo_route_says_what_the_page_offers() {
        let c = config(false);
        assert_eq!(c["enabled"], false, "never without an invite word");
        assert_eq!(
            c["bill"],
            json!({ "usdc": "0.002", "everySecs": 120, "cycles": 15 })
        );
        assert_eq!(c["delaySecs"], 30);
    }
}
