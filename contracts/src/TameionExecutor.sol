// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {
    IERC7579Module,
    IERC7579Execution,
    MODULE_TYPE_EXECUTOR
} from "@openzeppelin/contracts/interfaces/draft-IERC7579.sol";

/// @title  TameionExecutor
/// @notice The Tameion limits as an ERC-7579 executor module (PLAN 5.29): any
///         modular smart account (Safe7579, Kernel, Nexus, OpenZeppelin's
///         AccountERC7579, a 7702 wallet running one of them) installs it and
///         gives an agent key bounded power over one stablecoin — attested
///         payees only, a per-payment cap, a trailing 24-hour cap, a reserve
///         floor, a replay guard, Autopilot only after a delay, and an agent
///         term that lapses. The account's own signer (its owner) is the only
///         one who changes those terms; widening them waits the delay,
///         narrowing them is instant. The money never leaves the account
///         except as a payment the agent proved it may make.
/// @dev    One module serves every account that installs it: all state is
///         keyed by the account (`msg.sender` for the account's own calls, an
///         explicit `account` for the agent's). It moves money only through
///         `executeFromExecutor`, which the account allows only for installed
///         executors, so uninstalling the module ends its power at the account
///         as well as here.
contract TameionExecutor is IERC7579Module {
    struct Config {
        address agent;
        address token;
        uint64 delay;
        uint64 agentExpiry;
        bool autopilot;
        bool installed;
        uint64 autopilotEta;
        uint128 perActionCap;
        uint128 dailyCap;
        uint128 reserveFloor;
    }

    struct PendingLimits {
        uint128 perActionCap;
        uint128 dailyCap;
        uint128 reserveFloor;
        uint64 eta;
    }

    struct Debit {
        uint64 at;
        uint128 amount;
    }

    uint256 public constant WINDOW = 1 days;
    uint256 public constant MAX_LIVE_DEBITS = 64;
    uint64 public constant AGENT_TERM = 90 days;
    uint64 public constant MAX_AGENT_TERM = 365 days;

    mapping(address account => Config) public configOf;
    mapping(address account => PendingLimits) public pendingLimitsOf;
    /// @notice When a payee becomes payable for an account (0: not attested).
    mapping(address account => mapping(address payee => uint64)) public payableFrom;
    mapping(address account => mapping(bytes32 actionHash => bool)) public executed;
    mapping(address account => Debit[]) internal _debits;

    event Installed(
        address indexed account,
        address agent,
        address token,
        uint128 perActionCap,
        uint128 dailyCap,
        uint128 reserveFloor,
        uint64 delay
    );
    event Uninstalled(address indexed account);
    event PayeeSet(address indexed account, address indexed payee, bool attested, uint64 payableFrom);
    event LimitsChanged(address indexed account, uint128 perActionCap, uint128 dailyCap, uint128 reserveFloor);
    event LimitsProposed(
        address indexed account, uint128 perActionCap, uint128 dailyCap, uint128 reserveFloor, uint64 eta
    );
    event AutopilotProposed(address indexed account, uint64 eta);
    event AutopilotSet(address indexed account, bool on);
    event AgentChanged(address indexed account, address agent, uint64 until);
    event Paid(address indexed account, address indexed to, uint256 amount, bytes32 indexed actionHash);

    error NotInstalled();
    error AlreadyInstalled();
    error ZeroAddress();
    error ZeroAmount();
    error CapAboveDailyCap();
    error NotAgent();
    error NotAutopilot();
    error AgentLapsed(uint64 expiry);
    error NotAttested(address payee);
    error TooEarly(uint64 eta);
    error OverPerActionCap(uint256 amount, uint256 cap);
    error OverDailyCap(uint256 spent, uint256 amount, uint256 cap);
    error BelowFloor(uint256 balance, uint256 amount, uint256 floor);
    error AlreadyExecuted(bytes32 actionHash);
    error TooManyDebits();
    error TermTooLong(uint64 until);
    error NothingPending();
    error TransferFailed();

    modifier onlyInstalled() {
        _onlyInstalled();
        _;
    }

    function _onlyInstalled() internal view {
        if (!configOf[msg.sender].installed) revert NotInstalled();
    }

    // ------------------------------------------------------------------
    // ERC-7579 module lifecycle (called by the account)

    /// @param data abi.encode(agent, token, perActionCap, dailyCap, reserveFloor, delay)
    function onInstall(bytes calldata data) external {
        if (configOf[msg.sender].installed) revert AlreadyInstalled();
        (address agent, address token, uint128 perActionCap, uint128 dailyCap, uint128 reserveFloor, uint64 delay) =
            abi.decode(data, (address, address, uint128, uint128, uint128, uint64));
        if (agent == address(0) || token == address(0)) revert ZeroAddress();
        if (perActionCap > dailyCap) revert CapAboveDailyCap();
        uint64 until = uint64(block.timestamp) + AGENT_TERM;
        configOf[msg.sender] = Config({
            agent: agent,
            token: token,
            delay: delay,
            agentExpiry: until,
            autopilot: false,
            installed: true,
            autopilotEta: 0,
            perActionCap: perActionCap,
            dailyCap: dailyCap,
            reserveFloor: reserveFloor
        });
        emit Installed(msg.sender, agent, token, perActionCap, dailyCap, reserveFloor, delay);
        emit AgentChanged(msg.sender, agent, until);
    }

    function onUninstall(bytes calldata) external {
        delete configOf[msg.sender];
        delete pendingLimitsOf[msg.sender];
        delete _debits[msg.sender];
        emit Uninstalled(msg.sender);
    }

    function isModuleType(uint256 moduleTypeId) external pure returns (bool) {
        return moduleTypeId == MODULE_TYPE_EXECUTOR;
    }

    function isInitialized(address account) external view returns (bool) {
        return configOf[account].installed;
    }

    // ------------------------------------------------------------------
    // The account's owner changes its own terms (msg.sender is the account)

    /// @notice Attest a payee (payable after the delay) or revoke one (at once).
    function setPayee(address payee, bool attested) external onlyInstalled {
        if (payee == address(0)) revert ZeroAddress();
        uint64 from = attested ? uint64(block.timestamp) + configOf[msg.sender].delay : 0;
        payableFrom[msg.sender][payee] = from;
        emit PayeeSet(msg.sender, payee, attested, from);
    }

    /// @notice Narrowing applies at once; any widening waits the delay.
    function setLimits(uint128 perActionCap, uint128 dailyCap, uint128 reserveFloor) external onlyInstalled {
        if (perActionCap > dailyCap) revert CapAboveDailyCap();
        Config storage c = configOf[msg.sender];
        bool narrowing = perActionCap <= c.perActionCap && dailyCap <= c.dailyCap && reserveFloor >= c.reserveFloor;
        if (narrowing) {
            c.perActionCap = perActionCap;
            c.dailyCap = dailyCap;
            c.reserveFloor = reserveFloor;
            delete pendingLimitsOf[msg.sender];
            emit LimitsChanged(msg.sender, perActionCap, dailyCap, reserveFloor);
        } else {
            uint64 eta = uint64(block.timestamp) + c.delay;
            pendingLimitsOf[msg.sender] =
                PendingLimits({perActionCap: perActionCap, dailyCap: dailyCap, reserveFloor: reserveFloor, eta: eta});
            emit LimitsProposed(msg.sender, perActionCap, dailyCap, reserveFloor, eta);
        }
    }

    /// @notice Anyone applies a widening after its delay.
    function applyLimits(address account) external {
        PendingLimits memory p = pendingLimitsOf[account];
        if (p.eta == 0) revert NothingPending();
        if (block.timestamp < p.eta) revert TooEarly(p.eta);
        Config storage c = configOf[account];
        c.perActionCap = p.perActionCap;
        c.dailyCap = p.dailyCap;
        c.reserveFloor = p.reserveFloor;
        delete pendingLimitsOf[account];
        emit LimitsChanged(account, p.perActionCap, p.dailyCap, p.reserveFloor);
    }

    /// @notice Ask for Autopilot; it applies after the delay (`applyAutopilot`).
    function requestAutopilot() external onlyInstalled {
        uint64 eta = uint64(block.timestamp) + configOf[msg.sender].delay;
        configOf[msg.sender].autopilotEta = eta;
        emit AutopilotProposed(msg.sender, eta);
    }

    function applyAutopilot(address account) external {
        Config storage c = configOf[account];
        if (c.autopilotEta == 0) revert NothingPending();
        if (block.timestamp < c.autopilotEta) revert TooEarly(c.autopilotEta);
        c.autopilot = true;
        c.autopilotEta = 0;
        emit AutopilotSet(account, true);
    }

    /// @notice Back to proposing, at once (also cancels a pending request).
    function pause() external onlyInstalled {
        Config storage c = configOf[msg.sender];
        c.autopilot = false;
        c.autopilotEta = 0;
        emit AutopilotSet(msg.sender, false);
    }

    /// @notice Replace the agent key (zero revokes it); a new key starts a new term.
    function setAgent(address agent) external onlyInstalled {
        uint64 until = agent == address(0) ? 0 : uint64(block.timestamp) + AGENT_TERM;
        configOf[msg.sender].agent = agent;
        configOf[msg.sender].agentExpiry = until;
        emit AgentChanged(msg.sender, agent, until);
    }

    function renewAgent(uint64 until) external onlyInstalled {
        if (until > block.timestamp + MAX_AGENT_TERM) revert TermTooLong(until);
        configOf[msg.sender].agentExpiry = until;
        emit AgentChanged(msg.sender, configOf[msg.sender].agent, until);
    }

    // ------------------------------------------------------------------
    // The agent

    /// @notice Pay `amount` of the account's token to `to`, if every limit holds.
    function pay(address account, address to, uint256 amount, bytes32 actionHash) external {
        Config storage c = configOf[account];
        if (!c.installed) revert NotInstalled();
        if (msg.sender != c.agent || c.agent == address(0)) revert NotAgent();
        if (!c.autopilot) revert NotAutopilot();
        if (block.timestamp >= c.agentExpiry) revert AgentLapsed(c.agentExpiry);
        uint64 from = payableFrom[account][to];
        if (from == 0) revert NotAttested(to);
        if (block.timestamp < from) revert TooEarly(from);
        if (amount == 0) revert ZeroAmount();
        if (amount > c.perActionCap) revert OverPerActionCap(amount, c.perActionCap);
        if (executed[account][actionHash]) revert AlreadyExecuted(actionHash);
        uint256 spent = spentInWindow(account);
        if (spent + amount > c.dailyCap) revert OverDailyCap(spent, amount, c.dailyCap);
        uint256 balance = IERC20(c.token).balanceOf(account);
        if (balance < amount + c.reserveFloor) revert BelowFloor(balance, amount, c.reserveFloor);

        executed[account][actionHash] = true;
        _record(account, amount);

        // Single call, default exec type: the mode is all zeros (ERC-7579).
        bytes[] memory ret = IERC7579Execution(account)
            .executeFromExecutor(
                bytes32(0), abi.encodePacked(c.token, uint256(0), abi.encodeCall(IERC20.transfer, (to, amount)))
            );
        if (ret.length == 1 && ret[0].length >= 32 && !abi.decode(ret[0], (bool))) revert TransferFailed();
        emit Paid(account, to, amount, actionHash);
    }

    /// @notice What the agent paid for `account` in the trailing 24 hours.
    function spentInWindow(address account) public view returns (uint256 total) {
        Debit[] storage d = _debits[account];
        uint256 cutoff = block.timestamp > WINDOW ? block.timestamp - WINDOW : 0;
        for (uint256 i = 0; i < d.length; i++) {
            if (d[i].at > cutoff) total += d[i].amount;
        }
    }

    function _record(address account, uint256 amount) internal {
        Debit[] storage d = _debits[account];
        uint256 cutoff = block.timestamp > WINDOW ? block.timestamp - WINDOW : 0;
        // Drop what left the window (swap-and-pop; order does not matter for a sum).
        uint256 i = 0;
        while (i < d.length) {
            if (d[i].at <= cutoff) {
                d[i] = d[d.length - 1];
                d.pop();
            } else {
                i++;
            }
        }
        if (d.length >= MAX_LIVE_DEBITS) revert TooManyDebits();
        // forge-lint: disable-next-line(unsafe-typecast)
        d.push(Debit({at: uint64(block.timestamp), amount: uint128(amount)}));
    }
}
