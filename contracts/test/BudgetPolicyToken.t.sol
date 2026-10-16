// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {BudgetPolicyBase} from "../src/BudgetPolicy.sol";
import {BudgetPolicyToken} from "../src/BudgetPolicyToken.sol";

/// @dev Stands in for USDC: six decimals, freely minted. The policy only ever
///      calls `decimals`, `balanceOf` and `transfer`, which are OpenZeppelin's.
contract TestUsdc is ERC20 {
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

contract BudgetPolicyTokenTest is Test {
    BudgetPolicyToken internal policy;
    TestUsdc internal usdc;

    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    address internal agent = address(0xBEEF);
    address internal vendor = address(0x1111);
    address internal stranger = address(0x4444);
    address internal registry = 0x8004A818BFB912233c491871b3d84c89A494BD9e;

    uint256 internal constant USDC = 1_000000;

    function setUp() public {
        vm.warp(1_764_000_000);
        usdc = new TestUsdc(6);
        policy = _deploy(address(usdc));
        usdc.mint(address(policy), 1000 * USDC);
        vm.prank(principal);
        policy.setPayee(vendor, true);
        vm.warp(vm.getBlockTimestamp() + policy.delay());
    }

    function _deploy(address token) internal returns (BudgetPolicyToken) {
        return new BudgetPolicyToken(
            token,
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

    // ------------------------------------------------------------ the asset

    function test_a_limit_and_a_token_amount_are_the_same_integer() public {
        vm.prank(agent);
        policy.pay(vendor, 25 * USDC, _hash("rent"));
        assertEq(usdc.balanceOf(vendor), 25 * USDC);
        assertEq(policy.balanceUsdc(), 975 * USDC);
        assertEq(policy.spentInWindow(), 25 * USDC);
    }

    function test_a_token_without_six_decimals_is_refused_at_deployment() public {
        TestUsdc eighteen = new TestUsdc(18);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyToken.NotSixDecimals.selector, address(eighteen), 18));
        _deploy(address(eighteen));
    }

    function test_no_token_is_refused_at_deployment() public {
        vm.expectRevert(BudgetPolicyBase.ZeroAddress.selector);
        _deploy(address(0));
    }

    function test_native_coin_is_refused_since_nothing_could_account_for_it() public {
        vm.deal(address(this), 1 ether);
        (bool ok,) = address(policy).call{value: 1 ether}("");
        assertFalse(ok);
    }

    // ------------------------------------------------------- the same rules

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

    function test_the_window_cap_cannot_be_spent_twice_across_midnight() public {
        uint256 midnight = (vm.getBlockTimestamp() / 1 days + 1) * 1 days;
        vm.warp(midnight - 1);
        for (uint256 i = 0; i < 5; i++) {
            vm.prank(agent);
            policy.pay(vendor, 100 * USDC, keccak256(abi.encode("before", i)));
        }
        vm.warp(midnight);
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.OverDailyCap.selector, 501 * USDC, 500 * USDC));
        policy.pay(vendor, 1 * USDC, _hash("after"));
    }

    function test_a_replay_is_refused() public {
        vm.startPrank(agent);
        policy.pay(vendor, 10 * USDC, _hash("once"));
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AlreadyExecuted.selector, _hash("once")));
        policy.pay(vendor, 10 * USDC, _hash("once"));
        vm.stopPrank();
    }

    function test_the_reserve_floor_holds() public {
        vm.prank(principal);
        policy.withdraw(principal, 900 * USDC); // 100 left, floor 50
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.BelowReserveFloor.selector, 40 * USDC, 50 * USDC));
        policy.pay(vendor, 60 * USDC, _hash("x"));
    }

    function test_paying_more_than_the_balance_is_named_as_such() public {
        vm.prank(principal);
        policy.withdraw(principal, 990 * USDC); // 10 left
        vm.prank(operator);
        policy.setLimits(100 * USDC, 500 * USDC, 0);
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.InsufficientBalance.selector, 25 * USDC, 10 * USDC));
        policy.pay(vendor, 25 * USDC, _hash("x"));
    }

    // ------------------------------------------------------------ the owner

    function test_only_the_principal_withdraws() public {
        vm.prank(agent);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.withdraw(agent, 1 * USDC);
        vm.prank(operator);
        vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
        policy.withdraw(operator, 1 * USDC);

        vm.prank(principal);
        policy.withdraw(principal, 1000 * USDC);
        assertEq(usdc.balanceOf(principal), 1000 * USDC);
    }

    function test_a_withdrawal_beyond_the_balance_reverts_in_the_token() public {
        vm.prank(principal);
        vm.expectRevert(); // ERC20InsufficientBalance, from the token itself
        policy.withdraw(principal, 1001 * USDC);
    }

    function test_settling_by_hand_pays_in_the_token_and_closes_the_action() public {
        vm.prank(principal);
        policy.settleAsPrincipal(stranger, 300 * USDC, _hash("escalated"));
        assertEq(usdc.balanceOf(stranger), 300 * USDC);
        assertTrue(policy.executed(_hash("escalated")));
        assertEq(policy.spentInWindow(), 0, "a human's payment does not use the agent's room");
    }

    // ----------------------------------------------------------------- fuzz

    function testFuzz_no_window_ever_exceeds_the_cap(uint96[8] memory amounts, uint32[8] memory gaps) public {
        uint256[] memory times = new uint256[](8);
        uint256[] memory paid = new uint256[](8);
        uint256 n;
        for (uint256 i = 0; i < 8; i++) {
            vm.warp(vm.getBlockTimestamp() + (gaps[i] % 2 days));
            uint256 amount = bound(amounts[i], 1, 100 * USDC);
            vm.prank(agent);
            try policy.pay(vendor, amount, keccak256(abi.encode(i, amount))) {
                times[n] = vm.getBlockTimestamp();
                paid[n] = amount;
                n++;
            } catch {}
        }
        for (uint256 i = 0; i < n; i++) {
            uint256 sum;
            for (uint256 j = 0; j < n; j++) {
                if (times[j] <= times[i] && times[i] - times[j] < 1 days) sum += paid[j];
            }
            assertLe(sum, 500 * USDC);
        }
        assertEq(usdc.balanceOf(vendor) + policy.balanceUsdc(), 1000 * USDC, "no token created or lost");
    }
}
