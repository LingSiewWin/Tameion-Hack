// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test, console} from "forge-std/Test.sol";
import {Base64} from "@openzeppelin/contracts/utils/Base64.sol";
import {WebAuthn} from "@openzeppelin/contracts/utils/cryptography/WebAuthn.sol";
import {MessageHashUtils} from "@openzeppelin/contracts/utils/cryptography/MessageHashUtils.sol";
import {PasskeyPrincipal} from "../src/PasskeyPrincipal.sol";
import {P256WebAuthnVerifier} from "../src/verifiers/P256WebAuthnVerifier.sol";
import {Secp256k1Verifier} from "../src/verifiers/Secp256k1Verifier.sol";
import {MlDsaVerifier} from "../src/verifiers/MlDsaVerifier.sol";

/// Something for an approved call to change.
contract Pokee {
    uint256 public last;

    function poke(uint256 v) external {
        last = v;
    }
}

/// Real signatures: P-256 WebAuthn assertions built the way a phone builds
/// them, and an Ethereum key's signature over the challenge itself.
abstract contract AgilitySigner is Test {
    uint256 internal constant N = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551;
    uint256 internal constant SECP_N = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
    uint256 internal constant PASSKEY = 0x1d4bd2f8b1c2a3e4f5061728394a5b6c7d8e9fa0b1c2d3e4f50617283940abcd;
    uint256 internal constant PASSKEY2 = 0x2a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f70819;
    bytes1 internal constant UP_UV = 0x05;

    bytes32 internal constant SECP = keccak256("secp256k1");
    bytes32 internal constant SECP_B = keccak256("secp256k1-b");
    bytes32 internal constant MLDSA = keccak256("ml-dsa-65");
    bytes32 internal constant P256_EXT = keccak256("p256-webauthn-verifier");

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
        if (uint256(s) > N / 2) s = bytes32(N - uint256(s));
        a.r = r;
        a.s = s;
    }

    /// The `bytes` form `executeSigned` and `P256WebAuthnVerifier` read.
    function _enc(WebAuthn.WebAuthnAuth memory a) internal pure returns (bytes memory) {
        return abi.encode(a.r, a.s, a.challengeIndex, a.typeIndex, a.authenticatorData, a.clientDataJSON);
    }

    function _p256Key(uint256 key) internal pure returns (bytes memory) {
        (uint256 x, uint256 y) = vm.publicKeyP256(key);
        return abi.encode(x, y);
    }

    function _secpSig(uint256 pk, bytes32 challenge) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, challenge);
        return abi.encodePacked(r, s, v);
    }

    function _secpKey(uint256 pk) internal pure returns (bytes memory) {
        return abi.encode(vm.addr(pk));
    }
}

contract PasskeyAgilityTest is AgilitySigner {
    uint256 internal constant SK_A = 0xA11CE5EC9;
    uint256 internal constant SK_B = 0xB0B5EC9;
    uint256 internal constant DELAY = 1 days;

    PasskeyPrincipal internal owner;
    Pokee internal pokee;
    Secp256k1Verifier internal secp;
    P256WebAuthnVerifier internal p256v;
    MlDsaVerifier internal mldsa;
    address internal relayer = address(0xFEED);
    address internal recoveryAddr = address(0xD00D);

    function setUp() public {
        (uint256 x, uint256 y) = vm.publicKeyP256(PASSKEY);
        owner = new PasskeyPrincipal(bytes32(x), bytes32(y), recoveryAddr, DELAY);
        pokee = new Pokee();
        secp = new Secp256k1Verifier();
        p256v = new P256WebAuthnVerifier();
        mldsa = new MlDsaVerifier();
    }

    function _poke(uint256 v) internal pure returns (bytes memory) {
        return abi.encodeCall(Pokee.poke, (v));
    }

    function _challenge(address target, bytes memory data) internal view returns (bytes32) {
        return owner.challengeFor(target, 0, data, owner.nonce());
    }

    /// The passkey approves `data` on `target` through `execute`.
    function _byPasskey(address target, bytes memory data) internal {
        owner.execute(target, 0, data, _assert(PASSKEY, _challenge(target, data), UP_UV));
    }

    /// An Ethereum key approves `data` on `target` through `executeSigned`.
    function _bySecp(uint256 pk, address target, bytes memory data) internal {
        owner.executeSigned(target, 0, data, _secpSig(pk, _challenge(target, data)));
    }

    function _proposeSecp(bytes32 alg, uint256 pk) internal view returns (bytes memory) {
        return abi.encodeCall(PasskeyPrincipal.proposeVerifier, (alg, address(secp), _secpKey(pk)));
    }

    function _switchToSecp(uint256 pk) internal {
        _byPasskey(address(owner), _proposeSecp(SECP, pk));
        vm.warp(vm.getBlockTimestamp() + DELAY);
        owner.applyVerifier();
    }

    function _inner(bytes memory err) internal pure returns (bytes memory) {
        return abi.encodeWithSelector(PasskeyPrincipal.CallFailed.selector, err);
    }

    /// Sign first: `expectRevert` arms on the next call, and signing reads the chain.
    function _passkeyReverts(address target, bytes memory data, bytes memory err) internal {
        WebAuthn.WebAuthnAuth memory a = _assert(PASSKEY, _challenge(target, data), UP_UV);
        vm.expectRevert(err);
        owner.execute(target, 0, data, a);
    }

    function _secpReverts(uint256 pk, address target, bytes memory data, bytes memory err) internal {
        bytes memory sig = _secpSig(pk, _challenge(target, data));
        vm.expectRevert(err);
        owner.executeSigned(target, 0, data, sig);
    }

    // ---------------------------------------------------------------- start

    function test_a_new_account_is_on_the_passkey() public {
        assertEq(owner.activeAlg(), owner.P256_WEBAUTHN());
        assertEq(owner.activeAlg(), keccak256("p256-webauthn"));
        assertFalse(owner.retired(owner.P256_WEBAUTHN()));
        _byPasskey(address(pokee), _poke(1));
        assertEq(pokee.last(), 1);
        // The same assertion as bytes through the one entry for every algorithm.
        bytes memory data = _poke(2);
        owner.executeSigned(address(pokee), 0, data, _enc(_assert(PASSKEY, _challenge(address(pokee), data), UP_UV)));
        assertEq(pokee.last(), 2);
        assertEq(owner.nonce(), 2);
    }

    // ------------------------------------------------------ propose / apply

    function test_only_an_approved_call_proposes_a_verifier() public {
        bytes memory key = _secpKey(SK_A);
        address[3] memory who = [relayer, recoveryAddr, address(this)];
        for (uint256 i = 0; i < 3; i++) {
            vm.prank(who[i]);
            vm.expectRevert(PasskeyPrincipal.NotPasskeyOrRecovery.selector);
            owner.proposeVerifier(SECP, address(secp), key);
        }
    }

    function test_a_verifier_waits_the_delay_then_anyone_applies_it() public {
        uint64 eta = uint64(vm.getBlockTimestamp() + DELAY);
        vm.expectEmit(address(owner));
        emit PasskeyPrincipal.VerifierProposed(SECP, address(secp), _secpKey(SK_A), eta);
        _byPasskey(address(owner), _proposeSecp(SECP, SK_A));
        (bytes32 alg, address verifier, uint64 pendingEta, bytes memory key) = owner.pendingAlg();
        assertEq(alg, SECP);
        assertEq(verifier, address(secp));
        assertEq(pendingEta, eta);
        assertEq(key, _secpKey(SK_A));

        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.TooEarly.selector, eta));
        owner.applyVerifier();
        // The passkey still rules until the delay has passed.
        _byPasskey(address(pokee), _poke(7));
        assertEq(owner.activeAlg(), owner.P256_WEBAUTHN());

        vm.warp(eta);
        vm.expectEmit(address(owner));
        emit PasskeyPrincipal.VerifierApplied(SECP, address(secp), owner.P256_WEBAUTHN());
        vm.prank(relayer); // anyone
        owner.applyVerifier();
        assertEq(owner.activeAlg(), SECP);
        assertTrue(owner.retired(owner.P256_WEBAUTHN()));
        assertEq(owner.verifiers(SECP), address(secp));
        assertEq(owner.activeKey(), _secpKey(SK_A));
        (,, pendingEta,) = owner.pendingAlg();
        assertEq(pendingEta, 0, "nothing pending after apply");
    }

    function test_after_the_switch_every_passkey_path_is_refused() public {
        _switchToSecp(SK_A);
        bytes memory data = _poke(9);
        bytes32 ch = _challenge(address(pokee), data);
        WebAuthn.WebAuthnAuth memory a = _assert(PASSKEY, ch, UP_UV);

        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.AlgNotActive.selector, SECP));
        owner.execute(address(pokee), 0, data, a);
        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.AlgNotActive.selector, SECP));
        owner.executeAs(0, address(pokee), 0, data, a);
        uint256[] memory idx = new uint256[](1);
        WebAuthn.WebAuthnAuth[] memory auths = new WebAuthn.WebAuthnAuth[](1);
        auths[0] = a;
        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.AlgNotActive.selector, SECP));
        owner.executeWith(address(pokee), 0, data, idx, auths);
        // The same valid assertion as bytes: the Ethereum-key verifier reads it as nothing.
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.executeSigned(address(pokee), 0, data, _enc(a));
        // Another Ethereum key is refused; the registered one approves, nonce continuing.
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.executeSigned(address(pokee), 0, data, _secpSig(SK_B, ch));
        uint256 n = owner.nonce();
        owner.executeSigned(address(pokee), 0, data, _secpSig(SK_A, ch));
        assertEq(pokee.last(), 9);
        assertEq(owner.nonce(), n + 1);
        // ...and only once.
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.executeSigned(address(pokee), 0, data, _secpSig(SK_A, ch));
    }

    // ----------------------------------------------------------------- cancel

    function test_the_passkey_or_the_recovery_address_cancels() public {
        _byPasskey(address(owner), _proposeSecp(SECP, SK_A));
        vm.prank(relayer);
        vm.expectRevert(PasskeyPrincipal.NotPasskeyOrRecovery.selector);
        owner.cancelVerifier();

        vm.expectEmit(address(owner));
        emit PasskeyPrincipal.VerifierCancelled(SECP, address(owner));
        _byPasskey(address(owner), abi.encodeCall(PasskeyPrincipal.cancelVerifier, ()));
        vm.warp(vm.getBlockTimestamp() + DELAY);
        vm.expectRevert(PasskeyPrincipal.NothingPending.selector);
        owner.applyVerifier();

        _byPasskey(address(owner), _proposeSecp(SECP, SK_A));
        vm.prank(recoveryAddr);
        owner.cancelVerifier();
        vm.prank(recoveryAddr);
        vm.expectRevert(PasskeyPrincipal.NothingPending.selector);
        owner.cancelVerifier();
        assertEq(owner.activeAlg(), owner.P256_WEBAUTHN());
        assertFalse(owner.retired(owner.P256_WEBAUTHN()));
    }

    // ---------------------------------------------------------------- retired

    function test_a_retired_algorithm_never_comes_back() public {
        _switchToSecp(SK_A);
        // Secp -> a second Ethereum-key algorithm, approved by the active key.
        _bySecp(SK_A, address(owner), _proposeSecp(SECP_B, SK_B));
        vm.warp(vm.getBlockTimestamp() + DELAY);
        owner.applyVerifier();
        assertEq(owner.activeAlg(), SECP_B);
        assertTrue(owner.retired(SECP));

        // Back to SECP, even with a fresh key: refused.
        bytes memory back = _proposeSecp(SECP, SK_A);
        _secpReverts(
            SK_B, address(owner), back, _inner(abi.encodeWithSelector(PasskeyPrincipal.AlgRetired.selector, SECP))
        );
        // Back to the passkey: refused.
        bytes memory toPasskey = abi.encodeCall(
            PasskeyPrincipal.proposeVerifier, (owner.P256_WEBAUTHN(), address(p256v), _p256Key(PASSKEY))
        );
        _secpReverts(
            SK_B,
            address(owner),
            toPasskey,
            _inner(abi.encodeWithSelector(PasskeyPrincipal.AlgRetired.selector, owner.P256_WEBAUTHN()))
        );
    }

    function test_rotating_the_key_inside_an_algorithm_retires_nothing() public {
        _switchToSecp(SK_A);
        _bySecp(SK_A, address(owner), _proposeSecp(SECP, SK_B));
        vm.warp(vm.getBlockTimestamp() + DELAY);
        vm.expectEmit(address(owner));
        emit PasskeyPrincipal.VerifierApplied(SECP, address(secp), bytes32(0));
        owner.applyVerifier();
        assertEq(owner.activeAlg(), SECP);
        assertFalse(owner.retired(SECP));
        bytes memory data = _poke(3);
        bytes32 ch = _challenge(address(pokee), data);
        vm.expectRevert(PasskeyPrincipal.BadPasskeySignature.selector);
        owner.executeSigned(address(pokee), 0, data, _secpSig(SK_A, ch));
        owner.executeSigned(address(pokee), 0, data, _secpSig(SK_B, ch));
        assertEq(pokee.last(), 3);
    }

    // ------------------------------------------------------- what registers

    function test_a_verifier_is_code_and_never_the_built_in_id() public {
        bytes memory key = _secpKey(SK_A);
        bytes memory call = abi.encodeCall(PasskeyPrincipal.proposeVerifier, (SECP, relayer, key));
        _passkeyReverts(address(owner), call, _inner(abi.encodeWithSelector(PasskeyPrincipal.BadVerifier.selector)));
        call = abi.encodeCall(PasskeyPrincipal.proposeVerifier, (owner.P256_WEBAUTHN(), address(p256v), key));
        _passkeyReverts(address(owner), call, _inner(abi.encodeWithSelector(PasskeyPrincipal.BadVerifier.selector)));
        assertEq(owner.nonce(), 0, "no refused proposal used an approval");
    }

    /// As briefed, a proposal is not checked against the new key: the
    /// post-quantum slot registers, and once applied nothing can approve.
    /// The delay and the cancel are the only guard (report S24: a one-time
    /// proof of possession at propose is the proposed fix).
    function test_the_post_quantum_slot_registers_but_never_verifies() public {
        _byPasskey(address(owner), abi.encodeCall(PasskeyPrincipal.proposeVerifier, (MLDSA, address(mldsa), hex"01")));
        vm.warp(vm.getBlockTimestamp() + DELAY);
        owner.applyVerifier();
        assertEq(owner.activeAlg(), MLDSA);
        bytes memory data = _poke(5);
        bytes memory sig = _secpSig(SK_A, _challenge(address(pokee), data));
        vm.expectRevert(MlDsaVerifier.NotOnEvm.selector);
        owner.executeSigned(address(pokee), 0, data, sig);
    }

    // ---------------------------------------------- recovery and co-owners

    function test_recovery_and_co_owners_belong_to_the_passkey() public {
        (uint256 x2, uint256 y2) = vm.publicKeyP256(PASSKEY2);
        vm.prank(recoveryAddr);
        owner.proposeRecovery(bytes32(x2), bytes32(y2)); // pending from before the switch
        _switchToSecp(SK_A);

        vm.prank(recoveryAddr);
        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.AlgNotActive.selector, SECP));
        owner.proposeRecovery(bytes32(x2), bytes32(y2));
        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.AlgNotActive.selector, SECP));
        owner.applyRecovery();
        vm.prank(recoveryAddr);
        vm.expectRevert(abi.encodeWithSelector(PasskeyPrincipal.AlgNotActive.selector, SECP));
        owner.approveRecovery(bytes32(x2), bytes32(y2));

        bytes32[] memory xs = new bytes32[](1);
        bytes32[] memory ys = new bytes32[](1);
        xs[0] = bytes32(x2);
        ys[0] = bytes32(y2);
        bytes memory co = abi.encodeCall(PasskeyPrincipal.setCoOwners, (xs, ys, 2));
        _secpReverts(
            SK_A, address(owner), co, _inner(abi.encodeWithSelector(PasskeyPrincipal.AlgNotActive.selector, SECP))
        );
    }

    function test_leaving_the_passkey_returns_the_account_to_one_key() public {
        // 2-of-2 passkeys first.
        (uint256 x2, uint256 y2) = vm.publicKeyP256(PASSKEY2);
        bytes32[] memory xs = new bytes32[](1);
        bytes32[] memory ys = new bytes32[](1);
        xs[0] = bytes32(x2);
        ys[0] = bytes32(y2);
        _byPasskey(address(owner), abi.encodeCall(PasskeyPrincipal.setCoOwners, (xs, ys, 2)));
        assertEq(owner.approvalThreshold(), 2);

        // Both keys approve the move.
        bytes memory call = _proposeSecp(SECP, SK_A);
        bytes32 ch = _challenge(address(owner), call);
        uint256[] memory idx = new uint256[](2);
        idx[1] = 1;
        WebAuthn.WebAuthnAuth[] memory auths = new WebAuthn.WebAuthnAuth[](2);
        auths[0] = _assert(PASSKEY, ch, UP_UV);
        auths[1] = _assert(PASSKEY2, ch, UP_UV);
        owner.executeWith(address(owner), 0, call, idx, auths);
        vm.warp(vm.getBlockTimestamp() + DELAY);
        vm.expectEmit(address(owner));
        emit PasskeyPrincipal.CoOwnersSet(0, 1);
        owner.applyVerifier();
        assertEq(owner.coOwnerCount(), 0);
        assertEq(owner.approvalThreshold(), 0);
        _bySecp(SK_A, address(pokee), _poke(4));
        assertEq(pokee.last(), 4);
    }

    // ------------------------------------------------------------- verifiers

    function test_the_p256_verifier_reads_what_execute_reads() public view {
        bytes32 ch = keccak256("challenge");
        bytes memory key = _p256Key(PASSKEY);
        bytes memory sig = _enc(_assert(PASSKEY, ch, UP_UV));
        assertTrue(p256v.verify(ch, key, sig));
        assertFalse(p256v.verify(keccak256("other"), key, sig), "another challenge");
        assertFalse(p256v.verify(ch, _p256Key(PASSKEY2), sig), "another key");
        assertFalse(p256v.verify(ch, abi.encodePacked(key, uint8(0)), sig), "key not 64 bytes");
        assertFalse(p256v.verify(ch, key, hex"00"), "not an assertion");
        assertFalse(p256v.verify(ch, key, _enc(_assert(PASSKEY, ch, 0x01))), "a tap without biometrics");
    }

    function test_the_secp256k1_verifier() public view {
        bytes32 ch = keccak256("challenge");
        bytes memory key = _secpKey(SK_A);
        bytes memory sig = _secpSig(SK_A, ch);
        assertTrue(secp.verify(ch, key, sig));
        assertFalse(secp.verify(keccak256("other"), key, sig), "another challenge");
        assertFalse(secp.verify(ch, _secpKey(SK_B), sig), "another key");
        assertFalse(secp.verify(ch, abi.encodePacked(vm.addr(SK_A)), sig), "key not abi-encoded");
        assertFalse(secp.verify(ch, abi.encode(uint256(uint160(vm.addr(SK_A))) | (1 << 200)), sig), "dirty key");
        assertFalse(secp.verify(ch, abi.encode(address(0)), hex""), "no signature, zero key");
        // A personal_sign (EIP-191) of the challenge is a different digest: not an approval.
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(SK_A, MessageHashUtils.toEthSignedMessageHash(ch));
        assertFalse(secp.verify(ch, key, abi.encodePacked(r, s, v)), "EIP-191 digest");
        // The high-s twin of a valid signature is refused (one form per signature).
        (v, r, s) = vm.sign(SK_A, ch);
        bytes32 highS = bytes32(SECP_N - uint256(s));
        assertFalse(secp.verify(ch, key, abi.encodePacked(r, highS, v == 27 ? uint8(28) : uint8(27))), "high s");
    }

    function test_the_post_quantum_slot_always_reverts() public {
        vm.expectRevert(MlDsaVerifier.NotOnEvm.selector);
        mldsa.verify(bytes32(0), hex"", hex"");
    }
}

// --------------------------------------------------------------------------
// Invariants: random sequences of propose / cancel / apply / old-key attempts.

contract AgilityHandler is AgilitySigner {
    PasskeyPrincipal public owner;
    Pokee public pokee;
    address public recovery;
    Secp256k1Verifier internal secp;
    P256WebAuthnVerifier internal p256v;
    MlDsaVerifier internal mldsa;

    // 0 = built-in passkey, 1 = Ethereum key, 2 = passkey via P256WebAuthnVerifier, 3 = ML-DSA slot (no key works)
    uint8 public activeKind;
    uint256 public activePk = PASSKEY;
    bool public hasPending;
    uint8 internal pendingKind;
    uint256 internal pendingPk;

    uint256[4] internal secpKeys = [uint256(0x5EC1), 0x5EC2, 0x5EC3, 0x5EC4];

    // ghosts
    bool public oldAlgAccepted; // a key outside the active algorithm approved a call
    bool public retiredAccepted; // a retired or built-in id was accepted as a proposal
    bool public cancelFailed; // a pending change could not be cancelled
    bool public appliedEarly; // a change applied with nothing ripe pending
    uint256 public switches;
    uint256 public proposals;
    uint256 public cancels;
    uint256 public oldAttempts;
    uint256 public pokes;

    constructor(PasskeyPrincipal o, Pokee p, address r, Secp256k1Verifier s, P256WebAuthnVerifier pv, MlDsaVerifier m) {
        owner = o;
        pokee = p;
        recovery = r;
        secp = s;
        p256v = pv;
        mldsa = m;
    }

    function algs() public view returns (bytes32[5] memory) {
        return [owner.P256_WEBAUTHN(), SECP, SECP_B, MLDSA, P256_EXT];
    }

    /// Sign `data` on `target` with whatever key is active now.
    function _approveActive(address target, bytes memory data) internal returns (bool ok) {
        bytes32 ch = owner.challengeFor(target, 0, data, owner.nonce());
        if (activeKind == 0) {
            try owner.execute(target, 0, data, _assert(activePk, ch, UP_UV)) {
                ok = true;
            } catch {}
        } else if (activeKind == 1) {
            try owner.executeSigned(target, 0, data, _secpSig(activePk, ch)) {
                ok = true;
            } catch {}
        } else if (activeKind == 2) {
            try owner.executeSigned(target, 0, data, _enc(_assert(activePk, ch, UP_UV))) {
                ok = true;
            } catch {}
        }
    }

    function propose(uint256 algSeed, uint256 keySeed) external {
        // The ML-DSA slot leaves no working key once applied, so it is drawn rarely.
        uint256 r = algSeed % 16;
        bytes32 alg = r == 0 ? MLDSA : [owner.P256_WEBAUTHN(), SECP, SECP_B, P256_EXT][r % 4];
        uint8 kind;
        uint256 pk;
        address verifier;
        bytes memory key;
        if (alg == SECP || alg == SECP_B) {
            kind = 1;
            pk = secpKeys[keySeed % 4];
            verifier = address(secp);
            key = _secpKey(pk);
        } else if (alg == MLDSA) {
            kind = 3;
            verifier = address(mldsa);
            key = hex"01";
        } else {
            // the built-in id (always refused), or a passkey through the external verifier
            kind = 2;
            pk = keySeed % 2 == 0 ? PASSKEY : PASSKEY2;
            verifier = address(p256v);
            key = _p256Key(pk);
        }
        bool retiredBefore = owner.retired(alg);
        if (_approveActive(address(owner), abi.encodeCall(PasskeyPrincipal.proposeVerifier, (alg, verifier, key)))) {
            if (alg == owner.P256_WEBAUTHN() || retiredBefore) retiredAccepted = true;
            hasPending = true;
            pendingKind = kind;
            pendingPk = pk;
            proposals++;
        }
    }

    function cancel(bool byRecovery) external {
        if (!hasPending) return;
        bool ok;
        if (byRecovery || activeKind == 3) {
            vm.prank(recovery);
            try owner.cancelVerifier() {
                ok = true;
            } catch {}
        } else {
            ok = _approveActive(address(owner), abi.encodeCall(PasskeyPrincipal.cancelVerifier, ()));
        }
        if (!ok) cancelFailed = true;
        hasPending = false;
        cancels++;
    }

    function warpAndApply(uint256 dt) external {
        vm.warp(vm.getBlockTimestamp() + bound(dt, 0, 2 * owner.recoveryDelay()));
        (,, uint64 eta,) = owner.pendingAlg();
        bytes32 before = owner.activeAlg();
        try owner.applyVerifier() {
            if (!hasPending || block.timestamp < eta) appliedEarly = true;
            hasPending = false;
            activeKind = pendingKind;
            activePk = pendingPk;
            if (owner.activeAlg() != before) switches++;
        } catch {}
    }

    function pokeWithActive(uint256 v) external {
        if (_approveActive(address(pokee), abi.encodeCall(Pokee.poke, (v)))) pokes++;
    }

    /// Every key that is not the active one, on every path, is refused.
    function tryOldKey(uint256 which, uint256 v) external {
        bytes memory data = abi.encodeCall(Pokee.poke, (v));
        bytes32 ch = owner.challengeFor(address(pokee), 0, data, owner.nonce());
        uint256 w = which % 6;
        bool ok;
        if (w < 2) {
            uint256 pk = w == 0 ? PASSKEY : PASSKEY2;
            if ((activeKind == 0 || activeKind == 2) && activePk == pk) return; // that is the active key
            WebAuthn.WebAuthnAuth memory a = _assert(pk, ch, UP_UV);
            try owner.execute(address(pokee), 0, data, a) {
                ok = true;
            } catch {}
            try owner.executeSigned(address(pokee), 0, data, _enc(a)) {
                ok = true;
            } catch {}
        } else {
            uint256 pk = secpKeys[w - 2];
            if (activeKind == 1 && activePk == pk) return;
            try owner.executeSigned(address(pokee), 0, data, _secpSig(pk, ch)) {
                ok = true;
            } catch {}
        }
        oldAttempts++;
        if (ok) oldAlgAccepted = true;
    }
}

/// forge-config: default.invariant.runs = 256
/// forge-config: default.invariant.depth = 200
/// forge-config: default.invariant.fail-on-revert = false
contract PasskeyAgilityInvariantTest is Test {
    AgilityHandler internal h;
    PasskeyPrincipal internal owner;

    function setUp() public {
        vm.warp(1_764_000_000);
        (uint256 x, uint256 y) = vm.publicKeyP256(0x1d4bd2f8b1c2a3e4f5061728394a5b6c7d8e9fa0b1c2d3e4f50617283940abcd);
        address recovery = address(0xD00D);
        owner = new PasskeyPrincipal(bytes32(x), bytes32(y), recovery, 1 hours);
        h = new AgilityHandler(
            owner, new Pokee(), recovery, new Secp256k1Verifier(), new P256WebAuthnVerifier(), new MlDsaVerifier()
        );
        targetContract(address(h));
    }

    /// Exactly one algorithm is active, and every switch retired exactly one.
    function invariant_oneActiveAlg() public view {
        bytes32[5] memory a = h.algs();
        uint256 active;
        uint256 retiredCount;
        for (uint256 i = 0; i < 5; i++) {
            if (owner.activeAlg() == a[i]) active++;
            if (owner.retired(a[i])) retiredCount++;
        }
        assertEq(active, 1, "exactly one known algorithm is active");
        assertEq(retiredCount, h.switches(), "each switch retired exactly one algorithm");
        assertTrue(
            owner.activeAlg() == a[0] || owner.verifiers(owner.activeAlg()) != address(0), "active has a verifier"
        );
    }

    function invariant_retiredNeverActive() public view {
        bytes32[5] memory a = h.algs();
        for (uint256 i = 0; i < 5; i++) {
            if (owner.retired(a[i])) assertTrue(owner.activeAlg() != a[i], "a retired algorithm is active");
        }
        assertFalse(h.retiredAccepted(), "a retired or built-in id was accepted as a proposal");
    }

    function invariant_oldAlgSignatureReverts() public view {
        assertFalse(h.oldAlgAccepted(), "a key outside the active algorithm approved a call");
    }

    /// A pending change is cancellable until it applies, and applies only after its eta.
    function invariant_pendingCancellable() public view {
        assertFalse(h.cancelFailed(), "a pending change could not be cancelled");
        assertFalse(h.appliedEarly(), "a change applied before its eta or with nothing pending");
    }

    function afterInvariant() external view {
        console.log("switches", h.switches(), "proposals", h.proposals());
        console.log("cancels", h.cancels(), "oldAttempts", h.oldAttempts());
        console.log("pokes", h.pokes());
    }
}
