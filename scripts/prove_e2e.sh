#!/usr/bin/env bash
# Six months of real on-chain payment history, built in seconds, then
# rediscovered by the agent and acted on.
#
# The Rust replacement for archive/python/prove_e2e.py.
#   ./scripts/prove_e2e.sh
set -euo pipefail
# Anvil's published test mnemonic: its keys are derived when the script runs, never written into the repository.
anvil_key() { cast wallet private-key --mnemonic "test test test test test test test test test test test junk" --mnemonic-index "$1"; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export RPC_URL=http://127.0.0.1:8545
REGISTRY=0x8004A818BFB912233c491871b3d84c89A494BD9e

# anvil's published deterministic keys. In the Foundry book, worthless on
# every real network.
PRINCIPAL_PK=$(anvil_key 0)
export AGENT_PK=$(anvil_key 1)
export TREASURY_PK=$(anvil_key 2)

pkill -f 'anvil --port 8545' 2>/dev/null || true
sleep 1
anvil --port 8545 --silent > /tmp/anvil-e2e.log 2>&1 &
ANVIL_PID=$!
trap 'kill $ANVIL_PID 2>/dev/null || true' EXIT
# Keep the anvil audit record apart from the Arc one (docs/46 #1).
export TAMEION_LOG_DIR=${TAMEION_LOG_DIR:-runtime/log-anvil}
for _ in $(seq 1 30); do nc -z 127.0.0.1 8545 && break; sleep 0.3; done

export TREASURY=$(cast wallet address --private-key "$TREASURY_PK")
AGENT=$(cast wallet address --private-key "$AGENT_PK")
# Three distinct keys, as in deployment. Collapsing operator into principal
# here -- which this script did until 2026-09-25 -- makes the deploy line show
# one address twice and quietly demonstrates the two-role contract we stopped
# having on 2026-09-21. anvil key [2] is already the treasury, so this is [3].
OPERATOR_PK=$(anvil_key 3)
OPERATOR=$(cast wallet address --private-key "$OPERATOR_PK")

cd "$ROOT/contracts"
export TOKEN=$(forge create --broadcast src/TestUSDC.sol:TestUSDC \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" \
  | grep 'Deployed to:' | awk '{print $3}')
export POLICY=$(forge create --broadcast src/BudgetPolicy.sol:BudgetPolicy \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" \
  --constructor-args "$(cast wallet address --private-key "$PRINCIPAL_PK")" "$AGENT" "$OPERATOR" "$REGISTRY" 42 1000000000 5000000000 50000000 "${DELAY:-172800}" "${MODE:-2}" \
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

printf '\033[1mdeployed\033[0m\n'
echo "  TestUSDC      $TOKEN   (stand-in: a plain anvil emits no ERC-20 event for a native transfer)"
echo "  BudgetPolicy  $POLICY  (per-action 1000 USDC / daily 5000 / floor 50)"
echo "  MemoryAnchor  $ANCHOR"
echo "  treasury      $TREASURY"

# Fund the policy so it can pay: 5,000 USDC in native wei.
# An anvil account starts with exactly 10,000 ETH, so sending all of it
# leaves nothing for gas.
cast send "$POLICY" --value 5000000000000000000000 \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" > /dev/null
echo "  policy funded $(cast call "$POLICY" 'balanceUsdc()(uint256)' --rpc-url "$RPC_URL") USDC minor units"

cd "$ROOT"
cargo run --locked -p operator --quiet --features dev-anvil --example prove_e2e

echo
echo "## audit: recompute the anchored root from $TAMEION_LOG_DIR and compare with rootOf() on this chain"
AGENT=$AGENT ANCHOR=$ANCHOR RPC_URL=$RPC_URL cargo run --locked -p operator --quiet --example audit
echo "## audit, tampered copy: one canonical byte edited must fail"
cp "$TAMEION_LOG_DIR/anchors.jsonl" /tmp/e2e-anchors.bak
f="$TAMEION_LOG_DIR/$(tail -1 "$TAMEION_LOG_DIR/anchors.jsonl" | python3 -c 'import json,sys; print(json.load(sys.stdin)["file"])')"
python3 - "$f" <<'PY'
import sys,json
p=sys.argv[1]; lines=open(p).read().splitlines()
v=json.loads(lines[0]); c=v["canonical"]; v["canonical"]=c[:-1]+('0' if c[-1]!='0' else '1'); lines[0]=json.dumps(v)
open(p+".tampered","w").write("\n".join(lines)+"\n")
PY
if AUDIT_EVENTS="$f.tampered" AGENT=$AGENT ANCHOR=$ANCHOR RPC_URL=$RPC_URL cargo run --locked -p operator --quiet --example audit; then
  echo "FAIL: tampered events file passed the audit"; exit 1
else
  echo "ok: tampered events file refused (exit $?)"
fi
rm -f "$f.tampered"
