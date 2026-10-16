// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {BudgetPolicyNative} from "./BudgetPolicy.sol";

/// @title  TameionDelegate
/// @notice The Tameion policy, run inside the company's own wallet (EIP-7702,
///         PLAN 5.28). The wallet signs one authorization that points its code
///         at this contract and, in the same transaction, initialises itself:
///         its address does not change, its money does not move, there is no
///         allowance to grant. From then on the agent key can call `pay` on the
///         wallet's own address — to attested payees, under the caps, in
///         Autopilot only, until its term lapses — exactly as on a
///         `BudgetPolicy`, because it *is* that code. The owner's passkey (the
///         principal) attests payees, withdraws and renews the agent.
/// @dev    What this does not do, said plainly: EIP-7702 does not take the
///         wallet's own private key away. Whoever holds that key can still send
///         from the wallet directly, or point its code elsewhere. The delegate
///         bounds the *agent*, not the wallet's owner.
///         Storage is the wallet's own, from slot 0. A wallet that delegated to
///         other code before may carry that code's storage; `initialize`
///         overwrites every scalar it uses, but cannot clear old mapping
///         entries, so delegate from a wallet that never delegated, or whose
///         earlier delegate you trust.
contract TameionDelegate is BudgetPolicyNative {
    /// @notice Only the wallet itself may initialise its own storage.
    error NotSelf();

    /// @notice Initialise the policy in the wallet's own storage. Callable only
    ///         by the wallet itself (a self-call in the delegating transaction),
    ///         so nobody can initialise someone else's wallet, and the
    ///         implementation at its own address can never be initialised.
    function initialize(
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
    ) external {
        if (msg.sender != address(this)) revert NotSelf();
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
    }
}
