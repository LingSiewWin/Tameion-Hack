#!/usr/bin/env bash
# Pay one due cycle of a standing order (PLAN 5.25). Anyone may: the contract
# checks the due date, the payee list and the caps; the caller only pays gas.
#
#   scripts/pay_due.sh <policy> <id>                 # with the key file (default ~/.tameion/treasury.key)
#   scripts/pay_due.sh <policy> list                 # read the orders, no key
#
# Env: TAMEION_RPC_ARC (default Arc testnet public node), TAMEION_KEY_FILE.
set -euo pipefail
POLICY=${1:?policy address}
ID=${2:?schedule id, or "list"}
RPC=${TAMEION_RPC_ARC:-https://rpc.testnet.arc.network}
KEY_FILE=${TAMEION_KEY_FILE:-$HOME/.tameion/treasury.key}

if [ "$ID" = list ]; then
  n=$(cast call "$POLICY" 'scheduleCount()(uint256)' --rpc-url "$RPC")
  echo "$n standing order(s) on $POLICY"
  i=0
  while [ "$i" -lt "${n%% *}" ]; do
    cast call "$POLICY" 'schedules(uint256)(address,uint256,uint64,uint64,bool)' "$i" --rpc-url "$RPC" | paste -s -d ' ' - | sed "s/^/  #$i: payee amount every nextDue active = /"
    i=$((i+1))
  done
  exit 0
fi

PK=$(cat "$KEY_FILE")
echo "payDue($ID) on $POLICY from $(cast wallet address --private-key "$PK")"
cast send "$POLICY" 'payDue(uint256)' "$ID" --rpc-url "$RPC" --private-key "$PK" --json \
  | python3 -c 'import sys,json; r=json.load(sys.stdin); print("  tx", r["transactionHash"], "status", r["status"], "gasUsed", int(r["gasUsed"],16))'
cast call "$POLICY" 'schedules(uint256)(address,uint256,uint64,uint64,bool)' "$ID" --rpc-url "$RPC" | paste -s -d ' ' - | sed 's/^/  now: payee amount every nextDue active = /'
