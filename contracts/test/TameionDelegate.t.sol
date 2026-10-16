// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BudgetPolicyBase} from "../src/BudgetPolicy.sol";
import {TameionDelegate} from "../src/TameionDelegate.sol";

/// @dev The policy inside the company's own wallet, by EIP-7702 (PLAN 5.28).
contract TameionDelegateTest is Test {
    TameionDelegate internal impl;
    uint256 internal walletPk = 0xA11CE5;
    address internal wallet;

    address internal principal = address(0xB055); // the owner's passkey account
    address internal operator = address(0xEEEE);
    address internal agent = address(0xBEEF);
    address internal vendor = address(0x1111);
    address internal stranger = address(0x4444);
    address internal registry = 0x8004A818BFB912233c491871b3d84c89A494BD9e;

    uint256 internal constant USDC = 1_000000;
    uint256 internal constant WEI = 1e12;

    function setUp() public {
        vm.warp(1_764_000_000);
        impl = new TameionDelegate();
        wallet = vm.addr(walletPk);
        vm.deal(wallet, 1000 * USDC * WEI);
        // The wallet points its code at the delegate and initialises itself.
        vm.signAndAttachDelegation(address(impl), walletPk);
        vm.prank(wallet);
        TameionDelegate(payable(wallet))
            .initialize(
                principal,
                agent,
                operator,
                registry,
                42,
                10 * USDC,
                50 * USDC,
                100 * USDC,
                1 hours,
                BudgetPolicyBase.Mode.Autopilot
            );
    }

    function _w() internal view returns (TameionDelegate) {
        return TameionDelegate(payable(wallet));
    }

    function test_the_wallet_becomes_the_policy_at_its_own_address() public view {
        assertEq(_w().principal(), principal);
        assertEq(_w().agent(), agent);
        assertEq(_w().perActionCap(), 10 * USDC);
        assertEq(_w().balanceUsdc(), 1000 * USDC, "the money never moved");
        assertTrue(_w().initialized());
        assertFalse(impl.initialized(), "the implementation's own storage is untouched");
    }

    function test_nobody_else_can_initialise_a_wallet_or_the_implementation() public {
        vm.prank(stranger);
        vm.expectRevert(TameionDelegate.NotSelf.selector);
        _w().initialize(stranger, stranger, stranger, registry, 1, 1, 1, 0, 0, BudgetPolicyBase.Mode.Autopilot);
        vm.prank(stranger);
        vm.expectRevert(TameionDelegate.NotSelf.selector);
        impl.initialize(stranger, stranger, stranger, registry, 1, 1, 1, 0, 0, BudgetPolicyBase.Mode.Autopilot);
        // Not even the wallet itself, twice.
        vm.prank(wallet);
        vm.expectRevert(BudgetPolicyBase.AlreadyInitialized.selector);
        _w().initialize(stranger, stranger, stranger, registry, 1, 1, 1, 0, 0, BudgetPolicyBase.Mode.Autopilot);
    }

    function test_the_agent_pays_an_attested_payee_from_the_wallet_inside_the_caps() public {
        vm.prank(principal);
        _w().setPayee(vendor, true);
        vm.warp(vm.getBlockTimestamp() + 1 hours);
        vm.prank(agent);
        _w().pay(vendor, 7 * USDC, keccak256("rent-oct"));
        assertEq(vendor.balance, 7 * USDC * WEI);
        assertEq(_w().balanceUsdc(), 993 * USDC);
        assertTrue(_w().executed(keccak256("rent-oct")));
    }

    function test_the_contract_refuses_the_agent_inside_the_wallet_too() public {
        vm.prank(principal);
        _w().setPayee(vendor, true);
        // A payee attested just now is not payable until the delay passes.
        vm.prank(agent);
        vm.expectRevert();
        _w().pay(vendor, 1 * USDC, keccak256("too-soon"));
        vm.warp(vm.getBlockTimestamp() + 1 hours);
        vm.prank(agent);
        vm.expectRevert();
        _w().pay(vendor, 11 * USDC, keccak256("over-cap"));
        vm.prank(agent);
        vm.expectRevert();
        _w().pay(stranger, 1 * USDC, keccak256("not-attested"));
        // The agent cannot attest, raise its cap, or withdraw from the wallet.
        vm.startPrank(agent);
        vm.expectRevert();
        _w().setPayee(stranger, true);
        vm.expectRevert();
        _w().withdraw(agent, 1 * USDC);
        vm.stopPrank();
        assertEq(vendor.balance, 0);
        assertEq(stranger.balance, 0);
    }

    function test_the_owners_passkey_account_withdraws_from_the_wallet() public {
        vm.prank(principal);
        _w().withdraw(principal, 20 * USDC);
        assertEq(principal.balance, 20 * USDC * WEI);
    }

    /// Said plainly in the contract: 7702 bounds the agent, not the wallet's
    /// own key, which can still send directly.
    function test_the_wallets_own_key_keeps_its_power() public {
        vm.prank(wallet);
        (bool ok,) = stranger.call{value: 1 * USDC * WEI}("");
        assertTrue(ok);
        assertEq(stranger.balance, 1 * USDC * WEI);
    }
}
