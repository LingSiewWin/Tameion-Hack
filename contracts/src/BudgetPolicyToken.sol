// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {IERC20Metadata} from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {BudgetPolicyBase} from "./BudgetPolicy.sol";

/// @title  BudgetPolicyToken
/// @notice The policy on a chain where USDC is an ERC-20 rather than the
///         native asset: Arbitrum, Base, Ethereum. Same rules, same ABI for
///         the agent; only the asset differs.
/// @dev    The token is fixed at deployment and must have six decimals, so a
///         limit in USDC minor units and a token amount are the same integer.
///         A policy pointed at an 18-decimal token would read a cap of 100
///         USDC as 0.0000000001 of it; that is refused here, not discovered
///         later. Funding is a plain `transfer` to this contract; native coin
///         sent here is refused, since nothing could account for it.
contract BudgetPolicyToken is BudgetPolicyBase {
    using SafeERC20 for IERC20;

    /// @notice The USDC contract this policy holds and pays in.
    IERC20 public immutable usdc;

    error NotSixDecimals(address token, uint8 decimals);

    constructor(
        address _usdc,
        address _principal,
        address _agent,
        address _operator,
        address _identityRegistry,
        uint256 _agentId,
        uint256 _perActionCap,
        uint256 _dailyCap,
        uint256 _reserveFloor,
        uint256 _delay,
        Mode _mode
    ) {
        _init(
            _principal,
            _agent,
            _operator,
            _identityRegistry,
            _agentId,
            _perActionCap,
            _dailyCap,
            _reserveFloor,
            _delay,
            _mode
        );
        if (_usdc == address(0)) revert ZeroAddress();
        uint8 d = IERC20Metadata(_usdc).decimals();
        if (d != 6) revert NotSixDecimals(_usdc, d);
        usdc = IERC20(_usdc);
    }

    function usdcToken() public view override returns (address) {
        return address(usdc);
    }

    function balanceUsdc() public view override returns (uint256) {
        return usdc.balanceOf(address(this));
    }

    function _send(address to, uint256 usdcUnits) internal override {
        usdc.safeTransfer(to, usdcUnits);
    }
}
