// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";

/// @dev One `executed` map serves five producers of action hashes. This file
///      shows (1) that the honest producers' preimages can never be equal --
///      so their hashes can meet only through a keccak collision -- and (2)
///      what the one producer that is not a format, the agent key, can do with
///      a hash it picks (docs/68 F1). The PoC tests pin today's behaviour so a
///      contract change that closes the gap shows up as a test to update.
contract ActionHashNamespacesTest is Test {
    uint256 internal constant USDC = 1_000000;
    BudgetPolicy internal policy;
    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    address internal agent = address(0xBEEF);
    address internal landlord = address(0x1A2D);
    address internal hosting = address(0x4057);

    function setUp() public {
        vm.warp(1_764_000_000);
        policy = new BudgetPolicy(
            principal,
            agent,
            operator,
            address(0x8004),
            42,
            100 * USDC,
            300 * USDC,
            0,
            1 hours,
            BudgetPolicyBase.Mode.Autopilot
        );
        vm.deal(address(policy), 10_000 * USDC * 1e12);
        vm.startPrank(principal);
        policy.setPayee(landlord, true);
        policy.setPayee(hosting, true);
        vm.stopPrank();
        vm.warp(block.timestamp + 1 hours);
    }

    // --- the five preimages, as each producer builds them -------------------

    /// The agent's on-chain key: `ledger::canon::settlement_key_for` --
    /// u64be-length-prefixed "air/v0.1/settlement", key, scope.
    function _agentKey(bytes memory key, bytes memory scope) internal pure returns (bytes memory) {
        bytes memory tag = "air/v0.1/settlement";
        return abi.encodePacked(uint64(tag.length), tag, uint64(key.length), key, uint64(scope.length), scope);
    }

    /// `BudgetPolicy.scheduleActionHash`.
    function _schedule(address pol, uint256 id, uint64 dueAt) internal pure returns (bytes memory) {
        return abi.encode("tameion.schedule.v1", pol, id, dueAt);
    }

    /// `operator::invoice::id`.
    function _invoice(address payee, bytes32 ref) internal pure returns (bytes memory) {
        return abi.encode("tameion.invoice", payee, ref);
    }

    /// `purchaseDigest` is EIP-712: 0x1901 ‖ domain ‖ struct hash.
    function _purchase(bytes32 domain, bytes32 body) internal pure returns (bytes memory) {
        return abi.encodePacked(hex"1901", domain, body);
    }

    /// Pairwise, the preimages differ at a fixed position whatever the inputs:
    ///   agent key   byte 0 = 0x00, byte 7 = 19 (tag length), byte 8 = 'a'
    ///   schedule    abi word 0 = 0x80 (offset of the string after 4 head words)
    ///   invoice     abi word 0 = 0x60 (offset after 3 head words)
    ///   purchase    byte 0 = 0x19
    /// So equal hashes would need equal-length different preimages hashing the
    /// same -- a keccak collision.
    function testFuzz_honest_producers_preimages_never_meet(
        bytes calldata key,
        bytes calldata scope,
        address pol,
        uint256 id,
        uint64 dueAt,
        address payee,
        bytes32 ref,
        bytes32 domain,
        bytes32 body
    ) public pure {
        bytes memory a = _agentKey(key, scope);
        bytes memory s = _schedule(pol, id, dueAt);
        bytes memory i = _invoice(payee, ref);
        bytes memory p = _purchase(domain, body);

        assertEq(uint8(a[0]), 0);
        assertEq(uint8(a[7]), 19);
        assertEq(uint8(a[8]), uint8(bytes1("a")));
        assertEq(uint256(bytes32(_slice32(s, 0))), 0x80);
        assertEq(uint256(bytes32(_slice32(i, 0))), 0x60);
        assertEq(uint8(p[0]), 0x19);

        // agent vs abi forms: byte 8 is 'a' vs 0 in an offset word.
        assertTrue(a[8] != s[8] && a[8] != i[8]);
        // agent vs purchase: byte 0.
        assertTrue(a[0] != p[0]);
        // schedule vs invoice: the offset word.
        assertTrue(s[31] != i[31]);
        // abi forms vs purchase: byte 0.
        assertTrue(s[0] != p[0] && i[0] != p[0]);
        // And therefore the hashes differ (barring a keccak collision).
        assertTrue(keccak256(s) != keccak256(i));
        assertTrue(keccak256(a) != keccak256(s) && keccak256(a) != keccak256(i));
        assertTrue(keccak256(p) != keccak256(s) && keccak256(p) != keccak256(i) && keccak256(p) != keccak256(a));
    }

    /// The contract's own schedule hash is the format above.
    function test_schedule_hash_is_the_documented_preimage() public view {
        assertEq(policy.scheduleActionHash(7, 1_800_000_000), keccak256(_schedule(address(policy), 7, 1_800_000_000)));
    }

    function _slice32(bytes memory b, uint256 at) internal pure returns (bytes32 w) {
        assembly {
            w := mload(add(add(b, 32), at))
        }
    }

    // --- the producer that is not a format: the agent key --------------------

    /// F1a. A compromised agent can stall a standing order: it pays one unit
    /// to any approved payee under the order's next due hash; `payDue` then
    /// reverts for that cycle until the owner removes and re-adds the order.
    /// Bounded: one unit leaves, inside the caps, to an approved payee.
    function test_poc_agent_can_stall_a_standing_order() public {
        vm.prank(principal);
        uint256 id = policy.addSchedule(landlord, 50 * USDC, 30 days, uint64(block.timestamp + 1 days));
        bytes32 due = policy.scheduleActionHash(id, uint64(block.timestamp + 1 days));

        vm.prank(agent);
        policy.pay(hosting, 1, due); // one unit, to someone else, under the order's hash

        vm.warp(block.timestamp + 1 days);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AlreadyExecuted.selector, due));
        policy.payDue(id);
        assertEq(landlord.balance, 0, "the landlord was not paid");
    }

    /// F1b. A compromised agent can make an invoice look paid: it burns the
    /// invoice id with one unit to an approved payee; the owner's payment
    /// under that id then reverts, and `executed(id)` is true although the
    /// vendor got nothing. Reading "paid" from `executed(id)` alone is wrong;
    /// the `SettledByPrincipal(to, amount, id)` event is the evidence.
    function test_poc_agent_can_burn_an_invoice_id() public {
        address vendor = address(0x7E7D);
        bytes32 id = keccak256(_invoice(vendor, keccak256("INV-42")));

        vm.prank(agent);
        policy.pay(hosting, 1, id);

        assertTrue(policy.executed(id), "executed(id) says paid");
        assertEq(vendor.balance, 0, "the vendor got nothing");
        bytes32 terms = policy.fingerprint();
        vm.prank(principal);
        vm.expectRevert(abi.encodeWithSelector(BudgetPolicyBase.AlreadyExecuted.selector, id));
        policy.settleAsPrincipalIf(vendor, 7 * USDC, id, terms, uint64(block.timestamp));
    }

    /// F1c. The same front-run makes a reviewed run skip a row silently: the
    /// run reports one fewer paid and emits no error.
    function test_poc_agent_front_run_makes_a_run_skip_a_row() public {
        bytes32 rent = keccak256("rent-cycle-11");
        bytes32 host = keccak256("hosting-cycle-11");
        vm.prank(agent);
        policy.pay(hosting, 1, rent);

        address[] memory to = new address[](2);
        uint256[] memory amt = new uint256[](2);
        bytes32[] memory hs = new bytes32[](2);
        (to[0], amt[0], hs[0]) = (landlord, 50 * USDC, rent);
        (to[1], amt[1], hs[1]) = (hosting, 20 * USDC, host);
        bytes32 terms = policy.fingerprint();
        vm.prank(principal);
        uint256 paid = policy.settleRunIf(to, amt, hs, terms, uint64(block.timestamp));
        assertEq(paid, 1, "the rent row was skipped as already settled");
        assertEq(landlord.balance, 0);
    }
}
