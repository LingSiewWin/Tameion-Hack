// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";
import {BudgetPolicyToken} from "../src/BudgetPolicyToken.sol";
import {TestUSDC} from "../src/TestUSDC.sol";

/// PLAN 5.54: an x402 seller's facilitator redeems a purchase the agent
/// signed (ERC-7710 `redeemDelegations`), and the policy applies what `pay`
/// applies. Every refusal here is one an attacker or a buggy agent would
/// try: a replay, a forged or altered purchase, a seller nobody approved,
/// more than the cap, a lapsed or paused agent, a call that is not a USDC
/// transfer.
contract BudgetPolicyX402Test is Test {
    BudgetPolicy internal policy;

    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    uint256 internal agentKey = 0xA6E47;
    address internal agent;
    address internal seller = address(0x5E11E2);
    address internal facilitator = address(0xFAC1);
    address internal constant ARC_USDC = 0x3600000000000000000000000000000000000000;

    uint256 internal constant USDC = 1_000000;

    function setUp() public {
        vm.warp(1_764_000_000);
        agent = vm.addr(agentKey);
        policy = new BudgetPolicy(
            principal,
            agent,
            operator,
            address(0),
            0,
            1 * USDC, // per purchase
            3 * USDC, // per 24 h
            10 * USDC, // floor
            1 hours,
            BudgetPolicyBase.Mode.Autopilot
        );
        vm.deal(address(policy), 20 * USDC * policy.WEI_PER_USDC_UNIT());
        vm.prank(principal);
        policy.setPayee(seller, true);
        vm.warp(vm.getBlockTimestamp() + policy.delay());
    }

    function _exec(address token, address to, uint256 amount) internal pure returns (bytes memory) {
        return abi.encodePacked(token, uint256(0), abi.encodeWithSelector(0xa9059cbb, to, amount));
    }

    function _ctx(uint256 key, address to, uint256 amount, bytes32 nonce, uint64 deadline)
        internal
        view
        returns (bytes memory)
    {
        bytes32 digest = policy.purchaseDigest(to, amount, nonce, deadline);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(key, digest);
        return abi.encode(nonce, deadline, abi.encodePacked(r, s, v));
    }

    function _redeem(bytes memory ctx, bytes32 mode, bytes memory exec) internal {
        bytes[] memory c = new bytes[](1);
        c[0] = ctx;
        bytes32[] memory m = new bytes32[](1);
        m[0] = mode;
        bytes[] memory e = new bytes[](1);
        e[0] = exec;
        vm.prank(facilitator);
        policy.redeemDelegations(c, m, e);
    }

    function _buy(uint256 amount, bytes32 nonce) internal {
        uint64 deadline = uint64(block.timestamp + 60);
        _redeem(_ctx(agentKey, seller, amount, nonce, deadline), bytes32(0), _exec(ARC_USDC, seller, amount));
    }

    /// The digest is read before the revert is armed (docs/60 P-19: an
    /// argument's own call would otherwise consume `expectRevert`).
    function _buyRefused(uint256 amount, bytes32 nonce, bytes memory err) internal {
        uint64 deadline = uint64(block.timestamp + 60);
        bytes memory ctx = _ctx(agentKey, seller, amount, nonce, deadline);
        bytes memory exec = _exec(ARC_USDC, seller, amount);
        vm.expectRevert(err);
        _redeem(ctx, bytes32(0), exec);
    }

    function test_purchase_pays_the_seller_inside_the_caps() public {
        uint64 deadline = uint64(block.timestamp + 60);
        bytes32 digest = policy.purchaseDigest(seller, 1000, bytes32(uint256(1)), deadline);
        vm.expectEmit(true, true, false, true, address(policy));
        emit BudgetPolicyBase.Paid(seller, 1000, digest);
        _redeem(_ctx(agentKey, seller, 1000, bytes32(uint256(1)), deadline), bytes32(0), _exec(ARC_USDC, seller, 1000));
        assertEq(seller.balance, 1000 * policy.WEI_PER_USDC_UNIT());
        assertTrue(policy.executed(digest));
        assertEq(policy.spentInWindow(), 1000);
    }

    function test_a_purchase_is_redeemed_once() public {
        uint64 deadline = uint64(block.timestamp + 60);
        bytes memory ctx = _ctx(agentKey, seller, 1000, bytes32(uint256(7)), deadline);
        _redeem(ctx, bytes32(0), _exec(ARC_USDC, seller, 1000));
        bytes32 digest = policy.purchaseDigest(seller, 1000, bytes32(uint256(7)), deadline);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AlreadyExecuted.selector, digest));
        _redeem(ctx, bytes32(0), _exec(ARC_USDC, seller, 1000));
    }

    function test_someone_else_signing_is_refused() public {
        uint64 deadline = uint64(block.timestamp + 60);
        bytes memory ctx = _ctx(0xBAD, seller, 1000, bytes32(uint256(1)), deadline);
        vm.expectRevert(BudgetPolicyBase.BadAgentSignature.selector);
        _redeem(ctx, bytes32(0), _exec(ARC_USDC, seller, 1000));
    }

    function test_a_raised_amount_or_swapped_seller_breaks_the_signature() public {
        uint64 deadline = uint64(block.timestamp + 60);
        bytes memory ctx = _ctx(agentKey, seller, 1000, bytes32(uint256(1)), deadline);
        vm.expectRevert(BudgetPolicyBase.BadAgentSignature.selector);
        _redeem(ctx, bytes32(0), _exec(ARC_USDC, seller, 2000));
        vm.prank(principal);
        policy.setPayee(address(0xD00D), true);
        vm.expectRevert(BudgetPolicyBase.BadAgentSignature.selector);
        _redeem(ctx, bytes32(0), _exec(ARC_USDC, address(0xD00D), 1000));
    }

    function test_a_seller_nobody_approved_is_refused() public {
        address stranger = address(0x5757);
        uint64 deadline = uint64(block.timestamp + 60);
        bytes memory ctx = _ctx(agentKey, stranger, 1000, bytes32(uint256(1)), deadline);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.PayeeNotAttested.selector, stranger));
        _redeem(ctx, bytes32(0), _exec(ARC_USDC, stranger, 1000));
    }

    function test_more_than_the_cap_is_refused_and_purchases_share_the_window() public {
        _buyRefused(
            2 * USDC,
            bytes32(uint256(1)),
            abi.encodeWithSelector(BudgetPolicyBase.OverPerActionCap.selector, 2 * USDC, 1 * USDC)
        );
        _buy(1 * USDC, bytes32(uint256(2)));
        _buy(1 * USDC, bytes32(uint256(3)));
        // The agent's own `pay` and its purchases count against one window.
        vm.prank(agent);
        policy.pay(seller, 1 * USDC, keccak256("bill"));
        _buyRefused(
            1 * USDC,
            bytes32(uint256(4)),
            abi.encodeWithSelector(BudgetPolicyBase.OverDailyCap.selector, 4 * USDC, 3 * USDC)
        );
    }

    function test_the_floor_holds() public {
        vm.deal(address(policy), (10 * USDC + 500) * policy.WEI_PER_USDC_UNIT());
        _buyRefused(
            1000,
            bytes32(uint256(1)),
            abi.encodeWithSelector(BudgetPolicyBase.BelowReserveFloor.selector, 10 * USDC - 500, 10 * USDC)
        );
    }

    function test_paused_or_lapsed_agent_cannot_buy() public {
        vm.prank(principal);
        policy.setMode(BudgetPolicyBase.Mode.Propose);
        _buyRefused(1000, bytes32(uint256(1)), abi.encodeWithSelector(BudgetPolicyBase.NotAutopilot.selector, uint8(1)));

        BudgetPolicy p2 = new BudgetPolicy(
            principal, agent, operator, address(0), 0, USDC, 3 * USDC, 0, 0, BudgetPolicyBase.Mode.Autopilot
        );
        vm.deal(address(p2), 5 * USDC * p2.WEI_PER_USDC_UNIT());
        vm.prank(principal);
        p2.setPayee(seller, true);
        vm.warp(p2.agentExpiry());
        policy = p2;
        _buyRefused(
            1000, bytes32(uint256(1)), abi.encodeWithSelector(BudgetPolicyBase.AgentLapsed.selector, p2.agentExpiry())
        );
    }

    function test_an_expired_purchase_is_refused() public {
        uint64 deadline = uint64(block.timestamp + 60);
        bytes memory ctx = _ctx(agentKey, seller, 1000, bytes32(uint256(1)), deadline);
        vm.warp(deadline + 1);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.PurchaseExpired.selector, deadline));
        _redeem(ctx, bytes32(0), _exec(ARC_USDC, seller, 1000));
    }

    function test_only_a_plain_usdc_transfer_is_redeemable() public {
        uint64 deadline = uint64(block.timestamp + 60);
        bytes memory ctx = _ctx(agentKey, seller, 1000, bytes32(uint256(1)), deadline);
        // a batch / delegatecall mode
        vm.expectRevert(BudgetPolicyBase.UnsupportedExecution.selector);
        _redeem(ctx, bytes32(uint256(1) << 248), _exec(ARC_USDC, seller, 1000));
        // another token
        vm.expectRevert(BudgetPolicyBase.UnsupportedExecution.selector);
        _redeem(ctx, bytes32(0), _exec(address(0x7070), seller, 1000));
        // value attached
        vm.expectRevert(BudgetPolicyBase.UnsupportedExecution.selector);
        _redeem(
            ctx,
            bytes32(0),
            abi.encodePacked(ARC_USDC, uint256(1), abi.encodeWithSelector(0xa9059cbb, seller, uint256(1000)))
        );
        // approve instead of transfer
        vm.expectRevert(BudgetPolicyBase.UnsupportedExecution.selector);
        _redeem(
            ctx,
            bytes32(0),
            abi.encodePacked(ARC_USDC, uint256(0), abi.encodeWithSelector(0x095ea7b3, seller, uint256(1000)))
        );
    }

    function test_keyless_account_can_never_buy() public {
        address dead = 0x000000000000000000000000000000000000dEaD;
        BudgetPolicy keyless = new BudgetPolicy(
            principal, dead, operator, address(0), 0, USDC, 3 * USDC, 0, 0, BudgetPolicyBase.Mode.Autopilot
        );
        vm.deal(address(keyless), 5 * USDC * keyless.WEI_PER_USDC_UNIT());
        vm.prank(principal);
        keyless.setPayee(seller, true);
        policy = keyless;
        _buyRefused(1000, bytes32(uint256(1)), abi.encodeWithSelector(BudgetPolicyBase.BadAgentSignature.selector));
    }

    function test_erc20_policy_names_its_own_token() public {
        TestUSDC usdc = new TestUSDC();
        BudgetPolicyToken tp = new BudgetPolicyToken(
            address(usdc),
            principal,
            agent,
            operator,
            address(0),
            0,
            USDC,
            3 * USDC,
            0,
            0,
            BudgetPolicyBase.Mode.Autopilot
        );
        usdc.mint(address(tp), 5 * USDC);
        vm.prank(principal);
        tp.setPayee(seller, true);
        assertEq(tp.usdcToken(), address(usdc));
        policy = BudgetPolicy(payable(address(tp)));
        uint64 deadline = uint64(block.timestamp + 60);
        _redeem(
            _ctx(agentKey, seller, 1000, bytes32(uint256(1)), deadline), bytes32(0), _exec(address(usdc), seller, 1000)
        );
        assertEq(usdc.balanceOf(seller), 1000);
    }

    /// The service signs the same digest the contract checks: the vector the
    /// Rust test pins (`operator/src/x402.rs`), computed with `cast`.
    function test_digest_matches_the_service_vector() public {
        vm.chainId(5_042_002);
        address at = 0x1111111111111111111111111111111111111111;
        deployCodeTo(
            "BudgetPolicy.sol:BudgetPolicy",
            abi.encode(
                principal, agent, operator, address(0), 0, USDC, 3 * USDC, 0, 0, BudgetPolicyBase.Mode.Autopilot
            ),
            at
        );
        bytes32 d = BudgetPolicy(payable(at))
            .purchaseDigest(
                0x2222222222222222222222222222222222222222,
                1000,
                bytes32(uint256(0x3333333333333333333333333333333333333333333333333333333333333333)),
                1_800_000_000
            );
        assertEq(d, 0xeaefdc747acaa58aa77dfb453bd5b1a11a7fd2068c7d629234fae5dcb86f65b0);
    }
}
