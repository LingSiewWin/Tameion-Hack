// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";

/// @dev Drives random sequences of agent payments and clock moves, and keeps
///      its own record of what actually went through.
contract PolicyHandler is Test {
    BudgetPolicy public policy;
    address public agent;
    address public vendor;

    uint256[] public paidAt;
    uint256[] public paidAmt;
    uint256 internal nonce;

    constructor(BudgetPolicy _policy, address _agent, address _vendor) {
        policy = _policy;
        agent = _agent;
        vendor = _vendor;
    }

    function pay(uint256 amount) external {
        amount = bound(amount, 1, 150_000000);
        vm.prank(agent);
        try policy.pay(vendor, amount, keccak256(abi.encode("inv", nonce++))) {
            paidAt.push(vm.getBlockTimestamp());
            paidAmt.push(amount);
        } catch {}
    }

    function wait(uint256 secs) external {
        vm.warp(vm.getBlockTimestamp() + bound(secs, 1, 30 hours));
    }

    function count() external view returns (uint256) {
        return paidAt.length;
    }
}

/// forge-config: default.invariant.runs = 64
/// forge-config: default.invariant.depth = 80
contract BudgetPolicyInvariantTest is Test {
    BudgetPolicy internal policy;
    PolicyHandler internal handler;

    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    address internal agent = address(0xBEEF);
    address internal vendor = address(0x1111);
    uint256 internal constant USDC = 1_000000;

    function setUp() public {
        vm.warp(1_764_000_000);
        vm.prank(principal);
        policy = new BudgetPolicy(
            principal,
            agent,
            operator,
            address(0x8004),
            42,
            100 * USDC,
            300 * USDC,
            50 * USDC,
            48 hours,
            BudgetPolicyBase.Mode.Autopilot
        );
        vm.deal(address(policy), 100_000 * USDC * policy.WEI_PER_USDC_UNIT());
        vm.prank(principal);
        policy.setPayee(vendor, true);
        vm.warp(vm.getBlockTimestamp() + policy.delay());

        handler = new PolicyHandler(policy, agent, vendor);
        targetContract(address(handler));
    }

    /// Every window that ends at a payment holds at most the cap, judged from
    /// the handler's record of payments that succeeded.
    function invariant_no_trailing_window_exceeds_the_cap() public view {
        uint256 n = handler.count();
        uint256 window = policy.WINDOW();
        for (uint256 i = 0; i < n; i++) {
            uint256 ti = handler.paidAt(i);
            uint256 sum;
            for (uint256 j = 0; j <= i; j++) {
                if (ti - handler.paidAt(j) < window) sum += handler.paidAmt(j);
            }
            assertLe(sum, policy.dailyCap());
        }
    }

    /// The contract's own counter agrees with the record.
    function invariant_counter_matches_the_record() public view {
        uint256 n = handler.count();
        uint256 window = policy.WINDOW();
        uint256 sum;
        for (uint256 j = 0; j < n; j++) {
            if (vm.getBlockTimestamp() - handler.paidAt(j) < window) sum += handler.paidAmt(j);
        }
        assertEq(policy.spentInWindow(), sum);
    }

    function invariant_reserve_floor_holds() public view {
        assertGe(policy.balanceUsdc(), policy.reserveFloor());
    }
}
