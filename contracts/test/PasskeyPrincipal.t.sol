// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {Base64} from "@openzeppelin/contracts/utils/Base64.sol";
import {WebAuthn} from "@openzeppelin/contracts/utils/cryptography/WebAuthn.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";
import {PasskeyPrincipal} from "../src/PasskeyPrincipal.sol";

/// The principal's key is a passkey: these tests sign real P-256 WebAuthn
/// assertions, built the way a phone builds them, and check that only an
/// assertion over exactly the submitted call, made with user verification,
/// moves the policy.
contract PasskeyPrincipalTest is Test {
    uint256 internal constant N = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551;
    uint256 internal constant HALF_N = N / 2;
    uint256 internal constant PASSKEY = uint256(keccak256("tameion test scalar PasskeyPrincipal.t.sol PASSKEY"))
        % 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551; // derived, never a literal key
    uint256 internal constant OTHER_PASSKEY = uint256(
        keccak256("tameion test scalar PasskeyPrincipal.t.sol OTHER_PASSKEY")
    ) % 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551; // derived, never a literal key
    uint256 internal constant USDC = 1_000000;

    PasskeyPrincipal internal owner;
    BudgetPolicy internal policy;
    address internal agent = address(0xBEEF);
    address internal operator = address(0xEEEE);
    address internal vendor = address(0x1111);
    address internal relayer = address(0xFEED); // anyone; pays gas, holds no authority
    address internal recoveryAddr = address(0xD00D); // a hardware wallet in a drawer

    function setUp() public {
        (uint256 x, uint256 y) = vm.publicKeyP256(PASSKEY);
        owner = new PasskeyPrincipal(bytes32(x), bytes32(y), recoveryAddr, 7 days);
        policy = new BudgetPolicy(
            address(owner),
            agent,
            operator,
            address(0x8004),
            42,
            100 * USDC,
            500 * USDC,
            0,
            48 hours,
            BudgetPolicyBase.Mode.Autopilot
        );
        vm.deal(address(policy), 1000 * USDC * policy.WEI_PER_USDC_UNIT());
    }

    /// A WebAuthn assertion over `challenge`, as a platform authenticator
    /// returns it: authenticatorData (rpIdHash, flags, counter) and the
    /// clientDataJSON whose "challenge" is the base64url of the 32 bytes.
    function _assert(uint256 key, bytes32 challenge, bytes1 flags)
        internal
        pure
        returns (WebAuthn.WebAuthnAuth memory a)
    {
        a.authenticatorData = abi.encodePacked(sha256("localhost"), flags, bytes4(0x00000001));
        a.clientDataJSON = string.concat(
            '{"type":"webauthn.get","challenge":"',
            Base64.encodeURL(abi.encodePacked(challenge)),
            '","origin":"http://localhost:8787","crossOrigin":false}'
        );
        a.typeIndex = 1;
        a.challengeIndex = 23;
        bytes32 digest = sha256(abi.encodePacked(a.authenticatorData, sha256(bytes(a.clientDataJSON))));
        (bytes32 r, bytes32 s) = vm.signP256(key, digest);
        // P-256 signatures are malleable; the verifier accepts only low-s,
        // so a client normalises before submitting.
        if (uint256(s) > HALF_N) s = bytes32(N - uint256(s));
        a.r = r;
        a.s = s;
    }

    bytes1 internal constant UP_UV = 0x05; // user present + user verified (Face ID, fingerprint)
    bytes1 internal constant UP_ONLY = 0x01; // a tap, no biometric

    function _approve(uint256 key, address target, bytes memory data, bytes1 flags)
        internal
        view
        returns (WebAuthn.WebAuthnAuth memory)
    {
        return _assert(key, owner.challengeFor(target, 0, data, owner.nonce()), flags);
    }

    function test_the_passkey_attests_a_payee() public {
        bytes memory call = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        WebAuthn.WebAuthnAuth memory a = _approve(PASSKEY, address(policy), call, UP_UV);
        vm.prank(relayer);
        owner.execute(address(policy), 0, call, a);
        assertTrue(policy.attestedPayee(vendor));
        assertEq(owner.nonce(), 1);

        // docs/46 #3: the payee the owner just approved is payable after the delay.
        vm.warp(vm.getBlockTimestamp() + policy.delay());
        vm.prank(agent);
        policy.pay(vendor, 10 * USDC, keccak256("paid after the owner's Face ID"));
    }

    function test_an_approval_cannot_be_used_twice() public {
        bytes memory call = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        WebAuthn.WebAuthnAuth memory a = _approve(PASSKEY, address(policy), call, UP_UV);
        owner.execute(address(policy), 0, call, a);
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.execute(address(policy), 0, call, a);
    }

    function test_an_approval_cannot_be_pointed_at_a_different_call() public {
        // The owner approved attesting the vendor; a relayer swaps in its own.
        bytes memory approved = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        bytes memory swapped = abi.encodeCall(BudgetPolicyBase.setPayee, (relayer, true));
        WebAuthn.WebAuthnAuth memory a = _approve(PASSKEY, address(policy), approved, UP_UV);
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.execute(address(policy), 0, swapped, a);
        assertFalse(policy.attestedPayee(relayer));
    }

    function test_another_passkey_cannot_approve() public {
        bytes memory call = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        WebAuthn.WebAuthnAuth memory a = _approve(OTHER_PASSKEY, address(policy), call, UP_UV);
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.execute(address(policy), 0, call, a);
    }

    function test_a_tap_without_biometrics_is_not_enough() public {
        bytes memory call = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        WebAuthn.WebAuthnAuth memory a = _approve(PASSKEY, address(policy), call, UP_ONLY);
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.execute(address(policy), 0, call, a);
    }

    function test_an_approval_is_bound_to_its_chain() public {
        bytes memory call = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        WebAuthn.WebAuthnAuth memory a = _approve(PASSKEY, address(policy), call, UP_UV);
        vm.chainId(42161);
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.execute(address(policy), 0, call, a);
    }

    function test_without_the_passkey_nobody_holds_the_principal_role() public {
        // There is no key file for the principal any more. The agent, the
        // operator and the relayer are all refused.
        address[3] memory who = [agent, operator, relayer];
        for (uint256 i = 0; i < 3; i++) {
            vm.prank(who[i]);
            vm.expectRevert(BudgetPolicyBase.NotPrincipal.selector);
            policy.setPayee(who[i], true);
        }
    }

    function test_the_passkey_can_move_money_out() public {
        bytes memory call = abi.encodeCall(BudgetPolicyBase.withdraw, (vendor, 25 * USDC));
        WebAuthn.WebAuthnAuth memory a = _approve(PASSKEY, address(policy), call, UP_UV);
        owner.execute(address(policy), 0, call, a);
        assertEq(vendor.balance, 25 * USDC * policy.WEI_PER_USDC_UNIT());
    }

    function test_a_failed_inner_call_does_not_consume_the_approval() public {
        // Attesting the zero address reverts inside the policy; the whole
        // execute reverts, so the nonce stays and the owner can try again.
        bytes memory call = abi.encodeCall(BudgetPolicyBase.setPayee, (address(0), true));
        WebAuthn.WebAuthnAuth memory a = _approve(PASSKEY, address(policy), call, UP_UV);
        vm.expectRevert();
        owner.execute(address(policy), 0, call, a);
        assertEq(owner.nonce(), 0);
    }

    // ------------------------------------------ recovery (docs/46, 2026-10-13)

    function _otherKey() internal view returns (bytes32 x2, bytes32 y2) {
        (uint256 x, uint256 y) = vm.publicKeyP256(OTHER_PASSKEY);
        return (bytes32(x), bytes32(y));
    }

    function test_only_the_recovery_address_proposes_a_new_key() public {
        (bytes32 x2, bytes32 y2) = _otherKey();
        address[3] memory not = [agent, relayer, address(this)];
        for (uint256 i = 0; i < 3; i++) {
            vm.prank(not[i]);
            vm.expectRevert(PasskeyPrincipal.NotRecovery.selector);
            owner.proposeRecovery(x2, y2);
        }
        vm.prank(recoveryAddr);
        vm.expectRevert(PasskeyPrincipal.ZeroKey.selector);
        owner.proposeRecovery(bytes32(0), bytes32(0));
    }

    function test_a_proposed_key_waits_the_delay_then_anyone_applies_it() public {
        (bytes32 x2, bytes32 y2) = _otherKey();
        uint64 eta = uint64(vm.getBlockTimestamp() + owner.recoveryDelay());
        vm.prank(recoveryAddr);
        vm.expectEmit(address(owner));
        emit PasskeyPrincipal.RecoveryProposed(x2, y2, eta, recoveryAddr);
        owner.proposeRecovery(x2, y2);

        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.TooEarly.selector, eta));
        owner.applyRecovery();

        // The old phone still rules until the delay passes.
        bytes memory call = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, true));
        WebAuthn.WebAuthnAuth memory a = _approve(PASSKEY, address(policy), call, UP_UV);
        owner.execute(address(policy), 0, call, a);

        vm.warp(eta);
        vm.prank(relayer); // anyone
        owner.applyRecovery();
        assertEq(owner.qx(), x2);
        assertEq(owner.qy(), y2);
        (,, uint64 pendingEta) = owner.pendingKey();
        assertEq(pendingEta, 0, "nothing pending after apply");

        // The old passkey is refused; the new one approves, with the nonce continuing.
        bytes memory again = abi.encodeCall(BudgetPolicyBase.setPayee, (vendor, false));
        WebAuthn.WebAuthnAuth memory old = _approve(PASSKEY, address(policy), again, UP_UV);
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.execute(address(policy), 0, again, old);
        WebAuthn.WebAuthnAuth memory fresh = _approve(OTHER_PASSKEY, address(policy), again, UP_UV);
        owner.execute(address(policy), 0, again, fresh);
        assertFalse(policy.attestedPayee(vendor));
    }

    function test_the_phone_cancels_a_recovery_it_did_not_ask_for() public {
        (bytes32 x2, bytes32 y2) = _otherKey();
        vm.prank(recoveryAddr);
        owner.proposeRecovery(x2, y2);

        // Only the passkey (through execute) or the recovery address may cancel.
        vm.prank(relayer);
        vm.expectRevert(PasskeyPrincipal.NotPasskeyOrRecovery.selector);
        owner.cancelRecovery();

        bytes memory cancel = abi.encodeCall(PasskeyPrincipal.cancelRecovery, ());
        WebAuthn.WebAuthnAuth memory a = _approve(PASSKEY, address(owner), cancel, UP_UV);
        vm.expectEmit(address(owner));
        emit PasskeyPrincipal.RecoveryCancelled(address(owner));
        owner.execute(address(owner), 0, cancel, a);

        vm.warp(vm.getBlockTimestamp() + owner.recoveryDelay());
        vm.expectRevert(PasskeyPrincipal.NothingPending.selector);
        owner.applyRecovery();
        (uint256 x, uint256 y) = vm.publicKeyP256(PASSKEY);
        assertEq(owner.qx(), bytes32(x), "the key did not move");
        assertEq(owner.qy(), bytes32(y));
    }

    function test_the_recovery_address_can_withdraw_its_own_proposal() public {
        (bytes32 x2, bytes32 y2) = _otherKey();
        vm.prank(recoveryAddr);
        owner.proposeRecovery(x2, y2);
        vm.prank(recoveryAddr);
        owner.cancelRecovery();
        vm.prank(recoveryAddr);
        vm.expectRevert(PasskeyPrincipal.NothingPending.selector);
        owner.cancelRecovery();
    }

    function test_only_the_passkey_changes_the_recovery_address() public {
        vm.prank(recoveryAddr);
        vm.expectRevert(PasskeyPrincipal.NotPasskeyOrRecovery.selector);
        owner.setRecovery(relayer);

        bytes memory call = abi.encodeCall(PasskeyPrincipal.setRecovery, (relayer));
        WebAuthn.WebAuthnAuth memory a = _approve(PASSKEY, address(owner), call, UP_UV);
        owner.execute(address(owner), 0, call, a);
        assertEq(owner.recovery(), relayer);

        (bytes32 x2, bytes32 y2) = _otherKey();
        vm.prank(recoveryAddr);
        vm.expectRevert(PasskeyPrincipal.NotRecovery.selector);
        owner.proposeRecovery(x2, y2);
        vm.prank(relayer);
        owner.proposeRecovery(x2, y2);
    }

    function test_a_principal_without_recovery_cannot_be_recovered() public {
        (uint256 x, uint256 y) = vm.publicKeyP256(PASSKEY);
        PasskeyPrincipal lone = new PasskeyPrincipal(bytes32(x), bytes32(y), address(0), 7 days);
        (bytes32 x2, bytes32 y2) = _otherKey();
        vm.prank(address(0));
        vm.expectRevert(PasskeyPrincipal.NotRecovery.selector);
        lone.proposeRecovery(x2, y2);
    }

    /// docs/52: a key that is not a point on P-256 can never sign, so the
    /// contract refuses it at deploy and at recovery instead of locking the
    /// policy for good. A real key passes; the same key with y+1 does not.
    function test_a_key_off_the_curve_is_refused_at_deploy_and_at_recovery() public {
        (uint256 x, uint256 y) = vm.publicKeyP256(PASSKEY);
        vm.expectRevert(PasskeyPrincipal.NotOnCurve.selector);
        new PasskeyPrincipal(bytes32(x), bytes32(y + 1), recoveryAddr, 7 days);
        vm.expectRevert(PasskeyPrincipal.NotOnCurve.selector);
        new PasskeyPrincipal(bytes32(uint256(1)), bytes32(uint256(2)), recoveryAddr, 7 days);
        // Coordinates at or above the field prime are not points either.
        vm.expectRevert(PasskeyPrincipal.NotOnCurve.selector);
        new PasskeyPrincipal(bytes32(type(uint256).max), bytes32(y), recoveryAddr, 7 days);

        vm.prank(recoveryAddr);
        vm.expectRevert(PasskeyPrincipal.NotOnCurve.selector);
        owner.proposeRecovery(bytes32(x), bytes32(y + 1));

        // The real key still deploys.
        PasskeyPrincipal fine = new PasskeyPrincipal(bytes32(x), bytes32(y), recoveryAddr, 7 days);
        assertEq(fine.qx(), bytes32(x));
    }
}
