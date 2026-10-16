// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";

/// @dev The agent's authority lapses (PLAN 5.24, the EIP-8130 shape): a key
///      nobody re-confirmed in ninety days stops paying by itself; the owner
///      renews it with one call; the owner's own instructions do not depend on it.
contract BudgetPolicyAgentExpiryTest is Test {
    BudgetPolicy internal policy;

    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    address internal agent = address(0xBEEF);
    address internal vendor = address(0x1111);
    address internal registry = 0x8004A818BFB912233c491871b3d84c89A494BD9e;

    uint256 internal constant USDC = 1_000000;
    uint256 internal constant WEI_PER_USDC = 1e12;

    function setUp() public {
        vm.warp(1_764_000_000);
        vm.prank(principal);
        policy = new BudgetPolicy(
            principal,
            agent,
            operator,
            registry,
            42,
            100 * USDC,
            500 * USDC,
            0,
            48 hours,
            BudgetPolicyBase.Mode.Autopilot
        );
        vm.deal(address(policy), 1000 * USDC * WEI_PER_USDC);
        vm.prank(principal);
        policy.setPayee(vendor, true);
        vm.warp(vm.getBlockTimestamp() + policy.delay());
    }

    function test_the_term_starts_at_ninety_days_and_the_agent_pays_inside_it() public {
        assertEq(policy.agentExpiry(), 1_764_000_000 + 90 days);
        vm.prank(agent);
        policy.pay(vendor, 1 * USDC, keccak256("a"));
        assertEq(vendor.balance, 1 * USDC * WEI_PER_USDC);
    }

    function test_after_the_term_the_agent_is_refused_by_name() public {
        vm.warp(1_764_000_000 + 90 days);
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AgentLapsed.selector, uint64(1_764_000_000 + 90 days)));
        policy.pay(vendor, 1 * USDC, keccak256("a"));
    }

    function test_the_principal_renews_instantly_and_nobody_else_can() public {
        uint64 until = uint64(vm.getBlockTimestamp() + 30 days);
        vm.prank(operator);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.renewAgent(until);
        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.renewAgent(until);
        vm.expectEmit(true, true, true, true, address(policy));
        emit BudgetPolicyBase.AgentRenewed(until, principal);
        vm.prank(principal);
        policy.renewAgent(until);
        assertEq(policy.agentExpiry(), until);
    }

    function test_renewal_is_bounded_to_a_year_and_must_be_in_the_future() public {
        uint64 now_ = uint64(vm.getBlockTimestamp());
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.BadExpiry.selector, now_));
        policy.renewAgent(now_);
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.BadExpiry.selector, now_ + 366 days));
        policy.renewAgent(now_ + 366 days);
        vm.prank(principal);
        policy.renewAgent(now_ + 365 days);
    }

    function test_shortening_is_the_owners_brake_and_renewing_restores_payment() public {
        uint64 soon = uint64(vm.getBlockTimestamp() + 1 hours);
        vm.prank(principal);
        policy.renewAgent(soon);
        vm.warp(soon);
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AgentLapsed.selector, soon));
        policy.pay(vendor, 1 * USDC, keccak256("b"));
        vm.prank(principal);
        policy.renewAgent(uint64(vm.getBlockTimestamp() + 90 days));
        vm.prank(agent);
        policy.pay(vendor, 1 * USDC, keccak256("b"));
        assertEq(vendor.balance, 1 * USDC * WEI_PER_USDC);
    }

    function test_a_lapsed_agent_does_not_stop_the_owner_or_a_standing_order() public {
        uint64 due = uint64(vm.getBlockTimestamp());
        vm.prank(principal);
        uint256 id = policy.addSchedule(vendor, 5 * USDC, 1 days, due);
        vm.warp(1_764_000_000 + 91 days);
        // The agent itself is refused…
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AgentLapsed.selector, uint64(1_764_000_000 + 90 days)));
        policy.pay(vendor, 1 * USDC, keccak256("c"));
        // …the owner's standing order still pays (one cycle per call)…
        policy.payDue(id);
        // …and the owner still settles by hand.
        vm.prank(principal);
        policy.settleAsPrincipal(vendor, 2 * USDC, keccak256("manual"));
        assertEq(vendor.balance, 7 * USDC * WEI_PER_USDC);
    }
}
