// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {WebAuthn} from "@openzeppelin/contracts/utils/cryptography/WebAuthn.sol";
import {IPrincipalVerifier} from "./IPrincipalVerifier.sol";

/// @title  P256WebAuthnVerifier
/// @notice A passkey (P-256 WebAuthn assertion, user verification required),
///         the same check `PasskeyPrincipal.execute` runs inline.
///         `key` = abi.encode(qx, qy); `sig` = abi.encode(r, s, challengeIndex,
///         typeIndex, authenticatorData, clientDataJSON), the `WebAuthnAuth`
///         fields in order (what OpenZeppelin's `tryDecodeAuth` reads).
contract P256WebAuthnVerifier is IPrincipalVerifier {
    function verify(bytes32 challenge, bytes calldata key, bytes calldata sig) external view returns (bool) {
        if (key.length != 64) return false;
        (bool decoded, WebAuthn.WebAuthnAuth calldata auth) = WebAuthn.tryDecodeAuth(sig);
        return
            decoded && WebAuthn.verify(abi.encodePacked(challenge), auth, bytes32(key[0:32]), bytes32(key[32:64]), true);
    }
}
