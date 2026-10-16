// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";

/// @dev A recipient that refuses native value, to exercise TransferFailed.
contract Rejector {
    receive() external payable {
        revert("no");
    }
}

/// @dev Acts AS the agent, so its callback reaches `pay`'s own guards
///      rather than bouncing off `onlyAgent`.
contract ReentrantAgent {
    BudgetPolicy public policy;
    bytes32 public sameHash;
    uint256 public innerAmount;
    bytes32 public innerHash;
    bool public tried;
    bool public innerSucceeded;
    bytes public innerError;

    function arm(BudgetPolicy p, bytes32 _same, uint256 _amt, bytes32 _innerHash) external {
        policy = p;
        sameHash = _same;
        innerAmount = _amt;
        innerHash = _innerHash;
    }

    function go(uint256 amount, bytes32 h) external {
        policy.pay(address(this), amount, h);
    }

    receive() external payable {
        if (!tried && address(policy) != address(0)) {
            tried = true;
            bytes32 h = innerHash == bytes32(0) ? sameHash : innerHash;
            try policy.pay(address(this), innerAmount, h) {
                innerSucceeded = true;
            } catch (bytes memory err) {
                innerSucceeded = false;
                innerError = err;
            }
        }
    }
}

contract BudgetPolicyTest is Test {
    BudgetPolicy internal policy;

    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    address internal agent = address(0xBEEF);
    address internal vendor = address(0x1111);
    address internal registry = 0x8004A818BFB912233c491871b3d84c89A494BD9e;

    // USDC minor units: six decimals.
    uint256 internal constant USDC = 1_000000;

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
            500 * USDC, // per day
            50 * USDC, // reserve floor
            48 hours, // delay before a trust-expanding change applies (docs/46 #3)
            BudgetPolicyBase.Mode.Autopilot
        );
        // Fund with 1,000 USDC expressed in native wei.
        vm.deal(address(policy), 1000 * USDC * policy.WEI_PER_USDC_UNIT());
        // The agent may only pay addresses the principal has attested, and
        // only once the attestation is `delay` old.
        vm.prank(principal);
        policy.setPayee(vendor, true);
        vm.warp(vm.getBlockTimestamp() + policy.delay());
    }

    /// Attest and wait out the cooling-off, so tests about other limits are
    /// refused by the limit they name. The cooling-off itself is tested below.
    function _attest(address who) internal {
        vm.prank(principal);
        policy.setPayee(who, true);
        vm.warp(vm.getBlockTimestamp() + policy.delay());
    }

    /// Set limits as the operator and, if the change was trust-expanding and
    /// therefore scheduled, wait out the delay and apply it. Tests about other
    /// limits use this so they test the limit they name; the delay rule has
    /// its own tests below.
    function _setLimitsNow(uint256 pa, uint256 dc, uint256 rf) internal {
        vm.prank(operator);
        policy.setLimits(pa, dc, rf);
        (,,,,, uint8 kinds) = policy.pending();
        if (kinds != 0) {
            vm.warp(vm.getBlockTimestamp() + policy.delay());
            policy.applyPending();
        }
    }

    function _hash(string memory s) internal pure returns (bytes32) {
        return keccak256(bytes(s));
    }

    // Tests read the clock with vm.getBlockTimestamp(), never block.timestamp:
    // under via-IR a block.timestamp copied into a local can be re-read after
    // vm.warp, which silently moves the test's own reference point.

    // ---------------------------------------------------------------- happy

    function test_pays_within_every_limit() public {
        uint256 before = vendor.balance;
        vm.prank(agent);
        policy.pay(vendor, 20 * USDC, _hash("act_1"));

        assertEq(vendor.balance - before, 20 * USDC * policy.WEI_PER_USDC_UNIT());
        assertEq(policy.spentInWindow(), 20 * USDC);
        assertEq(policy.balanceUsdc(), 980 * USDC);
        assertTrue(policy.executed(_hash("act_1")));
    }

    function test_six_decimal_amounts_survive_the_conversion() public {
        // One USDC minor unit is 1e-6 USDC and 1e12 wei. The 6-vs-18 decimal
        // mismatch is the defect class this project exists to remove, so the
        // smallest representable payment is asserted explicitly.
        vm.prank(agent);
        policy.pay(vendor, 1, _hash("dust"));
        assertEq(vendor.balance, 1e12);
        assertEq(policy.spentInWindow(), 1);
    }

    // ------------------------------------------------------------ the caps

    function test_over_per_action_cap_reverts() public {
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.OverPerActionCap.selector, 150 * USDC, 100 * USDC));
        policy.pay(vendor, 150 * USDC, _hash("too_big"));
    }

    function test_daily_cap_accumulates_across_payments() public {
        for (uint256 i = 0; i < 5; i++) {
            vm.prank(agent);
            policy.pay(vendor, 100 * USDC, _hash(string(abi.encodePacked("d", i))));
        }
        assertEq(policy.spentInWindow(), 500 * USDC);

        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.OverDailyCap.selector, 501 * USDC, 500 * USDC));
        policy.pay(vendor, 1 * USDC, _hash("over_day"));
    }

    /// A payment counts against the cap for exactly one window, then stops
    /// counting, without anyone having to poke the contract.
    function test_a_payment_stops_counting_exactly_one_window_later() public {
        uint256 t0 = vm.getBlockTimestamp();
        vm.prank(agent);
        policy.pay(vendor, 100 * USDC, _hash("w1"));

        vm.warp(t0 + policy.WINDOW() - 1);
        assertEq(policy.spentInWindow(), 100 * USDC, "still inside the window");

        vm.warp(t0 + policy.WINDOW());
        assertEq(policy.spentInWindow(), 0, "out of the window at exactly WINDOW seconds");

        vm.prank(agent);
        policy.pay(vendor, 100 * USDC, _hash("w2"));
        assertEq(policy.spentInWindow(), 100 * USDC, "counted afresh, not accumulated");
    }

    /// The defect a calendar-day reset had: the cap could be spent at
    /// 23:59:59 and again at 00:00:00, twice in one second. Found from
    /// ERC-8427's rationale and proved against the old contract (docs/14).
    function test_the_cap_cannot_be_spent_twice_across_midnight() public {
        _setLimitsNow(100 * USDC, 100 * USDC, 0);

        uint256 midnight = (vm.getBlockTimestamp() / 1 days + 1) * 1 days;
        vm.warp(midnight - 1);
        vm.prank(agent);
        policy.pay(vendor, 100 * USDC, _hash("before_midnight"));

        vm.warp(midnight);
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.OverDailyCap.selector, 200 * USDC, 100 * USDC));
        policy.pay(vendor, 100 * USDC, _hash("after_midnight"));

        // And the room comes back one full window after the first payment.
        vm.warp(midnight - 1 + policy.WINDOW());
        vm.prank(agent);
        policy.pay(vendor, 100 * USDC, _hash("a_day_later"));
    }

    /// Payments expire one by one, oldest first, not all at once.
    function test_room_returns_payment_by_payment() public {
        uint256 t0 = vm.getBlockTimestamp();
        vm.prank(agent);
        policy.pay(vendor, 100 * USDC, _hash("p1"));
        vm.warp(t0 + 1 hours);
        vm.prank(agent);
        policy.pay(vendor, 100 * USDC, _hash("p2"));
        vm.warp(t0 + 2 hours);
        vm.prank(agent);
        policy.pay(vendor, 100 * USDC, _hash("p3"));
        assertEq(policy.spentInWindow(), 300 * USDC);

        vm.warp(t0 + 1 days);
        assertEq(policy.spentInWindow(), 200 * USDC, "only the first has expired");
        vm.warp(t0 + 1 days + 1 hours);
        assertEq(policy.spentInWindow(), 100 * USDC);
        vm.warp(t0 + 1 days + 2 hours);
        assertEq(policy.spentInWindow(), 0);
    }

    /// The ring of live payments is bounded. When it is full the next payment
    /// is refused rather than silently forgetting the oldest, which would let
    /// spending escape the cap.
    function test_a_full_window_refuses_rather_than_forgets() public {
        vm.prank(operator);
        policy.setLimits(1 * USDC, 1000 * USDC, 0);
        uint256 n = policy.MAX_LIVE_DEBITS();
        for (uint256 i = 0; i < n; i++) {
            vm.prank(agent);
            policy.pay(vendor, 1, keccak256(abi.encode("ring", i)));
        }
        assertEq(policy.spentInWindow(), n);

        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.WindowFull.selector);
        policy.pay(vendor, 1, _hash("one_too_many"));

        // Once the oldest expires there is room again.
        vm.warp(vm.getBlockTimestamp() + policy.WINDOW());
        vm.prank(agent);
        policy.pay(vendor, 1, _hash("after_expiry"));
        assertEq(policy.spentInWindow(), 1);
    }

    function test_reserve_floor_is_respected() public {
        // Spend down to just above the floor.
        _setLimitsNow(1000 * USDC, 100000 * USDC, 50 * USDC);

        vm.prank(agent);
        policy.pay(vendor, 950 * USDC, _hash("drain"));
        assertEq(policy.balanceUsdc(), 50 * USDC);

        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.BelowReserveFloor.selector, 49 * USDC, 50 * USDC));
        policy.pay(vendor, 1 * USDC, _hash("below_floor"));
    }

    function test_paying_more_than_the_balance_reverts_without_underflow() public {
        _setLimitsNow(100000 * USDC, 100000 * USDC, 0);

        uint256 balance = policy.balanceUsdc();
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.InsufficientBalance.selector, 5000 * USDC, balance));
        policy.pay(vendor, 5000 * USDC, _hash("overdraw"));
    }

    // ------------------------------------------------------- idempotency

    function test_the_same_action_hash_cannot_execute_twice() public {
        bytes32 h = _hash("act_replay");
        vm.prank(agent);
        policy.pay(vendor, 20 * USDC, h);

        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AlreadyExecuted.selector, h));
        policy.pay(vendor, 20 * USDC, h);
    }

    function test_reentrancy_on_the_same_hash_is_refused() public {
        // The re-entrant caller is installed AS the agent, so its callback
        // reaches pay's own guards instead of bouncing off onlyAgent. This
        // matters: an earlier version of this test used a non-agent
        // recipient and passed for the wrong reason.
        ReentrantAgent ra = new ReentrantAgent();
        vm.prank(operator);
        policy.setAgent(address(ra));
        _attest(address(ra));

        bytes32 h = _hash("act_reentrant");
        ra.arm(policy, h, 10 * USDC, bytes32(0)); // inner reuses the same hash

        ra.go(10 * USDC, h);

        assertTrue(ra.tried(), "the callback did fire");
        assertFalse(ra.innerSucceeded(), "the re-entrant call must be refused");
        assertEq(
            bytes4(ra.innerError()),
            BudgetPolicyBase.AlreadyExecuted.selector,
            "refused specifically because the hash was already spent"
        );
        assertEq(policy.spentInWindow(), 10 * USDC, "exactly one payment counted");
    }

    function test_reentrancy_with_a_fresh_hash_still_obeys_the_caps() public {
        // A callback under a different hash is simply another payment. It may
        // succeed, but spentToday is written before the transfer, so it
        // cannot reuse room the outer payment already consumed.
        _setLimitsNow(100 * USDC, 150 * USDC, 0);

        ReentrantAgent ra = new ReentrantAgent();
        vm.prank(operator);
        policy.setAgent(address(ra));
        _attest(address(ra));

        // 50 fits in the remaining 50 of daily room; 100 would not.
        ra.arm(policy, bytes32(0), 50 * USDC, _hash("inner"));
        ra.go(100 * USDC, _hash("outer"));

        assertTrue(ra.innerSucceeded(), "50 fits inside the remaining room");
        assertEq(policy.spentInWindow(), 150 * USDC, "outer 100 plus the inner 50");
        assertLe(policy.spentInWindow(), policy.dailyCap(), "daily cap held through reentry");
    }

    function test_reentrancy_cannot_exceed_the_daily_cap() public {
        // Same setup, but the callback asks for more than the room left.
        _setLimitsNow(100 * USDC, 150 * USDC, 0);

        ReentrantAgent ra = new ReentrantAgent();
        vm.prank(operator);
        policy.setAgent(address(ra));
        _attest(address(ra));

        ra.arm(policy, bytes32(0), 100 * USDC, _hash("inner_greedy"));
        ra.go(100 * USDC, _hash("outer2"));

        assertFalse(ra.innerSucceeded(), "the greedy callback must be refused");
        assertEq(
            bytes4(ra.innerError()),
            BudgetPolicyBase.OverDailyCap.selector,
            "refused by the daily cap, from inside the transfer"
        );
        assertEq(policy.spentInWindow(), 100 * USDC);
        assertLe(policy.spentInWindow(), policy.dailyCap());
    }

    // ------------------------------------------------------------ payees

    /// The agent can only pay addresses the principal attested. A look-alike
    /// address, a changed payee, or an attacker's address is refused here even
    /// if the off-chain verifier was fooled.
    function test_an_unattested_payee_is_refused() public {
        address lookalike = address(0x1111000000000000000000000000000000001111);
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.PayeeNotAttested.selector, lookalike));
        policy.pay(lookalike, 1 * USDC, _hash("lookalike"));
        assertFalse(policy.executed(_hash("lookalike")), "a refused payment must not consume its hash");
    }

    function test_only_the_principal_attests_payees() public {
        address[2] memory notPrincipal = [operator, agent];
        for (uint256 i = 0; i < 2; i++) {
            vm.prank(notPrincipal[i]);
            vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
            policy.setPayee(address(0xBEEF00), true);
        }
    }

    function test_one_approval_attests_a_whole_list() public {
        address[] memory list = new address[](3);
        list[0] = address(0xA1);
        list[1] = address(0xA2);
        list[2] = address(0xA3);
        vm.prank(principal);
        policy.setPayees(list, true);
        for (uint256 i = 0; i < 3; i++) {
            assertTrue(policy.attestedPayee(list[i]));
        }
        vm.warp(vm.getBlockTimestamp() + policy.delay());
        vm.prank(agent);
        policy.pay(list[2], 1 * USDC, _hash("third"));
    }

    function test_only_the_principal_attests_a_list() public {
        address[] memory list = new address[](1);
        list[0] = address(0xA1);
        address[2] memory notPrincipal = [operator, agent];
        for (uint256 i = 0; i < 2; i++) {
            vm.prank(notPrincipal[i]);
            vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
            policy.setPayees(list, true);
        }
    }

    function test_a_zero_address_in_a_list_rejects_the_whole_list() public {
        address[] memory list = new address[](2);
        list[0] = address(0xA1);
        list[1] = address(0);
        vm.prank(principal);
        vm.expectRevert(BudgetPolicyBase.ZeroAddress.selector);
        policy.setPayees(list, true);
        assertFalse(policy.attestedPayee(address(0xA1)), "never half-applied");
    }

    function test_revoking_a_payee_stops_payments_to_it() public {
        vm.prank(agent);
        policy.pay(vendor, 1 * USDC, _hash("before_revoke"));

        vm.prank(principal);
        policy.setPayee(vendor, false);

        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.PayeeNotAttested.selector, vendor));
        policy.pay(vendor, 1 * USDC, _hash("after_revoke"));
    }

    function test_the_zero_address_cannot_be_attested() public {
        vm.prank(principal);
        vm.expectRevert(BudgetPolicyBase.ZeroAddress.selector);
        policy.setPayee(address(0), true);
    }

    /// The principal's own settlement is not bound by the allowlist: it is the
    /// human acting on their own authority, as with the caps.
    function test_principal_settlement_is_not_bound_by_the_allowlist() public {
        address fresh = address(0xFEE);
        vm.prank(principal);
        policy.settleAsPrincipal(fresh, 5 * USDC, _hash("new_payee_by_hand"));
        assertEq(fresh.balance, 5 * USDC * policy.WEI_PER_USDC_UNIT());
    }

    // ------------------------------------------------------------ authority

    function test_the_agent_cannot_raise_its_own_cap() public {
        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.NotPrincipalOrOperator.selector);
        policy.setLimits(1_000_000 * USDC, 1_000_000 * USDC, 0);
    }

    function test_the_agent_cannot_replace_itself() public {
        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.NotOperator.selector);
        policy.setAgent(address(0xDEAD));
    }

    function test_the_agent_cannot_withdraw() public {
        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.withdraw(agent, 100 * USDC);
    }

    function test_a_stranger_cannot_pay() public {
        vm.prank(address(0xBAD));
        vm.expectRevert(BudgetPolicyBase.NotAgent.selector);
        policy.pay(vendor, 1 * USDC, _hash("stranger"));
    }

    function test_the_operator_can_rotate_a_compromised_agent() public {
        address fresh = address(0xC0FFEE);
        vm.prank(operator);
        policy.setAgent(fresh);

        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.NotAgent.selector);
        policy.pay(vendor, 1 * USDC, _hash("old_agent"));

        vm.prank(fresh);
        policy.pay(vendor, 1 * USDC, _hash("new_agent"));
    }

    function test_limits_cannot_be_set_inconsistently() public {
        vm.prank(operator);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.CapAboveDailyCap.selector, 600 * USDC, 500 * USDC));
        policy.setLimits(600 * USDC, 500 * USDC, 0);
    }

    function test_constructor_rejects_an_inconsistent_policy() public {
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.CapAboveDailyCap.selector, 600 * USDC, 500 * USDC));
        new BudgetPolicy(
            principal,
            agent,
            operator,
            registry,
            1,
            600 * USDC,
            500 * USDC,
            0,
            48 hours,
            BudgetPolicyBase.Mode.Autopilot
        );
    }

    // --------------------------------------------- split roles (the point)

    /// The reason the split exists: the key handled every day must not be the
    /// key that can empty the treasury.
    function test_the_operator_cannot_withdraw() public {
        vm.prank(operator);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.withdraw(operator, 100 * USDC);
    }

    /// And the converse, so neither role silently absorbs the other: the
    /// principal may set limits and mode (it is their money, docs/52) but
    /// swapping the agent stays the operator's.
    function test_the_principal_cannot_replace_the_agent() public {
        vm.prank(principal);
        vm.expectRevert(BudgetPolicyBase.NotOperator.selector);
        policy.setAgent(address(0xDEAD));
    }

    /// Losing the operator key must degrade to dormant, not stuck: limits
    /// freeze, payments continue, and the principal can appoint a successor.
    function test_a_lost_operator_key_does_not_stop_payments_or_recovery() public {
        address fresh = address(0xF00D);

        // Agent keeps working with the limits as they stand.
        vm.prank(agent);
        policy.pay(vendor, 10 * USDC, _hash("still_paying"));

        vm.prank(principal);
        policy.setOperator(fresh);

        vm.prank(operator);
        vm.expectRevert(BudgetPolicyBase.NotPrincipalOrOperator.selector);
        policy.setLimits(1 * USDC, 1 * USDC, 0);

        // A raise from the new operator is scheduled, not applied: the same
        // delay rule binds a legitimate successor and a thief alike.
        vm.prank(fresh);
        policy.setLimits(200 * USDC, 500 * USDC, 0);
        assertEq(policy.perActionCap(), 100 * USDC, "a raise waits");
        vm.warp(vm.getBlockTimestamp() + policy.delay());
        policy.applyPending();
        assertEq(policy.perActionCap(), 200 * USDC);
    }

    function test_only_the_principal_appoints_an_operator() public {
        vm.prank(operator);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.setOperator(address(0xDEAD));

        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.setOperator(address(0xDEAD));
    }

    // ------------------------------------------- settling an escalation

    /// The defect this closes: a human paid the bill with `withdraw`, the
    /// action hash stayed unrecorded, and the agent escalated the same
    /// obligation again on its next pass.
    function test_principal_settlement_records_the_cycle() public {
        bytes32 h = _hash("escalated_over_cap");
        assertFalse(policy.executed(h));

        uint256 before = vendor.balance;
        vm.prank(principal);
        policy.settleAsPrincipal(vendor, 150 * USDC, h);

        assertTrue(policy.executed(h), "the cycle must be marked settled");
        assertEq(vendor.balance - before, 150 * USDC * policy.WEI_PER_USDC_UNIT());
    }

    /// Not subject to the agent's caps -- those bound the agent, not the owner.
    function test_principal_settlement_is_not_bound_by_the_agent_caps() public {
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.OverPerActionCap.selector, 150 * USDC, 100 * USDC));
        policy.pay(vendor, 150 * USDC, _hash("over"));

        vm.prank(principal);
        policy.settleAsPrincipal(vendor, 150 * USDC, _hash("over"));
    }

    /// But it *is* subject to the replay guard. A human can double-pay too.
    function test_principal_settlement_refuses_a_replay() public {
        bytes32 h = _hash("settle_once");
        vm.prank(principal);
        policy.settleAsPrincipal(vendor, 10 * USDC, h);

        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AlreadyExecuted.selector, h));
        policy.settleAsPrincipal(vendor, 10 * USDC, h);
    }

    /// And once settled by hand, the agent cannot pay it again either.
    function test_the_agent_cannot_pay_a_cycle_the_principal_settled() public {
        bytes32 h = _hash("human_first");
        vm.prank(principal);
        policy.settleAsPrincipal(vendor, 10 * USDC, h);

        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AlreadyExecuted.selector, h));
        policy.pay(vendor, 10 * USDC, h);
    }

    function test_settlement_is_principal_only() public {
        for (uint256 i = 0; i < 2; i++) {
            address who = i == 0 ? operator : agent;
            vm.prank(who);
            vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
            policy.settleAsPrincipal(vendor, 1 * USDC, _hash("nope"));
        }
    }

    // ----------------------------------------------------------- shape

    function test_zero_amount_and_zero_address_are_refused() public {
        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.ZeroAmount.selector);
        policy.pay(vendor, 0, _hash("zero"));

        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.ZeroAddress.selector);
        policy.pay(address(0), 1 * USDC, _hash("nobody"));
    }

    function test_a_recipient_that_rejects_value_reverts_the_whole_payment() public {
        Rejector r = new Rejector();
        _attest(address(r));
        bytes32 h = _hash("rejector");
        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.TransferFailed.selector);
        policy.pay(address(r), 1 * USDC, h);

        // And the action hash must not be burned by a failed attempt.
        assertFalse(policy.executed(h), "a reverted payment must not consume its hash");
        assertEq(policy.spentInWindow(), 0);
    }

    // ------------------------------------------------------ remaining room

    function test_remaining_allowance_reports_the_binding_limit() public {
        // per-action 100 binds first
        assertEq(policy.remainingAllowance(), 100 * USDC);

        // spend to 450, leaving 50 of daily room, which now binds
        for (uint256 i = 0; i < 4; i++) {
            vm.prank(agent);
            policy.pay(vendor, 100 * USDC, _hash(string(abi.encodePacked("r", i))));
        }
        vm.prank(agent);
        policy.pay(vendor, 50 * USDC, _hash("r4"));
        assertEq(policy.spentInWindow(), 450 * USDC);
        assertEq(policy.remainingAllowance(), 50 * USDC);
    }

    function test_remaining_allowance_is_bounded_by_the_reserve() public {
        _setLimitsNow(100000 * USDC, 100000 * USDC, 990 * USDC);
        assertEq(policy.remainingAllowance(), 10 * USDC);
    }

    // ------------------------------------------------------------ fuzzing

    /// @dev No sequence of agent-initiated payments may take the balance
    ///      below the reserve floor, whatever the amounts.
    function testFuzz_the_reserve_floor_always_holds(uint96 a, uint96 b, uint96 c) public {
        uint256[3] memory amounts = [uint256(a), uint256(b), uint256(c)];
        for (uint256 i = 0; i < 3; i++) {
            uint256 amt = amounts[i] % (200 * USDC);
            if (amt == 0) continue;
            vm.prank(agent);
            try policy.pay(vendor, amt, _hash(string(abi.encodePacked("f", i, amt)))) {} catch {}
            assertGe(policy.balanceUsdc(), policy.reserveFloor(), "floor breached");
        }
    }

    /// @dev Spending in ANY 24-hour window never exceeds the cap, whatever the
    ///      amounts and whatever the gaps between payments. The old property,
    ///      "within one calendar day", passed while the contract let the cap be
    ///      spent twice across midnight.
    function testFuzz_no_window_ever_exceeds_the_cap(uint96[8] calldata raw, uint32[8] calldata gaps) public {
        uint256[8] memory paidAt;
        uint256[8] memory paidAmt;
        uint256 n;
        for (uint256 i = 0; i < 8; i++) {
            vm.warp(vm.getBlockTimestamp() + uint256(gaps[i]) % 2 days);
            uint256 amt = uint256(raw[i]) % (150 * USDC);
            if (amt == 0) continue;
            vm.prank(agent);
            try policy.pay(vendor, amt, keccak256(abi.encode("g", i, amt))) {
                paidAt[n] = vm.getBlockTimestamp();
                paidAmt[n] = amt;
                n++;
            } catch {}
            assertLe(policy.spentInWindow(), policy.dailyCap(), "window total exceeded");
        }
        // Check every window that ends at a payment, from the record of what
        // actually went through rather than from the contract's own counter.
        for (uint256 i = 0; i < n; i++) {
            uint256 sum;
            for (uint256 j = 0; j <= i; j++) {
                if (paidAt[i] - paidAt[j] < policy.WINDOW()) sum += paidAmt[j];
            }
            assertLe(sum, policy.dailyCap(), "some 24-hour window exceeded the cap");
        }
    }

    // ------------------------------------------- one delay rule (docs/46 #3)

    function _pending() internal view returns (uint256 pa, uint256 dc, uint256 rf, uint8 m, uint64 eta, uint8 kinds) {
        BudgetPolicyBase.Mode pm;
        (pa, dc, rf, pm, eta, kinds) = policy.pending();
        m = uint8(pm);
    }

    function test_a_newly_attested_payee_waits_the_delay() public {
        address fresh = address(0xFACE);
        vm.prank(principal);
        policy.setPayee(fresh, true);
        uint256 readyAt = vm.getBlockTimestamp() + policy.delay();
        assertEq(policy.attestedSince(fresh), vm.getBlockTimestamp());

        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.PayeeCoolingOff.selector, fresh, readyAt));
        policy.pay(fresh, 1 * USDC, _hash("too_soon"));

        vm.warp(readyAt - 1);
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.PayeeCoolingOff.selector, fresh, readyAt));
        policy.pay(fresh, 1 * USDC, _hash("one_second_early"));

        vm.warp(readyAt);
        vm.prank(agent);
        policy.pay(fresh, 1 * USDC, _hash("on_time"));
    }

    function test_the_principal_may_settle_a_payee_inside_its_cooling_off() public {
        address fresh = address(0xFACE);
        vm.prank(principal);
        policy.setPayee(fresh, true);
        vm.prank(principal);
        policy.settleAsPrincipal(fresh, 1 * USDC, _hash("by_hand"));
        assertEq(fresh.balance, 1 * USDC * policy.WEI_PER_USDC_UNIT());
    }

    function test_revoke_and_reattest_restarts_the_clock_but_reattesting_does_not() public {
        // vendor was attested in setUp and is past its delay.
        vm.prank(principal);
        policy.setPayee(vendor, true);
        vm.prank(agent);
        policy.pay(vendor, 1 * USDC, _hash("clock_untouched"));

        vm.prank(principal);
        policy.setPayee(vendor, false);
        assertEq(policy.attestedSince(vendor), 0);
        vm.prank(principal);
        policy.setPayee(vendor, true);
        uint256 readyAt = vm.getBlockTimestamp() + policy.delay();
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.PayeeCoolingOff.selector, vendor, readyAt));
        policy.pay(vendor, 1 * USDC, _hash("clock_restarted"));
    }

    function test_lowering_limits_is_instant_and_raising_waits() public {
        vm.prank(operator);
        policy.setLimits(50 * USDC, 400 * USDC, 60 * USDC);
        assertEq(policy.perActionCap(), 50 * USDC, "lowering applies at once");
        assertEq(policy.reserveFloor(), 60 * USDC);
        (,,,,, uint8 kinds0) = _pending();
        assertEq(kinds0, 0, "nothing pending after a shrink");

        uint256 eta = vm.getBlockTimestamp() + policy.delay();
        vm.prank(operator);
        vm.expectEmit(address(policy));
        emit BudgetPolicyBase.ChangeProposed(1, 200 * USDC, 400 * USDC, 60 * USDC, 0, uint64(eta));
        policy.setLimits(200 * USDC, 400 * USDC, 60 * USDC);
        assertEq(policy.perActionCap(), 50 * USDC, "a raise is not applied yet");
        (uint256 pa,,,, uint64 pEta, uint8 kinds) = _pending();
        assertEq(pa, 200 * USDC);
        assertEq(pEta, eta);
        assertEq(kinds, 1);

        // The agent is still bound by the old cap while the raise waits.
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.OverPerActionCap.selector, 60 * USDC, 50 * USDC));
        policy.pay(vendor, 60 * USDC, _hash("still_old_cap"));

        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.TooEarly.selector, uint64(eta)));
        policy.applyPending();

        vm.warp(eta);
        policy.applyPending(); // anyone
        assertEq(policy.perActionCap(), 200 * USDC);
        (,,,,, uint8 kindsAfter) = _pending();
        assertEq(kindsAfter, 0);
        vm.expectRevert(BudgetPolicyBase.NothingPending.selector);
        policy.applyPending();
    }

    function test_a_lowered_floor_is_a_raise_and_waits() public {
        vm.prank(operator);
        policy.setLimits(100 * USDC, 500 * USDC, 10 * USDC);
        assertEq(policy.reserveFloor(), 50 * USDC, "lowering the floor expands what the agent can spend");
    }

    function test_the_principal_or_operator_cancels_a_pending_raise() public {
        vm.prank(operator);
        policy.setLimits(200 * USDC, 500 * USDC, 50 * USDC);
        vm.prank(address(0xBAD));
        vm.expectRevert(BudgetPolicyBase.NotPrincipalOrOperator.selector);
        policy.cancelPending();
        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.NotPrincipalOrOperator.selector);
        policy.cancelPending();

        vm.prank(principal);
        vm.expectEmit(address(policy));
        emit BudgetPolicyBase.ChangeCancelled(1, principal);
        policy.cancelPending();
        (,,,,, uint8 kinds) = _pending();
        assertEq(kinds, 0);
        vm.warp(vm.getBlockTimestamp() + policy.delay());
        vm.expectRevert(BudgetPolicyBase.NothingPending.selector);
        policy.applyPending();
        assertEq(policy.perActionCap(), 100 * USDC, "the cancelled raise never applies");

        vm.prank(operator);
        policy.setLimits(200 * USDC, 500 * USDC, 50 * USDC);
        vm.prank(operator);
        policy.cancelPending();
        vm.prank(principal);
        vm.expectRevert(BudgetPolicyBase.NothingPending.selector);
        policy.cancelPending();
    }

    function test_a_second_proposal_merges_and_restarts_the_clock() public {
        vm.prank(operator);
        policy.setMode(BudgetPolicyBase.Mode.Propose); // lowering: instant
        vm.prank(operator);
        policy.setLimits(200 * USDC, 500 * USDC, 50 * USDC);
        vm.warp(vm.getBlockTimestamp() + 1 days);
        uint256 eta = vm.getBlockTimestamp() + policy.delay();
        vm.prank(operator);
        policy.setMode(BudgetPolicyBase.Mode.Autopilot); // raising: joins the pending change
        (uint256 pa,,, uint8 m, uint64 pEta, uint8 kinds) = _pending();
        assertEq(kinds, 3, "limits and mode pending together");
        assertEq(pa, 200 * USDC);
        assertEq(m, uint8(BudgetPolicyBase.Mode.Autopilot));
        assertEq(pEta, eta, "the clock restarted for both");

        vm.warp(eta - 1);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.TooEarly.selector, uint64(eta)));
        policy.applyPending();
        vm.warp(eta);
        policy.applyPending();
        assertEq(policy.perActionCap(), 200 * USDC);
        assertEq(uint8(policy.mode()), uint8(BudgetPolicyBase.Mode.Autopilot));
    }

    // ------------------------------------------ Watch / Propose / Autopilot (docs/46 #4)

    function test_pay_works_only_in_autopilot_and_lowering_the_mode_is_the_pause() public {
        vm.prank(operator);
        vm.expectEmit(address(policy));
        emit BudgetPolicyBase.ModeChanged(uint8(BudgetPolicyBase.Mode.Autopilot), uint8(BudgetPolicyBase.Mode.Propose));
        policy.setMode(BudgetPolicyBase.Mode.Propose);
        (,,,,, uint8 kinds) = _pending();
        assertEq(kinds, 0, "lowering is instant, nothing pending");

        vm.prank(agent);
        vm.expectRevert(
            abi.encodeWithSelector(BudgetPolicyBase.NotAutopilot.selector, uint8(BudgetPolicyBase.Mode.Propose))
        );
        policy.pay(vendor, 1 * USDC, _hash("proposed_not_paid"));

        // The principal still settles by hand: Propose is exactly that flow.
        vm.prank(principal);
        policy.settleAsPrincipal(vendor, 1 * USDC, _hash("proposed_not_paid"));

        vm.prank(operator);
        policy.setMode(BudgetPolicyBase.Mode.Watch);
        vm.prank(agent);
        vm.expectRevert(
            abi.encodeWithSelector(BudgetPolicyBase.NotAutopilot.selector, uint8(BudgetPolicyBase.Mode.Watch))
        );
        policy.pay(vendor, 1 * USDC, _hash("watching"));
    }

    function test_raising_the_mode_waits_the_delay() public {
        vm.prank(operator);
        policy.setMode(BudgetPolicyBase.Mode.Watch);
        uint256 eta = vm.getBlockTimestamp() + policy.delay();
        vm.prank(operator);
        policy.setMode(BudgetPolicyBase.Mode.Autopilot);
        assertEq(uint8(policy.mode()), uint8(BudgetPolicyBase.Mode.Watch), "still watching");
        (,,, uint8 m, uint64 pEta, uint8 kinds) = _pending();
        assertEq(kinds, 2);
        assertEq(m, uint8(BudgetPolicyBase.Mode.Autopilot));
        assertEq(pEta, eta);

        vm.prank(agent);
        vm.expectRevert(
            abi.encodeWithSelector(BudgetPolicyBase.NotAutopilot.selector, uint8(BudgetPolicyBase.Mode.Watch))
        );
        policy.pay(vendor, 1 * USDC, _hash("not_yet"));

        vm.warp(eta);
        policy.applyPending();
        assertEq(uint8(policy.mode()), uint8(BudgetPolicyBase.Mode.Autopilot));
        vm.prank(agent);
        policy.pay(vendor, 1 * USDC, _hash("autopilot_again"));
    }

    function test_only_the_principal_or_operator_changes_the_mode_or_limits() public {
        address[2] memory neither = [agent, address(0xBAD)];
        for (uint256 i = 0; i < 2; i++) {
            vm.prank(neither[i]);
            vm.expectRevert(BudgetPolicyBase.NotPrincipalOrOperator.selector);
            policy.setMode(BudgetPolicyBase.Mode.Watch);
            vm.prank(neither[i]);
            vm.expectRevert(BudgetPolicyBase.NotPrincipalOrOperator.selector);
            policy.setLimits(1, 1, 0);
        }
    }

    /// docs/52: the treasurer's passkey drives their own autonomy level.
    /// Lowering is instant from either seat; a raise waits the delay whoever
    /// asks, and the principal can drop it.
    function test_the_principal_lowers_at_once_and_a_raise_waits_whoever_asks() public {
        vm.prank(principal);
        policy.setMode(BudgetPolicyBase.Mode.Watch);
        assertEq(uint8(policy.mode()), uint8(BudgetPolicyBase.Mode.Watch), "the principal pauses at once");

        uint256 eta = vm.getBlockTimestamp() + policy.delay();
        vm.prank(principal);
        policy.setMode(BudgetPolicyBase.Mode.Autopilot);
        assertEq(uint8(policy.mode()), uint8(BudgetPolicyBase.Mode.Watch), "a raise by the principal still waits");
        (,,, uint8 m, uint64 pEta, uint8 kinds) = _pending();
        assertEq(kinds, 2);
        assertEq(m, uint8(BudgetPolicyBase.Mode.Autopilot));
        assertEq(pEta, eta);

        vm.prank(principal);
        policy.cancelPending();
        (,,,,, kinds) = _pending();
        assertEq(kinds, 0, "the principal dropped their own raise");

        // Shrinking the caps from the principal's seat is instant too (the
        // floor kept, so nothing in the change loosens anything).
        uint256 floor = policy.reserveFloor(); // read before the prank, which one call consumes
        vm.prank(principal);
        policy.setLimits(1 * USDC, 2 * USDC, floor);
        assertEq(policy.perActionCap(), 1 * USDC);
        assertEq(policy.dailyCap(), 2 * USDC);
    }

    function test_the_constructor_records_delay_and_mode() public {
        BudgetPolicy p = new BudgetPolicy(
            principal, agent, operator, registry, 7, 1 * USDC, 2 * USDC, 0, 10 minutes, BudgetPolicyBase.Mode.Propose
        );
        assertEq(p.delay(), 10 minutes);
        assertEq(uint8(p.mode()), uint8(BudgetPolicyBase.Mode.Propose));
    }
}
