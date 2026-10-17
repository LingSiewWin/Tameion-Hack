#!/usr/bin/env bash
# A guardian agrees to replace the owner's passkey (PLAN 5.30). When the
# threshold-th guardian agrees on the same key, the replacement becomes
# pending for the recovery delay; the passkey can cancel it until then.
#
#   scripts/guardian_approve.sh <owner-principal> <qx> <qy>      # signs with TAMEION_KEY_FILE (default ~/.tameion/treasury.key)
#   scripts/guardian_approve.sh <owner-principal> status         # guardians, threshold, pending key
set -euo pipefail
OWNER=${1:?owner PasskeyPrincipal address}
QX=${2:?new key x, or "status"}
RPC=${TAMEION_RPC_ARC:-https://rpc.testnet.arc.io}
KEY_FILE=${TAMEION_KEY_FILE:-$HOME/.tameion/treasury.key}

if [ "$QX" = status ]; then
  n=$(cast call "$OWNER" 'guardianCount()(uint256)' --rpc-url "$RPC" | awk '{print $1}')
  echo "guardians: $n, threshold $(cast call "$OWNER" 'guardianThreshold()(uint8)' --rpc-url "$RPC"), epoch $(cast call "$OWNER" 'recoveryEpoch()(uint64)' --rpc-url "$RPC" | awk '{print $1}')"
  i=0; while [ "$i" -lt "$n" ]; do echo "  #$i $(cast call "$OWNER" 'guardians(uint256)(address)' "$i" --rpc-url "$RPC")"; i=$((i+1)); done
  echo "pending key (qx, qy, eta): $(cast call "$OWNER" 'pendingKey()(bytes32,bytes32,uint64)' --rpc-url "$RPC" | paste -s -d ' ' -)"
  exit 0
fi
QY=${3:?new key y}
PK=$(cat "$KEY_FILE")
echo "approveRecovery on $OWNER from $(cast wallet address --private-key "$PK")"
cast send "$OWNER" 'approveRecovery(bytes32,bytes32)' "$QX" "$QY" --rpc-url "$RPC" --private-key "$PK" --json \
  | python3 -c 'import sys,json; r=json.load(sys.stdin); print("  tx", r["transactionHash"], "status", r["status"])'
echo "approvals so far: $(cast call "$OWNER" 'guardianApprovals(bytes32)(uint8)' "$(cast call "$OWNER" 'recoveryBallot(bytes32,bytes32)(bytes32)' "$QX" "$QY" --rpc-url "$RPC")" --rpc-url "$RPC") of $(cast call "$OWNER" 'guardianThreshold()(uint8)' --rpc-url "$RPC")"
echo "pending key (qx, qy, eta): $(cast call "$OWNER" 'pendingKey()(bytes32,bytes32,uint64)' --rpc-url "$RPC" | paste -s -d ' ' -)"
