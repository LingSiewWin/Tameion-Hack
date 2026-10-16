// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";

/// @dev The owner's reviewed run: many bills, one approval (PLAN 5.39).
contract BudgetPolicyRunTest is Test {
    BudgetPolicy internal policy;

    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    address internal agent = address(0xBEEF);
    address internal a = address(0x1111);
    address internal b = address(0x2222);
    address internal c = address(0x3333);
    address internal registry = 0x8004A818BFB912233c491871b3d84c89A494BD9e;

    uint256 internal constant USDC = 1_000000;
    uint256 internal constant WEI_PER_USDC = 1e12;

    event RunSettled(uint256 paid, uint256 skipped, uint256 usdcUnits);

    function setUp() public {
        vm.warp(1_764_000_000);
        vm.prank(principal);
        policy = new BudgetPolicy(
            principal, agent, operator, registry, 42, 100 * USDC, 500 * USDC, 0, 48 hours, BudgetPolicyBase.Mode.Propose
        );
        vm.deal(address(policy), 1000 * USDC * WEI_PER_USDC);
    }

    function _rows() internal pure returns (address[] memory to, uint256[] memory units, bytes32[] memory hs) {
        to = new address[](3);
        units = new uint256[](3);
        hs = new bytes32[](3);
        (to[0], to[1], to[2]) = (address(0x1111), address(0x2222), address(0x3333));
        (units[0], units[1], units[2]) = (2 * USDC, 3 * USDC, 4 * USDC);
        (hs[0], hs[1], hs[2]) = (keccak256("a"), keccak256("b"), keccak256("c"));
    }

    function _deadline() internal view returns (uint64) {
        return uint64(vm.getBlockTimestamp() + 600);
    }

    function test_one_approval_pays_every_row() public {
        (address[] memory to, uint256[] memory units, bytes32[] memory hs) = _rows();
        bytes32 terms = policy.fingerprint();
        vm.expectEmit(address(policy));
        emit RunSettled(3, 0, 9 * USDC);
        vm.prank(principal);
        uint256 paid = policy.settleRunIf(to, units, hs, terms, _deadline());
        assertEq(paid, 3);
        assertEq(a.balance, 2 * USDC * WEI_PER_USDC);
        assertEq(b.balance, 3 * USDC * WEI_PER_USDC);
        assertEq(c.balance, 4 * USDC * WEI_PER_USDC);
        assertTrue(policy.executed(hs[0]) && policy.executed(hs[1]) && policy.executed(hs[2]));
    }

    function test_a_row_already_paid_is_skipped_not_paid_twice() public {
        (address[] memory to, uint256[] memory units, bytes32[] memory hs) = _rows();
        // The owner settles the second bill on its own first (or the agent did).
        vm.prank(principal);
        policy.settleAsPrincipal(b, 3 * USDC, hs[1]);
        bytes32 terms = policy.fingerprint();
        vm.expectEmit(address(policy));
        emit RunSettled(2, 1, 6 * USDC);
        vm.prank(principal);
        uint256 paid = policy.settleRunIf(to, units, hs, terms, _deadline());
        assertEq(paid, 2);
        assertEq(b.balance, 3 * USDC * WEI_PER_USDC, "paid once, not twice");
    }

    function test_the_same_bill_twice_in_one_run_pays_once() public {
        (address[] memory to, uint256[] memory units, bytes32[] memory hs) = _rows();
        hs[2] = hs[0];
        to[2] = to[0];
        units[2] = units[0];
        bytes32 terms = policy.fingerprint();
        uint64 deadline = _deadline();
        vm.prank(principal);
        uint256 paid = policy.settleRunIf(to, units, hs, terms, deadline);
        assertEq(paid, 2);
        assertEq(a.balance, 2 * USDC * WEI_PER_USDC);
    }

    function test_refused_when_the_terms_moved_or_the_deadline_passed() public {
        (address[] memory to, uint256[] memory units, bytes32[] memory hs) = _rows();
        bytes32 terms = policy.fingerprint();
        vm.prank(principal);
        policy.setLimits(50 * USDC, 400 * USDC, 0); // tightening: instant, and a new term
        bytes32 moved = policy.fingerprint();
        uint64 later = _deadline();
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.TermsChanged.selector, terms, moved));
        policy.settleRunIf(to, units, hs, terms, later);

        uint64 deadline = _deadline();
        bytes32 now_ = policy.fingerprint();
        vm.warp(deadline + 1);
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.ApprovalExpired.selector, deadline));
        policy.settleRunIf(to, units, hs, now_, deadline);
        assertEq(a.balance, 0);
    }

    function test_only_the_principal_runs_it() public {
        (address[] memory to, uint256[] memory units, bytes32[] memory hs) = _rows();
        bytes32 terms = policy.fingerprint();
        uint64 deadline = _deadline();
        vm.prank(agent);
        vm.expectRevert();
        policy.settleRunIf(to, units, hs, terms, deadline);
        vm.prank(operator);
        vm.expectRevert();
        policy.settleRunIf(to, units, hs, terms, deadline);
        assertEq(a.balance, 0);
    }

    function test_the_shape_of_a_run_is_checked() public {
        (address[] memory to, uint256[] memory units, bytes32[] memory hs) = _rows();
        bytes32 terms = policy.fingerprint();
        uint64 deadline = _deadline();
        uint256[] memory short_ = new uint256[](2);
        vm.prank(principal);
        vm.expectRevert(BudgetPolicyBase.RunMismatch.selector);
        policy.settleRunIf(to, short_, hs, terms, deadline);

        address[] memory none = new address[](0);
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.RunLength.selector, 0));
        policy.settleRunIf(none, new uint256[](0), new bytes32[](0), terms, deadline);

        uint256 big = policy.MAX_RUN() + 1;
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.RunLength.selector, big));
        policy.settleRunIf(new address[](big), new uint256[](big), new bytes32[](big), terms, deadline);
    }
}
