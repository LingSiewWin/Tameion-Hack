// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {CreditLine} from "../src/credit/CreditLine.sol";
import {VouchBond} from "../src/credit/VouchBond.sol";

/// @dev A policy that refuses money: a draw into it must leave no debt behind.
contract RefusingPolicy {
    receive() external payable {
        revert("no");
    }
}

/// @dev A policy that tries to draw again while it is being paid.
contract ReenteringPolicy {
    CreditLine internal line;
    uint256 public reentries;

    constructor(CreditLine _line) {
        line = _line;
    }

    function open() external payable {
        line.lock{value: msg.value}();
        line.setPolicy(address(this));
    }

    function drawOnce(uint256 units) external {
        line.draw(units);
    }

    receive() external payable {
        if (reentries == 0) {
            reentries = 1;
            line.draw(1); // must fail: the debt is already recorded
        }
    }
}

contract CreditLineTest is Test {
    CreditLine internal line;

    address internal scorer = address(0x5C0);
    address internal judge = address(0x1D6E);
    address internal alice = address(0xA11CE);
    address internal policy = address(0xB0B);

    uint256 internal constant W = 1e12; // wei per USDC minor unit
    uint256 internal constant USDC = 1_000000;
    uint256 internal constant CAP = 100 * USDC;
    uint256 internal constant MAX_AGE = 1 days;
    uint256 internal constant DELAY = 2 days;

    function setUp() public {
        vm.warp(1_764_000_000);
        line = new CreditLine(scorer, judge, CAP, MAX_AGE, DELAY);
        vm.deal(alice, 1000 * USDC * W);
        vm.deal(address(this), 1000 * USDC * W);
        (bool ok,) = address(line).call{value: 50 * USDC * W}(""); // float
        assertTrue(ok);
    }

    function _open(uint256 lockUnits, uint256 bps) internal {
        vm.startPrank(alice);
        line.lock{value: lockUnits * W}();
        line.setPolicy(policy);
        vm.stopPrank();
        vm.prank(scorer);
        line.setScore(alice, bps);
    }

    function test_the_multiplier_steps_at_2000_5000_8000() public view {
        assertEq(line.multiplier(0), 0);
        assertEq(line.multiplier(1999), 0);
        assertEq(line.multiplier(2000), 5000);
        assertEq(line.multiplier(4999), 5000);
        assertEq(line.multiplier(5000), 10_000);
        assertEq(line.multiplier(7999), 10_000);
        assertEq(line.multiplier(8000), 15_000);
        assertEq(line.multiplier(10_000), 15_000);
    }

    function test_limit_is_deposit_times_multiplier_capped() public {
        _open(10 * USDC, 8000);
        assertEq(line.limit(alice), 15 * USDC);
        vm.prank(alice);
        line.lock{value: 90 * USDC * W}();
        assertEq(line.limit(alice), CAP); // 150 capped at 100
    }

    function test_a_score_older_than_max_age_gives_limit_zero() public {
        _open(10 * USDC, 9000);
        vm.warp(vm.getBlockTimestamp() + MAX_AGE);
        assertEq(line.limit(alice), 15 * USDC); // exactly maxAge: still fresh
        vm.warp(vm.getBlockTimestamp() + 1);
        assertEq(line.scoreOf(alice), 0);
        assertEq(line.limit(alice), 0);
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(CreditLine.OverLimit.selector, 1, 0));
        line.draw(1);
    }

    function test_draw_pays_the_policy_and_records_the_debt() public {
        _open(10 * USDC, 8000);
        vm.expectEmit(true, true, true, true, address(line));
        emit CreditLine.Drawn(alice, policy, 5 * USDC);
        vm.prank(alice);
        line.draw(5 * USDC);
        assertEq(line.debt(alice), 5 * USDC);
        assertEq(policy.balance, 5 * USDC * W);
        assertEq(alice.balance, 990 * USDC * W); // nothing reached the caller
    }

    function test_draw_past_the_limit_is_refused_by_name() public {
        _open(10 * USDC, 5000); // limit 10
        vm.startPrank(alice);
        line.draw(6 * USDC);
        vm.expectRevert(abi.encodeWithSelector(CreditLine.OverLimit.selector, 11 * USDC, 10 * USDC));
        line.draw(5 * USDC);
        vm.stopPrank();
    }

    function test_deposits_are_never_lent_only_the_float() public {
        CreditLine dry = new CreditLine(scorer, judge, CAP, MAX_AGE, DELAY);
        vm.startPrank(alice);
        dry.lock{value: 10 * USDC * W}();
        dry.setPolicy(policy);
        vm.stopPrank();
        vm.prank(scorer);
        dry.setScore(alice, 8000);
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(CreditLine.NoFloat.selector, 1, 0));
        dry.draw(1);
    }

    function test_no_policy_no_draw_and_the_policy_is_set_once() public {
        vm.startPrank(alice);
        line.lock{value: 10 * USDC * W}();
        vm.expectRevert(CreditLine.NoPolicy.selector);
        line.draw(1);
        line.setPolicy(policy);
        vm.expectRevert(abi.encodeWithSelector(CreditLine.PolicyAlreadySet.selector, policy));
        line.setPolicy(address(0xBAD));
        vm.stopPrank();
    }

    function test_a_failed_payment_leaves_no_debt() public {
        RefusingPolicy r = new RefusingPolicy();
        vm.startPrank(alice);
        line.lock{value: 10 * USDC * W}();
        line.setPolicy(address(r));
        vm.stopPrank();
        vm.prank(scorer);
        line.setScore(alice, 8000);
        vm.prank(alice);
        vm.expectRevert(VouchBond.TransferFailed.selector);
        line.draw(5 * USDC);
        assertEq(line.debt(alice), 0);
    }

    function test_a_policy_that_reenters_cannot_pass_the_limit() public {
        ReenteringPolicy p = new ReenteringPolicy(line);
        p.open{value: 10 * USDC * W}();
        vm.prank(scorer);
        line.setScore(address(p), 5000); // limit 10
        // The inner draw of 1 unit fails OverLimit (debt already 10), so the
        // outer payment fails with it and nothing is owed.
        vm.expectRevert(VouchBond.TransferFailed.selector);
        p.drawOnce(10 * USDC);
        assertEq(line.debt(address(p)), 0);
    }

    function test_paused_refuses_draws_but_not_repayment() public {
        _open(10 * USDC, 8000);
        vm.prank(alice);
        line.draw(4 * USDC);
        vm.prank(alice);
        vm.expectRevert(CreditLine.NotJudge.selector);
        line.setPaused(true);
        vm.prank(judge);
        line.setPaused(true);
        vm.startPrank(alice);
        vm.expectRevert(CreditLine.LinePaused.selector);
        line.draw(1);
        line.repay{value: 4 * USDC * W}();
        vm.stopPrank();
        assertEq(line.debt(alice), 0);
    }

    function test_repay_never_takes_more_than_owed_or_dust() public {
        _open(10 * USDC, 8000);
        vm.startPrank(alice);
        line.draw(3 * USDC);
        vm.expectRevert(abi.encodeWithSelector(CreditLine.OverRepay.selector, 4 * USDC, 3 * USDC));
        line.repay{value: 4 * USDC * W}();
        vm.expectRevert(abi.encodeWithSelector(VouchBond.NotWholeUnits.selector, 1));
        line.repay{value: 1}();
        line.repay{value: 3 * USDC * W}();
        vm.stopPrank();
        assertEq(line.debt(alice), 0);
    }

    function test_slash_covers_the_debt_and_names_only_hash_and_rule() public {
        _open(10 * USDC, 8000);
        vm.prank(alice);
        line.draw(4 * USDC);
        bytes32 evidence = keccak256("evidence");
        vm.prank(alice);
        vm.expectRevert(CreditLine.NotJudge.selector);
        line.slash(alice, evidence, 3);
        vm.prank(judge);
        vm.expectRevert(CreditLine.NoEvidence.selector);
        line.slash(alice, bytes32(0), 3);
        vm.expectEmit(true, true, true, true, address(line));
        emit CreditLine.Slashed(alice, 4 * USDC, evidence, 3);
        vm.prank(judge);
        line.slash(alice, evidence, 3);
        assertEq(line.debt(alice), 0);
        assertEq(line.deposit(alice), 6 * USDC);
        assertEq(line.totalDeposits(), 6 * USDC);
        vm.prank(judge);
        vm.expectRevert(CreditLine.NothingToSlash.selector);
        line.slash(alice, evidence, 3);
    }

    function test_slash_takes_at_most_the_deposit_the_rest_stays_owed() public {
        _open(10 * USDC, 8000); // limit 15
        vm.prank(alice);
        line.draw(15 * USDC);
        vm.prank(judge);
        line.slash(alice, keccak256("e"), 1);
        assertEq(line.deposit(alice), 0);
        assertEq(line.debt(alice), 5 * USDC);
    }

    function test_unlock_waits_the_delay_and_not_while_owed() public {
        _open(10 * USDC, 8000);
        vm.startPrank(alice);
        line.draw(2 * USDC);
        line.requestUnlock(10 * USDC);
        uint256 eta = vm.getBlockTimestamp() + DELAY;
        vm.expectRevert(abi.encodeWithSelector(VouchBond.TooEarly.selector, eta));
        line.unlock();
        vm.warp(eta);
        vm.expectRevert(abi.encodeWithSelector(CreditLine.StillOwed.selector, 2 * USDC));
        line.unlock();
        line.repay{value: 2 * USDC * W}();
        uint256 before = alice.balance;
        line.unlock();
        vm.stopPrank();
        assertEq(alice.balance - before, 10 * USDC * W);
        assertEq(line.deposit(alice), 0);
    }

    function test_a_deposit_stays_slashable_during_the_cooling_off() public {
        _open(10 * USDC, 8000);
        vm.startPrank(alice);
        line.draw(4 * USDC);
        line.requestUnlock(10 * USDC);
        vm.stopPrank();
        vm.prank(judge);
        line.slash(alice, keccak256("e"), 2);
        vm.warp(vm.getBlockTimestamp() + DELAY);
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(VouchBond.OverDeposit.selector, 10 * USDC, 6 * USDC));
        line.unlock();
    }

    function test_only_the_scorer_scores_and_only_up_to_10000() public {
        vm.prank(alice);
        vm.expectRevert(CreditLine.NotScorer.selector);
        line.setScore(alice, 9000);
        vm.prank(scorer);
        vm.expectRevert(abi.encodeWithSelector(CreditLine.BadScore.selector, 10_001));
        line.setScore(alice, 10_001);
    }

    function test_dust_and_zero_locks_are_refused() public {
        vm.startPrank(alice);
        vm.expectRevert(VouchBond.ZeroAmount.selector);
        line.lock{value: 0}();
        vm.expectRevert(abi.encodeWithSelector(VouchBond.NotWholeUnits.selector, W + 1));
        line.lock{value: W + 1}();
        vm.stopPrank();
    }
}
