#!/usr/bin/env bash
# The core claim, on Arc itself.
#
# Everything else runs on anvil, where a native transfer emits no event and the
# clock can be fast-forwarded. Neither is true here. Arc emits a Transfer from
# 0xffff…fffe for every USDC movement, and the only way to build a rhythm is to
# wait for it -- so this writes real payments at a real cadence and then has the
# agent rediscover them with eth_getLogs.
#
# Short intervals stand in for monthly ones. The detector keys on the
# regularity of the gaps, not their length, so a payment every 40 seconds is
# the same shape of evidence as one every 30 days and costs three minutes
# instead of three months.
#
#   scripts/prove_arc.sh deploy   # contracts + funding, records addresses
#   scripts/prove_arc.sh seed     # writes the payment history (slow)
#   scripts/prove_arc.sh run      # the agent reads it back and acts
#   scripts/prove_arc.sh refuse   # what the contract refuses, on Arc
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export RPC_URL=${RPC_URL:-https://rpc.testnet.arc.network}
REGISTRY=0x8004A818BFB912233c491871b3d84c89A494BD9e
CHAIN=5042002

TREASURY_PK=$(cat ~/.tameion/treasury.key)
AGENT_PK=$(cat ~/.tameion/agent.key)
TREASURY=$(cat ~/.tameion/treasury.address)
AGENT=$(cat ~/.tameion/agent.address)

# Three counterparties. Nobody tells the agent what any of them are.
SUB=0x00000000000000000000000000000000000000A1     # fixed amount, regular
CONTRACTOR=0x00000000000000000000000000000000000000A2  # varying amount, regular
BIG=0x00000000000000000000000000000000000000A3     # over the cap, must escalate

# Six decimals. Deliberately tiny: this is a demonstration, not a treasury.
SUB_AMT=2000            # 0.002000 USDC
BIG_AMT=30000           # 0.030000 USDC
CONTRACTOR_AMTS=(5000 6200 4100 5800 5350)
PER_ACTION=10000        # 0.010000 USDC -- SUB and CONTRACTOR pass, BIG cannot
DAILY=1000000           # 1.000000 USDC
RESERVE=0
# docs/46 #3/#4: trust-expanding changes wait DELAY seconds; mode 2 = Autopilot.
# 48 h is the recommended production value. The testnet proof uses one minute:
# with 600 s, waiting out the cooling-off left a 40 s-cadence history ten
# cycles behind and the agent (correctly) escalated instead of paying
# (2026-10-13, `0x0833…FB0D`). Seed with GAP=300 so a pass can land inside the
# pay window. Both values are read back on chain by scripts/max_loss.sh.
DELAY=${DELAY:-60}
MODE=${MODE:-2}
ROUNDS=${ROUNDS:-5}
GAP=${GAP:-40}

W=1000000000000          # wei per USDC minor unit
usdc() { python3 -c "print(f'{int($1)/1e6:.6f}')"; }

# Send as the principal, surviving the public endpoint's intermittent
# "-32601 method not supported". Retries ONLY when the pending nonce shows the
# transaction never left: a retry after a transaction did leave is how one
# payment becomes two, the failure this whole project exists to prevent.
psend() {
  local before after out tx
  for attempt in 1 2 3 4 5; do
    before=$(cast nonce "$TREASURY" --block pending --rpc-url "$RPC_URL")
    if out=$(cast send --async "$@" --rpc-url "$RPC_URL" --private-key "$TREASURY_PK" 2>&1); then
      tx=$(tail -1 <<<"$out")
      for _ in $(seq 1 45); do
        cast receipt "$tx" --rpc-url "$RPC_URL" >/dev/null 2>&1 && return 0
        sleep 2
      done
      echo "    no receipt yet for $tx; not resending" >&2
      return 0
    fi
    sleep 3
    after=$(cast nonce "$TREASURY" --block pending --rpc-url "$RPC_URL")
    if [ "$after" != "$before" ]; then
      echo "    endpoint errored but the nonce moved ($before -> $after): sent once, not resending" >&2
      return 0
    fi
    echo "    attempt $attempt refused before sending ($(head -1 <<<"$out" | cut -c1-80)); retrying" >&2
  done
  return 1
}
rule() { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }
bal()  { cast balance "$1" --rpc-url "$RPC_URL" | python3 -c "import sys;print(f'{int(sys.stdin.read())/1e18:.6f}')"; }

case "${1:-}" in
deploy)
  rule "chain"
  [ "$(cast chain-id --rpc-url "$RPC_URL")" = "$CHAIN" ] || { echo "not Arc testnet"; exit 1; }
  echo "  chain $CHAIN · treasury $TREASURY ($(bal $TREASURY) USDC)"

  # The identity the policy is bound to must be ours: scripts/register_agent.sh
  # registers it and records the id. (Until 2026-10-02 this passed 42, which
  # on Arc belongs to someone else's agent.)
  AGENT_ID=$(cat runtime/arc_agent_id 2>/dev/null) || { echo "run scripts/register_agent.sh arc first"; exit 1; }
  owner=$(cast call "$REGISTRY" "ownerOf(uint256)(address)" "$AGENT_ID" --rpc-url "$RPC_URL")
  [ "$(tr 'A-F' 'a-f' <<<"$owner")" = "$(tr 'A-F' 'a-f' <<<"$TREASURY")" ] || { echo "agent $AGENT_ID is not ours ($owner)"; exit 1; }
  echo "  ERC-8004 agent $AGENT_ID, owned by the treasury"

  rule "deploy"
  cd contracts
  POLICY=$(forge create --broadcast src/BudgetPolicy.sol:BudgetPolicy \
    --rpc-url "$RPC_URL" --private-key "$TREASURY_PK" \
    --constructor-args "$TREASURY" "$AGENT" "$TREASURY" "$REGISTRY" "$AGENT_ID" $PER_ACTION $DAILY $RESERVE "$DELAY" "$MODE" \
    | grep 'Deployed to:' | awk '{print $3}')
  cd "$ROOT"
  echo "  BudgetPolicy $POLICY"
  echo "  MemoryAnchor $(cat runtime/arc_anchor.addr)  (unchanged, reused)"
  echo "$POLICY" > runtime/arc_policy.addr

  rule "fund the policy"
  cast send "$POLICY" --value $((200000 * W)) --rpc-url "$RPC_URL" --private-key "$TREASURY_PK" >/dev/null
  echo "  policy holds $(bal $POLICY) USDC"

  rule "attest the payees"
  # The agent may pay only addresses the principal attested. All three are
  # real vendors here; A3 still escalates, because it is over the cap.
  for P in "$SUB" "$CONTRACTOR" "$BIG"; do
    TX=$(cast send "$POLICY" "setPayee(address,bool)" "$P" true --rpc-url "$RPC_URL" \
      --private-key "$TREASURY_PK" --json | python3 -c "import sys,json;print(json.load(sys.stdin)['transactionHash'])")
    echo "  $P attested=$(cast call "$POLICY" 'attestedPayee(address)(bool)' "$P" --rpc-url "$RPC_URL")  tx $TX"
  done

  # History starts here. Anything older belongs to a previous deployment.
  cast block-number --rpc-url "$RPC_URL" > runtime/arc_start_block
  echo "  history starts at block $(cat runtime/arc_start_block)"
  ;;

seed)
  POLICY=$(cat runtime/arc_policy.addr)
  rule "write $ROUNDS rounds of real payment history, ${GAP}s apart"
  echo "  Payments leave the *policy*, because that is the treasury the agent"
  echo "  watches -- its own past spending is what it has to learn from. Sent"
  echo "  with withdraw(), which is the principal paying vendors directly,"
  echo "  exactly as it would have before there was an agent."
  echo "  Arc emits a Transfer from 0xffff…fffe for each one."
  echo
  # RESTART=1 starts the history the agent reads from here, on the same
  # policy, so rounds seeded before a harness fix do not count.
  if [ "${RESTART:-0}" = 1 ]; then
    cast block-number --rpc-url "$RPC_URL" > runtime/arc_start_block
    echo "  history restarts at block $(cat runtime/arc_start_block)"
  fi
  # Rounds start on a fixed schedule, t0 + (i-1)*GAP, not GAP after the
  # previous round finished. Sleeping after each round let every retry of the
  # endpoint's -32601 push all later rounds back: on 10-02 that put 15% jitter
  # on a 40 s cadence -- days, at the monthly scale the cadence stands in for --
  # and the detector, correctly, refused to call it regular.
  t0=$(date +%s)
  for i in $(seq 1 $ROUNDS); do
    idx=$(( (i-1) % ${#CONTRACTOR_AMTS[@]} ))
    c=${CONTRACTOR_AMTS[$idx]}
    psend "$POLICY" "withdraw(address,uint256)" "$SUB"        $SUB_AMT
    psend "$POLICY" "withdraw(address,uint256)" "$CONTRACTOR" $c
    psend "$POLICY" "withdraw(address,uint256)" "$BIG"        $BIG_AMT
    printf '  round %d/%d  %s -> A1   %s -> A2   %s -> A3\n' \
      "$i" "$ROUNDS" "$(usdc $SUB_AMT)" "$(usdc $c)" "$(usdc $BIG_AMT)"
    if [ "$i" -lt "$ROUNDS" ]; then
      wait=$(( t0 + i * GAP - $(date +%s) ))
      [ "$wait" -gt 0 ] && sleep "$wait"
    fi
  done
  cast block-number --rpc-url "$RPC_URL" > runtime/arc_end_block
  echo
  echo "  treasury now holds $(bal $TREASURY) USDC · policy $(bal $POLICY) USDC"
  ;;

run)
  export POLICY=$(cat runtime/arc_policy.addr)
  export ANCHOR=$(cat runtime/arc_anchor.addr)
  export FROM=$(cat runtime/arc_start_block)
  export AGENT_PK
  [ -f ~/.tameion/anthropic.key ] && export ANTHROPIC_API_KEY="$(cat ~/.tameion/anthropic.key)"
  rule "the agent reads Arc and decides"
  cargo run --locked -p operator --quiet --example run_arc

  rule "read the anchored root back off chain"
  cast call "$ANCHOR" 'rootOf(address)(bytes32)' "$AGENT" --rpc-url "$RPC_URL"
  ;;

refuse)
  POLICY=$(cat runtime/arc_policy.addr)
  # Shares the first four and last four hex characters with A1: the shape of
  # an address-poisoning look-alike.
  LOOKALIKE=0x0000ffffffffffffffffffffffffffffffff00A1
  rule "what the contract refuses, on Arc"
  refusal() { local out; out=$("$@" 2>&1) && { echo "  NOT REFUSED: $out"; return 1; }
              grep -oE 'PayeeNotAttested|OverPerActionCap|NotPrincipal|NotAgent|AlreadyExecuted|0x[0-9a-f]{8}' <<<"$out" | head -1 | sed 's/^/  refused: /'; }
  for S in "PayeeNotAttested(address)" "OverPerActionCap(uint256,uint256)" "NotPrincipal()"; do
    echo "  selector $(cast sig "$S")  $S"
  done
  printf 'agent pays a look-alike of A1 (%s)\n' "$LOOKALIKE"
  refusal cast send "$POLICY" "pay(address,uint256,bytes32)" "$LOOKALIKE" 2000 "0x$(openssl rand -hex 32)" \
    --rpc-url "$RPC_URL" --private-key "$AGENT_PK"
  printf 'agent pays A3 over the per-action cap\n'
  refusal cast send "$POLICY" "pay(address,uint256,bytes32)" "$BIG" $BIG_AMT "0x$(openssl rand -hex 32)" \
    --rpc-url "$RPC_URL" --private-key "$AGENT_PK"
  printf 'agent attests the look-alike itself\n'
  refusal cast send "$POLICY" "setPayee(address,bool)" "$LOOKALIKE" true \
    --rpc-url "$RPC_URL" --private-key "$AGENT_PK"
  echo "  look-alike attested on chain: $(cast call "$POLICY" 'attestedPayee(address)(bool)' "$LOOKALIKE" --rpc-url "$RPC_URL")"
  ;;

*) sed -n '2,17p' "$0"; exit 1 ;;
esac
