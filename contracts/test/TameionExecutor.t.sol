// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {MODULE_TYPE_EXECUTOR} from "@openzeppelin/contracts/interfaces/draft-IERC7579.sol";
import {TameionExecutor} from "../src/TameionExecutor.sol";
import {TameionModularAccount} from "../src/TameionModularAccount.sol";
import {TestUSDC} from "../src/TestUSDC.sol";

/// @dev The Tameion limits as an ERC-7579 executor module (PLAN 5.29), on a
///      real modular account that a wallet runs by EIP-7702.
contract TameionExecutorTest is Test {
    TameionExecutor internal module;
    TameionModularAccount internal accountImpl;
    TestUSDC internal usdc;

    uint256 internal walletPk = 0x7579;
    address internal account;
    address internal agent = address(0xBEEF);
    address internal vendor = address(0x1111);
    address internal stranger = address(0x4444);

    uint128 internal constant USDC = 1_000000;

    function setUp() public {
        vm.warp(1_764_000_000);
        module = new TameionExecutor();
        accountImpl = new TameionModularAccount();
        usdc = new TestUSDC();
        account = vm.addr(walletPk);
        vm.signAndAttachDelegation(address(accountImpl), walletPk);
        usdc.mint(account, 1000 * USDC);
        // The wallet installs the module on itself (a self-call: no bundler).
        vm.prank(account);
        TameionModularAccount(payable(account))
            .installModule(
                MODULE_TYPE_EXECUTOR,
                address(module),
                abi.encode(agent, address(usdc), 10 * USDC, 30 * USDC, 100 * USDC, uint64(1 hours))
            );
    }

    function _attestAndAutopilot(address payee) internal {
        vm.startPrank(account);
        module.setPayee(payee, true);
        module.requestAutopilot();
        vm.stopPrank();
        vm.warp(vm.getBlockTimestamp() + 1 hours);
        module.applyAutopilot(account);
    }

    function test_installed_with_its_terms_and_reported_by_the_account() public view {
        (
            address a,
            address token,,
            uint64 expiry,
            bool autopilot,
            bool installed,,
            uint128 per,
            uint128 daily,
            uint128 floor
        ) = module.configOf(account);
        assertEq(a, agent);
        assertEq(token, address(usdc));
        assertTrue(installed);
        assertFalse(autopilot, "Propose until the owner asks and the delay passes");
        assertEq(per, 10 * USDC);
        assertEq(daily, 30 * USDC);
        assertEq(floor, 100 * USDC);
        assertGt(expiry, vm.getBlockTimestamp());
        assertTrue(TameionModularAccount(payable(account)).isModuleInstalled(MODULE_TYPE_EXECUTOR, address(module), ""));
    }

    function test_the_agent_pays_from_the_account_inside_every_limit() public {
        _attestAndAutopilot(vendor);
        vm.prank(agent);
        module.pay(account, vendor, 7 * USDC, keccak256("rent"));
        assertEq(usdc.balanceOf(vendor), 7 * USDC);
        assertEq(usdc.balanceOf(account), 993 * USDC);
        assertEq(module.spentInWindow(account), 7 * USDC);
        assertTrue(module.executed(account, keccak256("rent")));
    }

    function test_every_limit_refuses_the_agent() public {
        // Not in Autopilot yet.
        vm.prank(account);
        module.setPayee(vendor, true);
        vm.prank(agent);
        vm.expectRevert(TameionExecutor.NotAutopilot.selector);
        module.pay(account, vendor, 1 * USDC, keccak256("a"));
        _attestAndAutopilot(vendor);
        vm.startPrank(agent);
        vm.expectRevert(abi.encodeWithSelector(TameionExecutor.NotAttested.selector, stranger));
        module.pay(account, stranger, 1 * USDC, keccak256("b"));
        vm.expectRevert(abi.encodeWithSelector(TameionExecutor.OverPerActionCap.selector, 11 * USDC, 10 * USDC));
        module.pay(account, vendor, 11 * USDC, keccak256("c"));
        module.pay(account, vendor, 10 * USDC, keccak256("d1"));
        module.pay(account, vendor, 10 * USDC, keccak256("d2"));
        module.pay(account, vendor, 10 * USDC, keccak256("d3"));
        vm.expectRevert(abi.encodeWithSelector(TameionExecutor.OverDailyCap.selector, 30 * USDC, 1 * USDC, 30 * USDC));
        module.pay(account, vendor, 1 * USDC, keccak256("e"));
        vm.stopPrank();
        // A day later the window has rolled; the same hash is still refused.
        vm.warp(vm.getBlockTimestamp() + 1 days + 1);
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(TameionExecutor.AlreadyExecuted.selector, keccak256("d1")));
        module.pay(account, vendor, 1 * USDC, keccak256("d1"));
        vm.prank(stranger);
        vm.expectRevert(TameionExecutor.NotAgent.selector);
        module.pay(account, vendor, 1 * USDC, keccak256("f"));
    }

    function test_the_reserve_floor_holds() public {
        _attestAndAutopilot(vendor);
        // 1000 held, floor 100: drain to the floor in capped steps, then refused.
        vm.prank(account);
        module.setLimits(10 * USDC, 30 * USDC, 995 * USDC); // raising the floor is narrowing: instant
        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(TameionExecutor.BelowFloor.selector, 1000 * USDC, 6 * USDC, 995 * USDC));
        module.pay(account, vendor, 6 * USDC, keccak256("g"));
        vm.prank(agent);
        module.pay(account, vendor, 5 * USDC, keccak256("h"));
    }

    function test_only_the_account_changes_its_terms_and_widening_waits() public {
        vm.prank(agent);
        vm.expectRevert(TameionExecutor.NotInstalled.selector);
        module.setLimits(1000 * USDC, 1000 * USDC, 0); // the agent is not an installed account
        vm.prank(account);
        module.setLimits(20 * USDC, 60 * USDC, 100 * USDC); // widening
        (,,,,,,, uint128 per,,) = module.configOf(account);
        assertEq(per, 10 * USDC, "not yet");
        vm.expectRevert();
        module.applyLimits(account);
        vm.warp(vm.getBlockTimestamp() + 1 hours);
        module.applyLimits(account); // anyone, after the delay
        (,,,,,,, per,,) = module.configOf(account);
        assertEq(per, 20 * USDC);
    }

    function test_uninstalling_ends_the_modules_power_at_the_account() public {
        _attestAndAutopilot(vendor);
        vm.prank(account);
        TameionModularAccount(payable(account)).uninstallModule(MODULE_TYPE_EXECUTOR, address(module), "");
        vm.prank(agent);
        vm.expectRevert(TameionExecutor.NotInstalled.selector);
        module.pay(account, vendor, 1 * USDC, keccak256("i"));
        assertEq(usdc.balanceOf(vendor), 0);
    }

    function test_an_account_that_never_installed_it_is_out_of_reach() public {
        address other = vm.addr(0x9999);
        usdc.mint(other, 50 * USDC);
        vm.prank(agent);
        vm.expectRevert(TameionExecutor.NotInstalled.selector);
        module.pay(other, vendor, 1 * USDC, keccak256("j"));
    }

    function test_pausing_is_instant_and_revoking_the_agent_is_instant() public {
        _attestAndAutopilot(vendor);
        vm.prank(account);
        module.pause();
        vm.prank(agent);
        vm.expectRevert(TameionExecutor.NotAutopilot.selector);
        module.pay(account, vendor, 1 * USDC, keccak256("k"));
        vm.prank(account);
        module.setAgent(address(0));
        vm.prank(agent);
        vm.expectRevert(TameionExecutor.NotAgent.selector);
        module.pay(account, vendor, 1 * USDC, keccak256("l"));
    }
}
