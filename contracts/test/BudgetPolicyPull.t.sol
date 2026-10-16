// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {BudgetPolicyBase} from "../src/BudgetPolicy.sol";
import {BudgetPolicyPull} from "../src/BudgetPolicyPull.sol";

contract PullTestUsdc is ERC20 {
    uint8 private immutable dec;

    constructor(uint8 _dec) ERC20("USD Coin", "USDC") {
        dec = _dec;
    }

    function decimals() public view override returns (uint8) {
        return dec;
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

/// @dev The pull variant: the company's USDC never leaves its wallet until a
///      payment; the allowance is a cap the treasurer holds without Tameion.
contract BudgetPolicyPullTest is Test {
    BudgetPolicyPull internal policy;
    PullTestUsdc internal usdc;

    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    address internal agent = address(0xBEEF);
    address internal vendor = address(0x1111);
    address internal stranger = address(0x4444);
    address internal wallet = address(0xC0FFEE);
    address internal registry = 0x8004A818BFB912233c491871b3d84c89A494BD9e;

    uint256 internal constant USDC = 1_000000;

    function setUp() public {
        vm.warp(1_764_000_000);
        usdc = new PullTestUsdc(6);
        policy = _deploy(address(usdc), wallet);
        usdc.mint(wallet, 1000 * USDC);
        vm.prank(wallet);
        usdc.approve(address(policy), 300 * USDC);
        vm.prank(principal);
        policy.setPayee(vendor, true);
        vm.warp(vm.getBlockTimestamp() + policy.delay());
    }

    function _deploy(address token, address src) internal returns (BudgetPolicyPull) {
        return new BudgetPolicyPull(
            token,
            src,
            principal,
            agent,
            operator,
            registry,
            42,
            100 * USDC, // per action
            500 * USDC, // per window
            50 * USDC, // reserve floor
            48 hours,
            BudgetPolicyBase.Mode.Autopilot
        );
    }

    function _hash(string memory s) internal pure returns (bytes32) {
        return keccak256(bytes(s));
    }

    // ------------------------------------------------------------ the shape

    function test_the_policy_holds_nothing_and_the_wallet_pays() public {
        vm.prank(agent);
        policy.pay(vendor, 25 * USDC, _hash("rent"));
        assertEq(usdc.balanceOf(vendor), 25 * USDC);
        assertEq(usdc.balanceOf(wallet), 975 * USDC);
        assertEq(usdc.balanceOf(address(policy)), 0, "the contract never holds money");
        assertEq(policy.spentInWindow(), 25 * USDC);
    }

    function test_reach_is_the_smaller_of_balance_and_allowance() public {
        assertEq(policy.balanceUsdc(), 300 * USDC, "allowance below balance");
        vm.prank(wallet);
        usdc.approve(address(policy), 5000 * USDC);
        assertEq(policy.balanceUsdc(), 1000 * USDC, "balance below allowance");
        assertEq(policy.allowanceRemaining(), 5000 * USDC);
    }

    function test_the_treasurer_cuts_the_agent_off_with_one_approve_zero() public {
        vm.prank(wallet);
        usdc.approve(address(policy), 0);
        assertEq(policy.balanceUsdc(), 0);
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.InsufficientBalance.selector, 25 * USDC, 0));
        policy.pay(vendor, 25 * USDC, _hash("rent"));
    }

    function test_the_reserve_floor_means_leave_that_much_in_the_wallet() public {
        // Reach is 300 (allowance); floor 50; a 100 payment fits, repeatedly,
        // until reach - floor would go negative.
        vm.startPrank(agent);
        policy.pay(vendor, 100 * USDC, _hash("a"));
        policy.pay(vendor, 100 * USDC, _hash("b"));
        // reach is now 100 (allowance left); floor 50 → at most 50 more.
        vm.expectRevert();
        policy.pay(vendor, 60 * USDC, _hash("c"));
        policy.pay(vendor, 50 * USDC, _hash("d"));
        vm.stopPrank();
        assertEq(usdc.balanceOf(vendor), 250 * USDC);
    }

    function test_a_token_without_six_decimals_is_refused_at_deployment() public {
        PullTestUsdc bad = new PullTestUsdc(18);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyPull.NotSixDecimals.selector, address(bad), 18));
        _deploy(address(bad), wallet);
    }

    function test_no_source_is_refused_at_deployment() public {
        vm.expectRevert(BudgetPolicyBase.ZeroAddress.selector);
        _deploy(address(usdc), address(0));
    }

    function test_native_coin_is_refused_since_nothing_could_account_for_it() public {
        vm.deal(stranger, 1 ether);
        vm.prank(stranger);
        (bool ok,) = address(policy).call{value: 1 ether}("");
        assertFalse(ok);
    }

    // ------------------------------------------------- the same rules apply

    function test_an_unattested_payee_is_refused() public {
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.PayeeNotAttested.selector, stranger));
        policy.pay(stranger, 1 * USDC, _hash("x"));
    }

    function test_the_per_action_cap_holds() public {
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.OverPerActionCap.selector, 101 * USDC, 100 * USDC));
        policy.pay(vendor, 101 * USDC, _hash("x"));
    }

    function test_a_replay_is_refused() public {
        vm.startPrank(agent);
        policy.pay(vendor, 10 * USDC, _hash("same"));
        vm.expectRevert();
        policy.pay(vendor, 10 * USDC, _hash("same"));
        vm.stopPrank();
    }

    function test_paying_more_than_the_reach_is_named_as_such() public {
        vm.prank(wallet);
        usdc.approve(address(policy), 20 * USDC);
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.InsufficientBalance.selector, 30 * USDC, 20 * USDC));
        policy.pay(vendor, 30 * USDC, _hash("x"));
    }

    function test_only_the_principal_withdraws_and_it_pulls_from_the_wallet() public {
        vm.prank(stranger);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.withdraw(stranger, 1 * USDC);
        vm.prank(principal);
        policy.withdraw(principal, 7 * USDC);
        assertEq(usdc.balanceOf(principal), 7 * USDC);
        assertEq(usdc.balanceOf(wallet), 993 * USDC);
    }

    function test_settling_by_hand_pulls_from_the_wallet_and_closes_the_action() public {
        bytes32 h = _hash("manual");
        vm.prank(principal);
        policy.settleAsPrincipal(vendor, 12 * USDC, h);
        assertEq(usdc.balanceOf(vendor), 12 * USDC);
        assertTrue(policy.executed(h));
        vm.prank(agent);
        vm.expectRevert();
        policy.pay(vendor, 12 * USDC, h);
    }

    function test_a_paid_event_names_what_was_pulled() public {
        bytes32 h = _hash("rent");
        vm.expectEmit(true, true, true, true, address(policy));
        emit BudgetPolicyBase.Paid(vendor, 25 * USDC, h);
        vm.prank(agent);
        policy.pay(vendor, 25 * USDC, h);
    }

    /// PLAN 5.39: the owner's reviewed run pulls each row from the bill
    /// wallet, inside the allowance, and nothing stays in the policy.
    function test_the_owners_run_pulls_every_row_from_the_wallet() public {
        address[] memory to = new address[](2);
        uint256[] memory units = new uint256[](2);
        bytes32[] memory hs = new bytes32[](2);
        (to[0], to[1]) = (vendor, stranger);
        (units[0], units[1]) = (5 * USDC, 7 * USDC);
        (hs[0], hs[1]) = (keccak256("r1"), keccak256("r2"));
        bytes32 terms = policy.fingerprint();
        uint64 deadline = uint64(vm.getBlockTimestamp() + 600);
        uint256 before = usdc.balanceOf(wallet);
        vm.prank(principal);
        uint256 paid = policy.settleRunIf(to, units, hs, terms, deadline);
        assertEq(paid, 2);
        assertEq(usdc.balanceOf(vendor), 5 * USDC);
        assertEq(usdc.balanceOf(stranger), 7 * USDC);
        assertEq(usdc.balanceOf(wallet), before - 12 * USDC);
        assertEq(usdc.balanceOf(address(policy)), 0, "nothing stays in the policy");
    }
}
