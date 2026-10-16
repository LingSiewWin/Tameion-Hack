#!/usr/bin/env bash
# Fresh recurring bills on the demo bill wallet, so Start finds bills that are
# still recurring when the demo is recorded (docs/44 beat 0:12). The wallet's old
# test bills stopped weeks ago, and Start now says so ("5 not paid lately").
#
#   scripts/seed_bills.sh [rounds=4] [every_secs=600]
#
# Arbitrum Sepolia only. Each round pays three test payees from the wallet the
# treasury key controls (0x6c1c…9A25): 0.003, 0.002 and 0.001 USDC. Start counts
# a bill once it has repeated, and calls it "not paid lately" after two missed
# cycles — so finish seeding within one cycle of recording (4 rounds × 600 s:
# start 40 min before; the bills stay fresh for about 20 min after the last round).
# That wallet is also the service's relayer: the script stops below a gas floor.
set -euo pipefail
ROUNDS=${1:-4}
EVERY=${2:-600}
RPC=${RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}
USDC=0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d # Arbitrum Sepolia USDC
KEY_FILE=${TAMEION_TREASURY_KEY_FILE:-$HOME/.tameion/treasury.key}
FLOOR_WEI=${GAS_FLOOR_WEI:-800000000000000} # 0.0008 ETH left for the relayer
# Payees with no earlier history in this wallet (0x1111…/0x3333… carry a stopped
# 3-minute series from October 2 that would blur the new cadence).
PAYEES=(0x5555555555555555555555555555555555555555 0x7777777777777777777777777777777777777777 0x9999999999999999999999999999999999999999)
AMOUNTS=(3000 2000 1000) # USDC base units

[ -r "$KEY_FILE" ] || { echo "no key file at $KEY_FILE" >&2; exit 1; }
FROM=$(cast wallet address --private-key "$(cat "$KEY_FILE")")
echo "bill wallet $FROM on Arbitrum Sepolia: $ROUNDS rounds, every $EVERY s"
for ((r = 1; r <= ROUNDS; r++)); do
  wei=$(cast balance "$FROM" --rpc-url "$RPC")
  if [ "$wei" -lt "$FLOOR_WEI" ]; then
    echo "stopped: $wei wei of gas left, below the floor $FLOOR_WEI kept for the relayer; top it up from a faucet" >&2
    exit 1
  fi
  for i in "${!PAYEES[@]}"; do
    tx=$(cast send "$USDC" 'transfer(address,uint256)' "${PAYEES[$i]}" "${AMOUNTS[$i]}" \
      --rpc-url "$RPC" --private-key "$(cat "$KEY_FILE")" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["transactionHash"])')
    echo "round $r: ${AMOUNTS[$i]} units to ${PAYEES[$i]}  https://sepolia.arbiscan.io/tx/$tx"
  done
  [ "$r" -lt "$ROUNDS" ] && sleep "$EVERY"
done
echo "done: paste $FROM on Start; the three bills read as recurring for about two cycles"
