//! Settlement rails: how a payment reaches the payee, and how final it is
//! (docs/117 §7, docs/121).
//!
//! Each rail settles over the payment paths this crate already has
//! ([`ArcClient::pay_via_policy`], [`ArcClient::send_native`],
//! [`ArcClient::send_call`]), unchanged, and measures what they do not say:
//! when the send began, when its receipt was in hand, and when the chain's own
//! `finalized` tag first covered its block. That number is the deliverable.
//! Arc is one BFT chain; Arbitrum Sepolia is a rollup whose `finalized`
//! follows Ethereum's. A payee deciding when to ship needs the difference, and
//! nobody should have to take it on trust.
//!
//! A rail holds no key. It is handed a signing client per call.

use crate::{ARC_TESTNET_CHAIN_ID, ArcClient, ArcError, IUsdc, wei_from_usdc};
use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::BlockNumberOrTag;
use alloy::sol_types::SolCall;
use ledger::{Currency, Money};
use std::future::Future;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long [`SettlementRail::settle`] waits for the chain's `finalized` tag
/// to reach the payment's block. Past it the settlement is still returned,
/// with what the chain reported by then (docs/117 §7).
pub const FINALITY_CAP: Duration = Duration::from_secs(600);

/// One payment to make.
#[derive(Debug, Clone)]
pub struct Transfer {
    pub to: Address,
    pub amount: Money,
    /// `(policy, action hash)`: through the owner's policy contract, whose
    /// caps, payee list and replay guard apply. `None` sends from the
    /// signer's own balance.
    pub policy: Option<(Address, B256)>,
}

/// Whether a settlement can half-happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atomicity {
    /// One transaction: the checks and the transfer land together or not at all.
    OneTransaction,
    /// Two legs in two currencies: both settle or neither does.
    PaymentVersusPayment,
}

/// The strongest assurance the chain gave by the end of the wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalityKind {
    /// The chain reports no `finalized` block: the receipt is all there is.
    Receipt,
    /// `finalized` had not reached the block when [`FINALITY_CAP`] ran out;
    /// this many blocks had been built on top of it by then.
    Confirmations(u64),
    /// The chain's `finalized` tag covers the block.
    ChainFinalTag,
}

impl FinalityKind {
    pub fn label(&self) -> String {
        match self {
            FinalityKind::Receipt => "receipt".into(),
            FinalityKind::Confirmations(n) => format!("confirmations({n})"),
            FinalityKind::ChainFinalTag => "finalized-tag".into(),
        }
    }
}

/// A payment the chain accepted, with its three times in unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    pub tx: B256,
    /// The block its receipt names.
    pub block: u64,
    /// The send began (the client reads the nonce, gas and chain id first).
    pub sent_at: i64,
    /// The receipt was in hand.
    pub included_at: i64,
    /// `eth_getBlockByNumber("finalized")` first reached `block`; `None` when
    /// it did not within [`FINALITY_CAP`].
    pub confirmed_at: Option<i64>,
    pub finality: FinalityKind,
    /// The last `finalized` block number the chain reported while we waited.
    pub finalized_seen: Option<u64>,
}

impl Settled {
    /// Seconds from the send to the chain's finality, when it came within the cap.
    pub fn finality_secs(&self) -> Option<f64> {
        self.confirmed_at
            .map(|c| (c - self.sent_at) as f64 / 1000.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RailError {
    #[error("{0}")]
    NotAvailable(&'static str),
    #[error("this rail settles {rail}, not {asked}")]
    WrongCurrency { rail: Currency, asked: Currency },
    #[error(transparent)]
    Chain(#[from] ArcError),
}

pub trait SettlementRail {
    fn id(&self) -> &'static str;
    fn currency(&self) -> Currency;
    fn atomic(&self) -> Atomicity;
    /// Send `t` with `signer` and wait for the chain's finality, at most
    /// [`FINALITY_CAP`]. A reverted transaction is an error, never a settlement.
    fn settle(
        &self,
        t: Transfer,
        signer: &ArcClient,
    ) -> impl Future<Output = Result<Settled, RailError>> + Send;
}

/// USDC as Arc's native asset, which is gas too: the value moves in the
/// transaction itself (`BudgetPolicy.pay` sends it with `call{value}`).
#[derive(Debug, Clone, Copy)]
pub struct ArcNativeUsdc {
    pub chain_id: u64,
}

impl ArcNativeUsdc {
    pub const TESTNET: ArcNativeUsdc = ArcNativeUsdc {
        chain_id: ARC_TESTNET_CHAIN_ID,
    };
}

impl SettlementRail for ArcNativeUsdc {
    fn id(&self) -> &'static str {
        "arc-native-usdc"
    }
    fn currency(&self) -> Currency {
        Currency::Usdc
    }
    fn atomic(&self) -> Atomicity {
        Atomicity::OneTransaction
    }
    async fn settle(&self, t: Transfer, signer: &ArcClient) -> Result<Settled, RailError> {
        precheck(self, &t, signer, self.chain_id).await?;
        timed(signer, async {
            match t.policy {
                Some((policy, action)) => {
                    signer.pay_via_policy(policy, t.to, t.amount, action).await
                }
                None => signer.send_native(t.to, wei_from_usdc(t.amount)?).await,
            }
        })
        .await
    }
}

/// USDC as an ERC-20 on a chain whose gas is something else (Arbitrum
/// Sepolia: ETH): a `transfer` on the token (`BudgetPolicyToken.pay` uses
/// `safeTransfer`).
#[derive(Debug, Clone, Copy)]
pub struct Erc20Usdc {
    pub id: &'static str,
    pub chain_id: u64,
    pub token: Address,
}

impl Erc20Usdc {
    /// Circle's USDC on Arbitrum Sepolia (the watch preset's token).
    pub const ARBITRUM_SEPOLIA: Erc20Usdc = Erc20Usdc {
        id: "arbitrum-sepolia-usdc",
        chain_id: 421_614,
        token: Address::new(crate::hex_literal(
            b"75faf114eafb1BDbe2F0316DF893fd58CE46AA4d",
        )),
    };
}

impl SettlementRail for Erc20Usdc {
    fn id(&self) -> &'static str {
        self.id
    }
    fn currency(&self) -> Currency {
        Currency::Usdc
    }
    fn atomic(&self) -> Atomicity {
        Atomicity::OneTransaction
    }
    async fn settle(&self, t: Transfer, signer: &ArcClient) -> Result<Settled, RailError> {
        precheck(self, &t, signer, self.chain_id).await?;
        let minor = t.amount.to_minor();
        if minor <= 0 {
            return Err(ArcError::AmountOutOfRange(minor).into());
        }
        timed(signer, async {
            match t.policy {
                Some((policy, action)) => {
                    signer.pay_via_policy(policy, t.to, t.amount, action).await
                }
                None => {
                    let data = IUsdc::transferCall {
                        to: t.to,
                        value: U256::from(minor as u128),
                    }
                    .abi_encode();
                    signer.send_call(self.token, data).await
                }
            }
        })
        .await
    }
}

/// EURC through Circle StableFX: framed, not built (docs/121).
///
/// Circle's flow, from its StableFX OpenAPI (base
/// `https://api.circle.com/v1/exchange/stablefx`, bearer API key; the
/// `Currency` enum is `USDC | EURC`):
///
/// 1. `POST /quotes` with `type: tradable` and a `recipientAddress`: an
///    executable quote carrying the taker's Permit2 EIP-712 `typedData`.
/// 2. `POST /trades`: the quote id and the taker's signed Permit2 message.
/// 3. `GET /signatures/presign/{tradeId}`: the Permit2 payload the maker signs.
/// 4. `POST /signatures`: the maker's signature, registered.
/// 5. `POST /signatures/funding/presign`, then `POST /fund`: each side's
///    Permit2 funding signature is relayed and the escrow settles
///    payment-versus-payment ("both sides of a trade settle or neither does").
///
/// Access is permissioned ("only Circle whitelisted makers and takers can
/// access StableFX"; eligibility is "businesses that are incorporated in a
/// respectable jurisdiction, who undergo compliance screening by Circle") and
/// no sandbox is published, so `settle` refuses before anything is signed.
#[derive(Debug, Clone, Copy)]
pub struct EurcStableFx;

impl SettlementRail for EurcStableFx {
    fn id(&self) -> &'static str {
        "eurc-stablefx"
    }
    fn currency(&self) -> Currency {
        Currency::Eurc
    }
    fn atomic(&self) -> Atomicity {
        Atomicity::PaymentVersusPayment
    }
    async fn settle(&self, _t: Transfer, _signer: &ArcClient) -> Result<Settled, RailError> {
        Err(RailError::NotAvailable(
            "StableFX access pending (institutional)",
        ))
    }
}

/// The amount is in the rail's currency and the client is on the rail's chain:
/// a native rail pointed at another chain would send that chain's gas token.
async fn precheck(
    rail: &impl SettlementRail,
    t: &Transfer,
    signer: &ArcClient,
    chain_id: u64,
) -> Result<(), RailError> {
    if t.amount.currency() != rail.currency() {
        return Err(RailError::WrongCurrency {
            rail: rail.currency(),
            asked: t.amount.currency(),
        });
    }
    signer.address().ok_or(ArcError::NoSigner)?;
    Ok(signer.expect_chain(chain_id).await?)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Time `send`, read its receipt back, then wait for the chain's finality.
async fn timed(
    signer: &ArcClient,
    send: impl Future<Output = Result<B256, ArcError>>,
) -> Result<Settled, RailError> {
    let sent_at = now_ms();
    let tx = send.await?;
    let included_at = now_ms();
    let receipt = signer
        .provider
        .get_transaction_receipt(tx)
        .await
        .map_err(|e| ArcError::Transport(e.to_string()))?
        .ok_or_else(|| ArcError::Contract(format!("no receipt for {tx:#x}")))?;
    if !receipt.status() {
        return Err(ArcError::Contract(format!("{tx:#x} reverted")).into());
    }
    let block = receipt
        .block_number
        .ok_or_else(|| ArcError::Contract(format!("receipt of {tx:#x} names no block")))?;
    let (confirmed_at, finality, finalized_seen) = wait_final(signer, block).await;
    Ok(Settled {
        tx,
        block,
        sent_at,
        included_at,
        confirmed_at,
        finality,
        finalized_seen,
    })
}

/// Poll `finalized` until it reaches `block` or [`FINALITY_CAP`] runs out.
/// Twice a second for the first half minute (a BFT chain answers at once),
/// then every five seconds (a rollup's answer is minutes away).
async fn wait_final(c: &ArcClient, block: u64) -> (Option<i64>, FinalityKind, Option<u64>) {
    let start = Instant::now();
    let mut seen = None;
    loop {
        match c
            .provider
            .get_block_by_number(BlockNumberOrTag::Finalized)
            .await
        {
            Ok(Some(b)) => {
                seen = Some(b.header.number);
                if b.header.number >= block {
                    return (Some(now_ms()), FinalityKind::ChainFinalTag, seen);
                }
            }
            // No finalized block, or the tag refused past the client's
            // retries: stop waiting and say what we have.
            _ => break,
        }
        if start.elapsed() >= FINALITY_CAP {
            break;
        }
        let pause = if start.elapsed() < Duration::from_secs(30) {
            Duration::from_millis(500)
        } else {
            Duration::from_secs(5)
        };
        tokio::time::sleep(pause).await;
    }
    let finality = match (seen, c.block_number().await) {
        (Some(_), Ok(head)) => FinalityKind::Confirmations(head.saturating_sub(block)),
        _ => FinalityKind::Receipt,
    };
    (None, finality, seen)
}
