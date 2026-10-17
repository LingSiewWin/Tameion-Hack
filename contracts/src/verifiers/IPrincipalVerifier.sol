// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @title  IPrincipalVerifier
/// @notice One signature algorithm, as `PasskeyPrincipal` asks it (docs/117 §3).
///         The account keeps which algorithm is active and the key; a verifier
///         keeps only the math, so moving to a new algorithm is registering a
///         new verifier, not redeploying the account.
interface IPrincipalVerifier {
    /// @notice True when `sig` is a signature by `key` over `challenge`.
    ///         `key` and `sig` are in the verifier's own encoding.
    function verify(bytes32 challenge, bytes calldata key, bytes calldata sig) external view returns (bool);
}
