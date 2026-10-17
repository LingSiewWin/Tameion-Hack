#!/usr/bin/env bash
# A payment history for the recording story (docs/115 §3, docs/90 §0): a demo wallet pays the demo vendor
# three times, 90 s apart, then stops, so an account that connects the wallet finds the bill and offers its
# payee before the next one falls due (docs/60 P-228).
#
#   scripts/demo_seed_history.sh [vendor=$TAMEION_DEMO_VENDOR] [amount_units=2000] [every_secs=90]
#
# Arbitrum Sepolia only. The wallet is a test key at $DEMO_WALLET_KEY (default ~/.tameion/test-keys/demo-wallet.key, made here,
# mode 600, never printed); the relayer (~/.tameion/treasury.key) gives it the gas and the USDC for the three
# payments first, and stops below its own gas floor. Then connect the wallet printed at the end to a demo
# account (POST /api/level1/wallets, or "Connect a wallet" on the page) within a cycle of the last payment.
set -euo pipefail
VENDOR=${1:-${TAMEION_DEMO_VENDOR:-}}
UNITS=${2:-2000}
EVERY=${3:-90}
RPC=${RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}
USDC=0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d # Arbitrum Sepolia USDC
RELAYER_KEY=${TAMEION_RELAYER_KEY_FILE:-$HOME/.tameion/treasury.key}
WALLET_KEY=${DEMO_WALLET_KEY:-$HOME/.tameion/test-keys/demo-wallet.key}
GAS_WEI=60000000000000         # 0.00006 ETH: three transfers at Arbitrum Sepolia prices, with room
RELAYER_FLOOR_WEI=20000000000000000 # 0.02 ETH kept for the relayer's own work
EXP=https://sepolia.arbiscan.io/tx

[[ $VENDOR =~ ^0x[0-9a-fA-F]{40}$ ]] || { echo "name the vendor: demo_seed_history.sh 0x… (or set TAMEION_DEMO_VENDOR)" >&2; exit 1; }
[ -r "$RELAYER_KEY" ] || { echo "no relayer key at $RELAYER_KEY" >&2; exit 1; }
if [ ! -e "$WALLET_KEY" ]; then
  (umask 077; cast wallet new --json | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["private_key"])' > "$WALLET_KEY")
fi
chmod 600 "$WALLET_KEY"
WALLET=$(cast wallet address --private-key "$(cat "$WALLET_KEY")")
RELAYER=$(cast wallet address --private-key "$(cat "$RELAYER_KEY")")
send() { # key-file to sig args… -> tx hash (status checked)
  local key=$1; shift
  cast send "$@" --rpc-url "$RPC" --private-key "$(cat "$key")" --json |
    python3 -c 'import json,sys; r=json.load(sys.stdin); assert r["status"] in ("0x1", 1), r; print(r["transactionHash"])'
}
echo "demo wallet $WALLET pays $VENDOR $UNITS units of USDC three times, $EVERY s apart (Arbitrum Sepolia)"

relayer_wei=$(cast balance "$RELAYER" --rpc-url "$RPC")
if [ "$relayer_wei" -lt "$RELAYER_FLOOR_WEI" ]; then
  echo "stopped: the relayer holds $relayer_wei wei of gas, under its floor $RELAYER_FLOOR_WEI" >&2
  exit 1
fi
if [ "$(cast balance "$WALLET" --rpc-url "$RPC")" -lt "$GAS_WEI" ]; then
  echo "$(date +%T) gas for the wallet: $EXP/$(send "$RELAYER_KEY" "$WALLET" --value "$GAS_WEI")"
fi
need=$((3 * UNITS))
have=$(cast call "$USDC" 'balanceOf(address)(uint256)' "$WALLET" --rpc-url "$RPC" | awk '{print $1}')
if [ "$have" -lt "$need" ]; then
  echo "$(date +%T) USDC for the wallet ($((need - have)) units): $EXP/$(send "$RELAYER_KEY" "$USDC" 'transfer(address,uint256)' "$WALLET" "$((need - have))")"
fi
for r in 1 2 3; do
  echo "$(date +%T) payment $r: $UNITS units to $VENDOR $EXP/$(send "$WALLET_KEY" "$USDC" 'transfer(address,uint256)' "$VENDOR" "$UNITS")"
  [ "$r" -lt 3 ] && sleep "$EVERY"
done
echo "$(date +%T) stopped. Connect $WALLET to a demo account before $(date -r $(( $(date +%s) + EVERY )) +%T): the next payment falls due then"
