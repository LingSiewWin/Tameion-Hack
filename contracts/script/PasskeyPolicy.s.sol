// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Script, console2} from "forge-std/Script.sol";
import {WebAuthn} from "@openzeppelin/contracts/utils/cryptography/WebAuthn.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";
import {BudgetPolicyToken} from "../src/BudgetPolicyToken.sol";
import {PasskeyPrincipal} from "../src/PasskeyPrincipal.sol";

/// Deploy a budget whose principal is a passkey, and submit calls the passkey
/// approved. The submitter pays gas and holds no authority.
///
///   forge script script/PasskeyPolicy.s.sol --sig "deploy(bytes32,bytes32,address,uint256,address,address,uint256,uint256,uint256,uint256,uint256,uint8)" \
///     <qx> <qy> <recovery> <recoveryDelay> <agent> <operator> <agentId> <perAction> <daily> <floor> <delay> <mode> --rpc-url <rpc> --private-key <relayer> --broadcast
///   forge script script/PasskeyPolicy.s.sol --sig "deployToken(address,bytes32,bytes32,address,uint256,address,address,uint256,uint256,uint256,uint256,uint256,uint8)" \
///     <usdc> <qx> <qy> <recovery> <recoveryDelay> <agent> <operator> <agentId> <perAction> <daily> <floor> <delay> <mode> --rpc-url <rpc> --private-key <relayer> --broadcast
///   forge script script/PasskeyPolicy.s.sol --sig "submit(address,address,bytes,string)" \
///     <owner> <target> <calldata> <auth.json> --rpc-url <rpc> --private-key <relayer> --broadcast
///
/// `agentId` is the ERC-8004 id scripts/register_agent.sh recorded for this
/// chain. The registry is at the same address on every chain it is deployed to.
contract PasskeyPolicy is Script {
    address internal constant REGISTRY = 0x8004A818BFB912233c491871b3d84c89A494BD9e;

    /// Native USDC (Arc).
    function deploy(
        bytes32 qx,
        bytes32 qy,
        address recovery,
        uint256 recoveryDelay,
        address agent,
        address operator,
        uint256 agentId,
        uint256 perAction,
        uint256 daily,
        uint256 floor,
        uint256 delay,
        uint8 mode
    ) external {
        vm.startBroadcast();
        PasskeyPrincipal owner = new PasskeyPrincipal(qx, qy, recovery, recoveryDelay);
        BudgetPolicy policy = new BudgetPolicy(
            address(owner),
            agent,
            operator,
            REGISTRY,
            agentId,
            perAction,
            daily,
            floor,
            delay,
            BudgetPolicyBase.Mode(mode)
        );
        vm.stopBroadcast();
        console2.log("owner", address(owner));
        console2.log("policy", address(policy));
    }

    /// ERC-20 USDC (Arbitrum, Base, Ethereum).
    function deployToken(
        address usdc,
        bytes32 qx,
        bytes32 qy,
        address recovery,
        uint256 recoveryDelay,
        address agent,
        address operator,
        uint256 agentId,
        uint256 perAction,
        uint256 daily,
        uint256 floor,
        uint256 delay,
        uint8 mode
    ) external {
        vm.startBroadcast();
        PasskeyPrincipal owner = new PasskeyPrincipal(qx, qy, recovery, recoveryDelay);
        BudgetPolicyToken policy = new BudgetPolicyToken(
            usdc,
            address(owner),
            agent,
            operator,
            REGISTRY,
            agentId,
            perAction,
            daily,
            floor,
            delay,
            BudgetPolicyBase.Mode(mode)
        );
        vm.stopBroadcast();
        console2.log("owner", address(owner));
        console2.log("policy", address(policy));
    }

    function submit(address owner, address target, bytes calldata data, string calldata authPath) external {
        // Reads the approval the phone produced; fs access is limited to
        // runtime/ by foundry.toml.
        // forge-lint: disable-next-line(unsafe-cheatcode)
        string memory j = vm.readFile(authPath);
        WebAuthn.WebAuthnAuth memory a;
        a.r = vm.parseJsonBytes32(j, ".r");
        a.s = vm.parseJsonBytes32(j, ".s");
        a.challengeIndex = vm.parseJsonUint(j, ".challengeIndex");
        a.typeIndex = vm.parseJsonUint(j, ".typeIndex");
        a.authenticatorData = vm.parseJsonBytes(j, ".authenticatorData");
        a.clientDataJSON = vm.parseJsonString(j, ".clientDataJSON");
        vm.startBroadcast();
        PasskeyPrincipal(payable(owner)).execute(target, 0, data, a);
        vm.stopBroadcast();
        console2.log("executed with approval", PasskeyPrincipal(payable(owner)).nonce() - 1);
    }
}
