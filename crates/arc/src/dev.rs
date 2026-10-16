//! Test-chain helpers: advancing the clock, minting a stand-in token.
//!
//! Feature-gated behind `dev-anvil` and never compiled into a build that
//! touches a real chain. These call methods no real node implements --
//! `evm_increaseTime` does not exist on Arc, and it would be alarming if it
//! did.
//!
//! # Why a test chain is needed at all
//!
//! The obligation model recognises a subscription by seeing the same payee
//! paid a similar amount at a regular interval, at least three times. On a
//! real chain, establishing six months of monthly history takes six months.
//! On anvil the clock is a parameter, so it takes six seconds.
//!
//! The history produced this way is not mocked. Each payment is a real
//! transaction that is mined, produces a receipt, and emits a log. The agent
//! then rediscovers it through `eth_getLogs`, exactly as it would on mainnet,
//! and has no way to tell that the six months were compressed. What is fake
//! is the money and the passage of time -- not the transactions.
//!
//! # Why a stand-in token
//!
//! On Arc, USDC is the native asset and its movements emit ERC-20 `Transfer`
//! events. A plain anvil emits nothing for a native transfer, so there would
//! be no history to read back. `TestUSDC` exposes the identical surface, so
//! the agent runs the same code path and only the token address changes. An
//! anvil *fork* of Arc carries the real precompile but cannot execute its
//! `transfer`, because that logic lives in the Arc client rather than in EVM
//! bytecode.

use crate::{ArcError, IUsdc};
use alloy::primitives::{Address, U256};
use alloy::providers::ext::AnvilApi;
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use ledger::{Currency, Money};

/// A handle on a test chain, able to do things no real chain allows.
pub struct TestChain {
    provider: DynProvider,
}

impl TestChain {
    pub fn connect(rpc_url: &str, private_key: &str) -> Result<Self, ArcError> {
        let signer: PrivateKeySigner = private_key.trim().parse().map_err(|_| ArcError::BadKey)?;
        let url = rpc_url
            .parse()
            .map_err(|e| ArcError::Transport(format!("{e}")))?;
        Ok(TestChain {
            provider: ProviderBuilder::new()
                .wallet(signer)
                .connect_http(url)
                .erased(),
        })
    }

    /// Move the chain clock forward and mine, so history has real spacing.
    ///
    /// Both calls are required: `evm_increaseTime` only changes what the
    /// *next* block will claim, so without mining, nothing has happened yet.
    pub async fn advance_days(&self, days: u64) -> Result<(), ArcError> {
        self.provider
            .anvil_increase_time(days * 86_400)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?;
        self.provider
            .anvil_mine(Some(1), None)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?;
        Ok(())
    }

    /// Give an address native balance outright.
    pub async fn fund(&self, who: Address, amount: U256) -> Result<(), ArcError> {
        self.provider
            .anvil_set_balance(who, amount)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))
    }

    /// Mint stand-in USDC. `TestUSDC` has no access control on mint by
    /// design: it exists to fund wallets on a throwaway chain.
    pub async fn mint(&self, token: Address, to: Address, amount: Money) -> Result<(), ArcError> {
        let minor = amount.to_minor();
        if minor < 0 {
            return Err(ArcError::AmountOutOfRange(minor));
        }
        let t = ITestUsdc::new(token, &self.provider);
        t.mint(to, U256::from(minor as u128))
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .get_receipt()
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?;
        Ok(())
    }

    /// Send stand-in USDC, producing a real transfer with a real log.
    pub async fn transfer(
        &self,
        token: Address,
        to: Address,
        amount: Money,
    ) -> Result<String, ArcError> {
        let minor = amount.to_minor();
        if minor <= 0 {
            return Err(ArcError::AmountOutOfRange(minor));
        }
        let t = IUsdc::new(token, &self.provider);
        let receipt = t
            .transfer(to, U256::from(minor as u128))
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .get_receipt()
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?;
        Ok(format!("{:#x}", receipt.transaction_hash))
    }

    pub async fn balance_of(&self, token: Address, who: Address) -> Result<Money, ArcError> {
        let t = IUsdc::new(token, &self.provider);
        let raw = t
            .balanceOf(who)
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?;
        Ok(Money::minor(
            i128::try_from(raw).unwrap_or(i128::MAX),
            Currency::Usdc,
        ))
    }

    /// Current chain time, which on a test chain is whatever we set it to.
    pub async fn now(&self) -> Result<ledger::Timestamp, ArcError> {
        use alloy::rpc::types::{BlockId, BlockNumberOrTag};
        let b = self
            .provider
            .get_block(BlockId::Number(BlockNumberOrTag::Latest))
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?
            .ok_or(ArcError::MissingBlock(0))?;
        Ok(ledger::Timestamp::from_unix(b.header.timestamp as i64))
    }
}

alloy::sol! {
    #[sol(rpc)]
    interface ITestUsdc {
        function mint(address to, uint256 amount) external;
    }
}
