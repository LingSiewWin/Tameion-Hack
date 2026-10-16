// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test, console} from "forge-std/Test.sol";
import {BudgetPolicy, BudgetPolicyBase} from "../src/BudgetPolicy.sol";
import {BudgetPolicyToken} from "../src/BudgetPolicyToken.sol";
import {BudgetPolicyPull} from "../src/BudgetPolicyPull.sol";
import {TameionDelegate} from "../src/TameionDelegate.sol";
import {TestUSDC} from "../src/TestUSDC.sol";

/// @dev Every actor at once, against one policy: the agent (current and a
///      rotated-out key), the principal, the operator, a stranger, an x402
///      facilitator, anyone calling `payDue`. Each call checks, at the moment
///      it lands, the rule it could break, and counts a violation instead of
///      asserting (a handler revert is swallowed by the fuzzer). A ghost
///      ledger keeps what actually moved, independent of the contract's own
///      bookkeeping, for the invariants to compare.
contract Adversary is Test {
    enum Kind {
        Native,
        Token,
        Pull,
        Delegate
    }

    BudgetPolicyBase public immutable p;
    Kind public immutable kind;
    TestUSDC public immutable usdc;
    address public immutable source;
    address public immutable principal;
    address public immutable operator;
    address public constant STRANGER = address(0x5757);

    uint256[2] internal agentPks = [uint256(0xA6E1), uint256(0xA6E2)];
    uint256 public currentAgent; // index into agentPks
    address[5] public payees = [address(0x9001), address(0x9002), address(0x9003), address(0x9004), address(0x9005)];

    // --- ghost ledger ------------------------------------------------------
    uint256 public funded;
    uint256 public outflow;
    mapping(address => uint256) public received;
    mapping(bytes32 => bool) public seen;
    uint64[] internal debitAt;
    uint256[] internal debitAmt;
    bytes32[6] internal pool;
    uint256 internal fresh;

    // --- violations, by rule ----------------------------------------------
    uint256 public vCap;
    uint256 public vDaily;
    uint256 public vPayee;
    uint256 public vReplay;
    uint256 public vMode;
    uint256 public vExpiry;
    uint256 public vFloor;
    uint256 public vAuth;
    uint256 public vDelay;
    uint256 public vTerms;

    // --- coverage: successes per path ---------------------------------------
    uint256 public okPay;
    uint256 public okRedeem;
    uint256 public okDue;
    uint256 public okSettle;
    uint256 public okApply;

    constructor(BudgetPolicyBase _p, Kind _kind, TestUSDC _usdc, address _source, address _principal, address _op) {
        p = _p;
        kind = _kind;
        usdc = _usdc;
        source = _source;
        principal = _principal;
        operator = _op;
        for (uint256 i = 0; i < pool.length; i++) {
            pool[i] = keccak256(abi.encode("pool", i));
        }
    }

    function agentAddr(uint256 i) public view returns (address) {
        return vm.addr(agentPks[i]);
    }

    // --- helpers -------------------------------------------------------------

    function balOf(address who) public view returns (uint256) {
        if (kind == Kind.Native || kind == Kind.Delegate) return who.balance / 1e12;
        return usdc.balanceOf(who);
    }

    /// The account the money leaves from (the wallet in pull mode).
    function vault() public view returns (address) {
        return kind == Kind.Pull ? source : address(p);
    }

    function _hash(uint256 seed) internal returns (bytes32) {
        // Mostly a small pool, so replays happen; sometimes fresh.
        if (seed % 4 != 0) return pool[seed % pool.length];
        return keccak256(abi.encode("fresh", fresh++));
    }

    function _windowSum() internal view returns (uint256 s) {
        for (uint256 i = 0; i < debitAt.length; i++) {
            if (block.timestamp - debitAt[i] < p.WINDOW()) s += debitAmt[i];
        }
    }

    struct Caps {
        uint256 per;
        uint256 daily;
        uint256 floor;
        uint8 mode;
    }

    function _caps() internal view returns (Caps memory c) {
        c = Caps(p.perActionCap(), p.dailyCap(), p.reserveFloor(), uint8(p.mode()));
    }

    /// Outside `applyPending`, no call may expand what the agent can do.
    function _noRaise(Caps memory b) internal {
        Caps memory a = _caps();
        if (a.per > b.per || a.daily > b.daily || a.floor < b.floor || a.mode > b.mode) vDelay++;
    }

    function _agentPathChecks(address to, uint256 amount, bytes32 h, Caps memory c, uint256 windowBefore) internal {
        if (amount > c.per) vCap++;
        if (windowBefore + amount > c.daily) vDaily++;
        uint256 since = p.attestedSince(to);
        if (!p.attestedPayee(to) || since == 0 || block.timestamp < since + p.delay()) vPayee++;
        if (seen[h]) vReplay++;
        if (p.balanceUsdc() < c.floor) vFloor++;
    }

    function _book(address to, uint256 amount, bytes32 h, bool agentPath) internal {
        seen[h] = true;
        outflow += amount;
        received[to] += amount;
        if (agentPath) {
            debitAt.push(uint64(block.timestamp));
            debitAmt.push(amount);
        }
    }

    // --- the agent -----------------------------------------------------------

    function agentPay(uint256 who, uint256 amount, uint256 seed, bool staleKey) external {
        address to = payees[who % payees.length];
        amount = bound(amount, 1, 60_000000);
        bytes32 h = _hash(seed);
        Caps memory c = _caps();
        uint256 w = _windowSum();
        uint256 k = staleKey ? 1 - currentAgent : currentAgent;
        vm.prank(agentAddr(k));
        try p.pay(to, amount, h) {
            okPay++;
            if (k != currentAgent) vAuth++;
            if (c.mode != uint8(BudgetPolicyBase.Mode.Autopilot)) vMode++;
            if (block.timestamp >= p.agentExpiry()) vExpiry++;
            _agentPathChecks(to, amount, h, c, w);
            _book(to, amount, h, true);
        } catch {}
        _noRaise(c);
    }

    /// An x402 purchase the agent signed, redeemed by a facilitator.
    function redeem(uint256 who, uint256 amount, uint256 nonceSeed, bool staleKey) external {
        address to = payees[who % payees.length];
        amount = bound(amount, 1, 60_000000);
        bytes32 nonce = keccak256(abi.encode("n", nonceSeed % 8));
        uint64 deadline = uint64(block.timestamp + 1 hours);
        bytes32 digest = p.purchaseDigest(to, amount, nonce, deadline);
        uint256 k = staleKey ? 1 - currentAgent : currentAgent;
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(agentPks[k], digest);
        bytes[] memory ctx = new bytes[](1);
        ctx[0] = abi.encode(nonce, deadline, abi.encodePacked(r, s, v));
        bytes32[] memory modes = new bytes32[](1);
        bytes[] memory exec = new bytes[](1);
        exec[0] = abi.encodePacked(p.usdcToken(), uint256(0), abi.encodeWithSelector(bytes4(0xa9059cbb), to, amount));
        Caps memory c = _caps();
        uint256 w = _windowSum();
        vm.prank(STRANGER);
        try p.redeemDelegations(ctx, modes, exec) {
            okRedeem++;
            if (k != currentAgent) vAuth++;
            if (c.mode != uint8(BudgetPolicyBase.Mode.Autopilot)) vMode++;
            if (block.timestamp >= p.agentExpiry()) vExpiry++;
            _agentPathChecks(to, amount, digest, c, w);
            _book(to, amount, digest, true);
        } catch {}
        _noRaise(c);
    }

    /// The honest agent: an approved payee, inside the room left, a fresh
    /// hash, the current key. Same checks: honesty is not trusted either.
    function agentPayHonest(uint256 who, uint256 amount) external {
        address to = payees[who % 2];
        Caps memory c = _caps();
        uint256 w = _windowSum();
        uint256 room = c.daily > w ? c.daily - w : 0;
        if (room > c.per) room = c.per;
        if (room == 0) return;
        amount = bound(amount, 1, room);
        bytes32 h = keccak256(abi.encode("fresh", fresh++));
        vm.prank(agentAddr(currentAgent));
        try p.pay(to, amount, h) {
            okPay++;
            if (c.mode != uint8(BudgetPolicyBase.Mode.Autopilot)) vMode++;
            if (block.timestamp >= p.agentExpiry()) vExpiry++;
            _agentPathChecks(to, amount, h, c, w);
            _book(to, amount, h, true);
        } catch {}
        _noRaise(c);
    }

    /// The owner puts things back: renews the agent, asks for Autopilot and
    /// sane limits, waits the delay, applies. Any rule broken on the way is
    /// still counted by the calls it makes.
    function ownerRestores() external {
        vm.prank(principal);
        p.renewAgent(uint64(block.timestamp + 90 days));
        vm.prank(operator);
        p.setMode(BudgetPolicyBase.Mode.Autopilot);
        vm.prank(operator);
        p.setLimits(10_000000, 30_000000, 50_000000);
        (,,,, uint64 eta, uint8 kinds) = p.pending();
        vm.warp(block.timestamp + p.delay());
        if (kinds != 0) {
            if (block.timestamp < eta) vDelay++;
            p.applyPending();
            okApply++;
        }
    }

    // --- standing orders -----------------------------------------------------

    function addSchedule(uint256 who, uint256 amount, uint256 every, uint256 firstIn) external {
        address to = payees[who % payees.length];
        Caps memory c = _caps();
        amount = bound(amount, 1, c.per == 0 ? 1 : c.per);
        vm.prank(principal);
        try p.addSchedule(
            to, amount, uint64(bound(every, 1 hours, 3 days)), uint64(block.timestamp + bound(firstIn, 0, 2 days))
        ) {}
            catch {}
        _noRaise(c);
    }

    function payDue(uint256 idSeed) external {
        uint256 n = p.scheduleCount();
        if (n == 0) return;
        uint256 id = idSeed % n;
        (address to, uint256 amount,, uint64 nextDue,) = p.schedules(id);
        bytes32 h = p.scheduleActionHash(id, nextDue);
        Caps memory c = _caps();
        uint256 w = _windowSum();
        vm.prank(STRANGER);
        try p.payDue(id) {
            okDue++;
            if (c.mode == uint8(BudgetPolicyBase.Mode.Watch)) vMode++;
            if (block.timestamp < nextDue) vDelay++;
            _agentPathChecks(to, amount, h, c, w);
            _book(to, amount, h, true);
        } catch {}
        _noRaise(c);
    }

    // --- the principal -------------------------------------------------------

    function settle(uint256 who, uint256 amount, uint256 seed, uint256 callerSeed) external {
        address to = who % 7 == 6 ? STRANGER : payees[who % payees.length];
        amount = bound(amount, 1, 40_000000);
        bytes32 h = _hash(seed);
        address caller = _caller(callerSeed);
        Caps memory c = _caps();
        vm.prank(caller);
        try p.settleAsPrincipal(to, amount, h) {
            okSettle++;
            if (caller != principal) vAuth++;
            if (seen[h]) vReplay++;
            _book(to, amount, h, false);
        } catch {}
        _noRaise(c);
    }

    /// A bound approval: valid only on the exact terms it names.
    function settleIf(uint256 who, uint256 amount, uint256 seed, bool stale) external {
        address to = payees[who % payees.length];
        amount = bound(amount, 1, 40_000000);
        bytes32 h = _hash(seed);
        bytes32 terms = stale ? keccak256(abi.encode(seed)) : p.fingerprint();
        Caps memory c = _caps();
        vm.prank(principal);
        try p.settleAsPrincipalIf(to, amount, h, terms, uint64(block.timestamp)) {
            okSettle++;
            if (terms != p.fingerprint()) vTerms++;
            if (seen[h]) vReplay++;
            _book(to, amount, h, false);
        } catch {}
        _noRaise(c);
    }

    function withdraw(uint256 amount, uint256 callerSeed) external {
        amount = bound(amount, 1, 30_000000);
        address caller = _caller(callerSeed);
        Caps memory c = _caps();
        vm.prank(caller);
        try p.withdraw(principal, amount) {
            if (caller != principal) vAuth++;
            outflow += amount;
            received[principal] += amount;
        } catch {}
        _noRaise(c);
    }

    function setPayee(uint256 who, bool on, uint256 callerSeed) external {
        address to = payees[who % payees.length];
        address caller = _caller(callerSeed);
        Caps memory c = _caps();
        vm.prank(caller);
        try p.setPayee(to, on) {
            if (caller != principal) vAuth++;
        } catch {}
        _noRaise(c);
    }

    function renew(uint256 secs, uint256 callerSeed) external {
        address caller = _caller(callerSeed);
        Caps memory c = _caps();
        vm.prank(caller);
        try p.renewAgent(uint64(block.timestamp + bound(secs, 1, 400 days))) {
            if (caller != principal) vAuth++;
        } catch {}
        _noRaise(c);
    }

    // --- the operator (and the principal on the shared paths) -----------------

    function setLimits(uint256 per, uint256 daily, uint256 floor, uint256 callerSeed) external {
        per = bound(per, 0, 50_000000);
        daily = bound(daily, per, 200_000000);
        floor = bound(floor, 0, 300_000000);
        address caller = _caller(callerSeed);
        Caps memory c = _caps();
        bool shrinking = per <= c.per && daily <= c.daily && floor >= c.floor;
        vm.prank(caller);
        try p.setLimits(per, daily, floor) {
            if (caller != principal && caller != operator) vAuth++;
            Caps memory a = _caps();
            // A tightening applies at once; anything else waits.
            if (shrinking && (a.per != per || a.daily != daily || a.floor != floor)) vDelay++;
        } catch {}
        _noRaise(c);
    }

    function setMode(uint8 m, uint256 callerSeed) external {
        m = uint8(bound(m, 0, 2));
        address caller = _caller(callerSeed);
        Caps memory c = _caps();
        vm.prank(caller);
        try p.setMode(BudgetPolicyBase.Mode(m)) {
            if (caller != principal && caller != operator) vAuth++;
            if (m <= c.mode && uint8(p.mode()) != m) vDelay++; // lowering is instant
        } catch {}
        _noRaise(c);
    }

    function applyPending() external {
        (,,,, uint64 eta, uint8 kinds) = p.pending();
        vm.prank(STRANGER);
        try p.applyPending() {
            okApply++;
            if (kinds == 0 || block.timestamp < eta) vDelay++;
        } catch {}
    }

    function cancelPending(uint256 callerSeed) external {
        address caller = _caller(callerSeed);
        Caps memory c = _caps();
        vm.prank(caller);
        try p.cancelPending() {
            if (caller != principal && caller != operator) vAuth++;
        } catch {}
        _noRaise(c);
    }

    /// Rotate the agent key. The key rotated out must lose every power.
    function rotateAgent(uint256 callerSeed) external {
        address caller = _caller(callerSeed);
        uint256 next = 1 - currentAgent;
        Caps memory c = _caps();
        vm.prank(caller);
        try p.setAgent(agentAddr(next)) {
            if (caller != operator) vAuth++;
            currentAgent = next;
        } catch {}
        _noRaise(c);
    }

    // --- the world -----------------------------------------------------------

    function wait(uint256 secs) external {
        vm.warp(block.timestamp + bound(secs, 1, 3 days));
    }

    function fund(uint256 amount) external {
        amount = bound(amount, 1, 500_000000);
        if (kind == Kind.Native || kind == Kind.Delegate) {
            vm.deal(address(p), address(p).balance + amount * 1e12);
        } else {
            usdc.mint(vault(), amount);
        }
        funded += amount;
    }

    function _caller(uint256 seed) internal view returns (address) {
        uint256 i = seed % 5;
        if (i == 0) return principal;
        if (i == 1) return operator;
        if (i == 2) return agentAddr(currentAgent);
        if (i == 3) return agentAddr(1 - currentAgent);
        return STRANGER;
    }

    function payeeCount() external view returns (uint256) {
        return payees.length;
    }

    function windowSum() external view returns (uint256) {
        return _windowSum();
    }

    function violations() external view returns (uint256) {
        return vCap + vDaily + vPayee + vReplay + vMode + vExpiry + vFloor + vAuth + vDelay + vTerms;
    }
}

/// @dev The invariants, shared by every kind of policy. A subclass deploys
///      one kind and hands it to the adversary.
abstract contract AdversarialBase is Test {
    uint256 internal constant USDC = 1_000000;
    address internal principal = address(0xA11CE);
    address internal operator = address(0xEEEE);
    Adversary internal a;
    BudgetPolicyBase internal policy;

    function _wire(BudgetPolicyBase p, Adversary.Kind kind, TestUSDC usdc, address source, uint256 seedFunds) internal {
        policy = p;
        a = new Adversary(p, kind, usdc, source, principal, operator);
        // Two payees attested and cooled off; three left for the run.
        vm.startPrank(principal);
        p.setPayee(address(0x9001), true);
        p.setPayee(address(0x9002), true);
        vm.stopPrank();
        vm.warp(block.timestamp + p.delay());
        a.fund(seedFunds);
        targetContract(address(a));
    }

    function invariant_no_rule_was_broken() public view {
        assertEq(a.vCap(), 0, "per-action cap");
        assertEq(a.vDaily(), 0, "rolling daily cap");
        assertEq(a.vPayee(), 0, "unattested or cooling-off payee");
        assertEq(a.vReplay(), 0, "an action hash paid twice");
        assertEq(a.vMode(), 0, "agent paid outside Autopilot / order paid while paused");
        assertEq(a.vExpiry(), 0, "agent paid after its authority lapsed");
        assertEq(a.vFloor(), 0, "agent payment left less than the floor");
        assertEq(a.vAuth(), 0, "a caller without the right succeeded");
        assertEq(a.vDelay(), 0, "a trust-raising change applied early");
        assertEq(a.vTerms(), 0, "a bound approval landed on other terms");
    }

    /// What left is exactly what the ghost ledger says left.
    function invariant_money_is_conserved() public view {
        assertEq(a.balOf(a.vault()), a.funded() - a.outflow(), "vault = funded - outflow");
        for (uint256 i = 0; i < a.payeeCount(); i++) {
            address who = a.payees(i);
            assertEq(a.balOf(who), a.received(who), "each payee got exactly what was recorded");
        }
    }

    /// The contract's own window counter agrees with the ghost's record.
    function invariant_window_counter_matches_the_ghost() public view {
        assertEq(policy.spentInWindow(), a.windowSum(), "spentInWindow");
    }

    /// Coverage, printed per run with -vv: a suite that never paid proves nothing.
    function afterInvariant() external view {
        console.log("pay", a.okPay(), "redeem", a.okRedeem());
        console.log("payDue", a.okDue(), "settle", a.okSettle());
        console.log("applyPending", a.okApply());
    }
}

/// forge-config: default.invariant.runs = 256
/// forge-config: default.invariant.depth = 200
/// forge-config: default.invariant.fail-on-revert = false
contract NativePolicyAdversarialTest is AdversarialBase {
    function setUp() public {
        vm.warp(1_764_000_000);
        BudgetPolicy p = new BudgetPolicy(
            principal,
            vm.addr(0xA6E1),
            operator,
            address(0x8004),
            42,
            10 * USDC,
            30 * USDC,
            50 * USDC,
            1 hours,
            BudgetPolicyBase.Mode.Autopilot
        );
        _wire(p, Adversary.Kind.Native, TestUSDC(address(0)), address(0), 1000 * USDC);
    }
}

/// forge-config: default.invariant.runs = 256
/// forge-config: default.invariant.depth = 200
/// forge-config: default.invariant.fail-on-revert = false
contract TokenPolicyAdversarialTest is AdversarialBase {
    function setUp() public {
        vm.warp(1_764_000_000);
        TestUSDC usdc = new TestUSDC();
        BudgetPolicyToken p = new BudgetPolicyToken(
            address(usdc),
            principal,
            vm.addr(0xA6E1),
            operator,
            address(0x8004),
            42,
            10 * USDC,
            30 * USDC,
            50 * USDC,
            1 hours,
            BudgetPolicyBase.Mode.Autopilot
        );
        _wire(p, Adversary.Kind.Token, usdc, address(0), 1000 * USDC);
    }
}

/// forge-config: default.invariant.runs = 256
/// forge-config: default.invariant.depth = 200
/// forge-config: default.invariant.fail-on-revert = false
contract PullPolicyAdversarialTest is AdversarialBase {
    address internal wallet = address(0xBA11E7);

    function setUp() public {
        vm.warp(1_764_000_000);
        TestUSDC usdc = new TestUSDC();
        BudgetPolicyPull p = new BudgetPolicyPull(
            address(usdc),
            wallet,
            principal,
            vm.addr(0xA6E1),
            operator,
            address(0x8004),
            42,
            10 * USDC,
            30 * USDC,
            50 * USDC,
            1 hours,
            BudgetPolicyBase.Mode.Autopilot
        );
        vm.prank(wallet);
        usdc.approve(address(p), type(uint256).max);
        _wire(p, Adversary.Kind.Pull, usdc, wallet, 1000 * USDC);
    }
}

/// forge-config: default.invariant.runs = 256
/// forge-config: default.invariant.depth = 200
/// forge-config: default.invariant.fail-on-revert = false
contract DelegatePolicyAdversarialTest is AdversarialBase {
    function setUp() public {
        vm.warp(1_764_000_000);
        uint256 walletPk = 0x7702;
        address wallet = vm.addr(walletPk);
        TameionDelegate impl = new TameionDelegate();
        vm.signAndAttachDelegation(address(impl), walletPk);
        vm.prank(wallet);
        TameionDelegate(payable(wallet))
            .initialize(
                principal,
                vm.addr(0xA6E1),
                operator,
                address(0x8004),
                42,
                10 * USDC,
                30 * USDC,
                50 * USDC,
                1 hours,
                BudgetPolicyBase.Mode.Autopilot
            );
        _wire(BudgetPolicyBase(wallet), Adversary.Kind.Delegate, TestUSDC(address(0)), address(0), 1000 * USDC);
    }
}
