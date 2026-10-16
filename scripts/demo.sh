#!/usr/bin/env bash
# The interactive demo. Starts a throwaway chain, deploys the contracts,
# funds the treasury, and serves a page with a button that actually runs the
# agent. Every press produces new, verifiable transactions.
#
#   scripts/demo.sh        then open http://127.0.0.1:7878
set -euo pipefail
# Anvil's published test mnemonic: its keys are derived when the script runs, never written into the repository.
anvil_key() { cast wallet private-key --mnemonic "test test test test test test test test test test test junk" --mnemonic-index "$1"; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export RPC_URL=http://127.0.0.1:8545
REGISTRY=0x8004A818BFB912233c491871b3d84c89A494BD9e
PRINCIPAL_PK=$(anvil_key 0)
export AGENT_PK=$(anvil_key 1)

# The key never goes in the repo or the environment of anything but this run.
if [ -f "$HOME/.tameion/anthropic.key" ]; then
  export ANTHROPIC_API_KEY="$(cat "$HOME/.tameion/anthropic.key")"
fi

pkill -f 'anvil --port 8545' 2>/dev/null || true
sleep 1
anvil --port 8545 --silent > /tmp/anvil-demo.log 2>&1 &
ANVIL_PID=$!
trap 'kill $ANVIL_PID 2>/dev/null || true' EXIT
# Keep the anvil audit record apart from the Arc one (docs/46 #1).
export TAMEION_LOG_DIR=${TAMEION_LOG_DIR:-runtime/log-anvil}
for _ in $(seq 1 40); do nc -z 127.0.0.1 8545 && break; sleep 0.3; done

export TREASURY_PK=$(anvil_key 2)
export TREASURY=$(cast wallet address --private-key "$TREASURY_PK")
AGENT=$(cast wallet address --private-key "$AGENT_PK")
# Three distinct keys, as in deployment. Collapsing operator into principal
# here -- which this script did until 2026-09-25 -- makes the deploy line show
# one address twice and quietly demonstrates the two-role contract we stopped
# having on 2026-09-21. anvil key [2] is already the treasury, so this is [3].
OPERATOR_PK=$(anvil_key 3)
OPERATOR=$(cast wallet address --private-key "$OPERATOR_PK")
cd "$ROOT/contracts"
export POLICY=$(forge create --broadcast src/BudgetPolicy.sol:BudgetPolicy \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" \
  --constructor-args "$(cast wallet address --private-key "$PRINCIPAL_PK")" "$AGENT" "$OPERATOR" "$REGISTRY" 42 1000000000 50000000000 50000000 "${DELAY:-172800}" "${MODE:-2}" \
  | grep 'Deployed to:' | awk '{print $3}')
# The agent may pay only addresses the principal attested: the three real
# vendors in the seeded history. 0x4444 is left out on purpose; it is the one
# the agent must escalate.
for P in 0x1111111111111111111111111111111111111111 0x2222222222222222222222222222222222222222 \
         0x3333333333333333333333333333333333333333; do
  cast send "$POLICY" "setPayee(address,bool)" "$P" true --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null
done
# docs/46 #3: a payee attested just now is not payable for DELAY seconds, so
# move the local chain's clock past it before the agent's first pass.
cast rpc evm_increaseTime "${DELAY:-172800}" --rpc-url "$RPC_URL" >/dev/null
cast rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
# Stand-in USDC. A local EVM emits no event for a native transfer, so without
# this there is no history for the agent to read and the core claim of the
# project cannot be demonstrated at all.
export TOKEN=$(forge create --broadcast src/TestUSDC.sol:TestUSDC \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" \
  | grep 'Deployed to:' | awk '{print $3}')
export ANCHOR=$(forge create --broadcast \
  lib/arc-agent-kit/contracts/src/MemoryAnchor.sol:MemoryAnchor \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" \
  --constructor-args "$REGISTRY" \
  | grep 'Deployed to:' | awk '{print $3}')

# 5,000 USDC. anvil funds each account with 10,000 ETH, so this leaves gas.
cast send "$POLICY" --value 5000000000000000000000 \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" > /dev/null

cd "$ROOT"
echo "writing real payment history to chain..."
cargo run --locked -p operator --quiet --example seed_history --features dev-anvil

echo "  treasury $TREASURY"
echo "  token    $TOKEN"
exec cargo run --locked -p operator --quiet --example serve
