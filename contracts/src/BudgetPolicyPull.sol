// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {IERC20Metadata} from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {BudgetPolicyBase} from "./BudgetPolicy.sol";

/// @title  BudgetPolicyPull
/// @notice The policy that never holds the money (PLAN 5.26, docs/59 §3.2,
///         the ERC-5827 idea without a new token standard). The company's
///         USDC stays in its own wallet, `source`; the wallet grants this
///         contract an ERC-20 allowance; every payment is a `transferFrom`
///         inside the same caps, payee list, modes and delays as the holding
///         policy. What the agent can reach is the smaller of the wallet's
///         balance and the allowance, so the allowance is a second cap the
///         treasurer holds in their own hands and can cut to zero at any time
///         with one `approve(policy, 0)`, from any wallet software, without
///         Tameion.
/// @dev    Same ABI for the agent and the same `Paid` event, so the operator,
///         the read-back and the audit do not change. USDC must have six
///         decimals (Arc's native USDC answers the ERC-20 interface at
///         0x3600…0000 with `decimals() == 6`, checked 2026-10-14). Native coin
///         sent here is refused: nothing could account for it. `withdraw`
///         pulls from `source` to `to` on the principal's authority, which on
///         this variant is the treasurer moving their own money through the
///         same recorded path.
contract BudgetPolicyPull is BudgetPolicyBase {
    using SafeERC20 for IERC20;

    /// @notice The USDC contract payments are pulled in.
    IERC20 public immutable usdc;
    /// @notice The wallet the money stays in and is pulled from.
    address public immutable source;

    error NotSixDecimals(address token, uint8 decimals);

    constructor(
        address _usdc,
        address _source,
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
        if (_usdc == address(0) || _source == address(0)) revert ZeroAddress();
        uint8 d = IERC20Metadata(_usdc).decimals();
        if (d != 6) revert NotSixDecimals(_usdc, d);
        usdc = IERC20(_usdc);
        source = _source;
    }

    /// @notice What this policy can reach right now: the smaller of the
    ///         wallet's balance and what it allowed. The reserve floor applies
    ///         to this number, so "never spend below X" means "leave X in the
    ///         wallet", not "leave X in a contract".
    function usdcToken() public view override returns (address) {
        return address(usdc);
    }

    function balanceUsdc() public view override returns (uint256) {
        uint256 held = usdc.balanceOf(source);
        uint256 allowed = usdc.allowance(source, address(this));
        return held < allowed ? held : allowed;
    }

    /// @notice The allowance alone, for the page: "the agent may still pull
    ///         up to this much before you approve more".
    function allowanceRemaining() external view returns (uint256) {
        return usdc.allowance(source, address(this));
    }

    function _send(address to, uint256 usdcUnits) internal override {
        usdc.safeTransferFrom(source, to, usdcUnits);
    }
}
