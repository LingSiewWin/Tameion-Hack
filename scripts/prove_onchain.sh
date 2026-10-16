#!/usr/bin/env bash
# End-to-end proof that the off-chain verifier and the on-chain policy are the
# same system: the Rust verifier produces a canonical action hash, and the
# contract pays on the authority of that hash and refuses to spend it twice.
#
# Runs against a local anvil, so it costs nothing and needs no funded wallet.
#   ./scripts/prove_onchain.sh
set -euo pipefail
# Anvil's published test mnemonic: its keys are derived when the script runs, never written into the repository.
anvil_key() { cast wallet private-key --mnemonic "test test test test test test test test test test test junk" --mnemonic-index "$1"; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RPC=http://127.0.0.1:8545
VENDOR=0x1111111111111111111111111111111111111111
REGISTRY=0x8004A818BFB912233c491871b3d84c89A494BD9e

# anvil's published deterministic test keys. Documented in the Foundry book
# and worthless on every real network.
PRINCIPAL_PK=$(anvil_key 0)
AGENT_PK=$(anvil_key 1)
# Added 2026-09-25. The role split on 2026-09-21 gave the constructor a third
# address and this script was never updated, so it died on an unbound
# $OPERATOR at the deploy step. The proof it produces is the headline one;
# a broken script producing it is worse than no script.
OPERATOR_PK=$(anvil_key 2)

rule() { printf '\n== %s ==\n' "$1"; }
refusal() { # run a command that must revert, print only the decoded error
  local out
  out=$("$@" 2>&1 || true)
  echo "$out" | grep -oE '[A-Z][A-Za-z]+\([^)]*\)|NotPrincipal|NotOperator|NotAgent' | tail -1
}

pkill -f 'anvil --port 8545' 2>/dev/null || true
sleep 1
anvil --port 8545 --silent > /tmp/anvil-proof.log 2>&1 &
ANVIL_PID=$!
trap 'kill $ANVIL_PID 2>/dev/null || true' EXIT
# Keep the anvil audit record apart from the Arc one (docs/46 #1).
export TAMEION_LOG_DIR=${TAMEION_LOG_DIR:-runtime/log-anvil}
for _ in $(seq 1 30); do nc -z 127.0.0.1 8545 && break; sleep 0.3; done

echo "anvil chainId $(cast chain-id --rpc-url $RPC)"

rule "the verifier decides, off chain"
eval "$(cd "$ROOT/ledger" && cargo run --locked --quiet --example emit_hash | sed 's/^/export /')"
echo "decision     $DECISION"
echo "action hash  $HASH"
echo "amount       $AMOUNT_MINOR USDC minor units"

rule "deploy the policy"
AGENT=$(cast wallet address --private-key $AGENT_PK)
OPERATOR=$(cast wallet address --private-key $OPERATOR_PK)
POLICY=$(cd "$ROOT/contracts" && forge create --broadcast src/BudgetPolicy.sol:BudgetPolicy \
  --rpc-url $RPC --private-key $PRINCIPAL_PK \
  --constructor-args "$(cast wallet address --private-key $PRINCIPAL_PK)" "$AGENT" "$OPERATOR" "$REGISTRY" 42 100000000 500000000 50000000 "${DELAY:-172800}" "${MODE:-2}" \
  | grep 'Deployed to:' | awk '{print $3}')
echo "policy       $POLICY"
echo "agent        $AGENT"
echo "operator     $OPERATOR"
echo "principal    $(cast wallet address --private-key $PRINCIPAL_PK)"
echo "limits       per-action 100 USDC / daily 500 / reserve floor 50"

cast send "$POLICY" --value 1000000000000000000000 --rpc-url $RPC --private-key $PRINCIPAL_PK > /dev/null
echo "funded       $(cast call "$POLICY" 'balanceUsdc()(uint256)' --rpc-url $RPC) USDC minor units"
# The agent may pay only addresses the principal attested.
cast send "$POLICY" "setPayee(address,bool)" $VENDOR true --rpc-url $RPC --private-key $PRINCIPAL_PK > /dev/null
# docs/46 #3: a payee attested just now is not payable for DELAY seconds.
cast rpc evm_increaseTime "${DELAY:-172800}" --rpc-url $RPC >/dev/null; cast rpc evm_mine --rpc-url $RPC >/dev/null
echo "attested     $VENDOR $(cast call "$POLICY" 'attestedPayee(address)(bool)' $VENDOR --rpc-url $RPC)"

rule "pay, on the authority of that hash"
TX=$(cast send "$POLICY" "pay(address,uint256,bytes32)" $VENDOR "$AMOUNT_MINOR" "$HASH" \
  --rpc-url $RPC --private-key $AGENT_PK --json)
python3 - "$TX" <<'PY'
import json, sys
r = json.loads(sys.argv[1])
print(f"status       {r['status']}")
print(f"tx           {r['transactionHash']}")
print(f"gas used     {int(r['gasUsed'], 16)}")
PY
echo "vendor       $(cast balance $VENDOR --rpc-url $RPC) wei"
echo "policy       $(cast call "$POLICY" 'balanceUsdc()(uint256)' --rpc-url $RPC) USDC minor units"
echo "in window    $(cast call "$POLICY" 'spentInWindow()(uint256)' --rpc-url $RPC)"
echo "recorded     $(cast call "$POLICY" 'executed(bytes32)(bool)' "$HASH" --rpc-url $RPC)"

rule "what it refuses"
printf 'replay the same hash          -> '
refusal cast send "$POLICY" "pay(address,uint256,bytes32)" $VENDOR "$AMOUNT_MINOR" "$HASH" \
  --rpc-url $RPC --private-key $AGENT_PK
printf 'over the per-action cap       -> '
refusal cast send "$POLICY" "pay(address,uint256,bytes32)" $VENDOR 150000000 \
  "0x$(openssl rand -hex 32)" --rpc-url $RPC --private-key $AGENT_PK
printf 'an unattested payee           -> '
refusal cast send "$POLICY" "pay(address,uint256,bytes32)" 0x1111000000000000000000000000000000001111 1000000 \
  "0x$(openssl rand -hex 32)" --rpc-url $RPC --private-key $AGENT_PK
printf 'agent raising its own cap     -> '
refusal cast send "$POLICY" "setLimits(uint256,uint256,uint256)" 999000000 999000000 0 \
  --rpc-url $RPC --private-key $AGENT_PK
printf 'a stranger paying             -> '
refusal cast send "$POLICY" "pay(address,uint256,bytes32)" $VENDOR 1000000 \
  "0x$(openssl rand -hex 32)" --rpc-url $RPC --private-key $PRINCIPAL_PK
# The role split is only real if each role is refused the others' powers.
printf 'operator withdrawing          -> '
refusal cast send "$POLICY" "withdraw(address,uint256)" $VENDOR 1000000 \
  --rpc-url $RPC --private-key $OPERATOR_PK
printf 'principal rotating the agent  -> '
refusal cast send "$POLICY" "setAgent(address)" $VENDOR \
  --rpc-url $RPC --private-key $PRINCIPAL_PK

rule "state after the refusals"
echo "policy       $(cast call "$POLICY" 'balanceUsdc()(uint256)' --rpc-url $RPC) USDC minor units"
echo "spentInWindow $(cast call "$POLICY" 'spentInWindow()(uint256)' --rpc-url $RPC)"
echo "perActionCap $(cast call "$POLICY" 'perActionCap()(uint256)' --rpc-url $RPC)"
echo
echo "Nothing moved. Six attempts, six named refusals."
