#!/usr/bin/env bash
# Register the agent in the ERC-8004 IdentityRegistry and record the id it gets.
#
# Every policy stores (identityRegistry, agentId). Until 2026-10-02 the scripts
# passed agentId 42, which on Arc and on Arbitrum Sepolia is someone else's
# agent. A policy is only bound to an identity if the identity is ours, so the
# id comes from this registration and nowhere else.
#
#   scripts/register_agent.sh arc       # Arc testnet, chain 5042002
#   scripts/register_agent.sh arbsep    # Arbitrum Sepolia, chain 421614
#
# The registration file is a base64 data: URI, which ERC-8004 allows "for fully
# on-chain metadata": nothing to host, nothing to go stale. The treasury key
# registers, so the treasury owns the identity.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
REGISTRY=0x8004A818BFB912233c491871b3d84c89A494BD9e
REGISTERED_TOPIC=$(cast keccak "Registered(uint256,string,address)")

case "${1:-}" in
  arc)    RPC_URL=${RPC_URL:-https://rpc.testnet.arc.network};   CHAIN=5042002 ;;
  arbsep) RPC_URL=${RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}; CHAIN=421614 ;;
  *) echo "usage: $0 arc|arbsep" >&2; exit 2 ;;
esac
NET=$1
OUT=runtime/${NET}_agent_id

if [[ -s $OUT ]]; then
  echo "already registered on $NET: agent $(cat "$OUT") (delete $OUT to register again)"
  exit 0
fi

TREASURY_PK=$(cat ~/.tameion/treasury.key)
TREASURY=$(cat ~/.tameion/treasury.address)
AGENT=$(cat ~/.tameion/agent.address)

[[ $(cast chain-id --rpc-url "$RPC_URL") == "$CHAIN" ]] || { echo "RPC is not chain $CHAIN" >&2; exit 1; }
[[ $(cast codesize "$REGISTRY" --rpc-url "$RPC_URL") -gt 0 ]] || { echo "no registry at $REGISTRY on $NET" >&2; exit 1; }

# The four fields ERC-8004 makes mandatory -- type, name, description, image --
# plus the agent's paying address, so a reader can join the identity to the
# account that acts under it.
IMAGE="data:image/svg+xml;base64,$(printf '%s' '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="12" fill="#0f6b5c"/><text x="32" y="42" font-family="Georgia,serif" font-size="30" text-anchor="middle" fill="#fff">T</text></svg>' | base64 | tr -d '\n')"
JSON=$(python3 - "$AGENT" "$IMAGE" "$CHAIN" <<'EOF'
import json, sys
agent, image, chain = sys.argv[1], sys.argv[2], sys.argv[3]
print(json.dumps({
    "type": "https://eips.ethereum.org/EIPS/eip-8004#registration-v1",
    "name": "Tameion payout agent",
    "description": ("Pays its owner's recurring obligations in USDC from a contract-enforced "
                    "budget. Every decision is logged and anchored on chain; anything below the "
                    "owner's confidence floor or outside the budget is escalated, not paid."),
    "image": image,
    "services": [{"name": "wallet", "endpoint": f"eip155:{chain}:{agent}"}],
    "x402Support": False,
    "active": True,
}, separators=(",", ":")))
EOF
)
URI="data:application/json;base64,$(printf '%s' "$JSON" | base64 | tr -d '\n')"

echo "registering on $NET as $TREASURY ..."
RECEIPT=$(cast send "$REGISTRY" "register(string)" "$URI" \
  --rpc-url "$RPC_URL" --private-key "$TREASURY_PK" --json)
TX=$(jq -r .transactionHash <<<"$RECEIPT")
[[ $(jq -r .status <<<"$RECEIPT") == "0x1" ]] || { echo "registration reverted: $TX" >&2; exit 1; }

# The id is the first indexed topic of the registry's Registered event.
ID_HEX=$(jq -r --arg r "$(tr 'A-F' 'a-f' <<<"$REGISTRY")" --arg t "$REGISTERED_TOPIC" \
  '.logs[] | select((.address|ascii_downcase)==$r and .topics[0]==$t) | .topics[1]' <<<"$RECEIPT")
[[ -n $ID_HEX ]] || { echo "no Registered event in $TX" >&2; exit 1; }
ID=$(cast to-dec "$ID_HEX")

OWNER=$(cast call "$REGISTRY" "ownerOf(uint256)(address)" "$ID" --rpc-url "$RPC_URL")
[[ $(tr 'A-F' 'a-f' <<<"$OWNER") == $(tr 'A-F' 'a-f' <<<"$TREASURY") ]] \
  || { echo "agent $ID is owned by $OWNER, not the treasury" >&2; exit 1; }

echo "$ID" > "$OUT"
echo "agent $ID on $NET, owned by $OWNER"
echo "tx $TX"
