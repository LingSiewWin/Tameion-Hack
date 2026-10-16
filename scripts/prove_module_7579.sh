#!/usr/bin/env bash
# PLAN 5.29: the Tameion limits as an ERC-7579 executor module, proved on a real chain.
#
# A wallet runs OpenZeppelin's ERC-7579 account by EIP-7702 and installs
# TameionExecutor on itself with a self-call (no bundler, no EntryPoint); the
# agent key then pays an attested payee from the wallet, inside the caps, and
# is refused above them. Contracts go through the EIP-7997 factory, so their
# addresses are the same on every chain that has it.
#
#   scripts/prove_module_7579.sh <wallet-key-file> <agent-key-file> <payee>
#
# Keys are files (mode 600, outside the repo); nothing is printed but addresses
# and hashes. The relayer (TAMEION_KEY_FILE, default ~/.tameion/treasury.key)
# pays for the two deploys and for applying Autopilot after the delay.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WALLET_KEY_FILE=${1:?wallet key file}
AGENT_KEY_FILE=${2:?agent key file}
PAYEE=${3:?payee address}
RPC=${TAMEION_RPC_ARC:-https://rpc.testnet.arc.network}
USDC=${USDC:-0x3600000000000000000000000000000000000000}
RELAYER_KEY_FILE=${TAMEION_KEY_FILE:-$HOME/.tameion/treasury.key}
FACTORY=0x4e59b44847b379578588920cA78FbF26c0B4956C
DELAY=${DELAY:-60}
PER=${PER:-5000}      # 0.005 USDC per payment
DAILY=${DAILY:-20000} # 0.02 USDC per trailing 24 h
FLOOR=${FLOOR:-5000}  # never below 0.005 USDC

tx() { python3 -c 'import sys,json; r=json.load(sys.stdin); print(r["transactionHash"], "status", r["status"], "type", r.get("type"))'; }

(cd "$ROOT/contracts" && forge build --quiet --force)  # --force: forge lint leaves abi-only artifacts (docs/60 P-6)
# Sets DEPLOYED (not echoed through $(...): bash 3.2 does not carry set -e into
# command substitution, and a failed deploy must stop the script — docs/60 P-24).
deploy() { # name salt-tag
  local code salt
  code=$(python3 -c "import json; print(json.load(open('$ROOT/contracts/out/$1.sol/$1.json'))['bytecode']['object'])")
  salt=$(cast keccak "$2")
  DEPLOYED=$(cast create2 --deployer $FACTORY --salt "$salt" --init-code "$code" 2>/dev/null | awk '{print $NF}')
  if [ "$(cast code "$DEPLOYED" --rpc-url "$RPC")" = "0x" ]; then
    echo "  deploying $1 at $DEPLOYED"
    cast send $FACTORY "${salt}${code#0x}" --rpc-url "$RPC" --private-key "$(cat "$RELAYER_KEY_FILE")" --json > /tmp/tameion-deploy.json
    tx < /tmp/tameion-deploy.json
  else
    echo "  $1 already at $DEPLOYED"
  fi
  # Never hand a wallet an empty delegation target: its calls would succeed
  # and do nothing (docs/60 P-24).
  if [ "$(cast code "$DEPLOYED" --rpc-url "$RPC")" = "0x" ]; then echo "  $1 has no code at $DEPLOYED; stopping"; exit 1; fi
}
echo "== contracts (EIP-7997 factory, deterministic)"
deploy TameionExecutor "tameion.executor.v1"; EXECUTOR=$DEPLOYED
deploy TameionModularAccount "tameion.modular-account.v1"; ACCOUNT_IMPL=$DEPLOYED

WALLET=$(cast wallet address --private-key "$(cat "$WALLET_KEY_FILE")")
AGENT=$(cast wallet address --private-key "$(cat "$AGENT_KEY_FILE")")
echo "== wallet $WALLET becomes an ERC-7579 account and installs the module on itself (one type-4 tx)"
INIT=$(cast abi-encode "f(address,address,uint128,uint128,uint128,uint64)" "$AGENT" "$USDC" "$PER" "$DAILY" "$FLOOR" "$DELAY")
cast send "$WALLET" "installModule(uint256,address,bytes)" 2 "$EXECUTOR" "$INIT" --auth "$ACCOUNT_IMPL" \
  --rpc-url "$RPC" --private-key "$(cat "$WALLET_KEY_FILE")" --json | tx
echo "  wallet code: $(cast code "$WALLET" --rpc-url "$RPC")"
INSTALLED=$(cast call "$WALLET" 'isModuleInstalled(uint256,address,bytes)(bool)' 2 "$EXECUTOR" 0x --rpc-url "$RPC")
echo "  isModuleInstalled: $INSTALLED"
[ "$INSTALLED" = "true" ] || { echo "  the module is not installed; stopping"; exit 1; }

echo "== the wallet attests the payee and asks for Autopilot (its own calls; both wait $DELAY s)"
cast send "$EXECUTOR" "setPayee(address,bool)" "$PAYEE" true --rpc-url "$RPC" --private-key "$(cat "$WALLET_KEY_FILE")" --json | tx
cast send "$EXECUTOR" "requestAutopilot()" --rpc-url "$RPC" --private-key "$(cat "$WALLET_KEY_FILE")" --json | tx
ETA=$(( $(cast block latest --field timestamp --rpc-url "$RPC") + DELAY + 2 ))
until [ "$(cast block latest --field timestamp --rpc-url "$RPC")" -gt "$ETA" ]; do sleep 5; done
echo "  applyAutopilot (anyone, after the delay):"
cast send "$EXECUTOR" "applyAutopilot(address)" "$WALLET" --rpc-url "$RPC" --private-key "$(cat "$RELAYER_KEY_FILE")" --json | tx

echo "== the agent: above the cap is refused, inside it is paid from the wallet"
B0=$(cast call "$USDC" 'balanceOf(address)(uint256)' "$PAYEE" --rpc-url "$RPC" | awk '{print $1}')
if cast send "$EXECUTOR" "pay(address,address,uint256,bytes32)" "$WALLET" "$PAYEE" $((PER + 1)) "$(cast keccak over-cap)" \
     --rpc-url "$RPC" --private-key "$(cat "$AGENT_KEY_FILE")" --json >/dev/null 2>&1; then
  echo "  OVER-CAP PAYMENT WAS NOT REFUSED"; exit 1
else
  echo "  over the cap: refused (OverPerActionCap)"
fi
cast send "$EXECUTOR" "pay(address,address,uint256,bytes32)" "$WALLET" "$PAYEE" "$PER" "$(cast keccak "bill-$(date +%s)")" \
  --rpc-url "$RPC" --private-key "$(cat "$AGENT_KEY_FILE")" --json | tx
B1=$(cast call "$USDC" 'balanceOf(address)(uint256)' "$PAYEE" --rpc-url "$RPC" | awk '{print $1}')
echo "  payee received $((B1 - B0)) units; spentInWindow $(cast call "$EXECUTOR" 'spentInWindow(address)(uint256)' "$WALLET" --rpc-url "$RPC")"
echo "executor $EXECUTOR · account implementation $ACCOUNT_IMPL · wallet $WALLET · agent $AGENT"
