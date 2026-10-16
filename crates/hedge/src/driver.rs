//! Configure a step from the environment and run it. Shared by the demo
//! server and the chain examples so they wire the hedge in identically.
//!
//! Env:
//! - `HEDGE_SPOT`            holdings, `ETH=0.5,SOL=20`. Unset = no hedge step.
//! - `HEDGE_USER`            Hyperliquid master account (default: the caller's treasury).
//! - `HEDGE_LIVE=1`          send real orders. Anything else is a dry run.
//! - `HEDGE_AGENT_KEY_FILE`  trade-only API wallet key (default `~/.tameion/hedge-agent.key`).
//! - `HEDGE_HALT_DIR`        where `HALT` lives (default `runtime/hedge`).
//!
//! Live needs both the flag and the key. A key alone never trades: the
//! dashboard must not be able to send an order because a file exists.

use crate::engine::{self, Books, EngineError, Holding, Mode, StepInput, StepReport};
use crate::hl::{Venue, parse_address, parse_signer};
use crate::policy::HedgePolicy;
use crate::store;
use hypersdk::Address;
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct HedgeEnv {
    pub input: StepInput,
    pub mode: Mode,
}

impl HedgeEnv {
    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            Mode::DryRun => "dry-run",
            Mode::Live(_) => "live",
        }
    }

    pub fn coins(&self) -> Vec<&str> {
        self.input
            .holdings
            .iter()
            .map(|h| h.coin.as_str())
            .collect()
    }
}

/// `COIN=SIZE,COIN=SIZE` → holdings. Coins are upper-cased.
pub fn parse_holdings(s: &str) -> Result<Vec<Holding>, String> {
    s.split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (coin, size) = p
                .split_once('=')
                .ok_or_else(|| format!("expected COIN=SIZE, got {p:?}"))?;
            Ok(Holding {
                coin: coin.trim().to_uppercase(),
                size: Decimal::from_str(size.trim()).map_err(|e| format!("{p:?}: {e}"))?,
            })
        })
        .collect()
}

/// `None` when `HEDGE_SPOT` is unset: the caller runs without a hedge step.
pub fn from_env(default_user: Address) -> Option<Result<HedgeEnv, String>> {
    let spot = std::env::var("HEDGE_SPOT").ok()?;
    Some(build(default_user, &spot))
}

fn build(default_user: Address, spot: &str) -> Result<HedgeEnv, String> {
    let holdings = parse_holdings(spot)?;
    if holdings.is_empty() {
        return Err("HEDGE_SPOT is set but empty".into());
    }
    let user = match std::env::var("HEDGE_USER") {
        Ok(s) => parse_address(&s)?,
        Err(_) => default_user,
    };
    let halt_dir =
        PathBuf::from(std::env::var("HEDGE_HALT_DIR").unwrap_or_else(|_| "runtime/hedge".into()));
    let mode = mode_from_env()?;
    let policy = HedgePolicy::default();
    policy
        .validate()
        .map_err(|problems| format!("hedge policy: {}", problems.join("; ")))?;
    Ok(HedgeEnv {
        input: StepInput {
            user,
            holdings,
            policy,
            halt_dir,
            cover_usdc: cover_from_env()?,
            only_listed: false,
        },
        mode,
    })
}

/// The desk's step (docs/89 M3): exactly these holdings on exactly this
/// account. `HEDGE_USER` and `HEDGE_COVER_USDC` are the treasury's settings
/// and are ignored here; only the listed coins are traded. Live needs
/// `HEDGE_LIVE=1` and the trade-only key, as everywhere.
pub fn desk_env(user: Address, holdings: Vec<Holding>) -> Result<HedgeEnv, String> {
    desk_env_with_key(user, holdings, None)
}

/// [`desk_env`] trading with `key_file` instead of `HEDGE_AGENT_KEY_FILE`:
/// the desk's own key, which its owner's exchange wallet approved for this
/// account (`POST /api/desk/venue/approve`, docs/106). `None` = this Mac's key.
pub fn desk_env_with_key(
    user: Address,
    holdings: Vec<Holding>,
    key_file: Option<&std::path::Path>,
) -> Result<HedgeEnv, String> {
    if holdings.is_empty() {
        return Err("no holdings to offset".into());
    }
    let halt_dir =
        PathBuf::from(std::env::var("HEDGE_HALT_DIR").unwrap_or_else(|_| "runtime/hedge".into()));
    let policy = HedgePolicy::default();
    policy
        .validate()
        .map_err(|problems| format!("hedge policy: {}", problems.join("; ")))?;
    Ok(HedgeEnv {
        input: StepInput {
            user,
            holdings,
            policy,
            halt_dir,
            cover_usdc: None,
            only_listed: true,
        },
        mode: mode_from(live_from_env(), key_file)?,
    })
}

/// `HEDGE_COVER_USDC`: size the short to this many USDC of bills instead of
/// the whole holding; `full` or unset means the whole holding. The operator
/// examples set the cover from the obligations they inferred unless this is
/// `full`.
pub fn cover_from_env() -> Result<Option<Decimal>, String> {
    match std::env::var("HEDGE_COVER_USDC") {
        Err(_) => Ok(None),
        Ok(v) if v.trim().eq_ignore_ascii_case("full") || v.trim().is_empty() => Ok(None),
        Ok(v) => Decimal::from_str(v.trim())
            .map(Some)
            .map_err(|e| format!("HEDGE_COVER_USDC {v:?}: {e}")),
    }
}

/// True when the operator may size the hedge to the obligations it inferred
/// (the default); false when `HEDGE_COVER_USDC=full` asks for the holding.
pub fn cover_follows_obligations() -> bool {
    !matches!(std::env::var("HEDGE_COVER_USDC"), Ok(v) if v.trim().eq_ignore_ascii_case("full"))
}

/// Build the step from holdings read elsewhere (the chain, `docs/50` #2)
/// instead of `HEDGE_SPOT`. Same mode and policy rules as [`from_env`].
pub fn from_holdings(user: Address, holdings: Vec<Holding>) -> Result<HedgeEnv, String> {
    if holdings.is_empty() {
        return Err("no holdings to hedge".into());
    }
    let spot = holdings
        .iter()
        .map(|h| format!("{}={}", h.coin, h.size))
        .collect::<Vec<_>>()
        .join(",");
    // HEDGE_USER still overrides the account, as with HEDGE_SPOT.
    build(user, &spot)
}

/// Where the books for `user` live: `<halt_dir>/<0xaddress>`.
pub fn books_dir(halt_dir: &std::path::Path, user: Address) -> PathBuf {
    halt_dir.join(format!("{user:#x}"))
}

/// Run one step against Hyperliquid testnet with fresh books.
pub async fn step(env: &HedgeEnv) -> Result<(StepReport, Books), EngineError> {
    let venue = Venue::testnet();
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let coins = env.coins();
    // Books live under the HALT directory, one set per venue account, and
    // survive across runs, so marks are deltas against what was last booked
    // and the trial balance is the whole history, not one pass. Per account
    // because one journal cannot reconcile against two venue balances
    // (found 2026-10-13, docs/46). The HALT file itself stays at the root.
    let dir = books_dir(&env.input.halt_dir, env.input.user);
    let mut books = match store::load_or_new(&dir, &coins)? {
        Some(b) => b,
        None => Books::new(&coins, now_ms.saturating_sub(24 * 3600 * 1000)),
    };
    let report = engine::run_once(&venue, &env.mode, &env.input, &mut books).await?;
    store::save(&dir, &mut books)?;
    Ok((report, books))
}

/// Live with `HEDGE_LIVE=1` and the trade-only key; a dry run otherwise.
fn mode_from_env() -> Result<Mode, String> {
    mode_from(live_from_env(), None)
}

fn live_from_env() -> bool {
    std::env::var("HEDGE_LIVE")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// The signer for a live step: `key_file` when given, else
/// `HEDGE_AGENT_KEY_FILE` (default `~/.tameion/hedge-agent.key`).
fn mode_from(live: bool, key_file: Option<&std::path::Path>) -> Result<Mode, String> {
    Ok(if live {
        let key_file = key_file.map_or_else(
            || {
                std::env::var("HEDGE_AGENT_KEY_FILE").unwrap_or_else(|_| {
                    format!(
                        "{}/.tameion/hedge-agent.key",
                        std::env::var("HOME").unwrap_or_default()
                    )
                })
            },
            |p| p.display().to_string(),
        );
        let key = std::fs::read_to_string(&key_file)
            .map_err(|e| format!("HEDGE_LIVE=1 but no API wallet key at {key_file}: {e}"))?;
        Mode::Live(parse_signer(&key)?)
    } else {
        Mode::DryRun
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypersdk::hypercore::PrivateKeySigner;

    /// A desk connected by `ApproveAgent` signs with the key it connected,
    /// not with this Mac's (docs/103 S3-1).
    #[test]
    fn a_connected_desk_signs_with_its_own_key() {
        let own = PrivateKeySigner::random();
        let p = std::env::temp_dir().join(format!("desk-own-{}.agent.key", std::process::id()));
        std::fs::write(&p, format!("{}\n", own.to_bytes())).unwrap();
        match mode_from(true, Some(&p)).unwrap() {
            Mode::Live(signer) => assert_eq!(signer.address(), own.address()),
            Mode::DryRun => panic!("live with a key is live"),
        }
        assert!(
            matches!(mode_from(false, Some(&p)).unwrap(), Mode::DryRun),
            "no HEDGE_LIVE, no orders"
        );
        let _ = std::fs::remove_file(&p);
        assert!(
            mode_from(true, Some(&p)).is_err(),
            "a missing desk key refuses, never falls back"
        );
    }
}
