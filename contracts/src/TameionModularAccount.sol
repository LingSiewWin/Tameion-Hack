// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Account} from "@openzeppelin/contracts/account/Account.sol";
import {AccountERC7579} from "@openzeppelin/contracts/account/extensions/draft-AccountERC7579.sol";
import {SignerEIP7702} from "@openzeppelin/contracts/utils/cryptography/signers/SignerEIP7702.sol";
import {AbstractSigner} from "@openzeppelin/contracts/utils/cryptography/signers/AbstractSigner.sol";
import {PackedUserOperation} from "@openzeppelin/contracts/interfaces/IERC4337.sol";

/// @title  TameionModularAccount
/// @notice A minimal ERC-7579 modular account (OpenZeppelin's `AccountERC7579`
///         signed by the wallet's own key through EIP-7702), used to prove the
///         `TameionExecutor` module on a real chain without a bundler: the
///         wallet delegates to this code and installs the module with a
///         self-call. Any ERC-7579 account (Safe7579, Kernel, Nexus) installs
///         the module the same way; this one exists so the proof needs nothing
///         but a wallet and a chain.
contract TameionModularAccount is Account, AccountERC7579, SignerEIP7702 {
    function _validateUserOp(PackedUserOperation calldata userOp, bytes32 userOpHash, bytes calldata signature)
        internal
        virtual
        override(Account, AccountERC7579)
        returns (uint256)
    {
        return super._validateUserOp(userOp, userOpHash, signature);
    }

    /// @dev The wallet's own key (EIP-7702) signs; modules may add validators.
    function _rawSignatureValidation(bytes32 hash, bytes calldata signature)
        internal
        view
        virtual
        override(AbstractSigner, AccountERC7579, SignerEIP7702)
        returns (bool)
    {
        return SignerEIP7702._rawSignatureValidation(hash, signature);
    }
}
