// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";

/// @dev Approvals bound to the terms the owner saw, and to a deadline (PLAN 5.23).
contract BudgetPolicyTermsTest is Test {
    BudgetPolicy internal policy;

    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    address internal agent = address(0xBEEF);
    address internal vendor = address(0x1111);
    address internal other = address(0x2222);
    address internal registry = 0x8004A818BFB912233c491871b3d84c89A494BD9e;

    uint256 internal constant USDC = 1_000000;
    uint256 internal constant WEI_PER_USDC = 1e12;

    function setUp() public {
        vm.warp(1_764_000_000);
        vm.prank(principal);
        policy = new BudgetPolicy(
            principal, agent, operator, registry, 42, 100 * USDC, 500 * USDC, 0, 48 hours, BudgetPolicyBase.Mode.Propose
        );
        vm.deal(address(policy), 1000 * USDC * WEI_PER_USDC);
        vm.prank(principal);
        policy.setPayee(vendor, true);
        vm.warp(vm.getBlockTimestamp() + policy.delay());
    }

    function test_the_fingerprint_moves_with_every_term_and_with_nothing_else() public {
        bytes32 f0 = policy.fingerprint();
        // An agent payment does not move it (money moving is not a term).
        vm.prank(principal);
        policy.setMode(BudgetPolicyBase.Mode.Autopilot);
        bytes32 f1 = policy.fingerprint();
        assertTrue(f1 != f0, "a pending raise is a term");
        vm.warp(vm.getBlockTimestamp() + policy.delay());
        policy.applyPending();
        bytes32 f2 = policy.fingerprint();
        assertTrue(f2 != f1, "the mode is a term");
        vm.prank(agent);
        policy.pay(vendor, 1 * USDC, keccak256("x"));
        assertEq(policy.fingerprint(), f2, "a payment is not a term");
        vm.prank(principal);
        policy.setLimits(50 * USDC, 500 * USDC, 0); // tightening: instant
        bytes32 f3 = policy.fingerprint();
        assertTrue(f3 != f2, "the caps are a term");
        vm.prank(principal);
        policy.renewAgent(uint64(vm.getBlockTimestamp() + 10 days));
        assertTrue(policy.fingerprint() != f3, "the agent's expiry is a term");
    }

    function test_a_bound_settlement_pays_while_the_terms_hold() public {
        bytes32 terms = policy.fingerprint();
        uint64 deadline = uint64(vm.getBlockTimestamp() + 10 minutes);
        bytes32 h = keccak256("bill");
        vm.prank(principal);
        policy.settleAsPrincipalIf(vendor, 7 * USDC, h, terms, deadline);
        assertEq(vendor.balance, 7 * USDC * WEI_PER_USDC);
        assertTrue(policy.executed(h));
    }

    function test_a_bound_settlement_is_refused_once_the_terms_moved() public {
        bytes32 terms = policy.fingerprint();
        uint64 deadline = uint64(vm.getBlockTimestamp() + 10 minutes);
        // The operator raises the limits between the owner reading and signing.
        vm.prank(operator);
        policy.setLimits(200 * USDC, 900 * USDC, 0);
        bytes32 current = policy.fingerprint();
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.TermsChanged.selector, terms, current));
        policy.settleAsPrincipalIf(vendor, 7 * USDC, keccak256("bill"), terms, deadline);
        assertEq(vendor.balance, 0);
    }

    function test_a_bound_settlement_is_refused_after_its_deadline() public {
        bytes32 terms = policy.fingerprint();
        uint64 deadline = uint64(vm.getBlockTimestamp() + 10 minutes);
        vm.warp(deadline + 1);
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.ApprovalExpired.selector, deadline));
        policy.settleAsPrincipalIf(vendor, 7 * USDC, keccak256("bill"), terms, deadline);
    }

    function test_bound_payee_approval_holds_the_same_line() public {
        bytes32 terms = policy.fingerprint();
        uint64 deadline = uint64(vm.getBlockTimestamp() + 10 minutes);
        address[] memory ps = new address[](1);
        ps[0] = other;
        vm.prank(operator);
        policy.setLimits(200 * USDC, 900 * USDC, 0);
        vm.prank(principal);
        vm.expectRevert();
        policy.setPayeesIf(ps, true, terms, deadline);
        assertFalse(policy.attestedPayee(other));
        bytes32 fresh = policy.fingerprint();
        vm.prank(principal);
        policy.setPayeesIf(ps, true, fresh, deadline);
        assertTrue(policy.attestedPayee(other));
    }

    function test_only_the_principal_and_the_plain_settle_still_works() public {
        bytes32 terms = policy.fingerprint();
        vm.prank(operator);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.settleAsPrincipalIf(vendor, 1 * USDC, keccak256("a"), terms, uint64(vm.getBlockTimestamp() + 1));
        vm.prank(principal);
        policy.settleAsPrincipal(vendor, 1 * USDC, keccak256("plain"));
        assertEq(vendor.balance, 1 * USDC * WEI_PER_USDC);
    }
}
