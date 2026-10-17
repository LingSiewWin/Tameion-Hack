// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {VouchBond} from "./VouchBond.sol";

/// @title  CreditLine
/// @notice No external lender; testnet; not a lending protocol.
///
///         An account locks a deposit (`VouchBond`) and may then draw up to
///         `limit` = min(deposit x multiplier(score) / 1e4, cap) into the
///         mandate contract it registered once (its `BudgetPolicy`), where
///         the agent can spend it only inside the mandate. The score is set by
///         the `scorer` (the service) and counts as 0 once older than
///         `maxAge`. If the account does not repay, the `judge` slashes
///         min(debt, deposit) to cover the debt; a slash names only the hash
///         of its evidence and the id of the mandate rule that was broken,
///         never a model's output or any free text.
///
/// @dev    The vault is this contract's native balance. Deposits back the line
///         and are never lent: a draw comes out of the float above
///         `totalDeposits` (USDC anyone sent here; on testnet, the operator's
///         own treasury), and a slash turns the slashed deposit into float to
///         replace what was drawn. Roles are fixed at deployment.
contract CreditLine is VouchBond {
    /// @notice Sets scores. The service.
    address public immutable scorer;

    /// @notice Slashes and pauses.
    address public immutable judge;

    /// @notice Most any one account may owe, in USDC minor units.
    uint256 public immutable cap;

    /// @notice A score older than this many seconds counts as 0.
    uint256 public immutable maxAge;

    /// @notice Latest score per account, in basis points (0..10,000).
    mapping(address => uint256) public scoreBps;

    /// @notice When that score was set (unix seconds; 0 = never).
    mapping(address => uint256) public scoredAt;

    /// @notice What each account owes, in USDC minor units.
    mapping(address => uint256) public debt;

    /// @notice Where each account's draws are paid: its mandate contract,
    ///         registered once by the account itself.
    mapping(address => address) public policyOf;

    /// @notice While true, nothing can be drawn. Repaying and locking still work.
    bool public paused;

    event Funded(address indexed from, uint256 weiAmount);
    event ScoreSet(address indexed account, uint256 bps);
    event PolicySet(address indexed account, address indexed policy);
    event Drawn(address indexed account, address indexed policy, uint256 usdcUnits);
    event Repaid(address indexed account, uint256 usdcUnits);
    event Slashed(address indexed account, uint256 usdcUnits, bytes32 indexed evidenceHash, uint8 ruleId);
    event PauseSet(bool paused);

    error NotScorer();
    error NotJudge();
    error ZeroAddress();
    error BadScore(uint256 bps);
    error PolicyAlreadySet(address policy);
    error NoPolicy();
    error LinePaused();
    error OverLimit(uint256 wouldOwe, uint256 limit);
    error NoFloat(uint256 requested, uint256 available);
    error OverRepay(uint256 amount, uint256 debt);
    error StillOwed(uint256 debt);
    error NoEvidence();
    error NothingToSlash();

    constructor(address _scorer, address _judge, uint256 _cap, uint256 _maxAge, uint256 _delay) VouchBond(_delay) {
        if (_scorer == address(0) || _judge == address(0)) revert ZeroAddress();
        scorer = _scorer;
        judge = _judge;
        cap = _cap;
        maxAge = _maxAge;
    }

    /// @notice Float for draws. Anyone may add; it is never a deposit.
    receive() external payable {
        emit Funded(msg.sender, msg.value);
    }

    // --- reads -------------------------------------------------------------

    /// @notice Credit per unit of deposit, in basis points, for a score.
    function multiplier(uint256 bps) public pure returns (uint256) {
        if (bps < 2000) return 0;
        if (bps < 5000) return 5000;
        if (bps < 8000) return 10_000;
        return 15_000;
    }

    /// @notice The account's score if it is fresh, else 0.
    function scoreOf(address account) public view returns (uint256) {
        if (block.timestamp - scoredAt[account] > maxAge) return 0;
        return scoreBps[account];
    }

    /// @notice The most the account may owe right now.
    function limit(address account) public view returns (uint256) {
        uint256 l = deposit[account] * multiplier(scoreOf(account)) / 10_000;
        return l < cap ? l : cap;
    }

    /// @notice Float available to draw: the vault above the deposits.
    function available() public view returns (uint256) {
        uint256 vault = address(this).balance / WEI_PER_USDC_UNIT;
        return vault > totalDeposits ? vault - totalDeposits : 0;
    }

    // --- the account ---------------------------------------------------------

    /// @notice Register, once, the mandate contract draws are paid into.
    function setPolicy(address policy) external {
        if (policy == address(0)) revert ZeroAddress();
        address current = policyOf[msg.sender];
        if (current != address(0)) revert PolicyAlreadySet(current);
        policyOf[msg.sender] = policy;
        emit PolicySet(msg.sender, policy);
    }

    /// @notice Draw into the registered policy. The debt is recorded in the
    ///         same transaction as the payment: if the payment fails, so does
    ///         the debt.
    function draw(uint256 usdcUnits) external {
        if (paused) revert LinePaused();
        if (usdcUnits == 0) revert ZeroAmount();
        address policy = policyOf[msg.sender];
        if (policy == address(0)) revert NoPolicy();
        uint256 owe = debt[msg.sender] + usdcUnits;
        uint256 lim = limit(msg.sender);
        if (owe > lim) revert OverLimit(owe, lim);
        uint256 free = available();
        if (usdcUnits > free) revert NoFloat(usdcUnits, free);
        debt[msg.sender] = owe;
        emit Drawn(msg.sender, policy, usdcUnits);
        _send(policy, usdcUnits);
    }

    /// @notice Repay the caller's debt with native USDC. Never more than owed.
    function repay() external payable {
        uint256 units = _units(msg.value);
        uint256 d = debt[msg.sender];
        if (units > d) revert OverRepay(units, d);
        debt[msg.sender] = d - units;
        emit Repaid(msg.sender, units);
    }

    /// @dev A deposit cannot leave while anything is owed.
    function _beforeUnlock(address account) internal view override {
        uint256 d = debt[account];
        if (d != 0) revert StillOwed(d);
    }

    // --- the scorer and the judge -------------------------------------------

    function setScore(address account, uint256 bps) external {
        if (msg.sender != scorer) revert NotScorer();
        if (bps > 10_000) revert BadScore(bps);
        scoreBps[account] = bps;
        scoredAt[account] = block.timestamp;
        emit ScoreSet(account, bps);
    }

    /// @notice Cover the account's debt from its deposit: min(debt, deposit).
    ///         Looks at nothing but the evidence hash and the rule id it is
    ///         given; whatever exceeds the deposit stays owed.
    function slash(address account, bytes32 evidenceHash, uint8 ruleId) external {
        if (msg.sender != judge) revert NotJudge();
        if (evidenceHash == bytes32(0) || ruleId == 0) revert NoEvidence();
        uint256 d = debt[account];
        uint256 dep = deposit[account];
        uint256 amount = d < dep ? d : dep;
        if (amount == 0) revert NothingToSlash();
        debt[account] = d - amount;
        deposit[account] = dep - amount;
        totalDeposits -= amount;
        emit Slashed(account, amount, evidenceHash, ruleId);
    }

    function setPaused(bool p) external {
        if (msg.sender != judge) revert NotJudge();
        paused = p;
        emit PauseSet(p);
    }
}
