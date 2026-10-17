// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {IPrincipalVerifier} from "./IPrincipalVerifier.sol";

/// @title  MlDsaVerifier
/// @notice The slot for a post-quantum key (ML-DSA, FIPS 204). The interface is
///         here so the account's registry has a place for it; the math is not
///         (this repository carries no ML-DSA verifier), so `verify` always
///         reverts. An account that applied it could approve nothing more.
contract MlDsaVerifier is IPrincipalVerifier {
    error NotOnEvm();

    function verify(bytes32, bytes calldata, bytes calldata) external pure returns (bool) {
        revert NotOnEvm();
    }
}
