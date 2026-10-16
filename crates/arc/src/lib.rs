//! Reading and moving USDC on Arc.
//!
//! Arc settles in USDC and charges gas in USDC too, so there is no second
//! token to hold and no gas balance to top up separately. That is what makes
//! an agent paying forty small invoices a month affordable rather than absurd.
//!
//! # What lives here and what does not
//!
//! This crate is the only place that talks to a chain. Timeouts, retries,
//! reorgs, an RPC that answers slowly or wrongly: all of it is confined here.
//! [`ledger`] performs no I/O at all, so a transport bug cannot reach the code
//! that decides whether money should move.
//!
//! The boundary runs in one direction. This crate reads the chain and hands
//! [`ledger`] types across -- [`ledger::Transfer`], [`ledger::Money`] -- and
//! takes decisions back. It never decides anything itself.
//!
//! # Two scales, one conversion
//!
//! On Arc, USDC is the native gas and settlement asset. Native wei divided by
//! `10^12` gives USDC minor units, because native carries eighteen decimals
//! and USDC carries six. Every conversion between the two goes through
//! [`usdc_from_wei`] and [`wei_from_usdc`] and nowhere else.

#![forbid(unsafe_code)]

#[cfg(feature = "dev-anvil")]
pub mod dev;

use alloy::primitives::{Address, B256, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::{BlockId, BlockNumberOrTag, Filter};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use ledger::{Counterparty, Currency, Direction, Money, Timestamp, Transfer};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

/// Arc testnet. Verified live: `eth_chainId` returns `0x4cef52`.
pub const ARC_TESTNET_RPC: &str = "https://rpc.testnet.arc.network";
pub const ARC_TESTNET_CHAIN_ID: u64 = 5_042_002;

/// USDC on Arc: both the settlement asset and the gas token.
/// Verified live: `symbol()` returns `USDC`, `decimals()` returns `6`.
pub const USDC_ADDRESS: Address =
    Address::new(hex_literal(b"3600000000000000000000000000000000000000"));

/// Where Arc reports **every** USDC movement, native transfers included.
///
/// Measured against live Arc testnet on 2026-09-20, and it corrects an
/// assumption that would have broken the first real run:
///
/// | emitter | fires on | amount scale |
/// |---|---|---|
/// | `0xffff…fffe` | every USDC movement, native **and** ERC-20 | wei, 18 dp |
/// | `0x3600…0000` | ERC-20 calls only | minor units, 6 dp |
///
/// A plain value transfer emits from `0xffff…fffe` and **not** from the
/// precompile. `BudgetPolicy.pay` sends native value, so an agent watching
/// only the precompile would never see its own payments — it would forget
/// every bill it had ever settled and pay them all again.
///
/// Both emitters were confirmed to report the same transfer, differing by
/// exactly `10^12`, over seven sampled ERC-20 calls.
pub const NATIVE_TRANSFER_EMITTER: Address =
    Address::new(hex_literal(b"fffffffffffffffffffffffffffffffffffffffe"));

/// ERC-8004 IdentityRegistry on Arc testnet. Verified live: has deployed code.
pub const IDENTITY_REGISTRY: Address =
    Address::new(hex_literal(b"8004A818BFB912233c491871b3d84c89A494BD9e"));

/// How many times to retry a rate-limited request before giving up.
///
/// Raised from 5 to 8 on 2026-09-25 after a live run against Arc testnet gave
/// up mid-scan. With the ceiling below, five retries wait 7.75 seconds in
/// total, which is less patience than a public testnet's rate limiter
/// routinely wants. Eight waits about a minute.
///
/// The cost of being wrong in each direction is asymmetric: waiting too long
/// delays a pass that runs on a cadence of minutes anyway, while giving up too
/// early abandons the whole history scan and the agent then decides from
/// nothing. There is no correctness argument for impatience here.
const RATE_LIMIT_RETRIES: u32 = 8;

/// Longest single pause between retries.
///
/// Was 4 seconds, which with five retries meant the backoff flattened almost
/// immediately and stopped being exponential in any useful sense.
const RATE_LIMIT_MAX_DELAY_MS: u64 = 16_000;

/// Blocks per `eth_getLogs` call, to start with.
///
/// Arc testnet refuses a wider window with "requested range too large", and
/// the width it accepts has moved: measured 2026-09-20, 1,000 blocks answered
/// and 2,000 were refused; measured 2026-10-01, 10,000 answered and 10,001
/// were refused. So this is a starting width, not a promise. On a refusal
/// [`ArcClient::transfers`] halves the window and retries, down to
/// [`MIN_LOG_CHUNK_BLOCKS`].
///
/// The width matters because the operator's scan grows with the chain: at
/// about 165,000 blocks a day, 1,000-block chunks hit the operator's 2,000-call
/// ceiling after about 12 days, which was going to stop the agent the day
/// before the Tameion deadline (docs/14). At 10,000 the same ceiling is about
/// 121 days.
pub const LOG_CHUNK_BLOCKS: u64 = 10_000;

/// The narrowest window [`ArcClient::transfers`] will halve down to before
/// reporting the refusal.
pub const MIN_LOG_CHUNK_BLOCKS: u64 = 250;

/// Native wei in one USDC minor unit.
///
/// Native has eighteen decimals, USDC has six, so `10^12` wei is the smallest
/// amount of USDC that exists. The mismatch between these two scales is the
/// defect class this project exists to remove; it is resolved in exactly two
/// functions and nowhere else.
pub const WEI_PER_USDC_UNIT: u128 = 1_000_000_000_000;

/// Parse a 40-character hex address at compile time.
const fn hex_literal(s: &[u8; 40]) -> [u8; 20] {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("not a hex digit"),
        }
    }
    let mut out = [0u8; 20];
    let mut i = 0;
    while i < 20 {
        out[i] = (nibble(s[i * 2]) << 4) | nibble(s[i * 2 + 1]);
        i += 1;
    }
    out
}

sol! {
    /// The subset of ERC-20 the agent uses. Arc's USDC answers all of it.
    #[sol(rpc)]
    interface IUsdc {
        function balanceOf(address owner) external view returns (uint256);
        function transfer(address to, uint256 value) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
        function approve(address spender, uint256 value) external returns (bool);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
        /// Circle's USDC blocklist (PLAN 5.47). Tokens without one revert.
        function isBlacklisted(address account) external view returns (bool);
        event Transfer(address indexed from, address indexed to, uint256 value);
    }
}

sol! {
    /// Spending limits, enforced where the agent cannot reach them.
    #[sol(rpc)]
    interface IBudgetPolicy {
        function pay(address to, uint256 usdcUnits, bytes32 actionHash) external;
        function withdraw(address to, uint256 usdcUnits) external;
        function settleAsPrincipal(address to, uint256 usdcUnits, bytes32 actionHash) external;
        function balanceUsdc() external view returns (uint256);
        function remainingAllowance() external view returns (uint256);
        function perActionCap() external view returns (uint256);
        function dailyCap() external view returns (uint256);
        function spentInWindow() external view returns (uint256);
        function executed(bytes32 actionHash) external view returns (bool);
        function attestedPayee(address payee) external view returns (bool);
        function attestedSince(address payee) external view returns (uint64);
        function reserveFloor() external view returns (uint256);
        function delay() external view returns (uint256);
        function mode() external view returns (uint8);
        function pending() external view returns (uint256, uint256, uint256, uint8, uint64, uint8);
        function setPayee(address payee, bool attested) external;
        function cancelPending() external;
        function setMode(uint8 next) external;
        function setLimits(uint256 perActionCap, uint256 dailyCap, uint256 reserveFloor) external;
        function applyPending() external;
        function setPayees(address[] payees, bool attested) external;
        function principal() external view returns (address);
        event Paid(address indexed to, uint256 usdcUnits, bytes32 indexed actionHash);
        event SettledByPrincipal(address indexed to, uint256 usdcUnits, bytes32 indexed actionHash);
        // Terms the owner's approval rests on (PLAN 5.23).
        function fingerprint() external view returns (bytes32);
        function agent() external view returns (address);
        function operator() external view returns (address);
        function settleAsPrincipalIf(address to, uint256 usdcUnits, bytes32 actionHash, bytes32 terms, uint64 deadline) external;
        function setPayeesIf(address[] payees, bool attested, bytes32 terms, uint64 deadline) external;
        function settleRunIf(address[] to, uint256[] usdcUnits, bytes32[] actionHashes, bytes32 terms, uint64 deadline) external returns (uint256 paid);
        // Agent authority that lapses (PLAN 5.24).
        function agentExpiry() external view returns (uint64);
        function renewAgent(uint64 until) external;
        event AgentRenewed(uint64 until, address by);
        // Standing orders (PLAN 5.25).
        function schedules(uint256 id) external view returns (address payee, uint256 amount, uint64 every, uint64 nextDue, bool active);
        function scheduleCount() external view returns (uint256);
        function scheduleActionHash(uint256 id, uint64 dueAt) external view returns (bytes32);
        function addSchedule(address payee, uint256 amount, uint64 every, uint64 firstDue) external returns (uint256);
        function removeSchedule(uint256 id) external;
        function payDue(uint256 id) external returns (bytes32);
        event PaidDue(uint256 indexed id, bytes32 indexed actionHash, uint64 dueAt, address caller);
        // Paid APIs inside the caps (PLAN 5.54; x402 by ERC-7710).
        function redeemDelegations(bytes[] permissionContexts, bytes32[] modes, bytes[] executionCallData) external;
        function purchaseDigest(address payTo, uint256 amount, bytes32 nonce, uint64 deadline) external view returns (bytes32);
        function usdcToken() external view returns (address);
    }
}

/// The EIP-712 digest the agent signs for one x402 purchase (PLAN 5.54) —
/// the formula of `BudgetPolicyBase.purchaseDigest`, domain ("Tameion", "1",
/// chain, policy).
pub fn purchase_digest(
    chain_id: u64,
    policy: Address,
    pay_to: Address,
    amount_minor: u128,
    nonce: B256,
    deadline: u64,
) -> B256 {
    use alloy::primitives::keccak256;
    use alloy::sol_types::SolValue;
    let domain = keccak256(
        (
            keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
            keccak256("Tameion"),
            keccak256("1"),
            U256::from(chain_id),
            policy,
        )
            .abi_encode(),
    );
    let body = keccak256(
        (
            keccak256("Purchase(address payTo,uint256 amount,bytes32 nonce,uint64 deadline)"),
            pay_to,
            U256::from(amount_minor),
            nonce,
            U256::from(deadline),
        )
            .abi_encode(),
    );
    keccak256([&[0x19u8, 0x01][..], domain.as_slice(), body.as_slice()].concat())
}

/// The ERC-7710 permission context of one purchase: `abi.encode(nonce,
/// deadline, signature)`, as `redeemDelegations` decodes it.
pub fn purchase_context(nonce: B256, deadline: u64, signature: &[u8]) -> Vec<u8> {
    use alloy::sol_types::SolValue;
    (
        nonce,
        U256::from(deadline),
        alloy::primitives::Bytes::copy_from_slice(signature),
    )
        .abi_encode_params()
}

/// The ERC-7579 single execution an x402 facilitator builds from the
/// seller's requirement: `usdc | 0 | transfer(payTo, amount)`.
pub fn purchase_execution(usdc: Address, pay_to: Address, amount_minor: u128) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    let mut v = usdc.to_vec();
    v.extend_from_slice(&[0u8; 32]);
    v.extend(
        IUsdc::transferCall {
            to: pay_to,
            value: U256::from(amount_minor),
        }
        .abi_encode(),
    );
    v
}

/// `redeemDelegations([context], [single-call mode], [execution])`.
pub fn redeem_calldata(context: Vec<u8>, execution: Vec<u8>) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    IBudgetPolicy::redeemDelegationsCall {
        permissionContexts: vec![context.into()],
        modes: vec![B256::ZERO],
        executionCallData: vec![execution.into()],
    }
    .abi_encode()
}

/// The terms an owner's approval rests on (PLAN 5.23), as read from the
/// policy, with the contract's own hash over them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terms {
    pub per_action_cap: U256,
    pub daily_cap: U256,
    pub reserve_floor: U256,
    pub mode: u8,
    pub agent: Address,
    pub operator: Address,
    pub delay: U256,
    pub agent_expiry: u64,
    pub pending_eta: u64,
    pub pending_kinds: u8,
    pub fingerprint: B256,
}

impl Terms {
    /// The same hash the contract computes: keccak256(abi.encode(perActionCap,
    /// dailyCap, reserveFloor, uint8 mode, agent, operator, delay, agentExpiry,
    /// pending.eta, pending.kinds)). Pure, so a test pins it against the chain.
    pub fn recompute(&self) -> B256 {
        // Ten static words, exactly abi.encode's layout for these types.
        let mut m = Vec::with_capacity(320);
        let word_u = |v: U256| v.to_be_bytes::<32>();
        let word_n = |v: u128| U256::from(v).to_be_bytes::<32>();
        m.extend_from_slice(&word_u(self.per_action_cap));
        m.extend_from_slice(&word_u(self.daily_cap));
        m.extend_from_slice(&word_u(self.reserve_floor));
        m.extend_from_slice(&word_n(self.mode as u128));
        m.extend_from_slice(B256::left_padding_from(self.agent.as_slice()).as_slice());
        m.extend_from_slice(B256::left_padding_from(self.operator.as_slice()).as_slice());
        m.extend_from_slice(&word_u(self.delay));
        m.extend_from_slice(&word_n(self.agent_expiry as u128));
        m.extend_from_slice(&word_n(self.pending_eta as u128));
        m.extend_from_slice(&word_n(self.pending_kinds as u128));
        alloy::primitives::keccak256(m)
    }
}

/// One payment the owner settled by hand, as the policy recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalSettlement {
    pub to: Address,
    pub amount: Money,
    pub action_hash: B256,
    pub tx: B256,
    pub block: u64,
}

/// One standing order as the contract holds it (PLAN 5.25).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub id: u64,
    pub payee: Address,
    pub amount: Money,
    pub every_secs: u64,
    pub next_due: u64,
    pub active: bool,
}

sol! {
    /// A principal whose only key is a P-256 passkey (`PasskeyPrincipal.sol`).
    /// Anyone may relay a call to it; only the passkey can authorise one.
    #[sol(rpc)]
    interface IPasskeyPrincipal {
        /// OpenZeppelin `WebAuthn.WebAuthnAuth`, field order as in the library.
        struct WebAuthnAuth {
            bytes32 r;
            bytes32 s;
            uint256 challengeIndex;
            uint256 typeIndex;
            bytes authenticatorData;
            string clientDataJSON;
        }
        function nonce() external view returns (uint256);
        function challengeFor(address target, uint256 value, bytes data, uint256 n) external view returns (bytes32);
        function execute(address target, uint256 value, bytes data, WebAuthnAuth auth) external returns (bytes);
        function qx() external view returns (bytes32);
        function qy() external view returns (bytes32);
        function setRecovery(address next) external;
        function recovery() external view returns (address);
        // Guardians (PLAN 5.30).
        function setGuardians(address[] list, uint8 threshold) external;
        function approveRecovery(bytes32 _qx, bytes32 _qy) external;
        function guardianCount() external view returns (uint256);
        function guardians(uint256 i) external view returns (address);
        function guardianThreshold() external view returns (uint8);
        function recoveryEpoch() external view returns (uint64);
        function pendingKey() external view returns (bytes32 qx, bytes32 qy, uint64 eta);
        event RecoveryProposed(bytes32 qx, bytes32 qy, uint64 eta, address indexed by);
        // Co-owners and M-of-N approvals (PLAN 5.7).
        function setCoOwners(bytes32[] xs, bytes32[] ys, uint8 threshold) external;
        function coOwnerCount() external view returns (uint256);
        function keyAt(uint256 i) external view returns (bytes32, bytes32);
        function approvalThreshold() external view returns (uint8);
        function executeAs(uint256 keyIndex, address target, uint256 value, bytes data, WebAuthnAuth auth) external returns (bytes);
        function executeWith(address target, uint256 value, bytes data, uint256[] keyIndexes, WebAuthnAuth[] auths) external returns (bytes);
    }
}

/// The contract's form of a passkey assertion.
fn web_authn(auth: &PasskeyAuth) -> IPasskeyPrincipal::WebAuthnAuth {
    IPasskeyPrincipal::WebAuthnAuth {
        r: auth.r,
        s: auth.s,
        challengeIndex: auth.challenge_index,
        typeIndex: auth.type_index,
        authenticatorData: auth.authenticator_data.clone().into(),
        clientDataJSON: auth.client_data_json.clone(),
    }
}

/// `setCoOwners(xs, ys, threshold)` on the owner contract, for its passkey(s).
pub fn set_co_owners_calldata(keys: &[(B256, B256)], threshold: u8) -> String {
    use alloy::sol_types::SolCall;
    let call = IPasskeyPrincipal::setCoOwnersCall {
        xs: keys.iter().map(|k| k.0).collect(),
        ys: keys.iter().map(|k| k.1).collect(),
        threshold,
    };
    format!("0x{}", alloy::hex::encode(call.abi_encode()))
}

/// A WebAuthn assertion as the signing page produces it (`apps/classic/passkey`,
/// `apps/classic/level1.html`): the same six fields the contract verifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasskeyAuth {
    pub r: B256,
    pub s: B256,
    pub challenge_index: U256,
    pub type_index: U256,
    pub authenticator_data: Vec<u8>,
    pub client_data_json: String,
}

sol! {
    /// Commits a root of the decision log on chain.
    #[sol(rpc)]
    interface IMemoryAnchor {
        function anchor(bytes32 root) external;
        function rootOf(address agent) external view returns (bytes32);
    }
}

/// Anything that went wrong talking to a chain.
///
/// Never carries key material: a signer is held by value and its bytes are
/// never formatted into an error.
#[derive(Debug, thiserror::Error)]
pub enum ArcError {
    #[error("rpc transport: {0}")]
    Transport(String),
    #[error("contract call: {0}")]
    Contract(String),
    #[error("bad private key")]
    BadKey,
    #[error("no signer configured; build the client with `with_signer`")]
    NoSigner,
    #[error("block {0} has no timestamp")]
    MissingBlock(u64),
    #[error("amount {0} does not fit in a uint256")]
    AmountOutOfRange(i128),
    #[error("chain reported id {found}, expected {expected}")]
    WrongChain { found: u64, expected: u64 },
    #[error("log query for blocks {from}..={to} was refused: {detail}")]
    RangeTooWide { from: u64, to: u64, detail: String },
    /// The endpoint throttled us past the retry budget.
    ///
    /// The message names the remedy because the usual cause is not load but
    /// range: a treasury whose `history_from_block` was set at deployment
    /// rescans every block since, so the scan grows by roughly a day of chain
    /// per day elapsed and eventually cannot finish. Raising
    /// `Config::history_from_block` to a later block is the fix, at the cost
    /// of no longer seeing obligations whose evidence lies before it.
    #[error(
        "rate limited after {attempts} attempts on blocks {from}..={to}. \
         If this is a wide scan rather than a busy endpoint, raise \
         Config::history_from_block -- but note that obligations whose \
         evidence lies before the new start block stop being detected"
    )]
    RateLimited { from: u64, to: u64, attempts: u32 },
}

/// USDC minor units from native wei, truncating anything below one minor unit.
///
/// Truncation is correct here and is not a rounding choice: a fraction of a
/// minor unit is not an amount of USDC. It exists on the native scale and has
/// no representation on the settlement scale.
/// `BudgetPolicyBase.Mode`. Below Autopilot the contract refuses `pay`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PolicyMode {
    Watch,
    Propose,
    Autopilot,
}

impl PolicyMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => PolicyMode::Watch,
            1 => PolicyMode::Propose,
            _ => PolicyMode::Autopilot,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PolicyMode::Watch => "Watch",
            PolicyMode::Propose => "Propose",
            PolicyMode::Autopilot => "Autopilot",
        }
    }
}

/// A trust-expanding change waiting out the policy's delay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChange {
    /// bit 0 = limits, bit 1 = mode
    pub kinds: u8,
    pub per_action_cap: Money,
    pub daily_cap: Money,
    pub reserve_floor: Money,
    pub mode: PolicyMode,
    pub eta_unix: u64,
}

/// What the contract says about who may do what, and when (docs/46).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Governance {
    pub mode: PolicyMode,
    pub delay_secs: u64,
    pub reserve_floor: Money,
    pub pending: Option<PendingChange>,
}

pub fn usdc_from_wei(wei: U256) -> Money {
    let units = wei / U256::from(WEI_PER_USDC_UNIT);
    // A balance larger than i128::MAX minor units is ~1.7e32 USDC and cannot
    // arise; saturating keeps this total rather than panicking if it did.
    let minor = i128::try_from(units).unwrap_or(i128::MAX);
    Money::minor(minor, Currency::Usdc)
}

/// Native wei from USDC minor units. Exact: every minor unit is `10^12` wei.
pub fn wei_from_usdc(amount: Money) -> Result<U256, ArcError> {
    let minor = amount.to_minor();
    if minor < 0 {
        return Err(ArcError::AmountOutOfRange(minor));
    }
    Ok(U256::from(minor as u128) * U256::from(WEI_PER_USDC_UNIT))
}

/// Where transfer history is read from, and at which scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferSource {
    /// Arc's complete view. Every USDC movement, amounts in wei.
    ArcNative,
    /// One ERC-20 contract, amounts in that token's own units.
    ///
    /// For a stand-in token on a chain that is not Arc, where a native
    /// transfer emits nothing at all.
    Token(Address),
}

impl TransferSource {
    fn emitter(self) -> Address {
        match self {
            TransferSource::ArcNative => NATIVE_TRANSFER_EMITTER,
            TransferSource::Token(a) => a,
        }
    }

    /// Turn a raw log amount into `Money`.
    fn to_money(self, raw: U256) -> Money {
        match self {
            // 18 decimals on the native scale.
            TransferSource::ArcNative => usdc_from_wei(raw),
            // Already minor units.
            TransferSource::Token(_) => {
                Money::minor(i128::try_from(raw).unwrap_or(i128::MAX), Currency::Usdc)
            }
        }
    }
}

/// Whether a read failed because the contract reverted -- on an older build
/// the function is not there -- rather than because no answer came back.
/// Only a revert may be read as "this policy predates it"; a dropped
/// connection, a throttle or a node error is a read that failed.
pub fn is_revert(e: &ArcError) -> bool {
    matches!(e, ArcError::Contract(m) if m.to_ascii_lowercase().contains("revert"))
}

/// JSON-RPC error code for "method not supported". Arc testnet's public
/// endpoint returns it intermittently for methods it does support -- the
/// request reached a backend that lacked the method -- which means the method
/// was not executed and repeating the request is safe, even for
/// `eth_sendRawTransaction`: the signed bytes are identical.
const METHOD_NOT_SUPPORTED: i64 = -32601;

/// An HTTP client that retries what alloy already treats as retryable (rate
/// limits) and, on top, exactly `-32601`. Nothing else is retried here: a
/// request a node may have executed is not repeated blindly.
///
/// On 2026-10-02 a pass on Arc paid two bills and then hit `-32601` on a later
/// read; the pass errored and its decision log, the product, was never
/// anchored. The payments were right and the retry that followed paid nothing
/// twice -- but a record lost to a transient routing error is not acceptable.
/// A failure of the wire, not of the request: the same call a moment later
/// usually succeeds. Public endpoints drop connections under load, and a
/// scan of a million blocks makes hundreds of calls, so one dropped
/// connection must not end it (seen 2026-10-13: "error sending request for
/// url" at block 66,127,191 of a 1.2 M-block scan).
pub fn transient_transport_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    [
        "error sending request",
        "timed out",
        "timeout",
        "connection",
        "reset by peer",
        "unexpected eof",
        "bad gateway",
        "service unavailable",
        "gateway timeout",
    ]
    .iter()
    .any(|needle| m.contains(needle))
}

/// Public endpoints answer a burst with their own codes rather than HTTP
/// 429: Arc's -32005 "rate limit exceeded", Base's -32011 "request limit
/// reached". Treat any error that says "limit" as something to wait out,
/// and a dropped connection or a timed-out request the same way.
fn worth_retrying(e: &alloy::transports::TransportError) -> bool {
    e.as_error_resp().is_some_and(|p| {
        p.code == METHOD_NOT_SUPPORTED
            || p.code == -32005
            || p.code == -32011
            || p.message.to_ascii_lowercase().contains("limit")
    }) || transient_transport_error(&e.to_string())
}

fn retrying_client(url: alloy::transports::http::reqwest::Url) -> alloy::rpc::client::RpcClient {
    use alloy::transports::layers::{RateLimitRetryPolicy, RetryBackoffLayer};
    let policy = RateLimitRetryPolicy::default().or(worth_retrying);
    alloy::rpc::client::ClientBuilder::default()
        // The public Arc testnet RPC answers -32005 "rate limit exceeded" to a
        // history scan followed by writes (seen 2026-10-13). Twelve tries with
        // a two-second initial backoff rides it out; the cost is patience.
        .layer(RetryBackoffLayer::new_with_policy(12, 2_000, 1_000, policy))
        .http(url)
        // alloy waits 7 s between receipt polls on a remote endpoint; Arc and
        // Arbitrum make a block in under a second, so every send (an owner's
        // tap, a deploy, an anchor) sat ~7 s on a receipt that was already
        // there (docs/60 P-43: a first check after funding took 67 s).
        .with_poll_interval(std::time::Duration::from_millis(500))
}

/// A client for one Arc endpoint.
#[derive(Clone)]
pub struct ArcClient {
    provider: DynProvider,
    signer_address: Option<Address>,
    usdc: Address,
    source: TransferSource,
    /// First `eth_getLogs` window, in blocks (see [`ArcClient::with_log_chunk`]).
    log_chunk: u64,
    /// This sender's turn on this endpoint (see [`send_turn`]); `None` when
    /// the client cannot sign.
    turn: Option<Turn>,
}

/// A sender's turn to send on one endpoint.
type Turn = Arc<tokio::sync::Mutex<()>>;

/// One turn per (endpoint, sender) for the whole process. The service builds
/// a fresh client per operation and signs with one relayer key for every
/// tenant (deploys, gas top-ups, mints, keyless anchors), three passes at a
/// time; each client read the same pending nonce and all but one send failed
/// with -32003 "replacement transaction underpriced" (30 of 32 on anvil,
/// `tests/nonce.rs`; docs/60). A write holds the turn from filling the nonce
/// to its receipt, so the next one reads a nonce the chain has counted.
// ponytail: one send per sender at a time (a block each); a nonce allocator
// with gap repair if one key ever needs more than that.
fn send_turn(rpc_url: &str, who: Address) -> Turn {
    static TURNS: OnceLock<StdMutex<HashMap<(String, Address), Turn>>> = OnceLock::new();
    let mut turns = TURNS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    Arc::clone(turns.entry((rpc_url.to_string(), who)).or_default())
}

/// `read` over every item, eight at a time: enough overlap to hide a public
/// node's ~0.3 s round trip, few enough to stay under its rate limit (docs/60
/// P-12, P-42; docs/68 F4). Results come back in `items` order.
pub async fn eight_at_a_time<T, R, F, Fut>(items: Vec<T>, read: F) -> Result<Vec<R>, ArcError>
where
    F: Fn(T) -> Fut,
    Fut: std::future::Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let mut out = Vec::with_capacity(items.len());
    let mut items = items.into_iter().peekable();
    while items.peek().is_some() {
        let mut set = tokio::task::JoinSet::new();
        for (i, item) in items.by_ref().take(8).enumerate() {
            let read = read(item);
            set.spawn(async move { (i, read.await) });
        }
        let mut batch = Vec::with_capacity(8);
        while let Some(r) = set.join_next().await {
            batch.push(r.map_err(|e| ArcError::Transport(e.to_string()))?);
        }
        batch.sort_by_key(|(i, _)| *i);
        out.extend(batch.into_iter().map(|(_, r)| r));
    }
    Ok(out)
}

/// How long a write waits for its receipt while holding the sender's turn.
pub const RECEIPT_WAIT: std::time::Duration = std::time::Duration::from_secs(90);

/// A receipt, waited for at most [`RECEIPT_WAIT`]. Every write holds its
/// sender's turn until this returns; a transaction the node drops would
/// otherwise hold it forever and stop every later write of that key in the
/// process — passkey taps, deploys, anchors (code review, 2026-10-15). A plain
/// timer, not alloy's own timeout: that one is checked when blocks arrive.
trait Confirmed {
    async fn confirmed(self) -> Result<alloy::rpc::types::TransactionReceipt, ArcError>;
}

impl Confirmed for alloy::providers::PendingTransactionBuilder<alloy::network::Ethereum> {
    async fn confirmed(self) -> Result<alloy::rpc::types::TransactionReceipt, ArcError> {
        let hash = *self.tx_hash();
        match tokio::time::timeout(RECEIPT_WAIT, self.get_receipt()).await {
            // The hash is kept in the error: the transaction may have landed (docs/96 V8 (c)).
            Ok(r) => r.map_err(|e| {
                ArcError::Transport(format!(
                    "transaction {hash:#x} was sent; reading its receipt failed ({e}); check it on the explorer before sending again"
                ))
            }),
            Err(_) => Err(ArcError::Transport(format!(
                "transaction {hash:#x} was sent but not confirmed within {} s; check it on the explorer before sending again",
                RECEIPT_WAIT.as_secs()
            ))),
        }
    }
}

impl ArcClient {
    /// Read-only, for a person waiting on a page: a three-second cap per
    /// request and three quick retries (a stalled request is sent again)
    /// instead of the patient client's twelve two-second backoffs, so one
    /// stalled or rate-limited read costs about three seconds, not a minute,
    /// and is still answered (docs/110 item 8: the inbox took 38-62 s when a
    /// public node stalled one read). Passes keep [`ArcClient::read_only`]'s patience.
    pub fn read_only_quick(rpc_url: &str) -> Result<Self, ArcError> {
        use alloy::transports::layers::{RateLimitRetryPolicy, RetryBackoffLayer};
        let url: alloy::transports::http::reqwest::Url = rpc_url
            .parse()
            .map_err(|e| ArcError::Transport(format!("{e}")))?;
        // A connection to the public node sometimes stalls in its TCP connect
        // (7.6 s in 2 of 100 fresh curl requests to Arbitrum Sepolia's RPC,
        // 2026-10-16); cut it at one second and connect again.
        let http = alloy::transports::http::reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(1))
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .map_err(|e| ArcError::Transport(e.to_string()))?;
        let client = alloy::rpc::client::ClientBuilder::default()
            .layer(RetryBackoffLayer::new_with_policy(
                3,
                250,
                1_000,
                RateLimitRetryPolicy::default().or(worth_retrying),
            ))
            .transport(alloy::transports::http::Http::with_client(http, url), false);
        Ok(ArcClient {
            provider: ProviderBuilder::new().connect_client(client).erased(),
            signer_address: None,
            usdc: USDC_ADDRESS,
            source: TransferSource::ArcNative,
            log_chunk: LOG_CHUNK_BLOCKS,
            turn: None,
        })
    }

    /// Read-only. Cannot send a transaction; the type says so and
    /// [`ArcError::NoSigner`] says so at runtime if a write is attempted.
    pub fn read_only(rpc_url: &str) -> Result<Self, ArcError> {
        let url = rpc_url
            .parse()
            .map_err(|e| ArcError::Transport(format!("{e}")))?;
        Ok(ArcClient {
            provider: ProviderBuilder::new()
                .connect_client(retrying_client(url))
                .erased(),
            signer_address: None,
            usdc: USDC_ADDRESS,
            source: TransferSource::ArcNative,
            log_chunk: LOG_CHUNK_BLOCKS,
            turn: None,
        })
    }

    /// Able to sign and send.
    ///
    /// Signing happens in this process. `cast send --private-key` places the
    /// key in `argv`, where any local user can read it out of `ps` for the
    /// lifetime of the subprocess; building and signing here means the key
    /// never reaches an argument list or a child process environment.
    pub fn with_signer(rpc_url: &str, private_key: &str) -> Result<Self, ArcError> {
        let signer: PrivateKeySigner = private_key.trim().parse().map_err(|_| ArcError::BadKey)?;
        let address = signer.address();
        let url = rpc_url
            .parse()
            .map_err(|e| ArcError::Transport(format!("{e}")))?;
        Ok(ArcClient {
            // The nonce is read from the node on every send (no cache in the
            // client): with the sender's turn held to the receipt, the node's
            // pending count is the next free nonce, and a send that failed
            // after taking one leaves no gap behind.
            provider: ProviderBuilder::new()
                .disable_recommended_fillers()
                .with_gas_estimation()
                .with_simple_nonce_management()
                .fetch_chain_id()
                .wallet(signer)
                .connect_client(retrying_client(url))
                .erased(),
            signer_address: Some(address),
            usdc: USDC_ADDRESS,
            source: TransferSource::ArcNative,
            log_chunk: LOG_CHUNK_BLOCKS,
            turn: Some(send_turn(rpc_url, address)),
        })
    }

    /// Point at a stand-in token instead of the USDC precompile.
    ///
    /// An anvil fork of Arc can read the precompile's state but cannot execute
    /// its `transfer`, because that logic lives in the Arc client rather than
    /// in EVM bytecode. On a fork, a deployed stand-in exposes the identical
    /// surface so the agent runs the same code path and only the address
    /// changes.
    pub fn with_token(mut self, token: Address) -> Self {
        self.usdc = token;
        self.source = TransferSource::Token(token);
        self
    }

    /// Start log walks at `blocks` per call instead of [`LOG_CHUNK_BLOCKS`].
    /// The width a node accepts is the node's: Arbitrum's public endpoint
    /// takes 10,000,000 blocks and answers a wallet-filtered 5,000,000 in
    /// 0.6 s (measured 2026-10-14), so walking it 10,000 at a time turned a
    /// 90-day first read into 3,000 round trips and an hour (docs/60 P-38).
    /// A refusal still halves the window, so a guess that is too wide costs
    /// one round trip.
    pub fn with_log_chunk(mut self, blocks: u64) -> Self {
        self.log_chunk = blocks.max(MIN_LOG_CHUNK_BLOCKS);
        self
    }

    /// Wait for this sender's turn ([`send_turn`]); held until dropped.
    async fn turn(&self) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        Some(Arc::clone(self.turn.as_ref()?).lock_owned().await)
    }

    /// Where history is read from.
    pub fn source(&self) -> TransferSource {
        self.source
    }

    pub fn address(&self) -> Option<Address> {
        self.signer_address
    }

    pub fn token(&self) -> Address {
        self.usdc
    }

    /// Fail early if pointed at the wrong network.
    pub async fn expect_chain(&self, expected: u64) -> Result<(), ArcError> {
        let found = self.chain_id().await?;
        if found == expected {
            Ok(())
        } else {
            Err(ArcError::WrongChain { found, expected })
        }
    }

    pub async fn chain_id(&self) -> Result<u64, ArcError> {
        self.provider
            .get_chain_id()
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))
    }

    pub async fn block_number(&self) -> Result<u64, ArcError> {
        self.provider
            .get_block_number()
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))
    }

    /// Timestamp of a block, as a [`Timestamp`].
    pub async fn block_timestamp(&self, block: u64) -> Result<Timestamp, ArcError> {
        let b = self
            .provider
            .get_block(BlockId::Number(BlockNumberOrTag::Number(block)))
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?
            .ok_or(ArcError::MissingBlock(block))?;
        Ok(Timestamp::from_unix(b.header.timestamp as i64))
    }

    /// Native balance in wei, for chains whose native asset is not USDC
    /// (ETH on Arbitrum, Base, Ethereum): the hedge's exposure input.
    pub async fn native_wei(&self, who: Address) -> Result<U256, ArcError> {
        self.provider
            .get_balance(who)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))
    }

    /// The code at `at` (empty for a plain account; `0xef0100 ‖ address` for an
    /// account that delegated by EIP-7702).
    pub async fn code_at(&self, at: Address) -> Result<Vec<u8>, ArcError> {
        self.provider
            .get_code_at(at)
            .await
            .map(|c| c.to_vec())
            .map_err(|e| ArcError::Transport(e.to_string()))
    }

    /// Whether the code at `at` carries `selector` in its dispatcher (solc
    /// writes each external selector as a PUSH4 constant). Used to offer a
    /// feature only to policies deployed after it; false on any error.
    pub async fn code_has_selector(&self, at: Address, selector: [u8; 4]) -> bool {
        match self.provider.get_code_at(at).await {
            Ok(code) => code.windows(4).any(|w| w == selector),
            Err(_) => false,
        }
    }

    /// Native balance, read as USDC.
    pub async fn native_balance(&self, who: Address) -> Result<Money, ArcError> {
        let wei = self
            .provider
            .get_balance(who)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?;
        Ok(usdc_from_wei(wei))
    }

    /// ERC-20 balance from the USDC contract (or the configured stand-in).
    pub async fn usdc_balance(&self, who: Address) -> Result<Money, ArcError> {
        let usdc = IUsdc::new(self.usdc, &self.provider);
        let raw = usdc
            .balanceOf(who)
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?;
        let minor = i128::try_from(raw).unwrap_or(i128::MAX);
        Ok(Money::minor(minor, Currency::Usdc))
    }

    /// Every term an approval rests on, as the contract holds them, plus the
    /// contract's own `fingerprint()` over them (PLAN 5.23). `None` on a policy
    /// from before fingerprints. The service hands the fields to the page so
    /// it can recompute the hash itself before the passkey signs.
    pub async fn terms(&self, policy: Address) -> Option<Terms> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        // Ten reads at once, not one after another (the inbox spent 1.6 s
        // here on a public node). They may straddle a block; the caller
        // checks `recompute() == fingerprint` before binding to them.
        let (
            fingerprint,
            per_action,
            daily,
            floor,
            mode,
            agent,
            operator,
            delay,
            agent_expiry,
            pending,
        ) = tokio::join!(
            async { p.fingerprint().call().await },
            async { p.perActionCap().call().await },
            async { p.dailyCap().call().await },
            async { p.reserveFloor().call().await },
            async { p.mode().call().await },
            async { p.agent().call().await },
            async { p.operator().call().await },
            async { p.delay().call().await },
            async { p.agentExpiry().call().await },
            async { p.pending().call().await },
        );
        let (
            fingerprint,
            per_action,
            daily,
            floor,
            mode,
            agent,
            operator,
            delay,
            agent_expiry,
            pending,
        ) = (
            fingerprint.ok()?,
            per_action.ok()?,
            daily.ok()?,
            floor.ok()?,
            mode.ok()?,
            agent.ok()?,
            operator.ok()?,
            delay.ok()?,
            agent_expiry.ok()?,
            pending.ok()?,
        );
        Some(Terms {
            per_action_cap: per_action,
            daily_cap: daily,
            reserve_floor: floor,
            mode,
            agent,
            operator,
            delay,
            agent_expiry,
            pending_eta: pending._4,
            pending_kinds: pending._5,
            fingerprint,
        })
    }

    /// When the agent's authority to pay lapses (PLAN 5.24); `None` on a
    /// policy from before expiries existed.
    pub async fn agent_expiry(&self, policy: Address) -> Option<u64> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        p.agentExpiry().call().await.ok()
    }

    /// Every standing order of `policy`, ended ones included (PLAN 5.25). A
    /// policy from before standing orders has no `scheduleCount()`: the call
    /// reverts, and that reads as none. Any other failure is an error: an
    /// empty list read off a failed call let the agent pay a bill a standing
    /// order also pays (audit 2026-10-15, docs/60 P-74).
    pub async fn schedules(&self, policy: Address) -> Result<Vec<Schedule>, ArcError> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        let n = match p.scheduleCount().call().await {
            Ok(n) => n,
            Err(e) => {
                let e = ArcError::Contract(e.to_string());
                return if is_revert(&e) {
                    Ok(Vec::new())
                } else {
                    Err(e)
                };
            }
        };
        let n = u64::try_from(n).unwrap_or(0).min(10_000);
        let rows = eight_at_a_time((0..n).collect(), |id| {
            let c = self.clone();
            async move {
                let p = IBudgetPolicy::new(policy, &c.provider);
                (id, p.schedules(U256::from(id)).call().await)
            }
        })
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, r) in rows {
            let r = r.map_err(|e| ArcError::Contract(e.to_string()))?;
            out.push(Schedule {
                id,
                payee: r.payee,
                amount: Money::minor(
                    i128::try_from(r.amount).unwrap_or(i128::MAX),
                    Currency::Usdc,
                ),
                every_secs: r.every,
                next_due: r.nextDue,
                active: r.active,
            });
        }
        Ok(out)
    }

    /// Pay one due cycle of a standing order (anyone may; this client pays the
    /// gas). Returns the transaction; the `Paid` event carries
    /// `scheduleActionHash(id, dueAt)`, which `confirm_paid` checks.
    pub async fn pay_due(&self, policy: Address, id: u64) -> Result<B256, ArcError> {
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let p = IBudgetPolicy::new(policy, &self.provider);
        let receipt = p
            .payDue(U256::from(id))
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        if !receipt.status() {
            return Err(ArcError::Contract(format!(
                "payDue({id}) reverted in {:#x}",
                receipt.transaction_hash
            )));
        }
        Ok(receipt.transaction_hash)
    }

    /// The replay key one due date of a standing order will carry, read from
    /// the contract so the record and the chain agree by construction.
    pub async fn schedule_action_hash(
        &self,
        policy: Address,
        id: u64,
        due_at: u64,
    ) -> Result<B256, ArcError> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        p.scheduleActionHash(U256::from(id), due_at)
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))
    }

    /// What `owner` lets `spender` pull of its USDC (PLAN 5.26, pull mode):
    /// the second cap the treasurer holds in their own wallet.
    pub async fn usdc_allowance(
        &self,
        owner: Address,
        spender: Address,
    ) -> Result<Money, ArcError> {
        let usdc = IUsdc::new(self.usdc, &self.provider);
        let raw = usdc
            .allowance(owner, spender)
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?;
        Ok(Money::minor(
            i128::try_from(raw).unwrap_or(i128::MAX),
            Currency::Usdc,
        ))
    }

    /// The USDC contract this client reads (Arc: the native facade at 0x3600…).
    pub fn usdc_address(&self) -> Address {
        self.usdc
    }

    /// Every log matching any of `filters` in `[from_block, to_block]`.
    ///
    /// Walks the range in chunks. A node will not answer an arbitrarily wide
    /// window, and the width it accepts is a property of the node rather than
    /// of the request, so the client has to do the walking. On a refusal the
    /// chunk is halved and the same start block asked again; nothing from a
    /// refused chunk is kept.
    async fn walk_logs(
        &self,
        filters: &[Filter],
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<alloy::rpc::types::Log>, ArcError> {
        let mut logs = Vec::new();
        let mut chunk = self.log_chunk;
        let mut start = from_block;
        'walk: while start <= to_block {
            let end = start.saturating_add(chunk - 1).min(to_block);
            let mut got = Vec::new();
            // One filter after the other, not at once: the history scan is
            // paced for the public node's rate limit (docs/60 P-12), and two
            // filters together made Arc passes slower, not faster (docs/68 F4).
            for f in filters {
                let filter = f.clone().from_block(start).to_block(end);
                match self.get_logs_retrying(&filter, start, end).await {
                    Ok(found) => got.extend(found),
                    // A public node keeps a finite history: Arc's answers
                    // -32014 "requested data not available" for blocks it no
                    // longer serves (seen 2026-10-14, five days back). That is
                    // not a range too wide and not a blip: the history before
                    // the node's horizon is simply gone from this endpoint.
                    // Walk on; the window shrinks to what can be read.
                    Err(ArcError::RangeTooWide { detail, .. })
                        if detail.contains("-32014") || detail.contains("not available") =>
                    {
                        // Find the horizon once by bisection instead of
                        // walking every dead chunk: a 90-day window on Arc is
                        // 15M blocks, 1,500 refusals, and a rate-limited node
                        // for everyone else (seen 2026-10-14 on a new tenant).
                        // The search starts at the refused chunk itself: the
                        // horizon can fall inside it, and the blocks after it
                        // in that chunk are served (audit 2026-10-15).
                        let horizon = self.history_horizon(f, start, to_block).await?;
                        if horizon <= start {
                            // The probe was served where the chunk was not:
                            // this refusal is not the horizon. Never skip.
                            return Err(ArcError::RangeTooWide {
                                from: start,
                                to: end,
                                detail,
                            });
                        }
                        eprintln!(
                            "arc: blocks {start}..={} not served by this node ({}); continuing from {horizon}",
                            horizon.saturating_sub(1),
                            detail.trim()
                        );
                        start = horizon;
                        continue 'walk;
                    }
                    Err(ArcError::RangeTooWide { .. }) if chunk > MIN_LOG_CHUNK_BLOCKS => {
                        chunk = (chunk / 2).max(MIN_LOG_CHUNK_BLOCKS);
                        continue 'walk;
                    }
                    Err(e) => return Err(e),
                }
            }
            logs.extend(got);
            // Saturating, so a `to_block` at u64::MAX terminates instead of
            // wrapping to zero and starting over.
            let next = end.saturating_add(1);
            if next <= start {
                break;
            }
            start = next;
        }
        Ok(logs)
    }

    /// The first block in `[lo, hi]` this node still serves logs for, by
    /// bisection on one-block probes (about 24 round trips for 15M blocks).
    /// `hi + 1` when nothing in the range is served. A probe that fails for
    /// any other reason is an error: guessing past it skipped served blocks
    /// and returned a history with a hole in it (audit 2026-10-15).
    async fn history_horizon(&self, f: &Filter, mut lo: u64, hi: u64) -> Result<u64, ArcError> {
        let mut hi_ok = hi.saturating_add(1);
        while lo < hi_ok {
            let mid = lo + (hi_ok - lo) / 2;
            let probe = f.clone().from_block(mid).to_block(mid);
            match self.provider.get_logs(&probe).await {
                Ok(_) => hi_ok = mid,
                Err(e) => {
                    let d = e.to_string();
                    if d.contains("-32014") || d.contains("not available") {
                        lo = mid + 1;
                    } else {
                        return Err(ArcError::Transport(format!(
                            "finding the history horizon at block {mid}: {d}"
                        )));
                    }
                }
            }
        }
        Ok(lo)
    }

    /// Transactions in which the agent itself paid out of `policy`: the
    /// policy's `Paid` events.
    ///
    /// The operator needs these to keep its own payments out of the evidence
    /// it learns schedules from. An agent that learns from its own outputs
    /// moves its own schedule: pay a bill a day early and the next one is
    /// projected a day early, and with a look-ahead as long as the billing
    /// interval it pays the following month at once, then the one after.
    /// Found on 2026-10-02 by running two passes in a row on a local chain.
    pub async fn agent_payments(
        &self,
        policy: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<std::collections::BTreeSet<String>, ArcError> {
        let filter = Filter::new()
            .address(policy)
            .event("Paid(address,uint256,bytes32)");
        let logs = self.walk_logs(&[filter], from_block, to_block).await?;
        Ok(logs
            .into_iter()
            .filter_map(|l| l.transaction_hash.map(|h| format!("{h:#x}")))
            .collect())
    }

    /// The transaction that emitted `Paid(…, action_hash)` on `policy` in the
    /// last `lookback` blocks, if any: how the buyer finds a payment a
    /// seller made and did not report (PLAN 5.54).
    pub async fn paid_tx_for(
        &self,
        policy: Address,
        action_hash: B256,
        lookback: u64,
    ) -> Option<B256> {
        let head = self.block_number().await.ok()?;
        let filter = Filter::new()
            .address(policy)
            .event("Paid(address,uint256,bytes32)")
            .topic2(action_hash);
        let logs = self
            .walk_logs(&[filter], head.saturating_sub(lookback), head)
            .await
            .ok()?;
        logs.into_iter().find_map(|l| l.transaction_hash)
    }

    /// Payments the owner settled by hand (`SettledByPrincipal`), so the page
    /// counts them under Paid beside the agent's: who, how much, the action,
    /// the transaction, the block.
    pub async fn principal_settlements(
        &self,
        policy: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<PrincipalSettlement>, ArcError> {
        use alloy::sol_types::SolEvent;
        let filter = Filter::new()
            .address(policy)
            .event("SettledByPrincipal(address,uint256,bytes32)");
        let logs = self.walk_logs(&[filter], from_block, to_block).await?;
        let mut out = Vec::with_capacity(logs.len());
        for l in logs {
            let Ok(ev) = IBudgetPolicy::SettledByPrincipal::decode_log_data(&l.inner.data) else {
                continue;
            };
            out.push(PrincipalSettlement {
                to: ev.to,
                amount: Money::minor(
                    i128::try_from(ev.usdcUnits).unwrap_or(i128::MAX),
                    Currency::Usdc,
                ),
                action_hash: ev.actionHash,
                tx: l.transaction_hash.unwrap_or_default(),
                block: l.block_number.unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// The agent's payments as the policy recorded them (`Paid`): bills,
    /// purchases and standing orders alike, with the block each landed in.
    /// The account page reads these so a payment is shown even when the pass
    /// that made it failed afterwards and left no report (docs/60 P-35).
    pub async fn paid_events(
        &self,
        policy: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<PrincipalSettlement>, ArcError> {
        use alloy::sol_types::SolEvent;
        let filter = Filter::new()
            .address(policy)
            .event("Paid(address,uint256,bytes32)");
        let logs = self.walk_logs(&[filter], from_block, to_block).await?;
        let mut out = Vec::with_capacity(logs.len());
        for l in logs {
            let Ok(ev) = IBudgetPolicy::Paid::decode_log_data(&l.inner.data) else {
                continue;
            };
            out.push(PrincipalSettlement {
                to: ev.to,
                amount: Money::minor(
                    i128::try_from(ev.usdcUnits).unwrap_or(i128::MAX),
                    Currency::Usdc,
                ),
                action_hash: ev.actionHash,
                tx: l.transaction_hash.unwrap_or_default(),
                block: l.block_number.unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Which of `wallets` sent or received USDC in `[from_block, to_block]`
    /// (PLAN 5.6): two `eth_getLogs` over the chain's all-USDC emitter, one
    /// with the wallets as sender, one as receiver. For a short, recent range
    /// — the watcher's tick — not a history walk.
    pub async fn touched_among(
        &self,
        wallets: &[Address],
        from_block: u64,
        to_block: u64,
    ) -> Result<std::collections::BTreeSet<Address>, ArcError> {
        let mut out = std::collections::BTreeSet::new();
        if wallets.is_empty() || from_block > to_block {
            return Ok(out);
        }
        let words: Vec<B256> = wallets.iter().map(|w| w.into_word()).collect();
        let base = Filter::new()
            .address(self.source.emitter())
            .event("Transfer(address,address,uint256)")
            .from_block(from_block)
            .to_block(to_block);
        for f in [base.clone().topic1(words.clone()), base.topic2(words)] {
            let logs = self
                .provider
                .get_logs(&f)
                .await
                .map_err(|e| ArcError::Transport(e.to_string()))?;
            for log in logs {
                for t in log.topics().iter().skip(1).take(2) {
                    let a = Address::from_word(*t);
                    if wallets.contains(&a) {
                        out.insert(a);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Replacement keys proposed for any of `owners` in the block range
    /// (`RecoveryProposed`: the guardians' threshold or the recovery address):
    /// `(owner, eta, proposed by)`. One query for every owner on the chain, the
    /// new blocks only, as [`Self::touched_among`] does for transfers.
    pub async fn recoveries_proposed(
        &self,
        owners: &[Address],
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<(Address, u64, Address)>, ArcError> {
        use alloy::sol_types::SolEvent;
        if owners.is_empty() || from_block > to_block {
            return Ok(Vec::new());
        }
        let filter = Filter::new()
            .address(owners.to_vec())
            .event_signature(IPasskeyPrincipal::RecoveryProposed::SIGNATURE_HASH);
        let logs = self.walk_logs(&[filter], from_block, to_block).await?;
        Ok(logs
            .iter()
            .filter_map(|l| {
                let ev =
                    IPasskeyPrincipal::RecoveryProposed::decode_log_data(&l.inner.data).ok()?;
                Some((l.address(), ev.eta, ev.by))
            })
            .collect())
    }

    /// Every USDC transfer touching `wallet` in the given block range,
    /// returned as [`ledger::Transfer`] ready for obligation detection.
    ///
    /// Block timestamps are fetched once per distinct block rather than once
    /// per log: a wallet with forty transfers in one block should cost one
    /// `eth_getBlockByNumber`, not forty.
    pub async fn transfers(
        &self,
        wallet: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<Transfer>, ArcError> {
        let base = Filter::new()
            .address(self.source.emitter())
            .event("Transfer(address,address,uint256)");
        let logs = self
            .walk_logs(
                &[
                    base.clone().topic1(wallet.into_word()),
                    base.topic2(wallet.into_word()),
                ],
                from_block,
                to_block,
            )
            .await?;

        let mut block_times: BTreeMap<u64, Timestamp> = BTreeMap::new();
        // The blocks whose time the logs do not carry, read eight at a time
        // rather than one after another (a 20-transfer wallet was 8 s of
        // sequential block reads on Arbitrum; docs/60 P-42).
        let need: std::collections::BTreeSet<u64> = logs
            .iter()
            .filter(|l| l.block_timestamp.filter(|t| *t > 0).is_none())
            .filter_map(|l| l.block_number)
            .collect();
        let times = eight_at_a_time(need.into_iter().collect(), |b| {
            let c = self.clone();
            async move { (b, c.block_timestamp(b).await) }
        })
        .await?;
        for (b, t) in times {
            block_times.insert(b, t?);
        }
        let mut out = Vec::with_capacity(logs.len());

        for log in logs {
            let Some(block) = log.block_number else {
                continue;
            };
            let Some(tx_hash) = log.transaction_hash else {
                continue;
            };

            let decoded = log
                .log_decode::<IUsdc::Transfer>()
                .map_err(|e| ArcError::Contract(e.to_string()))?;
            let ev = decoded.inner.data;

            // Only transfers involving this wallet, and never a self-transfer,
            // which is neither an obligation nor revenue.
            let direction = if ev.from == wallet && ev.to != wallet {
                Direction::Out
            } else if ev.to == wallet && ev.from != wallet {
                Direction::In
            } else {
                continue;
            };
            let other = if direction == Direction::Out {
                ev.to
            } else {
                ev.from
            };

            // Arc's node puts the block's time on the log itself
            // (`blockTimestamp`); a block read per transfer was most of a first
            // check (19 transfers, 0.43 s each). Arbitrum's sends the field as
            // 0x0 — a placeholder, not 1970 (it emptied a bill list before
            // this guard, docs/60 P-42) — so zero means "read the block".
            let at = match (
                log.block_timestamp.filter(|t| *t > 0),
                block_times.get(&block),
            ) {
                (Some(ts), _) => Timestamp::from_unix(ts as i64),
                (None, Some(t)) => *t,
                (None, None) => {
                    let t = self.block_timestamp(block).await?;
                    block_times.insert(block, t);
                    t
                }
            };

            out.push(Transfer {
                tx_hash: format!("{tx_hash:#x}"),
                counterparty: Counterparty::new(format!("{other:#x}")),
                amount: self.source.to_money(ev.value),
                at,
                direction,
            });
        }

        out.sort_by_key(|t| t.at);
        Ok(out)
    }

    /// One `eth_getLogs` call, backing off while the endpoint is throttling.
    ///
    /// Walking a wallet's history means hundreds of calls in a row, and a
    /// public endpoint will start refusing partway through. Retrying with a
    /// widening delay is the difference between "history is unreadable" and
    /// "history takes a few more seconds", and the distinction between being
    /// throttled and being told the range is too wide matters: one is worth
    /// waiting out, the other never will be.
    async fn get_logs_retrying(
        &self,
        filter: &Filter,
        from: u64,
        to: u64,
    ) -> Result<Vec<alloy::rpc::types::Log>, ArcError> {
        let mut delay_ms = 250u64;
        for attempt in 0..=RATE_LIMIT_RETRIES {
            match self.provider.get_logs(filter).await {
                Ok(logs) => return Ok(logs),
                Err(e) => {
                    let msg = e.to_string();
                    let throttled = msg.contains("rate limit")
                        || msg.contains("-32005")
                        || msg.contains("too many requests");
                    let transient = transient_transport_error(&msg);
                    // Anything else is the node refusing the range itself,
                    // which the walker answers by halving the chunk. A
                    // dropped connection is not that: halving on it made
                    // every later call four times slower for nothing.
                    if !throttled && !transient {
                        return Err(ArcError::RangeTooWide {
                            from,
                            to,
                            detail: msg,
                        });
                    }
                    if attempt == RATE_LIMIT_RETRIES {
                        if transient && !throttled {
                            return Err(ArcError::Transport(format!(
                                "blocks {from}..={to}: {msg} ({} tries)",
                                attempt + 1
                            )));
                        }
                        return Err(ArcError::RateLimited {
                            from,
                            to,
                            attempts: attempt + 1,
                        });
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    delay_ms = (delay_ms * 2).min(RATE_LIMIT_MAX_DELAY_MS);
                }
            }
        }
        unreachable!("loop returns on every path")
    }

    /// Pay through the policy contract, citing the action that authorised it.
    ///
    /// Deliberately the only payment path. A direct transfer would move the
    /// same value while bypassing the cap, the daily total and the on-chain
    /// replay guard, which is to say it would bypass the reason any of this
    /// is safe.
    /// Ask the chain whether `pay()` would succeed, without sending it.
    ///
    /// An `eth_call` runs the contract's checks against current state and
    /// returns the revert instead of mining it. Two things this buys:
    /// a cap violation or a stale replay guard costs nothing to discover, and
    /// the failure arrives as a *reason* rather than as a receipt with
    /// `status 0` that has to be decoded after the fact.
    ///
    /// A pass here is not a promise: state can move between the simulation
    /// and the send, and the caps are evaluated again on the real call. It
    /// narrows the window; it does not close it.
    pub async fn simulate_payment(
        &self,
        policy: Address,
        to: Address,
        amount: Money,
        action_hash: B256,
    ) -> Result<(), ArcError> {
        let minor = amount.to_minor();
        if minor <= 0 {
            return Err(ArcError::AmountOutOfRange(minor));
        }
        let policy = IBudgetPolicy::new(policy, &self.provider);
        let mut call = policy.pay(to, U256::from(minor as u128), action_hash);
        // Simulate as the agent; called from an unfunded default sender the
        // onlyAgent guard would reject every payment and the check would be
        // useless.
        if let Some(from) = self.signer_address {
            call = call.from(from);
        }
        call.call()
            .await
            .map(|_| ())
            .map_err(|e| ArcError::Contract(e.to_string()))
    }

    pub async fn pay_via_policy(
        &self,
        policy: Address,
        to: Address,
        amount: Money,
        action_hash: B256,
    ) -> Result<B256, ArcError> {
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let minor = amount.to_minor();
        if minor <= 0 {
            return Err(ArcError::AmountOutOfRange(minor));
        }
        let policy = IBudgetPolicy::new(policy, &self.provider);
        let receipt = policy
            .pay(to, U256::from(minor as u128), action_hash)
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// Read a payment back from its receipt (PLAN 5.21, docs/59 §3.15): the
    /// transaction succeeded and the policy emitted exactly one `Paid` naming
    /// what the agent sent. Anything else is an error the operator treats as
    /// a halt, not a warning — the same discipline as the hedge's read-back.
    pub async fn confirm_paid(
        &self,
        tx: B256,
        policy: Address,
        to: Address,
        amount: Money,
        action_hash: B256,
    ) -> Result<(), ArcError> {
        let receipt = self
            .provider
            .get_transaction_receipt(tx)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?
            .ok_or_else(|| ArcError::Contract(format!("no receipt for {tx:#x}")))?;
        if !receipt.status() {
            return Err(ArcError::Contract(format!("{tx:#x} reverted")));
        }
        let minor = U256::from(amount.to_minor().max(0) as u128);
        let mut seen = 0usize;
        for log in receipt.inner.logs() {
            match paid_log_matches(&log.inner, policy, to, minor, action_hash) {
                None => {}
                Some(Ok(())) => seen += 1,
                Some(Err(e)) => {
                    return Err(ArcError::Contract(format!("read-back of {tx:#x}: {e}")));
                }
            }
        }
        if seen != 1 {
            return Err(ArcError::Contract(format!(
                "read-back of {tx:#x}: {seen} matching Paid events from the policy, expected exactly one"
            )));
        }
        Ok(())
    }

    /// The limits as the contract holds them.
    ///
    /// Read from chain rather than from configuration, so the verifier
    /// compares a proposed action against the same numbers that will enforce
    /// it. An agent that misreports its own authority is caught by the
    /// difference.
    pub async fn authority(&self, policy: Address) -> Result<ledger::Authority, ArcError> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        let to_money =
            |v: U256| Money::minor(i128::try_from(v).unwrap_or(i128::MAX), Currency::Usdc);
        let c = |e: alloy::contract::Error| ArcError::Contract(e.to_string());
        // Three independent reads in one round trip's time (docs/68 F4).
        let (per, daily, spent) = tokio::join!(
            async { p.perActionCap().call().await },
            async { p.dailyCap().call().await },
            async { p.spentInWindow().call().await },
        );
        Ok(ledger::Authority {
            per_action_cap: to_money(per.map_err(c)?),
            daily_cap: to_money(daily.map_err(c)?),
            // The contract's trailing 24-hour total, not a calendar day:
            // a calendar reset let the cap be spent twice across midnight.
            spent_today: to_money(spent.map_err(c)?),
        })
    }

    /// Whether the policy currently attests a payee.
    pub async fn is_attested(&self, policy: Address, payee: Address) -> Result<bool, ArcError> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        p.attestedPayee(payee)
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))
    }

    /// Whether an action hash has been settled on the policy (by the agent's
    /// `pay` or the principal's `settleAsPrincipal`).
    pub async fn executed(&self, policy: Address, action_hash: B256) -> Result<bool, ArcError> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        p.executed(action_hash)
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))
    }

    /// The caps as the contract holds them now: (per action, per day, floor).
    pub async fn caps(&self, policy: Address) -> Result<(Money, Money, Money), ArcError> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        let c = |e: alloy::contract::Error| ArcError::Contract(e.to_string());
        let (per, day, floor) = tokio::join!(
            async { p.perActionCap().call().await },
            async { p.dailyCap().call().await },
            async { p.reserveFloor().call().await },
        );
        let (per, day, floor) = (per.map_err(c)?, day.map_err(c)?, floor.map_err(c)?);
        let m = |u: U256| Money::minor(u.to::<i128>(), Currency::Usdc);
        Ok((m(per), m(day), m(floor)))
    }

    /// Operator action: set the autonomy level. Lowering applies at once;
    /// raising is scheduled behind the policy's delay.
    pub async fn set_mode(&self, policy: Address, mode: u8) -> Result<B256, ArcError> {
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let p = IBudgetPolicy::new(policy, &self.provider);
        let receipt = p
            .setMode(mode)
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// Operator action: set the caps. Lowering applies at once; raising is
    /// scheduled behind the policy's delay (anyone applies it after `eta`).
    pub async fn set_limits(
        &self,
        policy: Address,
        per_action: Money,
        daily: Money,
        floor: Money,
    ) -> Result<B256, ArcError> {
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let p = IBudgetPolicy::new(policy, &self.provider);
        let u = |m: Money| U256::from(m.to_minor().max(0) as u128);
        let receipt = p
            .setLimits(u(per_action), u(daily), u(floor))
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// Anyone: apply the pending change once its delay has passed.
    pub async fn apply_pending(&self, policy: Address) -> Result<B256, ArcError> {
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let p = IBudgetPolicy::new(policy, &self.provider);
        let receipt = p
            .applyPending()
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// The policy's autonomy level and delay rule, as the contract holds them
    /// (docs/46 #3, #4). `mode` is 0 Watch, 1 Propose, 2 Autopilot. A policy
    /// deployed before these existed has no such functions; the error says so.
    pub async fn governance(&self, policy: Address) -> Result<Governance, ArcError> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        let c = |e: alloy::contract::Error| ArcError::Contract(e.to_string());
        let (mode, delay, reserve_floor, pending) = tokio::join!(
            async { p.mode().call().await },
            async { p.delay().call().await },
            async { p.reserveFloor().call().await },
            async { p.pending().call().await },
        );
        let (mode, delay, reserve_floor, pending) = (
            mode.map_err(c)?,
            delay.map_err(c)?,
            reserve_floor.map_err(c)?,
            pending.map_err(c)?,
        );
        let to_money =
            |v: U256| Money::minor(i128::try_from(v).unwrap_or(i128::MAX), Currency::Usdc);
        Ok(Governance {
            mode: PolicyMode::from_u8(mode),
            delay_secs: u64::try_from(delay).unwrap_or(u64::MAX),
            reserve_floor: to_money(reserve_floor),
            pending: (pending._5 != 0).then(|| PendingChange {
                kinds: pending._5,
                per_action_cap: to_money(pending._0),
                daily_cap: to_money(pending._1),
                reserve_floor: to_money(pending._2),
                mode: PolicyMode::from_u8(pending._3),
                eta_unix: pending._4,
            }),
        })
    }

    /// How many approvals a `PasskeyPrincipal` has used: the nonce its next
    /// challenge must commit to.
    pub async fn passkey_nonce(&self, owner: Address) -> Result<U256, ArcError> {
        let p = IPasskeyPrincipal::new(owner, &self.provider);
        p.nonce()
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))
    }

    /// The P-256 public key a `PasskeyPrincipal` accepts, as the contract
    /// holds it. The service checks off-chain passkey assertions (where the
    /// agent reports) against this, so the chain stays the one source.
    pub async fn passkey_key(&self, owner: Address) -> Result<(B256, B256), ArcError> {
        let p = IPasskeyPrincipal::new(owner, &self.provider);
        let qx = p
            .qx()
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?;
        let qy = p
            .qy()
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?;
        Ok((qx, qy))
    }

    /// The owner's guardians and threshold (PLAN 5.30), and the pending
    /// replacement key's eta if one is waiting. `None` on a principal from
    /// before guardians.
    pub async fn guardians(&self, owner: Address) -> Option<(Vec<Address>, u8, u64)> {
        let p = IPasskeyPrincipal::new(owner, &self.provider);
        let n = p.guardianCount().call().await.ok()?;
        let n = u64::try_from(n).unwrap_or(0).min(16);
        let mut list = Vec::with_capacity(n as usize);
        for i in 0..n {
            list.push(p.guardians(U256::from(i)).call().await.ok()?);
        }
        let threshold = p.guardianThreshold().call().await.ok()?;
        let pending = p.pendingKey().call().await.ok()?;
        Some((list, threshold, pending.eta))
    }

    /// Who may attest payees and withdraw, read from the policy.
    pub async fn principal(&self, policy: Address) -> Result<Address, ArcError> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        p.principal()
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))
    }

    /// Balance held by the policy contract.
    pub async fn policy_balance(&self, policy: Address) -> Result<Money, ArcError> {
        let p = IBudgetPolicy::new(policy, &self.provider);
        let raw = p
            .balanceUsdc()
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?;
        Ok(Money::minor(
            i128::try_from(raw).unwrap_or(i128::MAX),
            Currency::Usdc,
        ))
    }

    /// What the policy will still allow right now, read from the contract
    /// rather than from the agent's own bookkeeping.
    pub async fn remaining_allowance(&self, policy: Address) -> Result<Money, ArcError> {
        let policy = IBudgetPolicy::new(policy, &self.provider);
        let raw = policy
            .remainingAllowance()
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?;
        Ok(Money::minor(
            i128::try_from(raw).unwrap_or(i128::MAX),
            Currency::Usdc,
        ))
    }

    /// Which of `candidates` the principal has attested on chain, the only
    /// addresses `pay` will send to.
    ///
    /// `None` when the policy predates the allowlist and has no
    /// `attestedPayee` to ask. That is reported as "no list" rather than as
    /// "nobody is attested", which would escalate every payment.
    ///
    /// When each of `candidates` was approved (`attestedSince`, unix seconds;
    /// 0 = not approved), read from the contract: with the delay, the moment
    /// `pay` to it stops answering `PayeeCoolingOff` (docs/60 P-181).
    pub async fn attested_since_among(
        &self,
        policy: Address,
        candidates: &[Address],
    ) -> Result<Vec<(Address, u64)>, ArcError> {
        let answers = eight_at_a_time(candidates.to_vec(), |who| {
            let c = self.clone();
            async move {
                let p = IBudgetPolicy::new(policy, &c.provider);
                (who, p.attestedSince(who).call().await)
            }
        })
        .await?;
        answers
            .into_iter()
            .map(|(who, r)| {
                r.map(|t| (who, t))
                    .map_err(|e| ArcError::Contract(e.to_string()))
            })
            .collect()
    }

    pub async fn attested_among(
        &self,
        policy: Address,
        candidates: &[Address],
    ) -> Result<Option<std::collections::BTreeSet<String>>, ArcError> {
        let answers = eight_at_a_time(candidates.to_vec(), |who| {
            let c = self.clone();
            async move {
                let p = IBudgetPolicy::new(policy, &c.provider);
                (who, p.attestedPayee(who).call().await)
            }
        })
        .await?;
        let mut out = std::collections::BTreeSet::new();
        for answer in answers {
            match answer {
                (who, Ok(true)) => {
                    out.insert(format!("{who:#x}"));
                }
                (_, Ok(false)) => {}
                // Only a revert means "no list". A read that failed is not
                // an approval (audit 2026-10-15, docs/60 P-74).
                (_, Err(e)) => {
                    let e = ArcError::Contract(e.to_string());
                    return if is_revert(&e) { Ok(None) } else { Err(e) };
                }
            }
        }
        Ok(Some(out))
    }

    /// Which of `who` the USDC issuer has frozen (`isBlacklisted`), as
    /// lowercase `0x…` strings. A token with no blocklist answers with a
    /// revert; that is read as "no list", an empty set, not as an error --
    /// the anvil test token and other stand-ins have none (PLAN 5.47).
    pub async fn issuer_blocked_among(
        &self,
        who: &[Address],
    ) -> std::collections::BTreeSet<String> {
        let answers = eight_at_a_time(who.to_vec(), |a| {
            let c = self.clone();
            async move {
                let usdc = IUsdc::new(c.usdc, &c.provider);
                (a, usdc.isBlacklisted(a).call().await)
            }
        })
        .await
        .unwrap_or_default();
        // A token whose `isBlacklisted` reverts has no list (every read errs,
        // nothing is inserted); one read that errs on a token that has one
        // keeps whatever the others found frozen. The token itself refuses a
        // transfer to a frozen address, so this read only words the refusal.
        answers
            .into_iter()
            .filter(|(_, r)| matches!(r, Ok(true)))
            .map(|(a, _)| format!("{a:#x}"))
            .collect()
    }

    /// Whether the policy has already spent this action hash.
    pub async fn already_executed(
        &self,
        policy: Address,
        action_hash: B256,
    ) -> Result<bool, ArcError> {
        let policy = IBudgetPolicy::new(policy, &self.provider);
        policy
            .executed(action_hash)
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))
    }

    /// Commit a root of the decision log.
    pub async fn anchor_root(&self, anchor: Address, root: B256) -> Result<B256, ArcError> {
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let contract = IMemoryAnchor::new(anchor, &self.provider);
        let receipt = contract
            .anchor(root)
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// Send native value (USDC on Arc; ETH elsewhere) to `to`. Used by the
    /// hosted service to give a tenant's agent key the gas it needs to anchor.
    pub async fn send_native(&self, to: Address, wei: U256) -> Result<B256, ArcError> {
        use alloy::network::TransactionBuilder;
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let tx = alloy::rpc::types::TransactionRequest::default()
            .with_to(to)
            .with_value(wei);
        let receipt = self
            .provider
            .send_transaction(tx)
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        Ok(receipt.transaction_hash)
    }

    /// Send `data` to `to` with no value, signed by this client's key, and
    /// wait for the receipt; a reverted call is an error. Used by the hosted
    /// service to relay a Gateway mint to a tenant's policy (PLAN 5.15): the
    /// attestation names the recipient, so the relayer only pays gas.
    /// `eth_call` of raw calldata, from `from` when given: a facilitator's
    /// dry run of a redeem, or any check that must not send.
    pub async fn call_raw(
        &self,
        from: Option<Address>,
        to: Address,
        data: Vec<u8>,
    ) -> Result<Vec<u8>, ArcError> {
        use alloy::network::TransactionBuilder;
        let mut tx = alloy::rpc::types::TransactionRequest::default()
            .with_to(to)
            .with_input(data);
        if let Some(f) = from {
            tx = tx.with_from(f);
        }
        self.provider
            .call(tx)
            .await
            .map(|b| b.to_vec())
            .map_err(|e| ArcError::Contract(e.to_string()))
    }

    pub async fn send_call(&self, to: Address, data: Vec<u8>) -> Result<B256, ArcError> {
        use alloy::network::TransactionBuilder;
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let tx = alloy::rpc::types::TransactionRequest::default()
            .with_to(to)
            .with_input(data);
        let receipt = self
            .provider
            .send_transaction(tx)
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        if !receipt.status() {
            return Err(ArcError::Contract(format!(
                "call to {to:#x} reverted in {:#x}",
                receipt.transaction_hash
            )));
        }
        Ok(receipt.transaction_hash)
    }

    /// Deploy through the EIP-7997 keyless CREATE2 factory (PLAN 5.22,
    /// docs/59 §3.10), so the same salt and initcode give the same address on
    /// every chain that has the factory (Arc testnet does: `cast code` shows
    /// its 69-byte runtime). Returns the address and the deploy transaction,
    /// or the address alone when code already lives there (a re-run is
    /// idempotent; a different chain gets the same address).
    pub async fn deploy_deterministic(
        &self,
        salt: B256,
        initcode: &[u8],
    ) -> Result<(Address, Option<B256>), ArcError> {
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let factory = EIP7997_FACTORY.parse::<Address>().expect("factory address");
        let predicted = predict_create2(factory, salt, initcode);
        let existing = self
            .provider
            .get_code_at(predicted)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?;
        if !existing.is_empty() {
            return Ok((predicted, None));
        }
        let factory_code = self
            .provider
            .get_code_at(factory)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?;
        if factory_code.is_empty() {
            return Err(ArcError::Contract(format!(
                "no EIP-7997 factory at {factory:#x} on this chain"
            )));
        }
        let mut data = salt.to_vec();
        data.extend_from_slice(initcode);
        let tx = self.send_call(factory, data).await?;
        let code = self
            .provider
            .get_code_at(predicted)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?;
        if code.is_empty() {
            return Err(ArcError::Contract(format!(
                "factory deploy {tx:#x} left no code at {predicted:#x}"
            )));
        }
        Ok((predicted, Some(tx)))
    }

    /// Deploy a contract from its creation bytecode and ABI-encoded
    /// constructor arguments; returns the address. Used by the hosted
    /// service (`docs/52`) so a treasurer's policy can be created without
    /// forge on the host; the bytecode comes from `contracts/bytecode/`.
    pub async fn deploy(
        &self,
        creation_code: &[u8],
        ctor_args: &[u8],
    ) -> Result<Address, ArcError> {
        use alloy::network::TransactionBuilder;
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let mut code = creation_code.to_vec();
        code.extend_from_slice(ctor_args);
        let tx = alloy::rpc::types::TransactionRequest::default().with_deploy_code(code);
        let receipt = self
            .provider
            .send_transaction(tx)
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        receipt
            .contract_address
            .ok_or_else(|| ArcError::Contract("deploy receipt carries no contract address".into()))
    }

    /// Relay a passkey-approved call through `PasskeyPrincipal.execute`. The
    /// relayer pays gas and holds no authority: a wrong `auth` reverts.
    pub async fn execute_as_passkey(
        &self,
        owner: Address,
        target: Address,
        value: U256,
        data: Vec<u8>,
        auth: &PasskeyAuth,
    ) -> Result<B256, ArcError> {
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let p = IPasskeyPrincipal::new(owner, &self.provider);
        let a = IPasskeyPrincipal::WebAuthnAuth {
            r: auth.r,
            s: auth.s,
            challengeIndex: auth.challenge_index,
            typeIndex: auth.type_index,
            authenticatorData: auth.authenticator_data.clone().into(),
            clientDataJSON: auth.client_data_json.clone(),
        };
        let receipt = p
            .execute(target, value, data.into(), a)
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        // A relay that mined but reverted is not done (audit 2026-10-15,
        // docs/60 P-73): the page would show "Paid · receipt" for nothing.
        if !receipt.status() {
            return Err(ArcError::Contract(format!(
                "the call reverted in {:#x}; nothing changed",
                receipt.transaction_hash
            )));
        }
        Ok(receipt.transaction_hash)
    }

    /// The owner's keys (index 0 = the primary passkey, then co-owners) and how
    /// many must approve (PLAN 5.7). An owner contract from before co-owners
    /// answers with its one key and a threshold of one.
    pub async fn owner_signers(&self, owner: Address) -> Result<(u8, Vec<(B256, B256)>), ArcError> {
        let p = IPasskeyPrincipal::new(owner, &self.provider);
        let c = |e: alloy::contract::Error| ArcError::Contract(e.to_string());
        let primary = (
            p.qx().call().await.map_err(c)?,
            p.qy().call().await.map_err(c)?,
        );
        let Ok(count) = p.coOwnerCount().call().await else {
            return Ok((1, vec![primary]));
        };
        let threshold = p.approvalThreshold().call().await.map_err(c)?.max(1);
        let mut keys = vec![primary];
        for i in 1..=count.to::<u64>() {
            let k = p.keyAt(U256::from(i)).call().await.map_err(c)?;
            keys.push((k._0, k._1));
        }
        Ok((threshold, keys))
    }

    /// Relay a call approved by co-owner key `key_index` (threshold one).
    pub async fn execute_as_key(
        &self,
        owner: Address,
        key_index: u64,
        target: Address,
        value: U256,
        data: Vec<u8>,
        auth: &PasskeyAuth,
    ) -> Result<B256, ArcError> {
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let p = IPasskeyPrincipal::new(owner, &self.provider);
        let receipt = p
            .executeAs(
                U256::from(key_index),
                target,
                value,
                data.into(),
                web_authn(auth),
            )
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        // A relay that mined but reverted is not done (audit 2026-10-15,
        // docs/60 P-73): the page would show "Paid · receipt" for nothing.
        if !receipt.status() {
            return Err(ArcError::Contract(format!(
                "the call reverted in {:#x}; nothing changed",
                receipt.transaction_hash
            )));
        }
        Ok(receipt.transaction_hash)
    }

    /// Relay a call approved by several keys (strictly increasing indexes).
    pub async fn execute_with_keys(
        &self,
        owner: Address,
        target: Address,
        value: U256,
        data: Vec<u8>,
        approvals: &[(u64, PasskeyAuth)],
    ) -> Result<B256, ArcError> {
        let _turn = self.turn().await;
        self.signer_address.ok_or(ArcError::NoSigner)?;
        let p = IPasskeyPrincipal::new(owner, &self.provider);
        let idx: Vec<U256> = approvals.iter().map(|(i, _)| U256::from(*i)).collect();
        let auths: Vec<IPasskeyPrincipal::WebAuthnAuth> =
            approvals.iter().map(|(_, a)| web_authn(a)).collect();
        let receipt = p
            .executeWith(target, value, data.into(), idx, auths)
            .send()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))?
            .confirmed()
            .await?;
        // A relay that mined but reverted is not done (audit 2026-10-15,
        // docs/60 P-73): the page would show "Paid · receipt" for nothing.
        if !receipt.status() {
            return Err(ArcError::Contract(format!(
                "the call reverted in {:#x}; nothing changed",
                receipt.transaction_hash
            )));
        }
        Ok(receipt.transaction_hash)
    }

    /// The passkey principal's public key and recovery address, read back.
    pub async fn passkey_owner(&self, owner: Address) -> Result<(B256, B256, Address), ArcError> {
        let p = IPasskeyPrincipal::new(owner, &self.provider);
        let c = |e: alloy::contract::Error| ArcError::Contract(e.to_string());
        Ok((
            p.qx().call().await.map_err(c)?,
            p.qy().call().await.map_err(c)?,
            p.recovery().call().await.map_err(c)?,
        ))
    }

    /// The root currently anchored for `agent`.
    /// Whether transaction `tx` anchored `root`: it succeeded, `agent` sent it
    /// to `anchor`, and its calldata ends with the root. `rootOf` keeps only
    /// an agent's latest root, so an earlier record is checked by the
    /// transaction that anchored it, read from the chain -- not by a line in
    /// the service's own index (docs/60 P-35).
    pub async fn anchored_in(
        &self,
        tx: B256,
        anchor: Address,
        agent: Address,
        root: B256,
    ) -> Result<bool, ArcError> {
        use alloy::consensus::Transaction as _;
        let t = self
            .provider
            .get_transaction_by_hash(tx)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?
            .ok_or_else(|| ArcError::Contract(format!("no transaction {tx:#x}")))?;
        let receipt = self
            .provider
            .get_transaction_receipt(tx)
            .await
            .map_err(|e| ArcError::Transport(e.to_string()))?
            .ok_or_else(|| ArcError::Contract(format!("no receipt for {tx:#x}")))?;
        let input = t.input();
        Ok(receipt.status()
            && receipt.from == agent
            && receipt.to == Some(anchor)
            && input.len() >= 32
            && input[input.len() - 32..] == root.0)
    }

    pub async fn anchored_root(&self, anchor: Address, agent: Address) -> Result<B256, ArcError> {
        let contract = IMemoryAnchor::new(anchor, &self.provider);
        contract
            .rootOf(agent)
            .call()
            .await
            .map_err(|e| ArcError::Contract(e.to_string()))
    }
}

/// A transaction composed but deliberately not sent.
///
/// The agent can always *build* a payment; whether it may *send* one is the
/// contract's decision. When the verifier routes an obligation to a human,
/// the useful handover is not "go and figure it out" but the exact bytes the
/// principal needs to sign. This is that handover.
///
/// It is also the honest shape of the boundary: composing calldata requires
/// no authority at all, which is why an agent over its cap can still produce
/// this while being unable to execute it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCall {
    /// Contract to call — the policy, never the payee directly.
    pub to: Address,
    /// ABI-encoded `pay(address,uint256,bytes32)`, `0x`-prefixed.
    pub data: String,
    /// Always zero: USDC moves as the policy's own balance, not as call value.
    pub value: U256,
    /// One line a person can check before signing.
    pub summary: String,
}

/// Encode the call **the principal can actually send** to settle an escalated
/// obligation: `BudgetPolicy.settleAsPrincipal(to, usdcUnits, actionHash)`.
///
/// It is deliberately not `pay()`. `pay` is `onlyAgent`, so handing a human
/// `pay` calldata produces a transaction that reverts with `NotAgent` — a
/// handover that looks helpful and is worthless.
///
/// Nor is it `withdraw()`, which a principal *can* send but which takes no
/// action hash: settling that way leaves `executed[actionHash]` false, so the
/// agent re-infers the obligation and escalates it again on its next pass.
/// The bill gets paid and the agent never finds out. `settleAsPrincipal`
/// pays out under principal authority and records the hash, closing the cycle
/// the same way `pay` does.
///
/// Pure: no provider, no signer, no network. A caller with no key and no RPC
/// can still produce the bytes, which is exactly why it is safe to run on the
/// escalation path.
pub fn prepare_principal_settlement(
    policy: Address,
    to: Address,
    amount: Money,
    action_hash: B256,
) -> Result<PreparedCall, ArcError> {
    use alloy::sol_types::SolCall;

    let minor = amount.to_minor();
    if minor <= 0 {
        return Err(ArcError::AmountOutOfRange(minor));
    }
    let call = IBudgetPolicy::settleAsPrincipalCall {
        to,
        usdcUnits: U256::from(minor as u128),
        actionHash: action_hash,
    };
    Ok(PreparedCall {
        to: policy,
        data: format!("0x{}", alloy::hex::encode(call.abi_encode())),
        value: U256::ZERO,
        summary: format!("settle {amount} to {to}, marking action {action_hash:#x} executed"),
    })
}

/// Encode the call that lets the agent pay `payee` from now on:
/// `BudgetPolicy.setPayee(payee, true)`.
///
/// This is the other answer a principal can give an escalation. When the only
/// thing between the agent and a payment is that the payee was never attested,
/// settling once by hand fixes this cycle and leaves the next one escalated
/// again. Attesting fixes the cause. Both are offered; the principal chooses.
///
/// Pure, like [`prepare_principal_settlement`].
pub fn prepare_payee_attestation(policy: Address, payee: Address) -> PreparedCall {
    use alloy::sol_types::SolCall;

    let call = IBudgetPolicy::setPayeeCall {
        payee,
        attested: true,
    };
    PreparedCall {
        to: policy,
        data: format!("0x{}", alloy::hex::encode(call.abi_encode())),
        value: U256::ZERO,
        summary: format!("allow the agent to pay {payee} from now on"),
    }
}

/// Encode one approval for several payees: `setPayees(payees, true)`.
///
/// Onboarding is the case it exists for. The agent finds everyone the owner
/// already pays on a schedule; approving them one tap at a time is exactly the
/// friction that made the replay over real treasuries ask the owner 27 times
/// per 100 payments, every one of them a first payment to a known payee.
/// Calldata for the passkey to name (or clear, with the zero address) the
/// recovery address on its own `PasskeyPrincipal`: `setRecovery(address)`.
/// The target is the owner contract, not the policy.
pub fn prepare_set_recovery(owner: Address, recovery: Address) -> PreparedCall {
    use alloy::sol_types::SolCall;
    let data = IPasskeyPrincipal::setRecoveryCall { next: recovery }.abi_encode();
    PreparedCall {
        to: owner,
        data: alloy::hex::encode_prefixed(data),
        value: U256::ZERO,
        summary: if recovery.is_zero() {
            "remove the recovery address: only this passkey can ever act".into()
        } else {
            format!(
                "name {recovery:#x} as the recovery address that may replace this passkey after the delay"
            )
        },
    }
}

/// Calldata for the principal to set the agent's limits:
/// `setLimits(perActionCap, dailyCap, reserveFloor)`. Tightening applies at
/// once on the contract; loosening is scheduled behind the delay.
pub fn prepare_set_limits(
    policy: Address,
    per_action: Money,
    daily: Money,
    floor: Money,
) -> Result<PreparedCall, String> {
    use alloy::sol_types::SolCall;
    if per_action.to_minor() > daily.to_minor() {
        return Err("the per-action cap cannot exceed the daily cap".into());
    }
    let data = IBudgetPolicy::setLimitsCall {
        perActionCap: U256::from(per_action.to_minor()),
        dailyCap: U256::from(daily.to_minor()),
        reserveFloor: U256::from(floor.to_minor()),
    }
    .abi_encode();
    Ok(PreparedCall {
        to: policy,
        data: alloy::hex::encode_prefixed(data),
        value: U256::ZERO,
        summary: format!(
            "set the agent's limits: {per_action} per payment, {daily} per day, keep {floor} untouched"
        ),
    })
}

/// Calldata for the principal to take money out: `withdraw(to, usdcUnits)`.
/// Principal only on the contract; the agent and the operator cannot.
pub fn prepare_withdraw(
    policy: Address,
    to: Address,
    amount: Money,
) -> Result<PreparedCall, String> {
    use alloy::sol_types::SolCall;
    let units = U256::from(amount.to_minor());
    if units.is_zero() {
        return Err("amount must be more than zero".into());
    }
    let data = IBudgetPolicy::withdrawCall {
        to,
        usdcUnits: units,
    }
    .abi_encode();
    Ok(PreparedCall {
        to: policy,
        data: alloy::hex::encode_prefixed(data),
        value: U256::ZERO,
        summary: format!("withdraw {amount} from the policy to {to:#x}"),
    })
}

/// Calldata for the principal to set the autonomy level: `setMode(uint8)`.
pub fn prepare_set_mode(policy: Address, mode: u8) -> Result<PreparedCall, String> {
    use alloy::sol_types::SolCall;
    let label = match mode {
        0 => "Watch: the agent reads and reports, pays nothing",
        1 => "Propose: the agent prepares each payment for your signature",
        2 => "Autopilot: the agent pays approved payees inside the caps (waits the delay)",
        other => return Err(format!("mode must be 0, 1 or 2, not {other}")),
    };
    let data = IBudgetPolicy::setModeCall { next: mode }.abi_encode();
    Ok(PreparedCall {
        to: policy,
        data: alloy::hex::encode_prefixed(data),
        value: U256::ZERO,
        summary: label.into(),
    })
}

/// Calldata for the principal to drop a pending raise: `cancelPending()`.
pub fn prepare_cancel_pending(policy: Address) -> PreparedCall {
    use alloy::sol_types::SolCall;
    let data = IBudgetPolicy::cancelPendingCall {}.abi_encode();
    PreparedCall {
        to: policy,
        data: alloy::hex::encode_prefixed(data),
        value: U256::ZERO,
        summary: "cancel the pending raise of the agent's limits or autonomy".into(),
    }
}

pub fn prepare_payee_batch(policy: Address, payees: &[Address]) -> PreparedCall {
    prepare_payee_batch_with(policy, payees, true)
}

/// `setPayees(payees, allow)`: `true` attests, `false` revokes. Revoking is
/// instant on the contract (trust-shrinking); attesting starts the
/// cooling-off.
pub fn prepare_payee_batch_with(policy: Address, payees: &[Address], allow: bool) -> PreparedCall {
    use alloy::sol_types::SolCall;

    let call = IBudgetPolicy::setPayeesCall {
        payees: payees.to_vec(),
        attested: allow,
    };
    PreparedCall {
        to: policy,
        data: format!("0x{}", alloy::hex::encode(call.abi_encode())),
        value: U256::ZERO,
        summary: if allow {
            format!(
                "allow the agent to pay these {} payees from now on",
                payees.len()
            )
        } else {
            format!(
                "stop the agent paying these {} payees, at once",
                payees.len()
            )
        },
    }
}

/// The 32 bytes a passkey signs to approve `call` through a
/// `PasskeyPrincipal` at `owner`, as approval number `nonce`.
///
/// Mirrors `PasskeyPrincipal.challengeFor`:
/// `keccak256(abi.encode(chainid, owner, nonce, target, value, keccak256(data)))`.
/// Computed here rather than asked of the chain so that the agent can hand
/// over a ready-to-sign request with no RPC round trip, and so that a relayer
/// can check a request before showing it to anyone. The two must agree; the
/// tests pin this function to challenges the deployed contract produced.
pub fn passkey_challenge(
    chain_id: u64,
    owner: Address,
    nonce: U256,
    call: &PreparedCall,
) -> Result<B256, ArcError> {
    use alloy::sol_types::SolValue;

    let data = alloy::hex::decode(call.data.trim_start_matches("0x"))
        .map_err(|e| ArcError::Contract(format!("calldata is not hex: {e}")))?;
    let encoded = (
        U256::from(chain_id),
        owner,
        nonce,
        call.to,
        call.value,
        alloy::primitives::keccak256(&data),
    )
        .abi_encode();
    Ok(alloy::primitives::keccak256(encoded))
}

/// `addSchedule(payee, amount, every, firstDue)` for the owner's passkey (PLAN
/// 5.25): a bill the owner confirms becomes a standing order anyone may pay
/// when due, inside the caps.
pub fn add_schedule_calldata(
    payee: Address,
    amount: Money,
    every_secs: u64,
    first_due: u64,
) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    IBudgetPolicy::addScheduleCall {
        payee,
        amount: U256::from(amount.to_minor().max(0) as u128),
        every: every_secs,
        firstDue: first_due,
    }
    .abi_encode()
}

/// `settleAsPrincipalIf(to, amount, actionHash, terms, deadline)` for the
/// owner's passkey (PLAN 5.23): the plain settlement, valid only while the
/// policy's terms still hash to `terms` and until `deadline`.
pub fn prepare_principal_settlement_bound(
    policy: Address,
    to: Address,
    amount: Money,
    action_hash: B256,
    terms: B256,
    deadline: u64,
) -> Result<PreparedCall, ArcError> {
    use alloy::sol_types::SolCall;
    let minor = amount.to_minor();
    if minor <= 0 {
        return Err(ArcError::AmountOutOfRange(minor));
    }
    let call = IBudgetPolicy::settleAsPrincipalIfCall {
        to,
        usdcUnits: U256::from(minor as u128),
        actionHash: action_hash,
        terms,
        deadline,
    };
    Ok(PreparedCall {
        to: policy,
        data: format!("0x{}", alloy::hex::encode(call.abi_encode())),
        value: U256::ZERO,
        summary: format!(
            "settle {amount} to {to}, marking action {action_hash:#x} executed; valid only while the terms hash to {terms:#x} and until unix {deadline}"
        ),
    })
}

/// The most rows one run may carry (`BudgetPolicyBase.MAX_RUN`).
pub const MAX_RUN: usize = 32;

/// The selector of `settleRunIf`, to tell a policy that has the owner's run
/// (PLAN 5.39) from one deployed before it.
pub fn settle_run_selector() -> [u8; 4] {
    use alloy::sol_types::SolCall;
    IBudgetPolicy::settleRunIfCall::SELECTOR
}

/// `settleRunIf(to[], amounts[], actionHashes[], terms, deadline)` for the
/// owner's passkey (PLAN 5.39): every row the owner reviewed, one approval,
/// bound like one row. Rows already settled are skipped by the contract.
pub fn prepare_principal_run_bound(
    policy: Address,
    rows: &[(Address, Money, B256)],
    terms: B256,
    deadline: u64,
) -> Result<PreparedCall, ArcError> {
    use alloy::sol_types::SolCall;
    if rows.is_empty() || rows.len() > MAX_RUN {
        return Err(ArcError::Contract(format!(
            "a run carries 1 to {MAX_RUN} rows, not {}",
            rows.len()
        )));
    }
    let mut to = Vec::with_capacity(rows.len());
    let mut units = Vec::with_capacity(rows.len());
    let mut hashes = Vec::with_capacity(rows.len());
    let mut total: i128 = 0;
    for (who, amount, hash) in rows {
        let minor = amount.to_minor();
        if minor <= 0 {
            return Err(ArcError::AmountOutOfRange(minor));
        }
        total += minor;
        to.push(*who);
        units.push(U256::from(minor as u128));
        hashes.push(*hash);
    }
    let call = IBudgetPolicy::settleRunIfCall {
        to,
        usdcUnits: units,
        actionHashes: hashes,
        terms,
        deadline,
    };
    Ok(PreparedCall {
        to: policy,
        data: format!("0x{}", alloy::hex::encode(call.abi_encode())),
        value: U256::ZERO,
        summary: format!(
            "pay {} bills yourself, {} in all; a bill already paid is skipped; valid only while the terms hash to {terms:#x} and until unix {deadline}",
            rows.len(),
            Money::minor(total, Currency::Usdc)
        ),
    })
}

/// `setPayeesIf(payees, attested, terms, deadline)`: the payee approval, bound
/// the same way.
pub fn prepare_payee_batch_bound(
    policy: Address,
    payees: &[Address],
    allow: bool,
    terms: B256,
    deadline: u64,
) -> PreparedCall {
    use alloy::sol_types::SolCall;
    let call = IBudgetPolicy::setPayeesIfCall {
        payees: payees.to_vec(),
        attested: allow,
        terms,
        deadline,
    };
    PreparedCall {
        to: policy,
        data: format!("0x{}", alloy::hex::encode(call.abi_encode())),
        value: U256::ZERO,
        summary: format!(
            "{} {} payee(s); valid only while the terms hash to {terms:#x} and until unix {deadline}",
            if allow { "approve" } else { "revoke" },
            payees.len()
        ),
    }
}

/// `setGuardians(list, threshold)` on the owner's own PasskeyPrincipal, for
/// the passkey to sign (PLAN 5.30): the addresses that may together propose a
/// replacement key, and how many must agree.
pub fn set_guardians_calldata(list: &[Address], threshold: u8) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    IPasskeyPrincipal::setGuardiansCall {
        list: list.to_vec(),
        threshold,
    }
    .abi_encode()
}

/// `renewAgent(until)` for the owner's passkey (PLAN 5.24): the agent may keep
/// paying until `until` (unix seconds, at most a year ahead); earlier than the
/// current expiry is the owner's brake.
pub fn renew_agent_calldata(until: u64) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    IBudgetPolicy::renewAgentCall { until }.abi_encode()
}

/// `removeSchedule(id)`: ending an order, instant.
pub fn remove_schedule_calldata(id: u64) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    IBudgetPolicy::removeScheduleCall { id: U256::from(id) }.abi_encode()
}

/// `approve(spender, minor)` on USDC, for the treasurer's wallet to sign in
/// pull mode: the policy may pull up to `minor` in total; `approve(spender, 0)`
/// cuts it off from any wallet software, without Tameion.
pub fn approve_calldata(spender: Address, minor: u128) -> Vec<u8> {
    use alloy::sol_types::SolCall;
    IUsdc::approveCall {
        spender,
        value: U256::from(minor),
    }
    .abi_encode()
}

/// The EIP-7997 keyless CREATE2 factory: the same address on every EVM chain
/// that deploys it (Arc testnet included, checked 2026-10-14).
pub const EIP7997_FACTORY: &str = "0x4e59b44847b379578588920cA78FbF26c0B4956C";

/// Where the factory puts `initcode` under `salt`: keccak256(0xff ‖ factory ‖
/// salt ‖ keccak256(initcode))[12..]. Pure, so the address can be printed
/// before anything is sent and checked by a test.
pub fn predict_create2(factory: Address, salt: B256, initcode: &[u8]) -> Address {
    factory.create2(salt, alloy::primitives::keccak256(initcode))
}

#[cfg(test)]
mod create2_tests {
    use super::*;

    #[test]
    fn the_terms_fingerprint_matches_cast_abi_encode_then_keccak() {
        // cast abi-encode 'f(uint256,uint256,uint256,uint8,address,address,uint256,uint64,uint64,uint8)'
        //   5000 50000 0 2 0x5245…5100 0x6c1c…9a25 60 1799736841 0 0 | cast keccak   (2026-10-14)
        let t = Terms {
            per_action_cap: U256::from(5000u64),
            daily_cap: U256::from(50_000u64),
            reserve_floor: U256::ZERO,
            mode: 2,
            agent: "0x524511aec2d76d9422e8550b9cb0014531005100"
                .parse()
                .unwrap(),
            operator: "0x6c1ce01bfe3f834a5fbfa0a9081d5d976b819a25"
                .parse()
                .unwrap(),
            delay: U256::from(60u64),
            agent_expiry: 1_799_736_841,
            pending_eta: 0,
            pending_kinds: 0,
            fingerprint: B256::ZERO,
        };
        assert_eq!(
            format!("{:#x}", t.recompute()),
            "0x95a4f8627252855cd26640bbeec95e9a4d924e1c6bceef775d858778547ad43c"
        );
        let mut moved = t.clone();
        moved.mode = 1;
        assert_ne!(
            moved.recompute(),
            t.recompute(),
            "the autonomy level is a term"
        );
    }

    #[test]
    fn approve_calldata_is_the_erc20_selector_spender_and_amount() {
        let d = approve_calldata(Address::repeat_byte(0xab), 1_500_000);
        let hex = alloy::hex::encode(&d);
        assert!(hex.starts_with("095ea7b3"), "{hex}");
        assert!(hex.contains(&"ab".repeat(20)), "{hex}");
        assert!(hex.ends_with(&format!("{:064x}", 1_500_000u64)), "{hex}");
        assert_eq!(d.len(), 4 + 64);
    }

    #[test]
    fn the_predicted_address_depends_only_on_factory_salt_and_initcode() {
        let factory = EIP7997_FACTORY.parse::<Address>().unwrap();
        let salt = B256::repeat_byte(0x01);
        let a = predict_create2(factory, salt, &[0x60, 0x00, 0x60, 0x00, 0xf3]);
        let b = predict_create2(factory, salt, &[0x60, 0x00, 0x60, 0x00, 0xf3]);
        assert_eq!(a, b, "same inputs, same address, whatever the chain");
        assert_ne!(
            a,
            predict_create2(
                factory,
                B256::repeat_byte(0x02),
                &[0x60, 0x00, 0x60, 0x00, 0xf3]
            )
        );
        assert_ne!(
            a,
            predict_create2(factory, salt, &[0x60, 0x00, 0x60, 0x00, 0xf3, 0x00])
        );
        // Spelled out: 0xff ‖ factory ‖ salt ‖ keccak(initcode), last 20 bytes.
        let mut pre = vec![0xffu8];
        pre.extend_from_slice(factory.as_slice());
        pre.extend_from_slice(salt.as_slice());
        pre.extend_from_slice(
            alloy::primitives::keccak256([0x60, 0x00, 0x60, 0x00, 0xf3]).as_slice(),
        );
        assert_eq!(
            a,
            Address::from_slice(&alloy::primitives::keccak256(pre)[12..])
        );
    }
}

/// What one receipt log says about a payment the agent just sent (PLAN 5.21):
/// `None` when it is not the policy's `Paid`, `Some(Ok)` when it names exactly
/// what was sent, `Some(Err)` with the difference in words.
pub fn paid_log_matches(
    log: &alloy::primitives::Log,
    policy: Address,
    to: Address,
    minor: U256,
    action_hash: B256,
) -> Option<Result<(), String>> {
    use alloy::sol_types::SolEvent;
    if log.address != policy {
        return None;
    }
    let ev = IBudgetPolicy::Paid::decode_log_data(&log.data).ok()?;
    Some(
        if ev.to == to && ev.usdcUnits == minor && ev.actionHash == action_hash {
            Ok(())
        } else {
            Err(format!(
                "the policy's Paid event says ({:#x}, {} minor, {:#x}); the agent sent ({to:#x}, {minor} minor, {action_hash:#x})",
                ev.to, ev.usdcUnits, ev.actionHash
            ))
        },
    )
}

#[cfg(test)]
mod paid_readback_tests {
    use super::*;
    use alloy::sol_types::SolEvent;

    #[test]
    fn a_paid_log_is_matched_field_by_field_and_other_logs_are_ignored() {
        let policy = Address::repeat_byte(0x11);
        let to = Address::repeat_byte(0x22);
        let hash = B256::repeat_byte(0x33);
        let ev = IBudgetPolicy::Paid {
            to,
            usdcUnits: U256::from(3000u64),
            actionHash: hash,
        };
        let log = alloy::primitives::Log {
            address: policy,
            data: ev.encode_log_data(),
        };
        assert_eq!(
            paid_log_matches(&log, policy, to, U256::from(3000u64), hash),
            Some(Ok(()))
        );
        assert!(
            matches!(
                paid_log_matches(&log, policy, to, U256::from(2999u64), hash),
                Some(Err(_))
            ),
            "amount"
        );
        assert!(
            matches!(
                paid_log_matches(
                    &log,
                    policy,
                    Address::repeat_byte(0x44),
                    U256::from(3000u64),
                    hash
                ),
                Some(Err(_))
            ),
            "payee"
        );
        assert!(
            matches!(
                paid_log_matches(
                    &log,
                    policy,
                    to,
                    U256::from(3000u64),
                    B256::repeat_byte(0x99)
                ),
                Some(Err(_))
            ),
            "action"
        );
        assert_eq!(
            paid_log_matches(
                &log,
                Address::repeat_byte(0x55),
                to,
                U256::from(3000u64),
                hash
            ),
            None,
            "another contract's log"
        );
        let transfer = alloy::primitives::Log {
            address: policy,
            data: IUsdc::Transfer {
                from: policy,
                to,
                value: U256::from(1u64),
            }
            .encode_log_data(),
        };
        assert_eq!(
            paid_log_matches(&transfer, policy, to, U256::from(3000u64), hash),
            None,
            "a Transfer is not a Paid"
        );
    }
}

#[cfg(test)]
mod passkey_tests {
    use super::*;

    #[test]
    fn the_owners_run_matches_cast_calldata() {
        let rows = [
            (
                Address::repeat_byte(0x11),
                Money::minor(2_000, Currency::Usdc),
                B256::repeat_byte(0xcc),
            ),
            (
                Address::repeat_byte(0x22),
                Money::minor(3_500, Currency::Usdc),
                B256::repeat_byte(0xdd),
            ),
        ];
        let c = prepare_principal_run_bound(
            Address::ZERO,
            &rows,
            B256::repeat_byte(0xab),
            1_791_961_000,
        )
        .unwrap();
        // cast calldata "settleRunIf(address[],uint256[],bytes32[],bytes32,uint64)" … (2026-10-14)
        assert_eq!(
            c.data,
            "0xbb7ae50000000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000160abababababababababababababababababababababababababababababababab000000000000000000000000000000000000000000000000000000006acf27a8000000000000000000000000000000000000000000000000000000000000000200000000000000000000000011111111111111111111111111111111111111110000000000000000000000002222222222222222222222222222222222222222000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000007d00000000000000000000000000000000000000000000000000000000000000dac0000000000000000000000000000000000000000000000000000000000000002ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
        );
        assert!(
            c.summary
                .starts_with("pay 2 bills yourself, 0.005500 USDC in all")
        );
        assert!(prepare_principal_run_bound(Address::ZERO, &[], B256::ZERO, 0).is_err());
        assert_eq!(
            &settle_run_selector()[..],
            &alloy::hex::decode(&c.data[2..10]).unwrap()[..]
        );
    }

    fn addr(s: &str) -> Address {
        s.parse().unwrap()
    }

    #[test]
    fn the_challenge_matches_what_the_deployed_contract_computed() {
        // Both vectors were produced by `PasskeyPrincipal.challengeFor` on
        // anvil (chain 31337) on 2026-10-02, and both were then signed in
        // Chrome and accepted by `execute`: proof/passkey-local-2026-10-02.txt.
        let owner = addr("0x5FC8d32690cc91D4c39d9d3abcBD16989F875707");
        let policy = addr("0x0165878A594ca255338adfa4d48449f69242Eb8F");
        let cases = [
            (
                "0x1111111111111111111111111111111111111111",
                0u64,
                "0x54f32e4c3fccc9d715f02c8d04d48f8c582784dffd55f04b5dc9459b08a7d32b",
            ),
            (
                "0x2222222222222222222222222222222222222222",
                1u64,
                "0xe9ff4f33afc246070f6470a88bc58baccb47826f470f32b2f4ed3fb96fca5f3f",
            ),
        ];
        for (payee, n, want) in cases {
            let call = prepare_payee_attestation(policy, addr(payee));
            let got = passkey_challenge(31337, owner, U256::from(n), &call).unwrap();
            assert_eq!(
                got,
                want.parse::<B256>().unwrap(),
                "payee {payee}, approval #{n}"
            );
        }
    }

    #[test]
    fn the_challenge_changes_with_every_field_it_binds() {
        let owner = addr("0x5FC8d32690cc91D4c39d9d3abcBD16989F875707");
        let policy = addr("0x0165878A594ca255338adfa4d48449f69242Eb8F");
        let call =
            prepare_payee_attestation(policy, addr("0x1111111111111111111111111111111111111111"));
        let base = passkey_challenge(31337, owner, U256::ZERO, &call).unwrap();

        let other_payee =
            prepare_payee_attestation(policy, addr("0x2222222222222222222222222222222222222222"));
        let mut other_target = call.clone();
        other_target.to = owner;
        let mut with_value = call.clone();
        with_value.value = U256::from(1);
        for (what, c) in [
            ("chain", passkey_challenge(421614, owner, U256::ZERO, &call)),
            ("owner", passkey_challenge(31337, policy, U256::ZERO, &call)),
            (
                "nonce",
                passkey_challenge(31337, owner, U256::from(1), &call),
            ),
            (
                "calldata",
                passkey_challenge(31337, owner, U256::ZERO, &other_payee),
            ),
            (
                "target",
                passkey_challenge(31337, owner, U256::ZERO, &other_target),
            ),
            (
                "value",
                passkey_challenge(31337, owner, U256::ZERO, &with_value),
            ),
        ] {
            assert_ne!(
                c.unwrap(),
                base,
                "changing the {what} must change the challenge"
            );
        }
    }

    #[test]
    fn a_batch_encodes_set_payees_with_every_address() {
        use alloy::sol_types::SolCall;
        let policy = addr("0x0165878A594ca255338adfa4d48449f69242Eb8F");
        let list = [
            addr("0x1111111111111111111111111111111111111111"),
            addr("0x3333333333333333333333333333333333333333"),
        ];
        let p = prepare_payee_batch(policy, &list);
        let decoded = IBudgetPolicy::setPayeesCall::abi_decode(
            &alloy::hex::decode(p.data.trim_start_matches("0x")).unwrap(),
        )
        .unwrap();
        assert_eq!(decoded.payees, list.to_vec());
        assert!(decoded.attested);
        assert!(p.summary.contains("2 payees"));
    }

    #[test]
    fn attestation_encodes_set_payee_true() {
        use alloy::sol_types::SolCall;
        let policy = addr("0x0165878A594ca255338adfa4d48449f69242Eb8F");
        let p =
            prepare_payee_attestation(policy, addr("0x3333333333333333333333333333333333333333"));
        let want = alloy::hex::encode(IBudgetPolicy::setPayeeCall::SELECTOR);
        assert!(p.data.starts_with(&format!("0x{want}")));
        assert!(
            p.data.ends_with(&format!("{:064x}", 1)),
            "attested must be true"
        );
        assert_eq!(p.to, policy);
    }
}

#[cfg(test)]
mod prepare_tests {
    use super::*;
    use ledger::Currency;

    #[test]
    fn encodes_settle_as_principal_and_neither_pay_nor_withdraw() {
        // Two bugs this guards. Encoding `pay` gives the human a transaction
        // that reverts NotAgent. Encoding `withdraw` gives them one that
        // works but leaves the cycle unmarked, so the agent escalates it
        // again forever.
        use alloy::sol_types::SolCall;
        let policy: Address = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
            .parse()
            .unwrap();
        let to: Address = "0x3333333333333333333333333333333333333333"
            .parse()
            .unwrap();
        let hash = B256::repeat_byte(0xab);
        let p = prepare_principal_settlement(
            policy,
            to,
            Money::minor(150_000_000, Currency::Usdc),
            hash,
        )
        .unwrap();

        let want = alloy::hex::encode(IBudgetPolicy::settleAsPrincipalCall::SELECTOR);
        assert!(
            p.data.starts_with(&format!("0x{want}")),
            "must be settleAsPrincipal"
        );
        for wrong in [
            alloy::hex::encode(IBudgetPolicy::payCall::SELECTOR),
            alloy::hex::encode(IBudgetPolicy::withdrawCall::SELECTOR),
        ] {
            assert!(
                !p.data.starts_with(&format!("0x{wrong}")),
                "must not be {wrong}"
            );
        }

        // settleAsPrincipal(address,uint256,bytes32): selector + three words.
        assert_eq!((p.data.len() - 2) / 2, 4 + 32 * 3);
        assert_eq!(p.to, policy);
        assert_eq!(p.value, U256::ZERO);
        let body = p.data.to_lowercase();
        assert!(body.contains(&"3333333333333333333333333333333333333333".to_string()));
        assert!(body.contains(&format!("{:064x}", 150_000_000u128)));
        assert!(body.contains(&"ab".repeat(32)));
    }

    #[test]
    fn summary_says_the_cycle_gets_marked() {
        let a: Address = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
            .parse()
            .unwrap();
        let p = prepare_principal_settlement(a, a, Money::minor(1, Currency::Usdc), B256::ZERO)
            .unwrap();
        assert!(p.summary.contains("executed"), "{}", p.summary);
    }

    #[test]
    fn refuses_to_prepare_a_non_positive_amount() {
        let a: Address = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
            .parse()
            .unwrap();
        for minor in [0i128, -1] {
            assert!(matches!(
                prepare_principal_settlement(a, a, Money::minor(minor, Currency::Usdc), B256::ZERO),
                Err(ArcError::AmountOutOfRange(_))
            ));
        }
    }

    #[test]
    fn preparing_needs_no_signer_and_no_network() {
        // The point of the handover: an agent forbidden to send this can
        // still compose it.
        let a: Address = "0x5FbDB2315678afecb367f032d93F642f64180aa3"
            .parse()
            .unwrap();
        let p = prepare_principal_settlement(a, a, Money::minor(1, Currency::Usdc), B256::ZERO)
            .unwrap();
        assert!(p.summary.contains("0.000001 USDC"));
    }
}

#[cfg(test)]
mod eight_at_a_time_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn keeps_order_and_never_more_than_eight_in_flight() {
        let (now, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let out = eight_at_a_time((0..30u64).rev().collect(), |i| {
            let (now, peak) = (now.clone(), peak.clone());
            async move {
                let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                // Later items finish first, so order comes from the helper.
                tokio::time::sleep(std::time::Duration::from_millis(i)).await;
                now.fetch_sub(1, Ordering::SeqCst);
                i
            }
        })
        .await
        .unwrap();
        assert_eq!(out, (0..30u64).rev().collect::<Vec<_>>());
        assert_eq!(peak.load(Ordering::SeqCst), 8);
    }
}

/// Audit 2026-10-15 (docs/60 P-74): a read that failed must not be taken for
/// an older contract (a revert), and a history walk must not skip blocks the
/// node serves. A local JSON-RPC stand-in answers by method and selector.
#[cfg(test)]
mod fail_closed_tests {
    use super::*;
    use alloy::primitives::keccak256;
    use alloy::sol_types::SolCall;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    type Reply = Result<Value, (i64, &'static str)>;

    /// Serve `answer(method, params)` over HTTP, one request per connection.
    async fn node(answer: impl Fn(&str, &Value) -> Reply + Send + Sync + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let answer = Arc::new(answer);
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let answer = answer.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let body = loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len: usize = text[..end]
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length")
                                        .then(|| v.trim().parse().ok())?
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break text[end + 4..end + 4 + len].to_string();
                            }
                        }
                    };
                    let req: Value = serde_json::from_str(&body).unwrap();
                    let reply = match answer(req["method"].as_str().unwrap_or(""), &req["params"]) {
                        Ok(r) => json!({"jsonrpc": "2.0", "id": req["id"], "result": r}),
                        Err((code, message)) => json!({"jsonrpc": "2.0", "id": req["id"],
                            "error": {"code": code, "message": message}}),
                    };
                    let out = reply.to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                        out.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        url
    }

    /// A page's read that stalls is sent again and answered (docs/110 item 8):
    /// the quick client gives up on the stalled request at its three-second cap
    /// and retries, instead of failing the read or waiting a minute.
    #[tokio::test]
    async fn a_stalled_quick_read_is_sent_again_and_answered() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (stalled, _) = listener.accept().await.unwrap();
            let (mut sock, _) = listener.accept().await.unwrap();
            let (mut buf, mut chunk) = (Vec::new(), [0u8; 4096]);
            let body = loop {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                let Some(end) = text.find("\r\n\r\n") else {
                    continue;
                };
                let len: usize = text[..end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")?
                            .trim()
                            .parse()
                            .ok()
                    })
                    .unwrap_or(0);
                if buf.len() >= end + 4 + len {
                    break text[end + 4..end + 4 + len].to_string();
                }
            };
            let id = serde_json::from_str::<Value>(&body).unwrap()["id"].clone();
            let out = json!({"jsonrpc": "2.0", "id": id, "result": "0x2a"}).to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                out.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            drop(stalled);
        });
        let started = std::time::Instant::now();
        let head = ArcClient::read_only_quick(&url)
            .unwrap()
            .block_number()
            .await;
        assert_eq!(head.unwrap(), 42);
        let took = started.elapsed();
        assert!(took.as_secs() >= 3 && took.as_secs() < 6, "{took:?}");
    }

    fn selector(params: &Value) -> [u8; 4] {
        let call = &params[0];
        let data = call["input"]
            .as_str()
            .or(call["data"].as_str())
            .unwrap_or("0x");
        let bytes = alloy::hex::decode(data).unwrap_or_default();
        bytes.get(..4).map_or([0; 4], |s| s.try_into().unwrap())
    }

    const REVERT: (i64, &str) = (3, "execution reverted");
    /// A node error that is neither a revert nor retried by the client.
    const NODE_ERROR: (i64, &str) = (-32000, "internal error");

    async fn schedules_when_count(err: (i64, &'static str)) -> Result<Vec<Schedule>, ArcError> {
        let url = node(move |method, params| {
            assert_eq!(method, "eth_call");
            assert_eq!(selector(params), IBudgetPolicy::scheduleCountCall::SELECTOR);
            Err(err)
        })
        .await;
        ArcClient::read_only(&url)
            .unwrap()
            .schedules(Address::ZERO)
            .await
    }

    #[tokio::test]
    async fn schedules_are_none_only_when_the_policy_has_no_such_function() {
        assert!(schedules_when_count(REVERT).await.unwrap().is_empty());
        let failed = schedules_when_count(NODE_ERROR).await;
        assert!(
            failed.is_err(),
            "a failed read is not an empty list: {failed:?}"
        );
    }

    async fn attested_when(err: (i64, &'static str)) -> Result<Option<BTreeSet<String>>, ArcError> {
        let url = node(move |_, params| {
            assert_eq!(selector(params), IBudgetPolicy::attestedPayeeCall::SELECTOR);
            Err(err)
        })
        .await;
        ArcClient::read_only(&url)
            .unwrap()
            .attested_among(Address::ZERO, &[Address::repeat_byte(0x11)])
            .await
    }

    #[tokio::test]
    async fn the_payee_list_is_absent_only_when_the_policy_has_no_such_function() {
        assert_eq!(attested_when(REVERT).await.unwrap(), None);
        let failed = attested_when(NODE_ERROR).await;
        assert!(
            failed.is_err(),
            "a failed read is not 'no list': {failed:?}"
        );
    }

    fn block_of(v: &Value) -> u64 {
        u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    }

    /// A node that serves logs from block 105 on, with one `Paid` at 110.
    /// `probe_fails` makes every one-block probe fail with a node error.
    async fn horizon_node(probe_fails: bool) -> String {
        node(move |method, params| {
            assert_eq!(method, "eth_getLogs");
            let (from, to) = (
                block_of(&params[0]["fromBlock"]),
                block_of(&params[0]["toBlock"]),
            );
            if probe_fails && from == to {
                return Err(NODE_ERROR);
            }
            if from < 105 {
                return Err((-32014, "requested data not available"));
            }
            let logs = if (from..=to).contains(&110) {
                vec![json!({
                    "address": format!("{:#x}", Address::ZERO),
                    "topics": [format!("{:#x}", keccak256("Paid(address,uint256,bytes32)"))],
                    "data": "0x",
                    "blockNumber": "0x6e",
                    "blockHash": format!("{:#x}", B256::repeat_byte(1)),
                    "transactionHash": format!("{:#x}", B256::repeat_byte(0xaa)),
                    "transactionIndex": "0x0",
                    "logIndex": "0x0",
                    "removed": false
                })]
            } else {
                vec![]
            };
            Ok(json!(logs))
        })
        .await
    }

    #[tokio::test]
    async fn the_served_part_of_the_boundary_chunk_is_read() {
        // One chunk, 100..=120, refused because 100..=104 are gone. The
        // payment at 110 is inside it and must be found.
        let url = horizon_node(false).await;
        let paid = ArcClient::read_only(&url)
            .unwrap()
            .agent_payments(Address::ZERO, 100, 120)
            .await
            .unwrap();
        assert!(
            paid.contains(&format!("{:#x}", B256::repeat_byte(0xaa))),
            "the payment at block 110 was skipped: {paid:?}"
        );
    }

    #[tokio::test]
    async fn a_horizon_probe_that_fails_fails_the_walk() {
        let url = horizon_node(true).await;
        let r = ArcClient::read_only(&url)
            .unwrap()
            .agent_payments(Address::ZERO, 100, 120)
            .await;
        assert!(
            r.is_err(),
            "a guessed horizon is a hole in the history: {r:?}"
        );
    }
}
