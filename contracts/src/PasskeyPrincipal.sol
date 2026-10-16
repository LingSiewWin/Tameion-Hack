// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {WebAuthn} from "@openzeppelin/contracts/utils/cryptography/WebAuthn.sol";

/// @title  PasskeyPrincipal
/// @notice An account whose only key is a passkey on the owner's phone.
///
/// @dev    Made to be a `BudgetPolicy` principal. The principal is the one
///         role that can change who the agent may pay, move money out, or
///         appoint the operator. Here that authority is a WebAuthn assertion
///         from the owner's device -- Face ID or a fingerprint -- verified on
///         chain, so it lives in silicon the owner carries rather than in a
///         key file on a laptop the agent runs on.
///
///         Anyone may submit an approved call and pay its gas; only the
///         passkey can approve one. Each approval names the chain, this
///         account, a nonce, the target, the value and the exact calldata,
///         and is consumed on use, so it can be neither replayed nor
///         re-pointed at a different call.
///
///         Verification is OpenZeppelin's `WebAuthn` (v5.7.0), which uses the
///         P-256 precompile at 0x100 where the chain has one (Arbitrum: RIP-7212;
///         Ethereum: EIP-7951) and falls back to Solidity where it does not
///         (Arc lists no P-256 precompile). User verification (the UV flag:
///         biometric or PIN, not just presence) is required.
contract PasskeyPrincipal {
    /// @notice The passkey's public key. Storage, not immutable, because a
    ///         lost phone must not mean a frozen treasury (docs/46 "after",
    ///         built 2026-10-13): `recovery` can propose a new key, the delay
    ///         runs, the current passkey can cancel, then anyone applies.
    bytes32 public qx;
    bytes32 public qy;

    /// @notice Approvals consumed so far. The next approval must sign this.
    uint256 public nonce;

    /// @notice The address that may propose a replacement passkey: a hardware
    ///         wallet in a drawer, a second phone, a multisig. Zero means no
    ///         recovery path. Changed only by the passkey itself.
    address public recovery;

    /// @notice How long a proposed replacement waits before it can apply.
    ///         The window in which the real owner, still holding the phone,
    ///         cancels a proposal they did not make.
    uint256 public immutable recoveryDelay;

    struct PendingKey {
        bytes32 qx;
        bytes32 qy;
        uint64 eta;
    }

    /// @notice The replacement key waiting out the delay, if any.
    PendingKey public pendingKey;

    event Executed(address indexed target, uint256 value, bytes4 selector, uint256 nonce);
    event Received(address indexed from, uint256 value);
    event RecoveryProposed(bytes32 qx, bytes32 qy, uint64 eta, address indexed by);
    event RecoveryCancelled(address indexed by);
    event KeyRotated(bytes32 qx, bytes32 qy);
    event RecoverySet(address indexed previous, address indexed next);

    error BadPasskeySignature();
    error CallFailed(bytes reason);
    error ZeroKey();
    error NotOnCurve();
    error NotRecovery();
    error NotPasskeyOrRecovery();
    error NothingPending();
    error TooEarly(uint64 eta);

    /// P-256 (secp256r1): p, a = -3, b. A key that is not a point on the curve
    /// can never produce a signature `verify` accepts, so a page that sent a
    /// wrong or garbled key would lock the policy for good; refuse it at the
    /// door instead (docs/52).
    uint256 private constant P256_P = 0xFFFFFFFF00000001000000000000000000000000FFFFFFFFFFFFFFFFFFFFFFFF;
    uint256 private constant P256_B = 0x5AC635D8AA3A93E7B3EBBD55769886BC651D06B0CC53B0F63BCE3C3E27D2604B;

    function _requireOnCurve(bytes32 _qx, bytes32 _qy) internal pure {
        uint256 x = uint256(_qx);
        uint256 y = uint256(_qy);
        if (x >= P256_P || y >= P256_P) revert NotOnCurve();
        // y^2 == x^3 - 3x + b  (mod p)
        uint256 lhs = mulmod(y, y, P256_P);
        uint256 rhs = addmod(
            addmod(mulmod(mulmod(x, x, P256_P), x, P256_P), P256_P - mulmod(3, x, P256_P), P256_P), P256_B, P256_P
        );
        if (lhs != rhs) revert NotOnCurve();
    }

    constructor(bytes32 _qx, bytes32 _qy, address _recovery, uint256 _recoveryDelay) {
        if (_qx == bytes32(0) && _qy == bytes32(0)) revert ZeroKey();
        _requireOnCurve(_qx, _qy);
        qx = _qx;
        qy = _qy;
        recovery = _recovery;
        recoveryDelay = _recoveryDelay;
        emit RecoverySet(address(0), _recovery);
    }

    /// @notice Propose a replacement passkey. Recovery address only. Applies
    ///         after `recoveryDelay` unless the passkey cancels it.
    function proposeRecovery(bytes32 _qx, bytes32 _qy) external {
        if (msg.sender != recovery || recovery == address(0)) revert NotRecovery();
        if (_qx == bytes32(0) && _qy == bytes32(0)) revert ZeroKey();
        _requireOnCurve(_qx, _qy);
        // block.timestamp + delay fits in 64 bits for any delay a deployer would set.
        // forge-lint: disable-next-line(unsafe-typecast)
        uint64 eta = uint64(block.timestamp + recoveryDelay);
        pendingKey = PendingKey({qx: _qx, qy: _qy, eta: eta});
        emit RecoveryProposed(_qx, _qy, eta, msg.sender);
    }

    /// @notice Drop the pending replacement. The passkey (through `execute`
    ///         on this contract) or the recovery address.
    function cancelRecovery() external {
        if (msg.sender != address(this) && msg.sender != recovery) revert NotPasskeyOrRecovery();
        if (pendingKey.eta == 0) revert NothingPending();
        delete pendingKey;
        recoveryEpoch++;
        emit RecoveryCancelled(msg.sender);
    }

    /// @notice Make the pending replacement the passkey, once its delay has
    ///         passed. Anyone may call; nothing else changes (the nonce goes on).
    function applyRecovery() external {
        PendingKey memory p = pendingKey;
        if (p.eta == 0) revert NothingPending();
        if (block.timestamp < p.eta) revert TooEarly(p.eta);
        delete pendingKey;
        recoveryEpoch++;
        qx = p.qx;
        qy = p.qy;
        // Recovery means a key was lost. A 2-of-2 that lost a device would be
        // stranded, so recovery returns the account to its one key; the
        // owner names co-owners again. (Whoever can recover could already
        // replace the key, so this hands them nothing new.)
        if (coQx.length > 0 || approvalThreshold > 1) {
            delete coQx;
            delete coQy;
            approvalThreshold = 0;
            emit CoOwnersSet(0, 1);
        }
        emit KeyRotated(p.qx, p.qy);
    }

    // ------------------------------------------------------------------
    // Guardians (PLAN 5.30; the ERC-7093 idea): several addresses the owner
    // names — co-founders, a lawyer, a hardware wallet in a drawer — of whom
    // `guardianThreshold` must agree on the same replacement key before it
    // becomes pending; the passkey can still cancel during the delay. The
    // single `recovery` address keeps working beside them.

    address[] public guardians;
    uint8 public guardianThreshold;
    /// @dev Bumped whenever guardians change or a recovery ends, so partial
    ///      approvals from before cannot be reused.
    uint64 public recoveryEpoch;
    mapping(bytes32 => mapping(address => bool)) public guardianApproved;
    mapping(bytes32 => uint8) public guardianApprovals;

    event GuardiansSet(address[] guardians, uint8 threshold, address by);
    event GuardianApproved(address indexed guardian, bytes32 qx, bytes32 qy, uint8 approvals, uint8 threshold);

    error NotGuardian();
    error BadGuardians();
    error AlreadyApproved();

    /// @notice Name the guardians and how many must agree. Only the passkey,
    ///         through `execute`. Up to sixteen, no zero address, no repeat;
    ///         an empty list with threshold zero removes them.
    function setGuardians(address[] calldata list, uint8 threshold) external {
        if (msg.sender != address(this)) revert NotPasskeyOrRecovery();
        if (list.length > 16) revert BadGuardians();
        if (list.length == 0 ? threshold != 0 : (threshold == 0 || threshold > list.length)) revert BadGuardians();
        for (uint256 i = 0; i < list.length; i++) {
            if (list[i] == address(0)) revert BadGuardians();
            for (uint256 j = 0; j < i; j++) {
                if (list[j] == list[i]) revert BadGuardians();
            }
        }
        guardians = list;
        guardianThreshold = threshold;
        recoveryEpoch++;
        emit GuardiansSet(list, threshold, msg.sender);
    }

    function isGuardian(address a) public view returns (bool) {
        for (uint256 i = 0; i < guardians.length; i++) {
            if (guardians[i] == a) return true;
        }
        return false;
    }

    function guardianCount() external view returns (uint256) {
        return guardians.length;
    }

    /// @notice The key an approval is counted under: this epoch and this key.
    function recoveryBallot(bytes32 _qx, bytes32 _qy) public view returns (bytes32) {
        // forge-lint: disable-next-line(asm-keccak256)
        return keccak256(abi.encode(recoveryEpoch, _qx, _qy));
    }

    /// @notice A guardian agrees to replace the passkey with (`_qx`, `_qy`).
    ///         The threshold-th agreement makes it pending, exactly as the
    ///         recovery address would; the passkey may cancel it until `eta`.
    function approveRecovery(bytes32 _qx, bytes32 _qy) external {
        if (!isGuardian(msg.sender)) revert NotGuardian();
        if (_qx == bytes32(0) && _qy == bytes32(0)) revert ZeroKey();
        _requireOnCurve(_qx, _qy);
        bytes32 ballot = recoveryBallot(_qx, _qy);
        if (guardianApproved[ballot][msg.sender]) revert AlreadyApproved();
        guardianApproved[ballot][msg.sender] = true;
        uint8 n = ++guardianApprovals[ballot];
        emit GuardianApproved(msg.sender, _qx, _qy, n, guardianThreshold);
        if (n >= guardianThreshold && pendingKey.eta == 0) {
            // forge-lint: disable-next-line(unsafe-typecast)
            uint64 eta = uint64(block.timestamp + recoveryDelay);
            pendingKey = PendingKey({qx: _qx, qy: _qy, eta: eta});
            emit RecoveryProposed(_qx, _qy, eta, msg.sender);
        }
    }

    /// @notice Name a new recovery address. Only the passkey, through `execute`.
    function setRecovery(address next) external {
        if (msg.sender != address(this)) revert NotPasskeyOrRecovery();
        emit RecoverySet(recovery, next);
        recovery = next;
    }

    receive() external payable {
        emit Received(msg.sender, msg.value);
    }

    /// @notice What the passkey must sign to approve this call as approval
    ///         number `n`. The phone signs these 32 bytes as the WebAuthn
    ///         challenge.
    function challengeFor(address target, uint256 value, bytes calldata data, uint256 n) public view returns (bytes32) {
        // Plain abi.encode, not assembly: a phone, a page and a relayer all
        // recompute this, and it has to read the same everywhere.
        // forge-lint: disable-next-line(asm-keccak256)
        return keccak256(abi.encode(block.chainid, address(this), n, target, value, keccak256(data)));
    }

    /// @notice Run `target.call{value}(data)` if the passkey approved exactly
    ///         this call as the next approval.
    function execute(address target, uint256 value, bytes calldata data, WebAuthn.WebAuthnAuth calldata auth)
        external
        returns (bytes memory)
    {
        uint8 need = _need();
        if (need > 1) revert NeedsMoreApprovals(1, need);
        uint256 n = nonce;
        bytes32 challenge = challengeFor(target, value, data, n);
        if (!WebAuthn.verify(abi.encodePacked(challenge), auth, qx, qy, true)) revert BadPasskeySignature();
        return _run(target, value, data, n);
    }

    function _run(address target, uint256 value, bytes calldata data, uint256 n) internal returns (bytes memory) {
        // Consumed before the call, so a call that re-enters cannot reuse it.
        nonce = n + 1;

        (bool ok, bytes memory ret) = target.call{value: value}(data);
        if (!ok) revert CallFailed(ret);

        // casting to 'bytes4' is safe because it reads the selector, the
        // first four bytes of the calldata (zero-padded if shorter).
        // forge-lint: disable-next-line(unsafe-typecast)
        emit Executed(target, value, bytes4(data), n);
        return ret;
    }

    // ------------------------------------------------------------------
    // Co-owners (PLAN 5.7): more passkeys on more devices — a second phone, a
    // co-founder, the CFO — and how many must approve (`approvalThreshold`,
    // M of N). Key 0 is `qx/qy` (the key recovery and guardians rotate);
    // keys 1..n are co-owners. With a threshold of one any key approves
    // alone (`execute` for key 0, `executeAs` for the others); with two or
    // more, one key is refused and `executeWith` collects that many distinct
    // keys over the same challenge. Only the account changes the set, through
    // a call its current owners approved.

    bytes32[] public coQx;
    bytes32[] public coQy;
    /// @notice Approvals a call needs (0 is read as 1: one key, as before).
    uint8 public approvalThreshold;
    uint8 public constant MAX_CO_OWNERS = 8;

    event CoOwnersSet(uint256 count, uint8 threshold);

    error BadCoOwners();
    error NeedsMoreApprovals(uint8 have, uint8 need);
    error BadKeyIndex();

    function _need() internal view returns (uint8) {
        return approvalThreshold == 0 ? 1 : approvalThreshold;
    }

    function coOwnerCount() external view returns (uint256) {
        return coQx.length;
    }

    /// @notice Key `i`: 0 is the primary passkey, 1..n the co-owners.
    function keyAt(uint256 i) public view returns (bytes32, bytes32) {
        if (i == 0) return (qx, qy);
        if (i > coQx.length) revert BadKeyIndex();
        return (coQx[i - 1], coQy[i - 1]);
    }

    /// @notice Replace the co-owners and the threshold. Only through an
    ///         approved call (`msg.sender` is this account), so adding a
    ///         device needs the approvals the account needs today.
    function setCoOwners(bytes32[] calldata xs, bytes32[] calldata ys, uint8 threshold) external {
        if (msg.sender != address(this)) revert NotPasskeyOrRecovery();
        uint256 n = xs.length;
        if (n != ys.length || n > MAX_CO_OWNERS) revert BadCoOwners();
        if (threshold == 0 || threshold > n + 1) revert BadCoOwners();
        delete coQx;
        delete coQy;
        for (uint256 i = 0; i < n; i++) {
            if (xs[i] == bytes32(0) && ys[i] == bytes32(0)) revert ZeroKey();
            _requireOnCurve(xs[i], ys[i]);
            if (xs[i] == qx && ys[i] == qy) revert BadCoOwners();
            for (uint256 j = 0; j < i; j++) {
                if (xs[j] == xs[i] && ys[j] == ys[i]) revert BadCoOwners();
            }
            coQx.push(xs[i]);
            coQy.push(ys[i]);
        }
        approvalThreshold = threshold;
        emit CoOwnersSet(n, threshold);
    }

    /// @notice `execute`, approved by co-owner key `keyIndex` (threshold one).
    function executeAs(
        uint256 keyIndex,
        address target,
        uint256 value,
        bytes calldata data,
        WebAuthn.WebAuthnAuth calldata auth
    ) external returns (bytes memory) {
        uint8 need = _need();
        if (need > 1) revert NeedsMoreApprovals(1, need);
        (bytes32 x, bytes32 y) = keyAt(keyIndex);
        uint256 n = nonce;
        bytes32 challenge = challengeFor(target, value, data, n);
        if (!WebAuthn.verify(abi.encodePacked(challenge), auth, x, y, true)) revert BadPasskeySignature();
        return _run(target, value, data, n);
    }

    /// @notice Run a call that `keyIndexes.length` distinct keys approved
    ///         (strictly increasing indexes, at least the threshold), each
    ///         over the same challenge.
    function executeWith(
        address target,
        uint256 value,
        bytes calldata data,
        uint256[] calldata keyIndexes,
        WebAuthn.WebAuthnAuth[] calldata auths
    ) external returns (bytes memory) {
        uint8 need = _need();
        uint256 k = keyIndexes.length;
        if (k != auths.length) revert BadCoOwners();
        // forge-lint: disable-next-line(unsafe-typecast)
        if (k < need) revert NeedsMoreApprovals(uint8(k), need);
        uint256 n = nonce;
        bytes memory challenge = abi.encodePacked(challengeFor(target, value, data, n));
        for (uint256 i = 0; i < k; i++) {
            if (i > 0 && keyIndexes[i] <= keyIndexes[i - 1]) revert BadKeyIndex();
            (bytes32 x, bytes32 y) = keyAt(keyIndexes[i]);
            if (!WebAuthn.verify(challenge, auths[i], x, y, true)) revert BadPasskeySignature();
        }
        return _run(target, value, data, n);
    }
}
