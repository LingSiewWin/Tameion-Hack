#!/usr/bin/env bash
# Put some of a wallet's USDC into Circle Gateway, so it shows on `/` and
# `/watch` as "Elsewhere" (PLAN 5.14) and can be spent on any chain Gateway
# serves with that wallet's signature.
#
#   scripts/gateway_deposit.sh check [address]     # Gateway balances, no key
#   scripts/gateway_deposit.sh deposit <USDC>      # approve + deposit on Arc testnet, signed by the key file
#
# Source (read 2026-10-14): developers.circle.com/gateway/quickstarts/unified-balance-evm
# and /gateway/references/contract-addresses. On every EVM testnet Gateway's
# wallet contract is 0x0077777d7EBA4688BDeF3E311b846F25870A19B9; Arc testnet
# is domain 26 and its USDC answers the ERC-20 interface at 0x3600…0000.
# "Do not transfer USDC directly to the Gateway Wallet contract with a
# standard ERC-20 transfer" — only deposit(token, amount) credits a balance.
#
# The key never leaves the file; `cast` reads it from the shell variable and
# nothing here prints it. Default file: ~/.tameion/treasury.key (the relayer).
set -euo pipefail

# GW_CHAIN=arc-testnet (default) | arbitrum-sepolia | base-sepolia | sepolia — the chain the USDC is on.
case "${GW_CHAIN:-arc-testnet}" in
  arc-testnet)      RPC=${TAMEION_RPC_ARC:-https://rpc.testnet.arc.network}; USDC=0x3600000000000000000000000000000000000000; DOMAIN=26 ;;
  arbitrum-sepolia) RPC=${RPC:-https://sepolia-rollup.arbitrum.io/rpc};     USDC=0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d; DOMAIN=3 ;;
  base-sepolia)     RPC=${RPC:-https://sepolia.base.org};                   USDC=0x036CbD53842c5426634e7929541eC2318f3dCF7e; DOMAIN=6 ;;
  sepolia)          RPC=${RPC:-https://ethereum-sepolia-rpc.publicnode.com}; USDC=0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238; DOMAIN=0 ;;
  *) echo "unknown GW_CHAIN ${GW_CHAIN}"; exit 1 ;;
esac
GATEWAY_WALLET=0x0077777d7EBA4688BDeF3E311b846F25870A19B9
KEY_FILE=${TAMEION_TREASURY_KEY_FILE:-$HOME/.tameion/treasury.key}
API=https://gateway-api-testnet.circle.com

balances() {
  curl -s -m 15 -X POST "$API/v1/balances" -H 'content-type: application/json' \
    -d "{\"token\":\"USDC\",\"sources\":[{\"depositor\":\"$1\"}]}" \
    | python3 -c 'import sys,json
r=json.load(sys.stdin)
for b in r["balances"]:
    if float(b["balance"]) > 0: print("  domain %3d  %s USDC" % (b["domain"], b["balance"]))
print("  total %s USDC in Circle Gateway" % sum(float(b["balance"]) for b in r["balances"]))'
}

case "${1:-}" in
  check)
    ADDR=${2:-$(cast wallet address --private-key "$(cat "$KEY_FILE")")}
    echo "Gateway balances of $ADDR:"; balances "$ADDR" ;;
  deposit)
    AMOUNT=${2:?USDC amount, e.g. 0.05}
    PK=$(cat "$KEY_FILE")
    ADDR=$(cast wallet address --private-key "$PK")
    MINOR=$(python3 -c "from decimal import Decimal; print(int(Decimal('$AMOUNT')*1000000))")
    echo "depositor $ADDR  amount $AMOUNT USDC ($MINOR minor)  chain ${GW_CHAIN:-arc-testnet} (domain $DOMAIN)"
    echo "approve:"
    cast send "$USDC" 'approve(address,uint256)' "$GATEWAY_WALLET" "$MINOR" --rpc-url "$RPC" --private-key "$PK" --json \
      | python3 -c 'import sys,json;r=json.load(sys.stdin);print("  tx",r["transactionHash"],"status",r["status"])'
    echo "deposit:"
    cast send "$GATEWAY_WALLET" 'deposit(address,uint256)' "$USDC" "$MINOR" --rpc-url "$RPC" --private-key "$PK" --json \
      | python3 -c 'import sys,json;r=json.load(sys.stdin);print("  tx",r["transactionHash"],"status",r["status"])'
    echo "Gateway balances now (finalisation may take a moment):"; balances "$ADDR" ;;
  *) sed -n 2,16p "$0"; exit 1 ;;
esac
