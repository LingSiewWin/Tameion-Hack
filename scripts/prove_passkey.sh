#!/usr/bin/env bash
# Only the owner's passkey can change who the agent may pay.
#
# The policy's principal is a PasskeyPrincipal: a contract whose only key is a
# P-256 passkey held in a phone or a laptop's secure enclave. The treasury EOA
# here is a relayer -- it pays gas and holds no authority. The page in
# apps/classic/passkey signs one exact call; this script turns that signature into a
# transaction and then tries every way around it.
#
#   scripts/prove_passkey.sh <net> preflight             # what is missing, before any gas is spent
#   scripts/prove_passkey.sh <net> serve                 # the signing page, http://localhost:8787
#   scripts/prove_passkey.sh <net> deploy <qx> <qy>      # passkey owner + USDC policy
#   scripts/prove_passkey.sh <net> challenge <payee>     # 32 bytes for the page to sign
#   scripts/prove_passkey.sh <net> submit <auth.json>    # relay the signed approval
#   scripts/prove_passkey.sh <net> refuse                # replay, re-point, bypass, unattested
#   scripts/prove_passkey.sh <net> pay <payee> <units>   # the agent pays, inside the budget
#   scripts/prove_passkey.sh <net> history               # the owner's old wallet pays the bills first
#   scripts/prove_passkey.sh <net> run                   # the agent's pass: pays, or asks the passkey
#
#   net = local   anvil on :8546 with a six-decimal test token (anvil's keys)
#         arbsep  Arbitrum Sepolia, Circle's USDC, ~/.tameion keys
#         arc     Arc testnet, where USDC is the native asset (BudgetPolicy)
#
# Arbitrum Sepolia and Arc testnet verify P-256 in a precompile at 0x100 (RIP-7212);
# OpenZeppelin's P256 uses it when present and falls back to Solidity when not,
# so the same contract runs on both nets.
set -euo pipefail
# Anvil's published test mnemonic: its keys are derived when the script runs, never written into the repository.
anvil_key() { cast wallet private-key --mnemonic "test test test test test test test test test test test junk" --mnemonic-index "$1"; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
mkdir -p runtime/passkey

NET=${1:-}
CMD=${2:-}
case "$NET" in
  local)
    RPC_URL=${RPC_URL:-http://127.0.0.1:8546}
    CHAIN=31337
    # anvil's published development keys 0 and 1: test values, no funds.
    RELAYER_PK=$(anvil_key 0)
    AGENT_PK=$(anvil_key 1)
    # anvil key 2 plays the wallet the owner paid bills from before the agent.
    WALLET_PK=$(anvil_key 2)
    ;;
  arbsep)
    RPC_URL=${RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}
    CHAIN=421614
    # Circle's USDC on Arbitrum Sepolia, from developers.circle.com/stablecoins/
    # usdc-contract-addresses; symbol USDC and 6 decimals checked on chain 2026-10-02.
    USDC=0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d
    RELAYER_PK=$(cat ~/.tameion/treasury.key)
    AGENT_PK=$(cat ~/.tameion/agent.key)
    # The treasury EOA plays the owner's old wallet: it pays the bills by
    # hand first, so the agent has someone's history to learn from.
    WALLET_PK=$RELAYER_PK
    ;;
  arc)
    RPC_URL=${RPC_URL:-https://rpc.testnet.arc.network}
    CHAIN=5042002
    # USDC is Arc's native asset: the policy is the native BudgetPolicy and
    # money moves as value. Amounts are thousandths, as in prove_arc.sh.
    USDC=native
    RELAYER_PK=$(cat ~/.tameion/treasury.key)
    AGENT_PK=$(cat ~/.tameion/agent.key)
    WALLET_PK=$RELAYER_PK
    PER_ACTION=${PER_ACTION:-10000}     # 0.01 USDC
    DAILY=${DAILY:-100000}              # 0.10 USDC per 24 hours
    SUB_AMT=${SUB_AMT:-3000}; NEWP_AMT=${NEWP_AMT:-2000}; GAP=${GAP:-180}
    FUND=${FUND:-50000}                 # 0.05 USDC into the policy
    ;;
  *) sed -n '2,26p' "$0"; exit 2 ;;
esac
RELAYER=$(cast wallet address --private-key "$RELAYER_PK")
AGENT=$(cast wallet address --private-key "$AGENT_PK")
STATE=runtime/passkey/$NET.env
REGISTRY=0x8004A818BFB912233c491871b3d84c89A494BD9e

# Limits in USDC minor units. Small on purpose: a testnet faucet's worth.
PER_ACTION=${PER_ACTION:-2000000}   # 2 USDC
DAILY=${DAILY:-5000000}             # 5 USDC per 24 hours
RESERVE=${RESERVE:-0}
# docs/46 #3/#4: trust-expanding changes wait DELAY seconds; MODE 2 = Autopilot.
# Local anvil can wear the recommended 48 h because the script moves its clock;
# on a live testnet the loop below approves and pays within minutes, so the
# delay there is one minute and the proof says so.
DELAY=${DELAY:-$([[ $NET == local ]] && echo 172800 || echo 60)}
MODE=${MODE:-2}
# docs/46 (2026-10-13): the passkey can be replaced from a recovery address
# after RECOVERY_DELAY, and the phone can cancel in the meantime. The proof
# uses the relayer as the recovery address and says so; a real owner names a
# hardware wallet or a second phone. Same testnet/local split as DELAY.
RECOVERY=${RECOVERY:-$RELAYER}
RECOVERY_DELAY=${RECOVERY_DELAY:-$([[ $NET == local ]] && echo 604800 || echo 60)}

rule() { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }
lc()   { tr 'A-F' 'a-f' <<<"$1"; }
usdc() { python3 -c "print(f'{int(\"$1\")/1e6:.6f}')"; }
need_state() { [[ -s $STATE ]] || { echo "no deployment on $NET yet: run '$0 $NET deploy <qx> <qy>'" >&2; exit 1; }
               # shellcheck disable=SC1090
               source "$STATE"; }
tx_of() { jq -r .transactionHash; }
balance() {
  if [[ $USDC == native ]]; then python3 -c "print($(cast balance "$1" --rpc-url "$RPC_URL") // 10**12)"
  else cast call "$USDC" "balanceOf(address)(uint256)" "$1" --rpc-url "$RPC_URL" | awk '{print $1}'; fi
}

# cast send that survives Arc's intermittent "-32601 method not supported",
# retrying only while the pending nonce proves nothing left: a retry after a
# transaction did leave is how one payment becomes two. Prints the tx hash.
xsend() {
  local pk=$1 from before out; shift
  from=$(cast wallet address --private-key "$pk")
  for _ in 1 2 3 4 5 6; do
    before=$(cast nonce "$from" --block pending --rpc-url "$RPC_URL")
    if out=$(cast send "$@" --rpc-url "$RPC_URL" --private-key "$pk" --json 2>&1); then
      jq -r .transactionHash <<<"$out"; return 0
    fi
    [[ $(cast nonce "$from" --block pending --rpc-url "$RPC_URL") == "$before" ]] \
      || { echo "a send left but its result was lost; not retrying: ${out:0:200}" >&2; return 1; }
    grep -qE -- "-32601|method not supported" <<<"$out" || { echo "${out:0:300}" >&2; return 1; }
    sleep 2
  done
  return 1
}

# A refusal is a revert at estimation against live state: nothing is mined,
# nothing is spent, and the error is the contract's own.
refusal() {
  local out
  if out=$("$@" 2>&1); then echo "  NOT REFUSED: $out"; return 1; fi
  grep -oE 'BadPasskeySignature|PayeeNotAttested|NotPrincipal|NotAgent|OverPerActionCap|OverDailyCap|InsufficientBalance|AlreadyExecuted' <<<"$out" \
    | head -1 | sed 's/^/  refused: /'
}

[[ $CMD == serve || $CMD == preflight ]] || [[ $(cast chain-id --rpc-url "$RPC_URL") == "$CHAIN" ]] \
  || { echo "$RPC_URL is not chain $CHAIN" >&2; exit 1; }

case "$CMD" in
preflight)
  # Read-only. Each line is ok or says what to do; the exit code is the
  # number of things missing.
  missing=0
  ok()   { printf '  \033[32mok\033[0m    %s\n' "$1"; }
  need() { printf '  \033[31mneed\033[0m  %s\n' "$1"; missing=$((missing + 1)); }
  rule "preflight: $NET"
  for t in cast forge cargo jq python3; do
    command -v "$t" >/dev/null && ok "$t installed" || need "$t: install it (Foundry for cast/forge)"
  done
  got=$(cast chain-id --rpc-url "$RPC_URL" 2>/dev/null || echo "?")
  [[ $got == "$CHAIN" ]] && ok "RPC $RPC_URL is chain $CHAIN" || need "RPC $RPC_URL answered chain $got, not $CHAIN"
  # This page's real signature (proof/passkey-local-2026-10-02.txt section 4):
  # 1 from a working P-256 precompile, empty from a chain without one.
  P256_VECTOR=0xdaf5fd5fa59630b472aef89dcb3b1fe47b6436a94867c5bcf903745ce864f0477fe1bddc77e6027957c2091a13f93e0ea8ebfe56a85c9f221ae7b1ae336a6269591826797f0cc45823bf2cbfd3f3d8972402c1af0b8ee09557e96c38d9dd7b93e8d08e6f2293cb4c5f3d3a4432421642784991b679859e4dfdb306e817202dbb04b1ca27beba212d4f1a83567e31d293d4169507ed11b2ba7db4c6625d0c31aa
  if [[ $NET != local ]]; then
    [[ $(cast call 0x0000000000000000000000000000000000000100 "$P256_VECTOR" --rpc-url "$RPC_URL" 2>/dev/null) == 0x0000000000000000000000000000000000000000000000000000000000000001 ]] \
      && ok "P-256 precompile verifies a real passkey signature" \
      || need "no P-256 precompile answer at 0x100 (OpenZeppelin falls back to Solidity: works, costs more)"
    [[ $(cast codesize "$REGISTRY" --rpc-url "$RPC_URL" 2>/dev/null || echo 0) -gt 0 ]] \
      && ok "ERC-8004 registry at $REGISTRY" || need "no ERC-8004 registry at $REGISTRY on this chain"
    if [[ -s runtime/${NET}_agent_id ]]; then
      id=$(cat "runtime/${NET}_agent_id")
      owner=$(cast call "$REGISTRY" "ownerOf(uint256)(address)" "$id" --rpc-url "$RPC_URL" 2>/dev/null || echo "?")
      [[ $(lc "$owner") == $(lc "$RELAYER") ]] && ok "agent identity $id, owned by the treasury" \
        || need "runtime/${NET}_agent_id says $id but its owner is $owner: run scripts/register_agent.sh $NET again"
    else
      need "no agent identity on $NET yet: scripts/register_agent.sh $NET"
    fi
  fi
  gas() { python3 -c "print($(cast balance "$1" --rpc-url "$RPC_URL" 2>/dev/null || echo 0) / 1e18)"; }
  # Measured on the fork rehearsal: the treasury spent 0.0000935 ETH on 19
  # transactions; the agent 0.0000013 on 6. Ask for ten times that.
  min_relayer=0.001; min_agent=0.0001
  [[ $USDC == native ]] && { min_relayer=0.3; min_agent=0.01; }   # Arc: gas is USDC, 25 gwei
  r=$(gas "$RELAYER"); a=$(gas "$AGENT")
  python3 -c "import sys; sys.exit(0 if $r >= $min_relayer else 1)" && ok "treasury $RELAYER has $r native for gas" \
    || need "treasury $RELAYER has $r native; send it at least $min_relayer (faucet; it needs a human)"
  python3 -c "import sys; sys.exit(0 if $a >= $min_agent else 1)" && ok "agent $AGENT has $a native for gas" \
    || need "agent $AGENT has $a native; send it at least $min_agent"
  if [[ $NET == arbsep ]]; then
    want=$(( 6 * (200000 + 150000) + 3000000 ))   # history + funding the policy
    have=$(balance "$RELAYER")
    (( have >= want )) && ok "treasury holds $(usdc "$have") USDC" \
      || need "treasury holds $(usdc "$have") USDC; get at least $(usdc "$want") from faucet.circle.com (Arbitrum Sepolia)"
  fi
  curl -s -o /dev/null -w '%{http_code}' http://localhost:8787/call.js 2>/dev/null | grep -q 200 \
    && ok "signing page served at http://localhost:8787" \
    || need "signing page not running: '$0 $NET serve' in another terminal"
  if [[ -s $STATE ]]; then ok "existing deployment recorded in $STATE (deploy again only to start over)"; fi
  echo; (( missing == 0 )) && echo "  ready" || echo "  $missing thing(s) to do first"
  exit "$missing"
  ;;

serve)
  echo "signing page: http://localhost:8787  (Ctrl-C to stop)"
  echo "WebAuthn needs a secure context; localhost is one, a LAN address is not."
  exec python3 -m http.server 8787 --bind 127.0.0.1 -d apps/classic/passkey
  ;;

deploy)
  QX=${3:?qx}; QY=${4:?qy}
  rule "chain $CHAIN"
  if [[ $NET == local ]]; then
    AGENT_ID=0  # no registry on anvil; 0 is never a minted ERC-8004 id
    cd contracts
    # --constructor-args is variadic: it goes last or it swallows the flags.
    USDC=$(forge create --broadcast test/BudgetPolicyToken.t.sol:TestUsdc \
      --rpc-url "$RPC_URL" --private-key "$RELAYER_PK" --constructor-args 6 | awk '/Deployed to:/{print $3}')
    cd "$ROOT"
    echo "  test token $USDC (six decimals)"
  else
    AGENT_ID=$(cat runtime/${NET}_agent_id 2>/dev/null) \
      || { echo "run scripts/register_agent.sh $NET first" >&2; exit 1; }
    owner=$(cast call "$REGISTRY" "ownerOf(uint256)(address)" "$AGENT_ID" --rpc-url "$RPC_URL")
    [[ $(lc "$owner") == $(lc "$RELAYER") ]] || { echo "agent $AGENT_ID is not ours ($owner)" >&2; exit 1; }
    echo "  ERC-8004 agent $AGENT_ID, owned by the treasury"
    echo "  relayer $RELAYER holds $(cast balance "$RELAYER" --rpc-url "$RPC_URL" -e) $([[ $USDC == native ]] && echo "USDC (native gas)" || echo ETH)"
  fi

  START=$(cast block-number --rpc-url "$RPC_URL")
  rule "deploy: the passkey owns the policy"
  cd contracts
  if [[ $USDC == native ]]; then
    OUT=$(forge script script/PasskeyPolicy.s.sol \
      --sig "deploy(bytes32,bytes32,address,uint256,address,address,uint256,uint256,uint256,uint256,uint256,uint8)" \
      "$QX" "$QY" "$RECOVERY" "$RECOVERY_DELAY" "$AGENT" "$RELAYER" "$AGENT_ID" $PER_ACTION $DAILY $RESERVE "$DELAY" "$MODE" \
      --rpc-url "$RPC_URL" --private-key "$RELAYER_PK" --broadcast --slow 2>&1)
  else
    OUT=$(forge script script/PasskeyPolicy.s.sol \
      --sig "deployToken(address,bytes32,bytes32,address,uint256,address,address,uint256,uint256,uint256,uint256,uint256,uint8)" \
      "$USDC" "$QX" "$QY" "$RECOVERY" "$RECOVERY_DELAY" "$AGENT" "$RELAYER" "$AGENT_ID" $PER_ACTION $DAILY $RESERVE "$DELAY" "$MODE" \
      --rpc-url "$RPC_URL" --private-key "$RELAYER_PK" --broadcast 2>&1)
  fi
  cd "$ROOT"
  OWNER=$(awk '/^ +owner /{print $2}' <<<"$OUT")
  POLICY=$(awk '/^ +policy /{print $2}' <<<"$OUT")
  [[ -n $OWNER && -n $POLICY ]] || { echo "$OUT" >&2; exit 1; }
  # Where the agent anchors its log root. MemoryAnchor is vendored from
  # arc-agent-kit under contracts/lib.
  if [[ $NET == arc && -s runtime/arc_anchor.addr ]]; then
    ANCHOR=$(cat runtime/arc_anchor.addr)   # the one prove_arc.sh uses; anchors are keyed by sender
  else
    cd contracts
    ANCHOR=$(forge create --broadcast lib/arc-agent-kit/contracts/src/MemoryAnchor.sol:MemoryAnchor \
      --rpc-url "$RPC_URL" --private-key "$RELAYER_PK" --constructor-args "$REGISTRY" | awk '/Deployed to:/{print $3}')
    cd "$ROOT"
  fi
  printf 'USDC=%s\nOWNER=%s\nPOLICY=%s\nANCHOR=%s\nSTART=%s\nAGENT_ID=%s\nQX=%s\nQY=%s\n' \
    "$USDC" "$OWNER" "$POLICY" "$ANCHOR" "$START" "$AGENT_ID" "$QX" "$QY" > "$STATE"
  echo "  PasskeyPrincipal  $OWNER  (qx ${QX:0:10}…, qy ${QY:0:10}…)"
  echo "  $([[ $USDC == native ]] && echo "BudgetPolicy     " || echo "BudgetPolicyToken") $POLICY"
  echo "  principal()       $(cast call "$POLICY" 'principal()(address)' --rpc-url "$RPC_URL")"
  echo "  operator()        $(cast call "$POLICY" 'operator()(address)' --rpc-url "$RPC_URL")  (the relayer: limits, not payees)"
  echo "  agent()           $(cast call "$POLICY" 'agent()(address)' --rpc-url "$RPC_URL")"
  echo "  agentId()         $(cast call "$POLICY" 'agentId()(uint256)' --rpc-url "$RPC_URL")"
  [[ $USDC == native ]] || echo "  usdc()            $(cast call "$POLICY" 'usdc()(address)' --rpc-url "$RPC_URL")"
  echo "  MemoryAnchor      $ANCHOR"
  if [[ $NET == local ]]; then
    cast send "$USDC" "mint(address,uint256)" "$POLICY" 20000000 --rpc-url "$RPC_URL" --private-key "$RELAYER_PK" >/dev/null
    echo "  minted 20 test USDC to the policy"
  elif [[ $USDC == native ]]; then
    tx=$(xsend "$RELAYER_PK" "$POLICY" --value $(( FUND * 10**12 )))
    echo "  funded the policy with $(usdc "$FUND") USDC: tx $tx"
  else
    echo "  fund it: send Circle testnet USDC to $POLICY (faucet.circle.com, Arbitrum Sepolia)"
  fi
  ;;

challenge)
  need_state
  # One payee address, or the calldata of a batch the agent printed
  # (`setPayees(address[],bool)`, the "calldata" line of `run`).
  ARG=${3:?payee address or setPayees calldata}
  if [[ ${#ARG} -eq 42 ]]; then
    DATA=$(cast calldata "setPayee(address,bool)" "$ARG" true)
    PAYEES=$ARG
  else
    DATA=$ARG
    [[ ${DATA:0:10} == $(cast sig "setPayees(address[],bool)") ]] || { echo "not setPayees calldata" >&2; exit 2; }
    PAYEES=$(cast calldata-decode "setPayees(address[],bool)" "$DATA" | head -1 | tr -d '[] ' )
  fi
  N=$(cast call "$OWNER" "nonce()(uint256)" --rpc-url "$RPC_URL")
  C=$(cast call "$OWNER" "challengeFor(address,uint256,bytes,uint256)(bytes32)" "$POLICY" 0 "$DATA" "$N" --rpc-url "$RPC_URL")
  printf 'PENDING_PAYEES=%s\nPENDING_DATA=%s\n' "$PAYEES" "$DATA" > "runtime/passkey/$NET.pending"
  echo "approve: allow $PAYEES on $POLICY, approval #$N"
  echo "challenge: $C"
  # The call itself goes in the link; the page shows it and recomputes $C.
  echo "open http://localhost:8787/?owner=$OWNER&chain=$CHAIN&n=$N&to=$POLICY&data=$DATA"
  echo "  (with '$0 $NET serve' running), check what it says, approve, press Copy, then:"
  echo "  pbpaste > runtime/passkey/$NET-auth.json && $0 $NET submit runtime/passkey/$NET-auth.json"
  ;;

submit)
  need_state
  AUTH=${3:?auth.json}
  # shellcheck disable=SC1090
  source "runtime/passkey/$NET.pending"
  rule "relay the passkey's approval"
  attested() { for p in ${PENDING_PAYEES//,/ }; do
                 echo "  $1 attestedPayee($p) = $(cast call "$POLICY" 'attestedPayee(address)(bool)' "$p" --rpc-url "$RPC_URL")"; done; }
  attested "before:"
  cd contracts
  OUT=$(forge script script/PasskeyPolicy.s.sol --sig "submit(address,address,bytes,string)" \
    "$OWNER" "$POLICY" "$PENDING_DATA" "../$AUTH" \
    --rpc-url "$RPC_URL" --private-key "$RELAYER_PK" --broadcast 2>&1) || { echo "$OUT" | tail -5 >&2; exit 1; }
  cd "$ROOT"
  TX=$(jq -r '.transactions[0].hash' "contracts/broadcast/PasskeyPolicy.s.sol/$CHAIN/submit-latest.json")
  attested "after: "
  echo "  tx $TX"
  echo "  approvals used: $(cast call "$OWNER" 'nonce()(uint256)' --rpc-url "$RPC_URL")"
  cp "$AUTH" "runtime/passkey/$NET-last-auth.json"
  cp "runtime/passkey/$NET.pending" "runtime/passkey/$NET.last"
  ;;

refuse)
  need_state
  # shellcheck disable=SC1090
  source "runtime/passkey/$NET.last"
  OTHER=0x000000000000000000000000000000000000dEaD
  OTHER_DATA=$(cast calldata "setPayee(address,bool)" "$OTHER" true)
  rule "every way around the passkey, against live state"
  cd contracts
  echo "the same approval, submitted again"
  refusal forge script script/PasskeyPolicy.s.sol --sig "submit(address,address,bytes,string)" \
    "$OWNER" "$POLICY" "$PENDING_DATA" "../runtime/passkey/$NET-last-auth.json" --rpc-url "$RPC_URL" --private-key "$RELAYER_PK"
  echo "the same approval, pointed at a different payee"
  refusal forge script script/PasskeyPolicy.s.sol --sig "submit(address,address,bytes,string)" \
    "$OWNER" "$POLICY" "$OTHER_DATA" "../runtime/passkey/$NET-last-auth.json" --rpc-url "$RPC_URL" --private-key "$RELAYER_PK"
  cd "$ROOT"
  echo "the relayer, which pays the gas, calls setPayee itself"
  refusal cast send "$POLICY" "setPayee(address,bool)" "$OTHER" true --rpc-url "$RPC_URL" --private-key "$RELAYER_PK"
  echo "the agent calls setPayee itself"
  refusal cast send "$POLICY" "setPayee(address,bool)" "$OTHER" true --rpc-url "$RPC_URL" --private-key "$AGENT_PK"
  echo "the agent pays an address the passkey never approved"
  refusal cast send "$POLICY" "pay(address,uint256,bytes32)" "$OTHER" 1000000 "0x$(openssl rand -hex 32)" \
    --rpc-url "$RPC_URL" --private-key "$AGENT_PK"
  echo "the relayer withdraws"
  refusal cast send "$POLICY" "withdraw(address,uint256)" "$RELAYER" 1 --rpc-url "$RPC_URL" --private-key "$RELAYER_PK"
  echo "  $OTHER attested: $(cast call "$POLICY" 'attestedPayee(address)(bool)' "$OTHER" --rpc-url "$RPC_URL")"
  ;;

pay)
  need_state
  PAYEE=${3:?payee}; UNITS=${4:?units}
  rule "the agent pays $(usdc "$UNITS") USDC to $PAYEE"
  echo "  attested: $(cast call "$POLICY" 'attestedPayee(address)(bool)' "$PAYEE" --rpc-url "$RPC_URL") · room: $(usdc "$(cast call "$POLICY" 'remainingAllowance()(uint256)' --rpc-url "$RPC_URL" | awk '{print $1}')") USDC"
  before=$(balance "$PAYEE")
  if ! OUT=$(cast send "$POLICY" "pay(address,uint256,bytes32)" "$PAYEE" "$UNITS" "0x$(openssl rand -hex 32)" \
    --rpc-url "$RPC_URL" --private-key "$AGENT_PK" --json 2>&1); then
    echo "  refused: $(grep -oE '[A-Z][A-Za-z]+\([^)]*\)' <<<"$OUT" | tail -1)"
    exit 1
  fi
  TX=$(tx_of <<<"$OUT")
  after=$(balance "$PAYEE")
  echo "  payee USDC $(usdc "$before") -> $(usdc "$after")"
  echo "  policy USDC $(usdc "$(balance "$POLICY")") · spent in window $(usdc "$(cast call "$POLICY" 'spentInWindow()(uint256)' --rpc-url "$RPC_URL" | awk '{print $1}')")"
  echo "  tx $TX"
  ;;

history)
  need_state
  WALLET=$(cast wallet address --private-key "$WALLET_PK")
  # Recorded first, not last: a pass run while the history is still being
  # written (to ask for approval early, as on 2026-10-02) must already know
  # whose payments to learn from. Recording it at the end left that pass
  # reading only the policy, and it found nothing.
  grep -q "^WALLET=" "$STATE" || echo "WALLET=$WALLET" >> "$STATE"
  SUB=${SUB:-0x1111111111111111111111111111111111111111}    # a subscription; attest it first
  NEWP=${NEWP:-0x3333333333333333333333333333333333333333}  # paid every cycle, never attested
  if [[ $NET != local ]]; then
    # A live chain cannot be fast-forwarded, so the cadence is compressed:
    # rounds GAP seconds apart, on a fixed schedule so a slow send delays only
    # itself. The detector keys on regularity, not on the length of the gap.
    ROUNDS=${ROUNDS:-6}; GAP=${GAP:-60}
    SUB_AMT=${SUB_AMT:-200000}; NEWP_AMT=${NEWP_AMT:-150000}   # 0.20 and 0.15 USDC
    need=$(( ROUNDS * (SUB_AMT + NEWP_AMT) ))
    have=$(balance "$WALLET")
    (( have >= need )) || { echo "wallet $WALLET holds $(usdc "$have") USDC; this needs $(usdc "$need")" >&2; exit 1; }
    rule "$ROUNDS rounds of bills from $WALLET, ${GAP}s apart"
    t0=$(date +%s)
    for i in $(seq 1 "$ROUNDS"); do
      if [[ $USDC == native ]]; then
        a=$(xsend "$WALLET_PK" "$SUB" --value $(( SUB_AMT * 10**12 )))
        b=$(xsend "$WALLET_PK" "$NEWP" --value $(( NEWP_AMT * 10**12 )))
      else
        a=$(xsend "$WALLET_PK" "$USDC" "transfer(address,uint256)" "$SUB" "$SUB_AMT")
        b=$(xsend "$WALLET_PK" "$USDC" "transfer(address,uint256)" "$NEWP" "$NEWP_AMT")
      fi
      printf '  round %d  %s -> %s… %s   %s -> %s… %s\n' "$i" "$(usdc $SUB_AMT)" "${SUB:0:6}" "${a:0:10}…" "$(usdc $NEWP_AMT)" "${NEWP:0:6}" "${b:0:10}…"
      if (( i < ROUNDS )); then w=$(( t0 + i * GAP - $(date +%s) )); (( w > 0 )) && sleep "$w"; fi
    done
    grep -q "^WALLET=" "$STATE" || echo "WALLET=$WALLET" >> "$STATE"
    exit 0
  fi
  rule "six months of bills from the owner's old wallet $WALLET"
  echo "  Monthly, on block time fast-forwarded 30 days at a time. $SUB is attested on"
  echo "  the policy; $NEWP never was -- the owner paid it by hand."
  cast send "$USDC" "mint(address,uint256)" "$WALLET" 50000000 --rpc-url "$RPC_URL" --private-key "$RELAYER_PK" >/dev/null
  t=$(( $(cast block --rpc-url "$RPC_URL" -f timestamp) + 3600 ))
  for i in 1 2 3 4 5 6; do
    cast rpc evm_setNextBlockTimestamp $t --rpc-url "$RPC_URL" >/dev/null
    cast send "$USDC" "transfer(address,uint256)" $SUB 1500000 --rpc-url "$RPC_URL" --private-key "$WALLET_PK" >/dev/null
    cast rpc evm_setNextBlockTimestamp $((t + 600)) --rpc-url "$RPC_URL" >/dev/null
    cast send "$USDC" "transfer(address,uint256)" $NEWP 1200000 --rpc-url "$RPC_URL" --private-key "$WALLET_PK" >/dev/null
    printf '  month %d  %s  1.500000 -> %s  1.200000 -> %s\n' $i "$(date -u -r $t +%Y-%m-%d)" "${SUB:0:6}…" "${NEWP:0:6}…"
    t=$((t + 30 * 86400))
  done
  # Stop one day before the next bill is due.
  cast rpc evm_setNextBlockTimestamp $((t - 30 * 86400 + 29 * 86400)) --rpc-url "$RPC_URL" >/dev/null
  cast rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
  echo "  chain time now $(date -u -r "$(cast block --rpc-url "$RPC_URL" -f timestamp)" '+%Y-%m-%d %H:%M') UTC, a day before month 7"
  grep -q "^WALLET=" "$STATE" || echo "WALLET=$WALLET" >> "$STATE"
  ;;

run)
  need_state
  rule "the agent's pass on $NET"
  [[ -n ${WALLET:-} ]] && echo "  learns from the policy and the owner's wallet $WALLET"
  # The model reasons only when asked to (REASONER=model) on a local chain;
  # the deterministic control otherwise. On arbsep the key file decides.
  if [[ $NET == local && ${REASONER:-} != model ]]; then unset ANTHROPIC_API_KEY
  elif [[ -f ~/.tameion/anthropic.key ]]; then export ANTHROPIC_API_KEY="$(cat ~/.tameion/anthropic.key)"; fi
  # Native USDC (Arc) is read from Arc's transfer emitter; a token, from its contract.
  if [[ $USDC == native ]]; then unset TOKEN; else export TOKEN=$USDC; fi
  RPC_URL=$RPC_URL CHAIN_ID=$CHAIN POLICY=$POLICY ANCHOR=$ANCHOR FROM=$START \
    AGENT_PK=$AGENT_PK ALSO_HISTORY=${WALLET:-} \
    cargo run --locked -p operator --quiet --example run_arc
  ;;

*) sed -n '2,26p' "$0"; exit 2 ;;
esac
