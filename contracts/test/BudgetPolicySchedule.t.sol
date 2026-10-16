// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";

/// @dev Standing orders (PLAN 5.25): a confirmed bill written into the
///      contract that anyone may trigger when due, inside the same caps.
contract BudgetPolicyScheduleTest is Test {
    BudgetPolicy internal policy;

    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    address internal agent = address(0xBEEF);
    address internal vendor = address(0x1111);
    address internal other = address(0x2222);
    address internal stranger = address(0x4444);
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
            100 * USDC, // per action
            500 * USDC, // per window
            50 * USDC, // reserve floor
            48 hours,
            BudgetPolicyBase.Mode.Propose
        );
        vm.deal(address(policy), 1000 * USDC * WEI_PER_USDC);
        vm.prank(principal);
        policy.setPayee(vendor, true);
        vm.warp(vm.getBlockTimestamp() + policy.delay());
    }

    function _add(uint256 amount, uint64 every, uint64 firstDue) internal returns (uint256 id) {
        vm.prank(principal);
        id = policy.addSchedule(vendor, amount, every, firstDue);
    }

    // ------------------------------------------------------------- adding

    function test_only_the_principal_adds_and_the_payee_must_be_attested() public {
        uint64 now_ = uint64(vm.getBlockTimestamp());
        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.addSchedule(vendor, 10 * USDC, 30 days, now_);
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.PayeeNotAttested.selector, other));
        policy.addSchedule(other, 10 * USDC, 30 days, now_);
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.OverPerActionCap.selector, 101 * USDC, 100 * USDC));
        policy.addSchedule(vendor, 101 * USDC, 30 days, now_);
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.IntervalTooShort.selector, uint64(10 minutes)));
        policy.addSchedule(vendor, 10 * USDC, 10 minutes, now_);
        uint256 id = _add(10 * USDC, 30 days, now_);
        assertEq(id, 0);
        assertEq(policy.scheduleCount(), 1);
        (address payee, uint256 amount, uint64 every, uint64 nextDue, bool active) = policy.schedules(0);
        assertEq(payee, vendor);
        assertEq(amount, 10 * USDC);
        assertEq(every, 30 days);
        assertEq(nextDue, now_);
        assertTrue(active);
    }

    // ------------------------------------------------------------- paying

    function test_anyone_pays_a_due_order_in_propose_mode_and_the_order_advances() public {
        uint64 due = uint64(vm.getBlockTimestamp());
        uint256 id = _add(10 * USDC, 30 days, due);
        bytes32 h = policy.scheduleActionHash(id, due);
        vm.expectEmit(true, true, true, true, address(policy));
        emit BudgetPolicyBase.Paid(vendor, 10 * USDC, h);
        vm.prank(stranger);
        bytes32 got = policy.payDue(id);
        assertEq(got, h);
        assertEq(vendor.balance, 10 * USDC * WEI_PER_USDC);
        assertTrue(policy.executed(h));
        (,,, uint64 nextDue,) = policy.schedules(id);
        assertEq(nextDue, due + 30 days);
        assertEq(policy.spentInWindow(), 10 * USDC, "counts against the agent's window");
    }

    function test_not_due_yet_is_refused_by_name() public {
        uint64 due = uint64(vm.getBlockTimestamp()) + 1 days;
        uint256 id = _add(10 * USDC, 30 days, due);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.ScheduleNotDue.selector, id, due));
        policy.payDue(id);
        vm.warp(due);
        policy.payDue(id);
    }

    function test_missed_cycles_are_paid_one_call_at_a_time() public {
        uint64 due = uint64(vm.getBlockTimestamp());
        uint256 id = _add(10 * USDC, 1 days, due);
        vm.warp(due + 3 days);
        policy.payDue(id);
        policy.payDue(id);
        policy.payDue(id);
        policy.payDue(id);
        (,,, uint64 nextDue,) = policy.schedules(id);
        assertEq(nextDue, due + 4 days);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.ScheduleNotDue.selector, id, due + 4 days));
        policy.payDue(id);
        assertEq(vendor.balance, 40 * USDC * WEI_PER_USDC);
    }

    function test_paused_refuses_and_a_removed_order_never_pays() public {
        uint64 due = uint64(vm.getBlockTimestamp());
        uint256 id = _add(10 * USDC, 30 days, due);
        vm.prank(principal);
        policy.setMode(BudgetPolicyBase.Mode.Watch);
        vm.expectRevert(BudgetPolicyBase.Paused.selector);
        policy.payDue(id);
        vm.prank(principal);
        policy.setMode(BudgetPolicyBase.Mode.Propose);
        vm.prank(stranger);
        vm.expectRevert(BudgetPolicyBase.NotPrincipalOrOperator.selector);
        policy.removeSchedule(id);
        vm.prank(operator);
        policy.removeSchedule(id);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.ScheduleInactive.selector, id));
        policy.payDue(id);
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.ScheduleInactive.selector, id));
        policy.removeSchedule(id);
    }

    function test_the_caps_still_bound_a_standing_order() public {
        uint64 due = uint64(vm.getBlockTimestamp());
        // 100 per action, 500 per window: six orders of 100 cannot all pay today.
        uint256 a = _add(100 * USDC, 1 days, due);
        uint256 b = _add(100 * USDC, 1 days, due);
        uint256 c = _add(100 * USDC, 1 days, due);
        uint256 d = _add(100 * USDC, 1 days, due);
        uint256 e = _add(100 * USDC, 1 days, due);
        uint256 f = _add(100 * USDC, 1 days, due);
        policy.payDue(a);
        policy.payDue(b);
        policy.payDue(c);
        policy.payDue(d);
        policy.payDue(e);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.OverDailyCap.selector, 600 * USDC, 500 * USDC));
        policy.payDue(f);
        // The failed call must not have advanced the order.
        (,,, uint64 nextDue,) = policy.schedules(f);
        assertEq(nextDue, due);
    }

    function test_a_revoked_payee_stops_its_standing_order() public {
        uint64 due = uint64(vm.getBlockTimestamp());
        uint256 id = _add(10 * USDC, 30 days, due);
        vm.prank(principal);
        policy.setPayee(vendor, false);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.PayeeNotAttested.selector, vendor));
        policy.payDue(id);
    }

    function test_the_agent_cannot_replay_a_cycle_the_order_paid() public {
        uint64 due = uint64(vm.getBlockTimestamp());
        uint256 id = _add(10 * USDC, 30 days, due);
        bytes32 h = policy.scheduleActionHash(id, due);
        policy.payDue(id);
        vm.prank(principal);
        policy.setMode(BudgetPolicyBase.Mode.Autopilot);
        vm.warp(vm.getBlockTimestamp() + policy.delay());
        policy.applyPending();
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AlreadyExecuted.selector, h));
        policy.pay(vendor, 10 * USDC, h);
    }

    function test_the_agents_own_pay_still_needs_autopilot() public {
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.NotAutopilot.selector, uint8(1)));
        policy.pay(vendor, 1 * USDC, keccak256("x"));
    }
}
