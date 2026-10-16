//! The one module that talks to Hyperliquid.
//!
//! Everything comes in through `hypersdk` (alloy 2 + rustls, the SDK listed
//! in Hyperliquid's own API docs) and is converted at this boundary into the
//! pure types the rest of the crate reasons about. Testnet only: there is no
//! mainnet constructor here on purpose (`CLAUDE.md`, hard constraints).
//!
//! [`VenueApi`] is the seam the engine is written against. [`Venue`] is the
//! real one; the tests script a fake so the read-back-and-halt path can be
//! exercised without a funded account.
//!
//! Prices and margin-table ids come from the raw `metaAndAssetCtxs` reply
//! because the typed `PerpMarket` drops the table id, and the id is what the
//! safety engine needs to know the maintenance tier.

use crate::safety::{AccountState, Tier};
use hypersdk::Address;
use hypersdk::hypercore::{
    self, Chain, HttpClient, NonceHandler, OrderResponseStatus, PerpMarket, PositionData,
    PrivateKeySigner, UserFundingEntry,
};
use rust_decimal::{Decimal, RoundingStrategy};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

/// Why a venue call did not give us what we needed.
#[derive(Debug, thiserror::Error)]
pub enum VenueError {
    #[error("venue api: {0}")]
    Api(anyhow::Error),
    /// The reply parsed but did not have the shape the docs describe.
    #[error("venue reply shape: {0}")]
    Shape(String),
    #[error("unknown market {0}")]
    UnknownMarket(String),
    #[error("price {0} does not round to a valid tick")]
    BadTick(Decimal),
}

impl From<anyhow::Error> for VenueError {
    fn from(e: anyhow::Error) -> Self {
        VenueError::Api(e)
    }
}

/// What the engine needs to know about a perp market. The venue's richer
/// type stays inside [`Venue`], which keeps it for tick rounding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketInfo {
    pub name: String,
    /// Asset index used in orders.
    pub index: usize,
    /// Decimal places allowed in a size.
    pub sz_decimals: u32,
    pub max_leverage: u32,
}

/// One perp's live context from `metaAndAssetCtxs`.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetCtx {
    pub mark_px: Decimal,
    pub oracle_px: Decimal,
    /// Hourly funding rate as a fraction. Positive: longs pay shorts.
    pub funding: Decimal,
    pub open_interest: Decimal,
    pub max_leverage: u32,
    pub margin_table_id: Option<u32>,
}

/// The account as the venue sees it, plus every open perp position by coin.
#[derive(Debug, Clone)]
pub struct AccountSnapshot {
    pub state: AccountState,
    pub withdrawable: Decimal,
    pub positions: HashMap<String, PositionData>,
    pub time_ms: u64,
}

impl AccountSnapshot {
    /// Signed position size in a coin; zero when flat.
    pub fn size(&self, coin: &str) -> Decimal {
        self.positions.get(coin).map_or(Decimal::ZERO, |p| p.szi)
    }
}

/// What the engine asks of a venue. Implemented by [`Venue`] for Hyperliquid
/// testnet and by a scripted fake in the tests.
#[allow(async_fn_in_trait)]
pub trait VenueApi {
    /// Live, tradable perp markets by coin name.
    async fn markets(&self) -> Result<HashMap<String, MarketInfo>, VenueError>;
    /// Mark, oracle, funding, open interest and margin table id by coin.
    async fn prices(&self) -> Result<HashMap<String, AssetCtx>, VenueError>;
    /// Cross-margin account state and open positions for `user`.
    async fn account(&self, user: Address) -> Result<AccountSnapshot, VenueError>;
    /// Maintenance tiers for a margin table. Empty when the id is unknown.
    async fn tiers(&self, margin_table_id: Option<u32>) -> Result<Vec<Tier>, VenueError>;
    /// Funding payments to `user` since `start_ms` (inclusive).
    async fn funding_since(
        &self,
        user: Address,
        start_ms: u64,
    ) -> Result<Vec<UserFundingEntry>, VenueError>;
    /// Set cross leverage for a market.
    async fn set_leverage(
        &self,
        signer: &PrivateKeySigner,
        market: &MarketInfo,
        leverage: u32,
    ) -> Result<(), VenueError>;
    /// Place a market order that fills immediately up to `worst_px`.
    /// `size` is positive base units already rounded to the market tick.
    async fn place_market(
        &self,
        signer: &PrivateKeySigner,
        market: &MarketInfo,
        is_buy: bool,
        size: Decimal,
        worst_px: Decimal,
    ) -> Result<Vec<OrderResponseStatus>, VenueError>;
}

/// A Hyperliquid testnet connection.
pub struct Venue {
    client: HttpClient,
    nonces: NonceHandler,
    chain: Chain,
    /// The venue's own market records, kept for price-tick rounding. Filled
    /// by [`VenueApi::markets`]; an order on a market never listed is refused.
    perps: Mutex<HashMap<String, PerpMarket>>,
}

impl Venue {
    pub fn testnet() -> Self {
        Venue {
            client: hypercore::testnet(),
            nonces: NonceHandler::default(),
            chain: Chain::Testnet,
            perps: Mutex::new(HashMap::new()),
        }
    }

    pub fn chain(&self) -> Chain {
        self.chain
    }

    fn cached(&self, name: &str) -> Result<PerpMarket, VenueError> {
        self.perps
            .lock()
            .map_err(|_| VenueError::Shape("market cache poisoned".into()))?
            .get(name)
            .cloned()
            .ok_or_else(|| VenueError::UnknownMarket(name.to_string()))
    }

    /// Ask the venue to withdraw `amount` USDC to `destination`, signed by
    /// `signer`. Exists for one proof (`docs/46` §6, `examples/prove_no_withdraw`):
    /// a trade-only API wallet signing this must be refused by the venue.
    /// The engine never calls it.
    pub async fn withdraw_as(
        &self,
        signer: &PrivateKeySigner,
        destination: Address,
        amount: Decimal,
    ) -> Result<(), VenueError> {
        self.client
            .withdraw(signer, destination, amount, self.nonces.next(), None, None)
            .await?;
        Ok(())
    }

    /// Approve `agent` as an API wallet for the account `master` signs for.
    ///
    /// Signed by the master key exactly once; afterwards the agent trades
    /// and the master key stays cold. The agent can place orders. It cannot
    /// withdraw the master's funds: proved live on 2026-10-13
    /// (`proof/hedge-testnet-nowithdraw-2026-10-13.txt`) — a `withdraw3`
    /// signed by the agent key is attributed by the venue to the signer's
    /// own, non-existent account ("Must deposit before performing actions"),
    /// never to the master, whose balance did not move. Never reuse an agent
    /// address: when a wallet is pruned its nonce set goes with it and old
    /// signatures replay.
    pub async fn approve_agent(
        &self,
        master: &PrivateKeySigner,
        agent: Address,
        name: &str,
    ) -> Result<(), VenueError> {
        self.client
            .approve_agent(master, agent, name.to_string(), self.nonces.next())
            .await?;
        Ok(())
    }

    /// Submit an `ApproveAgent` the master signed in its own wallet over
    /// [`approve_agent_typed_data`] (65-byte hex, as `eth_signTypedData_v4`
    /// returns it). The master key never comes here. Returns the master the
    /// signature recovers to; with `expect`, a signature from anyone else is
    /// refused before the venue sees it. The venue's own refusal (for example
    /// testnet's "Must deposit before performing actions") comes back as `Api`.
    pub async fn approve_agent_signed(
        &self,
        agent: Address,
        name: &str,
        nonce: u64,
        signature: &str,
        expect: Option<Address>,
    ) -> Result<Address, VenueError> {
        let req = approve_agent_request(agent, name, nonce, signature)?;
        // Not recoverable is our finding, not the venue's: it never sees this request.
        let master = req
            .recover(self.chain)
            .map_err(|e| VenueError::Shape(format!("signature: {e}")))?;
        if expect.is_some_and(|e| e != master) {
            return Err(VenueError::Shape(format!(
                "the signature is from {master:#x}, not the wallet that was asked"
            )));
        }
        self.client.send(req).await?.into_default()?;
        Ok(master)
    }

    /// API wallets currently approved for `user`, as the venue reports them.
    pub async fn api_agents(
        &self,
        user: Address,
    ) -> Result<Vec<hypersdk::hypercore::ApiAgent>, VenueError> {
        Ok(self.client.api_agents(user).await?)
    }
}

impl VenueApi for Venue {
    async fn markets(&self) -> Result<HashMap<String, MarketInfo>, VenueError> {
        let perps = self.client.perps().await?;
        let mut cache = self
            .perps
            .lock()
            .map_err(|_| VenueError::Shape("market cache poisoned".into()))?;
        cache.clear();
        let mut out = HashMap::with_capacity(perps.len());
        for m in perps.into_iter().filter(|m| !m.delisted) {
            out.insert(
                m.name.clone(),
                MarketInfo {
                    name: m.name.clone(),
                    index: m.index,
                    sz_decimals: m.sz_decimals.clamp(0, 18) as u32,
                    max_leverage: m.max_leverage.min(u32::MAX as u64) as u32,
                },
            );
            cache.insert(m.name.clone(), m);
        }
        Ok(out)
    }

    async fn prices(&self) -> Result<HashMap<String, AssetCtx>, VenueError> {
        let v = self.client.meta_and_asset_ctxs(None).await?;
        let universe = v
            .get(0)
            .and_then(|m| m.get("universe"))
            .and_then(|u| u.as_array())
            .ok_or_else(|| VenueError::Shape("metaAndAssetCtxs[0].universe missing".into()))?;
        let ctxs = v
            .get(1)
            .and_then(|c| c.as_array())
            .ok_or_else(|| VenueError::Shape("metaAndAssetCtxs[1] missing".into()))?;
        if universe.len() != ctxs.len() {
            return Err(VenueError::Shape(format!(
                "universe has {} entries, asset contexts {}",
                universe.len(),
                ctxs.len()
            )));
        }
        let mut out = HashMap::with_capacity(universe.len());
        for (meta, ctx) in universe.iter().zip(ctxs) {
            let name = meta
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| VenueError::Shape("universe[].name missing".into()))?;
            let dec = |key: &str| -> Result<Decimal, VenueError> {
                let s = ctx
                    .get(key)
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| VenueError::Shape(format!("ctx.{key} missing for {name}")))?;
                Decimal::from_str(s)
                    .map_err(|e| VenueError::Shape(format!("ctx.{key}={s:?} for {name}: {e}")))
            };
            out.insert(
                name.to_string(),
                AssetCtx {
                    mark_px: dec("markPx")?,
                    oracle_px: dec("oraclePx")?,
                    funding: dec("funding")?,
                    open_interest: dec("openInterest")?,
                    max_leverage: meta
                        .get("maxLeverage")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(1) as u32,
                    margin_table_id: meta
                        .get("marginTableId")
                        .and_then(|x| x.as_u64())
                        .map(|x| x as u32),
                },
            );
        }
        Ok(out)
    }

    /// Query the master account, never the API wallet: the venue returns an
    /// empty state for an agent address.
    async fn account(&self, user: Address) -> Result<AccountSnapshot, VenueError> {
        let s = self.client.clearinghouse_state(user, None).await?;
        let positions: HashMap<String, PositionData> = s
            .asset_positions
            .into_iter()
            .map(|ap| (ap.position.coin.clone(), ap.position))
            .collect();
        let total_notional = positions
            .values()
            .fold(Decimal::ZERO, |acc, p| acc + p.position_value);
        Ok(AccountSnapshot {
            state: AccountState {
                account_value: s.cross_margin_summary.account_value,
                margin_used: s.cross_margin_summary.total_margin_used,
                maintenance_margin_used: s.cross_maintenance_margin_used,
                total_notional,
            },
            withdrawable: s.withdrawable,
            positions,
            time_ms: s.time,
        })
    }

    async fn tiers(&self, margin_table_id: Option<u32>) -> Result<Vec<Tier>, VenueError> {
        let Some(id) = margin_table_id else {
            return Ok(Vec::new());
        };
        let table = self.client.margin_table(id).await?;
        Ok(table
            .margin_tiers
            .into_iter()
            .map(|t| Tier {
                lower_bound: t.lower_bound,
                max_leverage: t.max_leverage,
            })
            .collect())
    }

    async fn funding_since(
        &self,
        user: Address,
        start_ms: u64,
    ) -> Result<Vec<UserFundingEntry>, VenueError> {
        Ok(self.client.user_funding(user, start_ms, None).await?)
    }

    async fn set_leverage(
        &self,
        signer: &PrivateKeySigner,
        market: &MarketInfo,
        leverage: u32,
    ) -> Result<(), VenueError> {
        self.client
            .update_leverage(
                signer,
                market.index,
                true,
                leverage,
                self.nonces.next(),
                None,
                None,
            )
            .await?;
        Ok(())
    }

    /// The price is rounded to the venue tick here because only the venue's
    /// own market record knows its tick.
    async fn place_market(
        &self,
        signer: &PrivateKeySigner,
        market: &MarketInfo,
        is_buy: bool,
        size: Decimal,
        worst_px: Decimal,
    ) -> Result<Vec<OrderResponseStatus>, VenueError> {
        let perp = self.cached(&market.name)?;
        let px = perp
            .round_price(worst_px)
            .ok_or(VenueError::BadTick(worst_px))?;
        Ok(self
            .client
            .market_open(
                signer,
                perp,
                is_buy,
                px,
                size,
                self.nonces.next(),
                None,
                None,
                None,
            )
            .await?)
    }
}

/// Round a base-unit size toward zero to the market's size decimals, so a
/// hedge is never larger than the controller asked for.
pub fn round_size(market: &MarketInfo, size: Decimal) -> Decimal {
    size.round_dp_with_strategy(market.sz_decimals.min(18), RoundingStrategy::ToZero)
}

/// The worst price the hedge will accept: `slippage_bps` away from the mark
/// in the direction that still fills.
pub fn worst_price(mark_px: Decimal, is_buy: bool, slippage_bps: u32) -> Decimal {
    let slip = Decimal::from(slippage_bps) / Decimal::from(10_000u32);
    if is_buy {
        mark_px * (Decimal::ONE + slip)
    } else {
        mark_px * (Decimal::ONE - slip)
    }
}

/// Parse a 0x address. Thin wrapper so callers do not import alloy.
pub fn parse_address(s: &str) -> Result<Address, String> {
    Address::from_str(s.trim()).map_err(|e| format!("bad address {s:?}: {e}"))
}

/// Parse a hex private key into a signer. The key never leaves the caller's
/// memory as a string once this returns.
pub fn parse_signer(hex: &str) -> Result<PrivateKeySigner, String> {
    PrivateKeySigner::from_str(hex.trim()).map_err(|e| format!("bad private key: {e}"))
}

/// The EIP-712 typed data a master wallet signs to approve `agent` under
/// `name` on Hyperliquid testnet, as `eth_signTypedData_v4` takes it: the same
/// message [`Venue::approve_agent`] signs with a local key (domain
/// `HyperliquidSignTransaction` v1 on chain 421614). The test below holds it
/// to the bytes the venue verifies.
pub fn approve_agent_typed_data(agent: Address, name: &str, nonce: u64) -> serde_json::Value {
    serde_json::json!({
        "domain": {
            "name": "HyperliquidSignTransaction",
            "version": "1",
            "chainId": 421614,
            "verifyingContract": "0x0000000000000000000000000000000000000000",
        },
        "types": {
            "EIP712Domain": [
                { "name": "name", "type": "string" },
                { "name": "version", "type": "string" },
                { "name": "chainId", "type": "uint256" },
                { "name": "verifyingContract", "type": "address" },
            ],
            "HyperliquidTransaction:ApproveAgent": [
                { "name": "hyperliquidChain", "type": "string" },
                { "name": "agentAddress", "type": "address" },
                { "name": "agentName", "type": "string" },
                { "name": "nonce", "type": "uint64" },
            ],
        },
        "primaryType": "HyperliquidTransaction:ApproveAgent",
        "message": {
            "hyperliquidChain": "Testnet",
            "agentAddress": format!("{agent:#x}"),
            "agentName": name,
            "nonce": nonce,
        },
    })
}

/// The `/exchange` request for an `ApproveAgent` signed elsewhere.
fn approve_agent_request(
    agent: Address,
    name: &str,
    nonce: u64,
    signature: &str,
) -> Result<hypercore::ActionRequest, VenueError> {
    let mut sig = hypercore::Signature::from_str(signature.trim())
        .map_err(|e| VenueError::Shape(format!("signature: {e}")))?;
    // Some wallets return the recovery id as 0/1 instead of 27/28.
    if sig.v < 27 {
        sig.v += 27;
    }
    Ok(hypercore::ActionRequest {
        action: hypercore::Action::ApproveAgent(hypercore::types::api::ApproveAgent {
            signature_chain_id: Chain::Testnet.arbitrum_id().to_string(),
            hyperliquid_chain: Chain::Testnet,
            agent_address: agent,
            agent_name: Some(name.to_string()),
            nonce,
        }),
        nonce,
        signature: sig,
        vault_address: None,
        expires_after: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway key (never funded anywhere) and what `cast wallet sign
    /// --data` produced over `approve_agent_typed_data(0x…01, "desk-cd8d71",
    /// 1792140000000)` on 2026-10-16: an independent EIP-712 implementation
    /// signing our JSON must recover, through hypersdk's own action, to the key.
    const TEST_KEY_ADDR: &str = "0x14791697260E4c9A71f18484C9f997B308e59325";
    const CAST_SIG: &str = "0x51a0dc72c5d42be84c635a3b635f1767f3e0b8e3befd0514bc8be899b2da7e9209fa3579fdc409d684a735642184d133e937419de4a32a5ac2f039f47f533c5e1b";

    #[test]
    fn a_wallet_signature_over_our_typed_data_is_what_the_venue_verifies() {
        let agent = Address::from_str("0x0000000000000000000000000000000000000001").unwrap();
        let req = approve_agent_request(agent, "desk-cd8d71", 1_792_140_000_000, CAST_SIG).unwrap();
        assert_eq!(
            req.recover(Chain::Testnet).unwrap(),
            Address::from_str(TEST_KEY_ADDR).unwrap()
        );
        // And the JSON hashes to the digest hypersdk signs for that action.
        let td: alloy::dyn_abi::TypedData = serde_json::from_value(approve_agent_typed_data(
            agent,
            "desk-cd8d71",
            1_792_140_000_000,
        ))
        .unwrap();
        assert_eq!(
            td.eip712_signing_hash().unwrap(),
            req.action
                .prehash(1_792_140_000_000, None, None, Chain::Testnet)
                .unwrap()
        );
        // Another name or nonce is another message: the signature no longer recovers to the key.
        let other =
            approve_agent_request(agent, "desk-000000", 1_792_140_000_000, CAST_SIG).unwrap();
        assert_ne!(
            other.recover(Chain::Testnet).ok(),
            Some(Address::from_str(TEST_KEY_ADDR).unwrap())
        );
    }
}
