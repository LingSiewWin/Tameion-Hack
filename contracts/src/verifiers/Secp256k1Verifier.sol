// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {IPrincipalVerifier} from "./IPrincipalVerifier.sol";

/// @title  Secp256k1Verifier
/// @notice An Ethereum key: `key` = abi.encode(signer address), `sig` = the
///         65-byte (r, s, v) signature of the 32-byte challenge itself
///         (ecrecover over the challenge). Low-s only (OpenZeppelin `ECDSA`),
///         so a signature has one form.
contract Secp256k1Verifier is IPrincipalVerifier {
    function verify(bytes32 challenge, bytes calldata key, bytes calldata sig) external pure returns (bool) {
        if (key.length != 32) return false;
        uint256 word = abi.decode(key, (uint256));
        if (word >> 160 != 0) return false; // not an abi-encoded address
        // casting to 'uint160' is safe because the line above checked the top 96 bits are zero.
        // forge-lint: disable-next-line(unsafe-typecast)
        address expected = address(uint160(word));
        (address signer, ECDSA.RecoverError err,) = ECDSA.tryRecoverCalldata(challenge, sig);
        return err == ECDSA.RecoverError.NoError && signer == expected;
    }
}
