// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {PasskeyPrincipal} from "../src/PasskeyPrincipal.sol";

/// @dev Guardians (PLAN 5.30): several named addresses, a threshold of whom
///      must agree on the same replacement key; the passkey may still cancel.
contract PasskeyGuardiansTest is Test {
    PasskeyPrincipal internal p;

    // A valid P-256 point (generator) as the current key, another as the replacement.
    bytes32 internal constant GX = 0x6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296;
    bytes32 internal constant GY = 0x4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5;
    // 2G
    bytes32 internal constant X2 = 0x7cf27b188d034f7e8a52380304b51ac3c08969e277f21b35a60b48fc47669978;
    bytes32 internal constant Y2 = 0x07775510db8ed040293d9ac69f7430dbba7dade63ce982299e04b79d227873d1;

    address internal recoveryAddr = address(0xAEC0);
    address internal g1 = address(0x6001);
    address internal g2 = address(0x6002);
    address internal g3 = address(0x6003);
    address internal stranger = address(0x4444);

    function setUp() public {
        vm.warp(1_764_000_000);
        p = new PasskeyPrincipal(GX, GY, recoveryAddr, 2 days);
    }

    function _asPasskey() internal {
        // Admin functions are callable only by the contract itself (through
        // `execute`); pranking the contract's own address stands in for a
        // verified passkey approval.
        vm.prank(address(p));
    }

    function _set(address[] memory list, uint8 t) internal {
        _asPasskey();
        p.setGuardians(list, t);
    }

    function _three() internal view returns (address[] memory l) {
        l = new address[](3);
        l[0] = g1;
        l[1] = g2;
        l[2] = g3;
    }

    function test_only_the_passkey_sets_guardians_and_the_list_is_checked() public {
        address[] memory l = _three();
        vm.prank(recoveryAddr);
        vm.expectRevert(PasskeyPrincipal.NotPasskeyOrRecovery.selector);
        p.setGuardians(l, 2);
        _asPasskey();
        vm.expectRevert(PasskeyPrincipal.BadGuardians.selector);
        p.setGuardians(l, 0);
        _asPasskey();
        vm.expectRevert(PasskeyPrincipal.BadGuardians.selector);
        p.setGuardians(l, 4);
        address[] memory dup = new address[](2);
        dup[0] = g1;
        dup[1] = g1;
        _asPasskey();
        vm.expectRevert(PasskeyPrincipal.BadGuardians.selector);
        p.setGuardians(dup, 1);
        _set(l, 2);
        assertEq(p.guardianCount(), 3);
        assertEq(p.guardianThreshold(), 2);
        assertTrue(p.isGuardian(g2));
        assertFalse(p.isGuardian(stranger));
    }

    function test_two_of_three_make_a_replacement_pending_and_the_passkey_cancels() public {
        _set(_three(), 2);
        vm.prank(stranger);
        vm.expectRevert(PasskeyPrincipal.NotGuardian.selector);
        p.approveRecovery(X2, Y2);
        vm.prank(g1);
        p.approveRecovery(X2, Y2);
        (,, uint64 eta0) = p.pendingKey();
        assertEq(eta0, 0, "one of two is not enough");
        vm.prank(g1);
        vm.expectRevert(PasskeyPrincipal.AlreadyApproved.selector);
        p.approveRecovery(X2, Y2);
        vm.prank(g2);
        p.approveRecovery(X2, Y2);
        (bytes32 pqx,, uint64 eta) = p.pendingKey();
        assertEq(pqx, X2);
        assertEq(eta, uint64(vm.getBlockTimestamp() + 2 days));
        // The passkey notices and cancels; the ballot is void afterwards.
        uint64 epochBefore = p.recoveryEpoch();
        _asPasskey();
        p.cancelRecovery();
        (,, uint64 etaAfter) = p.pendingKey();
        assertEq(etaAfter, 0);
        assertEq(p.recoveryEpoch(), epochBefore + 1);
        assertEq(p.qx(), GX, "the key did not change");
        // The same two guardians must agree again from scratch.
        vm.prank(g1);
        p.approveRecovery(X2, Y2);
        (,, uint64 eta2) = p.pendingKey();
        assertEq(eta2, 0);
    }

    function test_guardians_disagreeing_on_the_key_never_reach_the_threshold() public {
        _set(_three(), 2);
        vm.prank(g1);
        p.approveRecovery(X2, Y2);
        vm.prank(g2);
        p.approveRecovery(GX, GY);
        (,, uint64 eta) = p.pendingKey();
        assertEq(eta, 0);
    }

    function test_after_the_delay_anyone_applies_and_the_key_rotates() public {
        _set(_three(), 2);
        vm.prank(g1);
        p.approveRecovery(X2, Y2);
        vm.prank(g3);
        p.approveRecovery(X2, Y2);
        vm.warp(vm.getBlockTimestamp() + 2 days);
        vm.prank(stranger);
        p.applyRecovery();
        assertEq(p.qx(), X2);
        assertEq(p.qy(), Y2);
    }

    function test_changing_the_guardians_voids_partial_approvals() public {
        _set(_three(), 2);
        vm.prank(g1);
        p.approveRecovery(X2, Y2);
        address[] memory l2 = new address[](2);
        l2[0] = g1;
        l2[1] = g2;
        _set(l2, 2);
        vm.prank(g2);
        p.approveRecovery(X2, Y2);
        (,, uint64 eta) = p.pendingKey();
        assertEq(eta, 0, "g1's earlier approval belongs to the old epoch");
        vm.prank(g1);
        p.approveRecovery(X2, Y2);
        (,, uint64 eta2) = p.pendingKey();
        assertTrue(eta2 != 0);
    }

    function test_the_single_recovery_address_still_works_beside_guardians() public {
        _set(_three(), 3);
        vm.prank(recoveryAddr);
        p.proposeRecovery(X2, Y2);
        (bytes32 pqx,,) = p.pendingKey();
        assertEq(pqx, X2);
    }

    function test_removing_guardians_is_an_empty_list_with_threshold_zero() public {
        _set(_three(), 2);
        address[] memory none = new address[](0);
        _set(none, 0);
        assertEq(p.guardianCount(), 0);
        vm.prank(g1);
        vm.expectRevert(PasskeyPrincipal.NotGuardian.selector);
        p.approveRecovery(X2, Y2);
    }
}
