// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test, console} from "forge-std/Test.sol";
import {CreditLine} from "../src/credit/CreditLine.sol";

/// @dev Three accounts, the scorer, the judge and anyone sending float, in
///      random order. Each call checks, at the moment it lands, the rule it
///      could break and counts a violation instead of asserting (a handler
///      revert is swallowed by the fuzzer). A ghost ledger keeps what actually
///      moved, independent of the contract's own books.
contract CreditHandler is Test {
    CreditLine public immutable line;
    address public immutable scorer;
    address public immutable judge;

    uint256 internal constant W = 1e12;
    uint256 internal constant USDC = 1_000000;

    address[3] public actors = [address(0xA1), address(0xA2), address(0xA3)];
    address[3] internal policies = [address(0xF1), address(0xF2), address(0xF3)];

    // --- ghost ledger, USDC minor units ---------------------------------------
    uint256 public locked;
    uint256 public unlocked;
    uint256 public slashed;
    uint256 public floated;
    uint256 public drawn;
    uint256 public repaid;

    // --- violations, by rule ----------------------------------------------------
    uint256 public vLimit; // a draw left the debt above the limit it was checked against
    uint256 public vSlash; // a slash took other than min(debt, deposit)
    uint256 public vPaused; // a draw went through while paused
    uint256 public vStale; // a draw went through on a score older than maxAge

    // --- coverage ---------------------------------------------------------------
    uint256 public okDraw;
    uint256 public okRepay;
    uint256 public okSlash;
    uint256 public okUnlock;
    uint256 public refusedPaused;
    uint256 public refusedStale;

    constructor(CreditLine _line, address _scorer, address _judge) {
        line = _line;
        scorer = _scorer;
        judge = _judge;
        vm.deal(address(this), 1_000_000 * USDC * W);
        for (uint256 i = 0; i < 3; i++) {
            vm.deal(actors[i], 1_000_000 * USDC * W);
            vm.prank(actors[i]);
            line.setPolicy(policies[i]);
        }
    }

    function _actor(uint256 seed) internal view returns (address) {
        return actors[seed % 3];
    }

    function lock(uint256 seed, uint256 units) external {
        units = bound(units, 1, 1000 * USDC);
        vm.prank(_actor(seed));
        line.lock{value: units * W}();
        locked += units;
    }

    function fund(uint256 units) external {
        units = bound(units, 1, 500 * USDC);
        (bool ok,) = address(line).call{value: units * W}("");
        require(ok, "fund");
        floated += units;
    }

    function score(uint256 seed, uint256 bps) external {
        vm.prank(scorer);
        line.setScore(_actor(seed), bound(bps, 0, 10_000));
    }

    function draw(uint256 seed, uint256 units) external {
        address a = _actor(seed);
        uint256 lim = line.limit(a);
        uint256 d = line.debt(a);
        // Three draws in four aim inside the room left; the fourth asks for anything.
        units = seed % 4 != 0 && lim > d ? bound(units, 1, lim - d) : bound(units, 1, 1500 * USDC);
        bool wasPaused = line.paused();
        bool stale = vm.getBlockTimestamp() - line.scoredAt(a) > line.maxAge();
        vm.prank(a);
        try line.draw(units) {
            drawn += units;
            okDraw++;
            if (line.debt(a) > lim) vLimit++;
            if (wasPaused) vPaused++;
            if (stale) vStale++;
        } catch {
            if (wasPaused) refusedPaused++;
            else if (stale) refusedStale++;
        }
    }

    function repay(uint256 seed, uint256 units) external {
        address a = _actor(seed);
        uint256 d = line.debt(a);
        if (d == 0) return;
        units = bound(units, 1, d);
        vm.prank(a);
        line.repay{value: units * W}();
        repaid += units;
        okRepay++;
    }

    function slash(uint256 seed, uint256 rule) external {
        address a = _actor(seed);
        uint256 dep = line.deposit(a);
        uint256 d = line.debt(a);
        vm.prank(judge);
        // forge-lint: disable-next-line(unsafe-typecast)
        try line.slash(a, keccak256(abi.encode(a, okSlash)), uint8(bound(rule, 1, 255))) {
            uint256 depAfter = line.deposit(a);
            uint256 debtAfter = line.debt(a);
            uint256 expected = d < dep ? d : dep;
            if (depAfter > dep || debtAfter > d) {
                vSlash++;
                return;
            }
            if (dep - depAfter != expected || d - debtAfter != expected) vSlash++;
            slashed += dep - depAfter;
            okSlash++;
        } catch {}
    }

    function requestUnlock(uint256 seed, uint256 units) external {
        address a = _actor(seed);
        uint256 dep = line.deposit(a);
        if (dep == 0) return;
        vm.prank(a);
        line.requestUnlock(bound(units, 1, dep));
    }

    function unlock(uint256 seed) external {
        address a = _actor(seed);
        (uint256 amount,) = line.pendingUnlock(a);
        vm.prank(a);
        try line.unlock() {
            unlocked += amount;
            okUnlock++;
        } catch {}
    }

    function pause(uint256 x) external {
        vm.prank(judge);
        line.setPaused(x % 4 == 0);
    }

    function wait(uint256 secs) external {
        vm.warp(vm.getBlockTimestamp() + bound(secs, 1, 12 hours));
    }
}

/// forge-config: default.invariant.runs = 256
/// forge-config: default.invariant.depth = 200
/// forge-config: default.invariant.fail-on-revert = false
contract CreditInvariantTest is Test {
    CreditLine internal line;
    CreditHandler internal h;

    address internal scorer = address(0x5C0);
    address internal judge = address(0x1D6E);

    uint256 internal constant W = 1e12;
    uint256 internal constant USDC = 1_000000;

    function setUp() public {
        vm.warp(1_764_000_000);
        line = new CreditLine(scorer, judge, 2000 * USDC, 1 days, 2 days);
        h = new CreditHandler(line, scorer, judge);
        targetContract(address(h));
    }

    /// Vault balance >= the deposits net of unlocks, minus what was slashed:
    /// deposits back the line and are never lent.
    function invariant_vault_covers_deposits_less_slashed() public view {
        uint256 net = h.locked() - h.unlocked() - h.slashed();
        assertGe(address(line).balance / W, net, "vault >= deposits - slashed");
        assertEq(line.totalDeposits(), net, "totalDeposits = ghost");
        uint256 sum;
        for (uint256 i = 0; i < 3; i++) {
            sum += line.deposit(h.actors(i));
        }
        assertEq(sum, line.totalDeposits(), "sum of deposits = totalDeposits");
    }

    /// Every wei in the vault is accounted for by the ghost ledger.
    function invariant_money_is_conserved() public view {
        uint256 expected = h.floated() + h.locked() - h.unlocked() - h.drawn() + h.repaid();
        assertEq(address(line).balance, expected * W, "vault = float + locked - unlocked - drawn + repaid");
    }

    /// debt(a) <= limit(a) at the moment of every draw.
    function invariant_debt_within_the_limit_at_draw_time() public view {
        assertEq(h.vLimit(), 0);
    }

    /// slash(a) = min(debt, deposit), so never more than the deposit.
    function invariant_slash_never_exceeds_the_deposit() public view {
        assertEq(h.vSlash(), 0);
    }

    /// No draw while paused.
    function invariant_no_draw_while_paused() public view {
        assertEq(h.vPaused(), 0);
    }

    /// A score older than maxAge gives limit 0, and no draw went through on one.
    function invariant_a_stale_score_gives_limit_zero() public view {
        assertEq(h.vStale(), 0);
        for (uint256 i = 0; i < 3; i++) {
            address a = h.actors(i);
            if (block.timestamp - line.scoredAt(a) > line.maxAge()) assertEq(line.limit(a), 0);
        }
    }

    /// Coverage, printed per run with -vv: a suite that never drew proves nothing.
    function afterInvariant() external view {
        console.log("draw", h.okDraw(), "repay", h.okRepay());
        console.log("slash", h.okSlash(), "unlock", h.okUnlock());
        console.log("refused paused", h.refusedPaused(), "refused stale", h.refusedStale());
    }
}
