#!/usr/bin/env bash
# One full pass of the agent loop, end to end, against a local chain.
#
#   agent proposes -> verifier decides -> BudgetPolicy enforces -> log records
#   -> root anchored on chain
#
# Runs against a throwaway anvil, so it costs nothing and needs no funded
# wallet.  ./scripts/prove_operator.sh
set -euo pipefail
# Anvil's published test mnemonic: its keys are derived when the script runs, never written into the repository.
anvil_key() { cast wallet private-key --mnemonic "test test test test test test test test test test test junk" --mnemonic-index "$1"; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export RPC_URL=http://127.0.0.1:8545
REGISTRY=0x8004A818BFB912233c491871b3d84c89A494BD9e

# anvil's published deterministic keys. Documented in the Foundry book and
# worthless on every real network.
PRINCIPAL_PK=$(anvil_key 0)
export AGENT_PK=$(anvil_key 1)

rule() { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }

pkill -f 'anvil --port 8545' 2>/dev/null || true
sleep 1
anvil --port 8545 --silent > /tmp/anvil-operator.log 2>&1 &
ANVIL_PID=$!
trap 'kill $ANVIL_PID 2>/dev/null || true' EXIT
# Keep the anvil audit record apart from the Arc one (docs/46 #1).
export TAMEION_LOG_DIR=${TAMEION_LOG_DIR:-runtime/log-anvil}
for _ in $(seq 1 30); do nc -z 127.0.0.1 8545 && break; sleep 0.3; done

rule "deploy"
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
  --constructor-args "$(cast wallet address --private-key "$PRINCIPAL_PK")" "$AGENT" "$OPERATOR" "$REGISTRY" 42 100000000 500000000 50000000 "${DELAY:-172800}" "${MODE:-2}" \
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
export ANCHOR=$(forge create --broadcast \
  lib/arc-agent-kit/contracts/src/MemoryAnchor.sol:MemoryAnchor \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" \
  --constructor-args "$REGISTRY" \
  | grep 'Deployed to:' | awk '{print $3}')
echo "BudgetPolicy  $POLICY"
echo "MemoryAnchor  $ANCHOR"
echo "limits        per-action 100 USDC / daily 500 / reserve floor 50"

# 1,000 USDC, expressed in native wei.
cast send "$POLICY" --value 1000000000000000000000 \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" > /dev/null
echo "funded        $(cast call "$POLICY" 'balanceUsdc()(uint256)' --rpc-url "$RPC_URL") USDC minor units"

rule "run the loop"
cd "$ROOT"
cargo run --locked -p operator --quiet --example run_loop

rule "the root the agent anchored, read back off chain"
# rootOf is keyed on msg.sender, and the agent is who called anchor().
ONCHAIN=$(cast call "$ANCHOR" 'rootOf(address)(bytes32)' "$AGENT" --rpc-url "$RPC_URL")
echo "anchored root  $ONCHAIN"
echo
echo "The log root printed above and this value are the same number, held in"
echo "two places: the agent's own record, and a contract the agent cannot edit."
