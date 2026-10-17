// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Script, console2} from "forge-std/Script.sol";
import {Base64} from "@openzeppelin/contracts/utils/Base64.sol";
import {WebAuthn} from "@openzeppelin/contracts/utils/cryptography/WebAuthn.sol";
import {PasskeyPrincipal} from "../src/PasskeyPrincipal.sol";

/// Signatures and calldata for the live signature-agility proof
/// (scripts/prove_passkey_agility.sh). Nothing here broadcasts: each function
/// reads the chain, signs, and logs one hex value that the shell script sends
/// with cast. The test keys come from the environment (AGILITY_P256_KEY,
/// AGILITY_SECP_KEY), set inside the command; the relayer key never reaches
/// this script.
contract PasskeyAgility is Script {
    bytes32 internal constant SECP256K1 = keccak256("secp256k1");
    uint256 internal constant N = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551;

    /// abi.encode(qx, qy) of the test passkey.
    function passkey() external view {
        (uint256 x, uint256 y) = vm.publicKeyP256(vm.envUint("AGILITY_P256_KEY"));
        console2.logBytes(abi.encode(x, y));
    }

    /// The passkey approves `proposeVerifier(secp256k1, verifier, abi.encode(signer))`.
    function proposeCalldata(address payable owner, address verifier, address signer) external view {
        bytes memory call = abi.encodeCall(PasskeyPrincipal.proposeVerifier, (SECP256K1, verifier, abi.encode(signer)));
        console2.logBytes(_byPasskey(PasskeyPrincipal(owner), owner, call));
    }

    /// A valid passkey assertion over the next approval of a no-op call to `target`.
    function passkeyExecuteCalldata(address payable owner, address target) external view {
        console2.logBytes(_byPasskey(PasskeyPrincipal(owner), target, ""));
    }

    /// The secp256k1 test key approves the same no-op call through `executeSigned`.
    function secpExecuteCalldata(address payable owner, address target) external view {
        PasskeyPrincipal p = PasskeyPrincipal(owner);
        bytes32 ch = p.challengeFor(target, 0, "", p.nonce());
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(vm.envUint("AGILITY_SECP_KEY"), ch);
        console2.logBytes(abi.encodeCall(PasskeyPrincipal.executeSigned, (target, 0, "", abi.encodePacked(r, s, v))));
    }

    /// `execute(target, 0, data, auth)` with a WebAuthn assertion built the way
    /// a platform authenticator returns it (UP and UV set), over the account's
    /// next challenge.
    function _byPasskey(PasskeyPrincipal p, address target, bytes memory data) internal view returns (bytes memory) {
        bytes32 ch = p.challengeFor(target, 0, data, p.nonce());
        WebAuthn.WebAuthnAuth memory a;
        a.authenticatorData = abi.encodePacked(sha256("localhost"), bytes1(0x05), bytes4(0x00000001));
        a.clientDataJSON = string.concat(
            '{"type":"webauthn.get","challenge":"',
            Base64.encodeURL(abi.encodePacked(ch)),
            '","origin":"http://localhost:8787","crossOrigin":false}'
        );
        a.typeIndex = 1;
        a.challengeIndex = 23;
        bytes32 digest = sha256(abi.encodePacked(a.authenticatorData, sha256(bytes(a.clientDataJSON))));
        (bytes32 r, bytes32 s) = vm.signP256(vm.envUint("AGILITY_P256_KEY"), digest);
        if (uint256(s) > N / 2) s = bytes32(N - uint256(s));
        a.r = r;
        a.s = s;
        return abi.encodeCall(PasskeyPrincipal.execute, (target, 0, data, a));
    }
}
