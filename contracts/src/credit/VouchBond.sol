// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @title  VouchBond
/// @notice The deposit an account locks to back its own credit line. It is
///         native USDC (Arc) and leaves the vault only two ways: back to the
///         account, after a cooling-off, or to cover what the account failed
///         to repay (`CreditLine.slash`).
/// @dev    Amounts are USDC minor units (six decimals), the integers the
///         off-chain ledger uses. Native wei divided by 1e12 gives minor units;
///         the conversion happens here and nowhere else, and a value that is
///         not a whole number of minor units is refused rather than rounded,
///         so the book and the balance never drift apart.
abstract contract VouchBond {
    /// @dev 1 USDC minor unit (1e-6 USDC) in native wei on Arc.
    uint256 public constant WEI_PER_USDC_UNIT = 1e12;

    /// @notice How long an unlock waits, in seconds. A withdrawal shrinks what
    ///         backs the line, so it waits, and the deposit stays slashable
    ///         until the money has actually left.
    uint256 public immutable delay;

    /// @notice What each account has locked, net of unlocks and slashes.
    mapping(address => uint256) public deposit;

    /// @notice The sum of every `deposit`. A draw must leave at least this
    ///         much in the vault: deposits back the line, they are never lent.
    uint256 public totalDeposits;

    /// @dev One pending unlock per account; a new request replaces it and
    ///      restarts the clock.
    struct Unlock {
        uint256 amount;
        uint256 eta;
    }

    mapping(address => Unlock) public pendingUnlock;

    event Locked(address indexed account, uint256 usdcUnits);
    event UnlockRequested(address indexed account, uint256 usdcUnits, uint256 eta);
    event Unlocked(address indexed account, uint256 usdcUnits);

    error ZeroAmount();
    error NotWholeUnits(uint256 weiAmount);
    error OverDeposit(uint256 requested, uint256 deposit);
    error NothingPending();
    error TooEarly(uint256 eta);
    error TransferFailed();

    constructor(uint256 _delay) {
        delay = _delay;
    }

    /// @notice Lock native USDC as this account's deposit.
    function lock() external payable {
        uint256 units = _units(msg.value);
        deposit[msg.sender] += units;
        totalDeposits += units;
        emit Locked(msg.sender, units);
    }

    /// @notice Start the cooling-off for taking `usdcUnits` back.
    function requestUnlock(uint256 usdcUnits) external {
        if (usdcUnits == 0) revert ZeroAmount();
        uint256 d = deposit[msg.sender];
        if (usdcUnits > d) revert OverDeposit(usdcUnits, d);
        uint256 eta = block.timestamp + delay;
        pendingUnlock[msg.sender] = Unlock({amount: usdcUnits, eta: eta});
        emit UnlockRequested(msg.sender, usdcUnits, eta);
    }

    /// @notice Take the requested amount back once the cooling-off is over.
    ///         Re-checked against the deposit as it is now: a slash during
    ///         the wait shrinks what can leave.
    function unlock() external {
        Unlock memory u = pendingUnlock[msg.sender];
        if (u.amount == 0) revert NothingPending();
        if (block.timestamp < u.eta) revert TooEarly(u.eta);
        _beforeUnlock(msg.sender);
        uint256 d = deposit[msg.sender];
        if (u.amount > d) revert OverDeposit(u.amount, d);
        delete pendingUnlock[msg.sender];
        deposit[msg.sender] = d - u.amount;
        totalDeposits -= u.amount;
        emit Unlocked(msg.sender, u.amount);
        _send(msg.sender, u.amount);
    }

    /// @dev The line's own condition for letting a deposit leave.
    function _beforeUnlock(address account) internal view virtual;

    function _units(uint256 weiAmount) internal pure returns (uint256) {
        if (weiAmount == 0) revert ZeroAmount();
        if (weiAmount % WEI_PER_USDC_UNIT != 0) revert NotWholeUnits(weiAmount);
        return weiAmount / WEI_PER_USDC_UNIT;
    }

    /// @dev Every state change is written before this call, so a receiver
    ///      that re-enters finds the books already updated.
    function _send(address to, uint256 usdcUnits) internal {
        (bool ok,) = to.call{value: usdcUnits * WEI_PER_USDC_UNIT}("");
        if (!ok) revert TransferFailed();
    }
}
