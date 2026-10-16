// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {Base64} from "@openzeppelin/contracts/utils/Base64.sol";
import {WebAuthn} from "@openzeppelin/contracts/utils/cryptography/WebAuthn.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";
import {PasskeyPrincipal} from "../src/PasskeyPrincipal.sol";

/// @dev Co-owner passkeys and M-of-N approvals (PLAN 5.7), with real P-256
///      WebAuthn assertions.
contract PasskeyCoOwnersTest is Test {
    uint256 internal constant N = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551;
    uint256 internal constant KEY0 = 0x1d4bd2f8b1c2a3e4f5061728394a5b6c7d8e9fa0b1c2d3e4f50617283940abcd;
    uint256 internal constant KEY1 = 0x2a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f70819;
    uint256 internal constant KEY2 = 0x3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f80912;
    uint256 internal constant USDC = 1_000000;
    bytes1 internal constant UP_UV = 0x05;

    PasskeyPrincipal internal owner;
    BudgetPolicy internal policy;
    address internal vendor = address(0x1111);
    address internal relayer = address(0xFEED);
    address internal recoveryAddr = address(0xD00D);

    function setUp() public {
        (uint256 x, uint256 y) = vm.publicKeyP256(KEY0);
        owner = new PasskeyPrincipal(bytes32(x), bytes32(y), recoveryAddr, 7 days);
        policy = new BudgetPolicy(
            address(owner),
            address(0xBEEF),
            address(0xEEEE),
            address(0x8004),
            42,
            100 * USDC,
            500 * USDC,
            0,
            48 hours,
            BudgetPolicyBase.Mode.Propose
        );
    }

    function _pub(uint256 key) internal pure returns (bytes32, bytes32) {
        (uint256 x, uint256 y) = vm.publicKeyP256(key);
        return (bytes32(x), bytes32(y));
    }

    function _assert(uint256 key, bytes32 challenge) internal pure returns (WebAuthn.WebAuthnAuth memory a) {
        a.authenticatorData = abi.encodePacked(sha256("localhost"), UP_UV, bytes4(0x00000001));
        a.clientDataJSON = string.concat(
            '{"type":"webauthn.get","challenge":"',
            Base64.encodeURL(abi.encodePacked(challenge)),
            '","origin":"http://localhost:7880","crossOrigin":false}'
        );
        a.typeIndex = 1;
        a.challengeIndex = 23;
        bytes32 digest = sha256(abi.encodePacked(a.authenticatorData, sha256(bytes(a.clientDataJSON))));
        (bytes32 r, bytes32 s) = vm.signP256(key, digest);
        if (uint256(s) > N / 2) s = bytes32(N - uint256(s));
        a.r = r;
        a.s = s;
    }

    function _ch(address target, bytes memory data) internal view returns (bytes32) {
        return owner.challengeFor(target, 0, data, owner.nonce());
    }

    function _setCoOwnersCall(uint256[] memory keys, uint8 threshold) internal view returns (bytes memory) {
        bytes32[] memory xs = new bytes32[](keys.length);
        bytes32[] memory ys = new bytes32[](keys.length);
        for (uint256 i = 0; i < keys.length; i++) {
            (xs[i], ys[i]) = _pub(keys[i]);
        }
        return abi.encodeCall(PasskeyPrincipal.setCoOwners, (xs, ys, threshold));
    }

    function _one(uint256 k) internal pure returns (uint256[] memory a) {
        a = new uint256[](1);
        a[0] = k;
    }

    function _two(uint256 k0, uint256 k1) internal pure returns (uint256[] memory a) {
        a = new uint256[](2);
        (a[0], a[1]) = (k0, k1);
    }

    function test_a_second_device_signs_alone_at_threshold_one() public {
        bytes memory add = _setCoOwnersCall(_one(KEY1), 1);
        WebAuthn.WebAuthnAuth memory a = _assert(KEY0, _ch(address(owner), add));
        vm.prank(relayer);
        owner.execute(address(owner), 0, add, a);
        assertEq(owner.coOwnerCount(), 1);
        bytes memory attest = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        WebAuthn.WebAuthnAuth memory b = _assert(KEY1, _ch(address(policy), attest));
        vm.prank(relayer);
        owner.executeAs(1, address(policy), 0, attest, b);
        assertTrue(policy.attestedPayee(vendor), "the second device approved alone");
    }

    function test_two_of_two_refuses_one_key_and_runs_with_both() public {
        bytes memory add = _setCoOwnersCall(_one(KEY1), 2);
        vm.prank(relayer);
        owner.execute(address(owner), 0, add, _assert(KEY0, _ch(address(owner), add)));
        assertEq(owner.approvalThreshold(), 2);

        bytes memory attest = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        bytes32 ch = _ch(address(policy), attest);
        WebAuthn.WebAuthnAuth memory a0 = _assert(KEY0, ch);
        WebAuthn.WebAuthnAuth memory a1 = _assert(KEY1, ch);
        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.NeedsMoreApprovals.selector, 1, 2));
        owner.execute(address(policy), 0, attest, a0);
        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.NeedsMoreApprovals.selector, 1, 2));
        owner.executeAs(1, address(policy), 0, attest, a1);

        WebAuthn.WebAuthnAuth[] memory one = new WebAuthn.WebAuthnAuth[](1);
        one[0] = a0;
        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.NeedsMoreApprovals.selector, 1, 2));
        owner.executeWith(address(policy), 0, attest, _one(0), one);

        WebAuthn.WebAuthnAuth[] memory both = new WebAuthn.WebAuthnAuth[](2);
        (both[0], both[1]) = (a0, a1);
        vm.prank(relayer);
        owner.executeWith(address(policy), 0, attest, _two(0, 1), both);
        assertTrue(policy.attestedPayee(vendor), "two people approved");
        assertEq(owner.nonce(), 2);
    }

    function test_the_same_key_twice_or_the_wrong_key_is_refused() public {
        uint256[] memory keys = new uint256[](2);
        (keys[0], keys[1]) = (KEY1, KEY2);
        bytes memory add = _setCoOwnersCall(keys, 2);
        vm.prank(relayer);
        owner.execute(address(owner), 0, add, _assert(KEY0, _ch(address(owner), add)));

        bytes memory attest = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        bytes32 ch = _ch(address(policy), attest);
        WebAuthn.WebAuthnAuth[] memory two = new WebAuthn.WebAuthnAuth[](2);
        (two[0], two[1]) = (_assert(KEY1, ch), _assert(KEY1, ch));
        vm.expectRevert(PasskeyPrincipal.BadKeyIndex.selector);
        owner.executeWith(address(policy), 0, attest, _two(1, 1), two); // one person, twice
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.executeWith(address(policy), 0, attest, _two(1, 2), two); // key 2's slot signed by key 1
        (two[0], two[1]) = (_assert(KEY2, ch), _assert(KEY1, ch)); // each slot its own key
        vm.expectRevert(PasskeyPrincipal.BadKeyIndex.selector);
        owner.executeWith(address(policy), 0, attest, _two(2, 1), two); // ...but not increasing
        assertFalse(policy.attestedPayee(vendor));
    }

    function test_only_the_account_changes_its_owners_and_the_set_is_checked() public {
        bytes memory add = _setCoOwnersCall(_one(KEY1), 1);
        (bool ok,) = address(owner).call(add);
        assertFalse(ok, "a stranger cannot add a key");
        vm.startPrank(address(owner));
        (bytes32 x1, bytes32 y1) = _pub(KEY1);
        bytes32[] memory xs = new bytes32[](1);
        bytes32[] memory ys = new bytes32[](1);
        (xs[0], ys[0]) = (x1, y1);
        vm.expectRevert(PasskeyPrincipal.BadCoOwners.selector);
        owner.setCoOwners(xs, ys, 3); // more approvals than keys
        vm.expectRevert(PasskeyPrincipal.BadCoOwners.selector);
        owner.setCoOwners(xs, ys, 0);
        (xs[0], ys[0]) = _pub(KEY0);
        vm.expectRevert(PasskeyPrincipal.BadCoOwners.selector);
        owner.setCoOwners(xs, ys, 1); // the primary key again
        vm.stopPrank();
    }

    function test_recovery_returns_a_stranded_two_of_two_to_one_key() public {
        bytes memory add = _setCoOwnersCall(_one(KEY1), 2);
        vm.prank(relayer);
        owner.execute(address(owner), 0, add, _assert(KEY0, _ch(address(owner), add)));
        // Both devices lost: the recovery address proposes a new key.
        (bytes32 x2, bytes32 y2) = _pub(KEY2);
        vm.prank(recoveryAddr);
        owner.proposeRecovery(x2, y2);
        vm.warp(vm.getBlockTimestamp() + 7 days);
        owner.applyRecovery();
        assertEq(owner.coOwnerCount(), 0);
        assertEq(owner.approvalThreshold(), 0);
        bytes memory attest = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        vm.prank(relayer);
        owner.execute(address(policy), 0, attest, _assert(KEY2, _ch(address(policy), attest)));
        assertTrue(policy.attestedPayee(vendor), "the recovered key approves alone");
    }
}
