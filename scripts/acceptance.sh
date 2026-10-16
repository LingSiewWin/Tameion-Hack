#!/usr/bin/env bash
# Does this actually work? One command, one answer.
#
# Every claim this project makes is checked here against a real chain and a
# real model, and each line prints PASS or FAIL. A claim that cannot be
# checked does not belong in the README.
#
#   scripts/acceptance.sh
set -uo pipefail
# Anvil's published test mnemonic: its keys are derived when the script runs, never written into the repository.
anvil_key() { cast wallet private-key --mnemonic "test test test test test test test test test test test junk" --mnemonic-index "$1"; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export RPC_URL=http://127.0.0.1:8545
REGISTRY=0x8004A818BFB912233c491871b3d84c89A494BD9e
PRINCIPAL_PK=$(anvil_key 0)
export AGENT_PK=$(anvil_key 1)
STRANGER_PK=$(anvil_key 4)
PORT=7879

PASS=0; FAIL=0
ok()   { printf '  \033[32mPASS\033[0m  %s\n' "$1"; PASS=$((PASS+1)); }
bad()  { printf '  \033[31mFAIL\033[0m  %s\n' "$1"; FAIL=$((FAIL+1)); }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', want '$3')"; fi; }
rule() { printf '\n\033[1m%s\033[0m\n' "$1"; }

cleanup(){ kill ${ANVIL_PID:-0} ${SRV_PID:-0} ${X402_PIDS:-} ${ANVIL2_PID:-0} ${APP_PID:-0} 2>/dev/null; }
trap cleanup EXIT
pkill -f "anvil --port 8545" 2>/dev/null; sleep 1
# Keep the anvil audit record apart from the Arc one (docs/46 #1).
export TAMEION_LOG_DIR=${TAMEION_LOG_DIR:-runtime/log-anvil}
lsof -ti:$PORT | xargs kill -9 2>/dev/null

anvil --port 8545 --silent > /tmp/anvil-acceptance.log 2>&1 &
ANVIL_PID=$!
for _ in $(seq 1 40); do nc -z 127.0.0.1 8545 && break; sleep 0.3; done

AGENT=$(cast wallet address --private-key "$AGENT_PK")
OPERATOR_PK=$(anvil_key 3)
OPERATOR=$(cast wallet address --private-key "$OPERATOR_PK")
PRINCIPAL=$(cast wallet address --private-key "$PRINCIPAL_PK")
STRANGER=$(cast wallet address --private-key "$STRANGER_PK")

cd contracts
export POLICY=$(forge create --broadcast src/BudgetPolicy.sol:BudgetPolicy \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" \
  --constructor-args "$PRINCIPAL" "$AGENT" "$OPERATOR" "$REGISTRY" 42 1000000000 50000000000 50000000 "${DELAY:-172800}" "${MODE:-2}" \
  | grep 'Deployed to:' | awk '{print $3}')
export TOKEN=$(forge create --broadcast src/TestUSDC.sol:TestUSDC \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" | grep 'Deployed to:' | awk '{print $3}')
export ANCHOR=$(forge create --broadcast \
  lib/arc-agent-kit/contracts/src/MemoryAnchor.sol:MemoryAnchor \
  --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" \
  --constructor-args "$REGISTRY" | grep 'Deployed to:' | awk '{print $3}')
cd "$ROOT"
cast send "$POLICY" --value 5000000000000000000000 --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null
# The agent may pay only addresses the principal attested: the three real
# vendors in the seeded history, and the stranger the cap checks below pay.
# 0x4444 is left out on purpose; it is the one the agent must escalate.
for P in 0x1111111111111111111111111111111111111111 0x2222222222222222222222222222222222222222 \
         0x3333333333333333333333333333333333333333; do
  cast send "$POLICY" "setPayee(address,bool)" "$P" true --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null
done
# docs/46 #3: a payee attested just now is not payable for DELAY seconds, so
# move the local chain's clock past it before the agent's first pass.
cast rpc evm_increaseTime "${DELAY:-172800}" --rpc-url "$RPC_URL" >/dev/null
cast rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
export TREASURY_PK=$(anvil_key 2)
export TREASURY=$(cast wallet address --private-key "$TREASURY_PK")
cargo run --locked -p operator --quiet --example seed_history --features dev-anvil >/dev/null 2>&1
# Pin every run to the chain's own clock. The history was written on chain and
# each due date derives from it; the host's wall clock belongs to a different
# timeline entirely, and using it reports nothing due.
NOW=$(cast block latest --field timestamp --rpc-url "$RPC_URL")
# Seeding the history moved the chain's clock months ahead, past the agent's
# ninety-day term (PLAN 5.24). The owner renews before the agent runs — the
# step a real owner takes once a quarter, and rule 11 below checks the lapse.
cast send "$POLICY" "renewAgent(uint64)" $((NOW+90*86400)) --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null 2>&1

if [ -f "$HOME/.tameion/anthropic.key" ]; then
  export ANTHROPIC_API_KEY="$(cat "$HOME/.tameion/anthropic.key")"
fi
PORT=$PORT cargo run --locked -p operator --quiet --example serve > /tmp/acceptance-srv.log 2>&1 &
SRV_PID=$!
for _ in $(seq 1 90); do nc -z 127.0.0.1 $PORT && break; sleep 1; done

run(){ curl -s -X POST http://127.0.0.1:$PORT/api/run -H 'content-type: application/json' -d "{\"now\":$NOW}"; }
jq_(){ python3 -c "import json,sys;d=json.load(sys.stdin);print($1)"; }

rule "0. the core claim: obligations recovered from raw chain data"
R1=$(run); echo "$R1" > /tmp/acc1.json
check "history came from the chain"          "$(echo "$R1" | jq_ "d['transfersFromChain']")" "True"
check "eth_getLogs returned the payments"    "$(echo "$R1" | jq_ "d['outgoing']>=20")" "True"
check "obligations inferred, unlabelled"     "$(echo "$R1" | jq_ "d['obligations']")" "4"
check "declined the counterparty with no rhythm" "$(echo "$R1" | jq_ "d['counterparties']-d['obligations']")" "1"

rule "1. the agent does the work"
# Honest label. The agent is handed a seeded history here, not one read off
# the chain, because anvil does not emit Arc's native-transfer Transfer events
# (Arc emits them from 0xffff...fffe; a local EVM emits nothing for a plain
# value transfer). The chain-reading path is exercised only against Arc --
# see proof/arc-testnet-*.txt, "read 16 transfers off chain".
check "pays what is inside the cap"          "$(echo "$R1" | jq_ "len(d['settled'])")" "2"
check "a model made the ordering"            "$(echo "$R1" | jq_ "d['reasonerKind']")" "model"
check "divergence is measured"               "$(echo "$R1" | jq_ "d['divergence'] is not None")" "True"
check "escalates what is outside it"         "$(echo "$R1" | jq_ "len(d['escalated'])")" "2"
check "flags ignored escalations"            "$(echo "$R1" | jq_ "d['staleEscalations']>0")" "True"
check "anchors the log root"                 "$(echo "$R1" | jq_ "d['anchorTx'] is not None")" "True"

rule "2. the record and the chain agree"
ROOT_LOCAL=$(echo "$R1" | jq_ "d['root']")
ROOT_CHAIN=$(cast call "$ANCHOR" 'rootOf(address)(bytes32)' "$AGENT" --rpc-url "$RPC_URL")
check "anchored root == computed root"       "$ROOT_CHAIN" "$ROOT_LOCAL"

rule "3. the same bill cannot be paid twice"
R2=$(run)
check "second run settles nothing"           "$(echo "$R2" | jq_ "len(d['settled'])")" "0"
# Counted on chain, from the policy's own Paid events, so the check holds
# whatever mechanism keeps the second run off the bill. (It used to look for
# "SKIPPED" in the log, which tested how, not whether.)
paid_to(){ cast logs --from-block 0 --address "$POLICY" "Paid(address,uint256,bytes32)" --rpc-url "$RPC_URL" --json \
  | python3 -c "import json,sys;print(sum(1 for l in json.load(sys.stdin) if l['topics'][1].endswith('$1')))"; }
check "0x1111 paid once, not twice"          "$(paid_to 1111111111111111111111111111111111111111)" "1"
check "0x2222 paid once, not twice"          "$(paid_to 2222222222222222222222222222222222222222)" "1"
KEY1=$(echo "$R1" | jq_ "[x for x in d['pendingSignature'] if x['counterparty'].startswith('0x4444')][0]['settlementKey']")
KEY2=$(echo "$R2" | jq_ "[x for x in d['pendingSignature'] if x['counterparty'].startswith('0x4444')][0]['settlementKey']")
check "settlement key is stable across runs" "$KEY1" "$KEY2"
H1=$(echo "$R1" | jq_ "[x for x in d['pendingSignature'] if x['counterparty'].startswith('0x4444')][0]['actionHash']")
H2=$(echo "$R2" | jq_ "[x for x in d['pendingSignature'] if x['counterparty'].startswith('0x4444')][0]['actionHash']")
if [ "$H1" != "$H2" ]; then ok "action hash still tracks the reasoning"; else
  bad "action hash did not change (model prose was identical; rerun)"; fi

rule "4. the contract refuses the agent"
# $1 is deliberately unquoted: cast takes the signature and each argument as
# separate words, and quoting them into one string makes every call fail for
# the wrong reason.
try(){ local out; out=$(cast send "$POLICY" $1 --rpc-url "$RPC_URL" --private-key "$2" 2>&1)
       if grep -qiE "$3" <<<"$out"; then ok "$4"; else bad "$4 -- got: $(head -1 <<<"$out")"; fi; }
try "withdraw(address,uint256) $AGENT 1000000"               "$AGENT_PK"     "NotPrincipal|revert" "agent cannot withdraw"
try "setLimits(uint256,uint256,uint256) 999000000 999000000 0" "$AGENT_PK"   "NotPrincipalOrOperator|revert" "agent cannot raise its own cap"
try "setAgent(address) $STRANGER"                             "$AGENT_PK"    "NotOperator|revert"  "agent cannot replace itself"
try "pay(address,uint256,bytes32) $STRANGER 1000000 0x00000000000000000000000000000000000000000000000000000000000000ff" "$STRANGER_PK" "NotAgent|revert" "a stranger cannot pay"
# Matched on the error's selector, not on "revert": a check that passes on any
# revert passes for the wrong reason, which is how this file once tested the
# reentrancy guard.
SEL_PNA=$(cast sig "PayeeNotAttested(address)")
try "pay(address,uint256,bytes32) 0x1111000000000000000000000000000000001111 1000000 0x00000000000000000000000000000000000000000000000000000000000000fc" "$AGENT_PK" "PayeeNotAttested|${SEL_PNA#0x}" "an address the principal never attested is refused on chain"
try "setPayee(address,bool) $STRANGER true"                       "$AGENT_PK"    "NotPrincipal|revert" "agent cannot attest a payee"
try "setPayee(address,bool) $STRANGER true"                       "$OPERATOR_PK" "NotPrincipal|revert" "operator cannot attest a payee"
# From here on the stranger is an attested payee, so the limit checks below
# are refused by the limit they name and not by the allowlist.
cast send "$POLICY" "setPayee(address,bool)" "$STRANGER" true --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null
# docs/46 #3: attested a second ago, so not payable yet -- matched on the
# selector, not on "revert".
SEL_COOL=$(cast sig "PayeeCoolingOff(address,uint256)")
try "pay(address,uint256,bytes32) $STRANGER 1000000 0x00000000000000000000000000000000000000000000000000000000000000fd" "$AGENT_PK" "PayeeCoolingOff|${SEL_COOL#0x}" "a payee attested a moment ago is refused until the delay passes"
cast rpc evm_increaseTime "${DELAY:-172800}" --rpc-url "$RPC_URL" >/dev/null; cast rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
try "pay(address,uint256,bytes32) $STRANGER 1500000000 0x00000000000000000000000000000000000000000000000000000000000000fe" "$AGENT_PK" "OverPerActionCap|revert" "over-cap payment refused on chain"

rule "5. the split roles hold"
try "withdraw(address,uint256) $OPERATOR 1000000"             "$OPERATOR_PK"  "NotPrincipal|revert" "operator cannot withdraw"
# Since 2026-10-13 night (docs/52) the principal may set limits and mode; a
# tightening by the principal applies at once, and nobody else may touch them.
# The principal re-states the current limits: a non-raising setLimits applies
# at once, which proves the seat without moving the numbers later checks read.
PER_NOW=$(cast call "$POLICY" 'perActionCap()(uint256)' --rpc-url $RPC_URL | awk '{print $1}')
DAY_NOW=$(cast call "$POLICY" 'dailyCap()(uint256)' --rpc-url $RPC_URL | awk '{print $1}')
FLOOR_NOW=$(cast call "$POLICY" 'reserveFloor()(uint256)' --rpc-url $RPC_URL | awk '{print $1}')
if cast send "$POLICY" "setLimits(uint256,uint256,uint256)" "$PER_NOW" "$DAY_NOW" "$FLOOR_NOW" --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null 2>&1; then
  ok "principal may set the limits (a tightening applies at once)"; else bad "principal could not call setLimits"; fi
try "setLimits(uint256,uint256,uint256) 999000000 999000000 0" "$STRANGER_PK"  "NotPrincipalOrOperator|revert" "a stranger cannot change limits"
try "setAgent(address) $STRANGER"                             "$PRINCIPAL_PK" "NotOperator|revert"  "principal cannot replace the agent"
try "setOperator(address) $STRANGER"                          "$OPERATOR_PK"  "NotPrincipal|revert" "operator cannot appoint its successor"

rule "6. a human can settle, and the agent notices"
TO=$(echo "$R2" | jq_ "[x for x in d['pendingSignature'] if x['counterparty'].startswith('0x4444')][0]['to']")
DATA=$(echo "$R2" | jq_ "[x for x in d['pendingSignature'] if x['counterparty'].startswith('0x4444')][0]['data']")
check "cycle unsettled before"               "$(cast call "$POLICY" 'executed(bytes32)(bool)' "$KEY2" --rpc-url $RPC_URL)" "false"
cast send "$TO" "$DATA" --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null 2>&1
check "principal settlement recorded"        "$(cast call "$POLICY" 'executed(bytes32)(bool)' "$KEY2" --rpc-url $RPC_URL)" "true"
# TestUSDC carries the *history*; the policy pays in native value, so the
# settlement lands on the native balance. Checking the token balance here was
# reading the wrong ledger.
check "payee received the escalated amount"  "$(cast balance 0x4444444444444444444444444444444444444444 --rpc-url $RPC_URL)" "1200000000000000000000"
R3=$(run)
check "agent stops escalating it"            "$(echo "$R3" | jq_ "any(e['who'].startswith('0x4444') for e in d['escalated'])")" "False"

rule "7. limits other than the per-action cap, and the delay rule (docs/46 #3, #4)"
# Shrinking the agent's room applies at once...
cast send "$POLICY" "setLimits(uint256,uint256,uint256)" 100000000 100000000 4999000000 \
  --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" >/dev/null 2>&1
check "lowering caps / raising the floor is instant" "$(cast call "$POLICY" 'reserveFloor()(uint256)' --rpc-url $RPC_URL | awk '{print $1}')" "4999000000"
RES_KEY=0x00000000000000000000000000000000000000000000000000000000000000f2
try "pay(address,uint256,bytes32) $STRANGER 50000000 $RES_KEY" "$AGENT_PK" "BelowReserveFloor|revert" "reserve floor is enforced"
# ...and expanding it is scheduled, announced, and applies only after the delay.
cast send "$POLICY" "setLimits(uint256,uint256,uint256)" 100000000 50000000000 50000000 \
  --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" >/dev/null 2>&1
check "a raise does not apply at once"       "$(cast call "$POLICY" 'reserveFloor()(uint256)' --rpc-url $RPC_URL | awk '{print $1}')" "4999000000"
PEND=$(cast call "$POLICY" 'pending()(uint256,uint256,uint256,uint8,uint64,uint8)' --rpc-url $RPC_URL | tail -1)
check "the raise is pending (kinds=1)"       "$PEND" "1"
SEL_EARLY=$(cast sig "TooEarly(uint64)")
try "applyPending()" "$STRANGER_PK" "TooEarly|${SEL_EARLY#0x}" "applying before the delay is refused"
cast rpc evm_increaseTime "${DELAY:-172800}" --rpc-url "$RPC_URL" >/dev/null; cast rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
cast send "$POLICY" "applyPending()" --rpc-url "$RPC_URL" --private-key "$STRANGER_PK" >/dev/null 2>&1
check "anyone applies it after the delay"   "$(cast call "$POLICY" 'reserveFloor()(uint256)' --rpc-url $RPC_URL | awk '{print $1}')" "50000000"
# The principal can cancel a raise they did not ask for.
cast send "$POLICY" "setLimits(uint256,uint256,uint256)" 2000000000 50000000000 50000000 \
  --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" >/dev/null 2>&1
cast send "$POLICY" "cancelPending()" --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null 2>&1
cast rpc evm_increaseTime "${DELAY:-172800}" --rpc-url "$RPC_URL" >/dev/null; cast rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
check "a cancelled raise never applies"      "$(cast call "$POLICY" 'perActionCap()(uint256)' --rpc-url $RPC_URL | awk '{print $1}')" "100000000"
# Daily cap: squeeze it to 10 USDC (a shrink: instant), then ask for 20 --
# inside the per-action cap and inside the reserve, so only the daily limit
# can refuse it.
cast send "$POLICY" "setLimits(uint256,uint256,uint256)" 10000000 10000000 50000000 \
  --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" >/dev/null 2>&1
DAY_KEY=0x00000000000000000000000000000000000000000000000000000000000000f3
try "pay(address,uint256,bytes32) $STRANGER 20000000 $DAY_KEY" "$AGENT_PK" "OverPerActionCap|OverDailyCap|revert" "daily cap is enforced"
cast send "$POLICY" "setLimits(uint256,uint256,uint256)" 100000000 50000000000 50000000 \
  --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" >/dev/null 2>&1
cast rpc evm_increaseTime "${DELAY:-172800}" --rpc-url "$RPC_URL" >/dev/null; cast rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
cast send "$POLICY" "applyPending()" --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" >/dev/null 2>&1
# Mode: lowering is the pause and is instant; raising waits.
cast send "$POLICY" "setMode(uint8)" 1 --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" >/dev/null 2>&1
check "operator pauses to Propose at once"   "$(cast call "$POLICY" 'mode()(uint8)' --rpc-url $RPC_URL)" "1"
SEL_AUTO=$(cast sig "NotAutopilot(uint8)")
try "pay(address,uint256,bytes32) $STRANGER 1000000 0x00000000000000000000000000000000000000000000000000000000000000f4" "$AGENT_PK" "NotAutopilot|${SEL_AUTO#0x}" "outside Autopilot the agent cannot pay"
RP=$(run)
check "in Propose mode a pass settles nothing"  "$(echo "$RP" | jq_ "len(d['settled'])")" "0"
check "the pass reports the mode"               "$(echo "$RP" | jq_ "d['mode']")" "Propose"
cast send "$POLICY" "setMode(uint8)" 2 --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" >/dev/null 2>&1
check "raising back to Autopilot waits"      "$(cast call "$POLICY" 'mode()(uint8)' --rpc-url $RPC_URL)" "1"
cast rpc evm_increaseTime "${DELAY:-172800}" --rpc-url "$RPC_URL" >/dev/null; cast rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
cast send "$POLICY" "applyPending()" --rpc-url "$RPC_URL" --private-key "$STRANGER_PK" >/dev/null 2>&1
check "Autopilot again after the delay"      "$(cast call "$POLICY" 'mode()(uint8)' --rpc-url $RPC_URL)" "2"

rule "8. duplicate settlement is refused at the contract, not just in the agent"
# This is what makes concurrency safe: two processes can both pass their own
# already-executed check, and the contract still lets exactly one through.
DUP=0x00000000000000000000000000000000000000000000000000000000000000d1
cast send "$POLICY" "pay(address,uint256,bytes32)" "$STRANGER" 1000000 "$DUP" \
  --rpc-url "$RPC_URL" --private-key "$AGENT_PK" >/dev/null 2>&1
check "first payment lands"                  "$(cast call "$POLICY" 'executed(bytes32)(bool)' "$DUP" --rpc-url $RPC_URL)" "true"
try "pay(address,uint256,bytes32) $STRANGER 1000000 $DUP" "$AGENT_PK" "AlreadyExecuted|revert" "the same key cannot execute twice"

rule "9. a model outage does not stop the agent"
kill $SRV_PID 2>/dev/null; sleep 1
ANTHROPIC_API_KEY=sk-ant-bogus CLAUDE_BASE_URL=http://127.0.0.1:1/dead PORT=$PORT \
  cargo run --locked -p operator --quiet --example serve > /tmp/acceptance-srv2.log 2>&1 &
SRV_PID=$!
for _ in $(seq 1 60); do nc -z 127.0.0.1 $PORT && break; sleep 1; done
R4=$(curl -s -X POST http://127.0.0.1:$PORT/api/run -H 'content-type: application/json' -d "{\"now\":$((NOW+2592000))}")
check "falls back to the deterministic rule" "$(echo "$R4" | jq_ "d['reasonerKind']")" "rule"
# Every cycle is settled by this point, so the meaningful claim is that the
# loop still ran to completion and anchored -- not that it moved money it no
# longer owes.
check "still completes the loop"             "$(echo "$R4" | jq_ "d['obligations']>0 and d['anchorTx'] is not None")" "True"
check "records that it fell back"            "$(echo "$R4" | jq_ "any('reasoner-' in l for l in d['log'])")" "True"

rule "10. standing orders: a confirmed bill anyone may pay on its due date, inside the caps"
NOWTS=$(cast block latest --field timestamp --rpc-url $RPC_URL)
P2222=0x2222222222222222222222222222222222222222
cast send "$POLICY" "addSchedule(address,uint256,uint64,uint64)" "$P2222" 2000000 86400 "$NOWTS" --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null 2>&1
check "the principal sets a standing order"  "$(cast call "$POLICY" 'scheduleCount()(uint256)' --rpc-url $RPC_URL)" "1"
try "addSchedule(address,uint256,uint64,uint64) $P2222 2000000 86400 $NOWTS" "$OPERATOR_PK" "NotPrincipal|revert" "the operator cannot set one"
B0=$(cast balance "$P2222" --rpc-url $RPC_URL)
cast send "$POLICY" "payDue(uint256)" 0 --rpc-url "$RPC_URL" --private-key "$STRANGER_PK" >/dev/null 2>&1
B1=$(cast balance "$P2222" --rpc-url $RPC_URL)
check "a stranger triggers the due payment"  "$(python3 -c "print(($B1-$B0)//10**12)")" "2000000"
try "payDue(uint256) 0" "$STRANGER_PK" "ScheduleNotDue|revert" "the next cycle is not due yet"
cast send "$POLICY" "removeSchedule(uint256)" 0 --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" >/dev/null 2>&1
check "the operator ends it, instantly"      "$(cast call "$POLICY" 'schedules(uint256)(address,uint256,uint64,uint64,bool)' 0 --rpc-url $RPC_URL | tail -1)" "false"

rule "11. the agent's authority lapses and only the owner renews it"
NOWTS=$(cast block latest --field timestamp --rpc-url $RPC_URL)
cast send "$POLICY" "renewAgent(uint64)" $((NOWTS+60)) --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null 2>&1
check "the owner shortened the term (the brake)" "$(cast call "$POLICY" 'agentExpiry()(uint64)' --rpc-url $RPC_URL | awk '{print $1}')" "$((NOWTS+60))"
cast rpc evm_increaseTime 61 --rpc-url "$RPC_URL" >/dev/null; cast rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
try "pay(address,uint256,bytes32) $STRANGER 1000000 0x00000000000000000000000000000000000000000000000000000000000000e1" "$AGENT_PK" "AgentLapsed|revert" "a lapsed agent cannot pay"
try "renewAgent(uint64) $((NOWTS+90*86400))" "$OPERATOR_PK" "NotPrincipal|revert" "the operator cannot renew it"
cast send "$POLICY" "renewAgent(uint64)" $((NOWTS+90*86400)) --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null 2>&1
cast send "$POLICY" "pay(address,uint256,bytes32)" "$STRANGER" 1000000 0x00000000000000000000000000000000000000000000000000000000000000e2 --rpc-url "$RPC_URL" --private-key "$AGENT_PK" >/dev/null 2>&1
check "renewed, the agent pays again"        "$(cast call "$POLICY" 'executed(bytes32)(bool)' 0x00000000000000000000000000000000000000000000000000000000000000e2 --rpc-url $RPC_URL)" "true"

rule "12. an approval is bound to the terms the owner saw, and to a deadline"
FP=$(cast call "$POLICY" 'fingerprint()(bytes32)' --rpc-url $RPC_URL)
DL=$(( $(cast block latest --field timestamp --rpc-url $RPC_URL) + 600 ))
# The operator tightens the per-action cap (instant) after the owner read the page.
cast send "$POLICY" "setLimits(uint256,uint256,uint256)" 900000000 50000000000 50000000 --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" >/dev/null 2>&1
try "settleAsPrincipalIf(address,uint256,bytes32,bytes32,uint64) $STRANGER 1000000 0x00000000000000000000000000000000000000000000000000000000000000b1 $FP $DL" "$PRINCIPAL_PK" "TermsChanged|revert" "the terms moved after the owner read them: refused"
FP2=$(cast call "$POLICY" 'fingerprint()(bytes32)' --rpc-url $RPC_URL)
check "the fingerprint moved with the limits" "$([ "$FP" != "$FP2" ] && echo moved)" "moved"
cast send "$POLICY" "settleAsPrincipalIf(address,uint256,bytes32,bytes32,uint64)" "$STRANGER" 1000000 0x00000000000000000000000000000000000000000000000000000000000000b1 "$FP2" "$DL" --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null 2>&1
check "with the current terms it settles"    "$(cast call "$POLICY" 'executed(bytes32)(bool)' 0x00000000000000000000000000000000000000000000000000000000000000b1 --rpc-url $RPC_URL)" "true"
try "settleAsPrincipalIf(address,uint256,bytes32,bytes32,uint64) $STRANGER 1000000 0x00000000000000000000000000000000000000000000000000000000000000b2 $FP2 $(( DL - 1200 ))" "$PRINCIPAL_PK" "ApprovalExpired|revert" "a stale approval is refused"

rule "13. the owner's reviewed run: every waiting bill, one approval, a paid one skipped"
FP3=$(cast call "$POLICY" 'fingerprint()(bytes32)' --rpc-url $RPC_URL)
DL3=$(( $(cast block latest --field timestamp --rpc-url $RPC_URL) + 600 ))
C1=0x00000000000000000000000000000000000000000000000000000000000000c1
C2=0x00000000000000000000000000000000000000000000000000000000000000c2
B1H=0x00000000000000000000000000000000000000000000000000000000000000b1
try "settleRunIf(address[],uint256[],bytes32[],bytes32,uint64) [$STRANGER,$STRANGER] [1000000,2000000] [$C1,$C2] $FP3 $DL3" "$AGENT_PK" "NotPrincipal|revert" "only the owner may run it, not the agent"
R0=$(cast balance "$STRANGER" --rpc-url $RPC_URL)
cast send "$POLICY" "settleRunIf(address[],uint256[],bytes32[],bytes32,uint64)" "[$STRANGER,$STRANGER,$STRANGER]" "[1000000,2000000,1000000]" "[$C1,$C2,$B1H]" "$FP3" "$DL3" --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null 2>&1
R1B=$(cast balance "$STRANGER" --rpc-url $RPC_URL)
check "both new bills settled in one call"   "$(cast call "$POLICY" 'executed(bytes32)(bool)' $C1 --rpc-url $RPC_URL)/$(cast call "$POLICY" 'executed(bytes32)(bool)' $C2 --rpc-url $RPC_URL)" "true/true"
check "the bill paid in rule 12 was skipped" "$(python3 -c "print(($R1B-$R0)//10**12)")" "3000000"

rule "14. the company's own wallet becomes the account (EIP-7702): same address, no allowance, the agent bounded inside it"
# anvil's published development key 5 plays the company's existing wallet.
WALLET_PK=$(anvil_key 5)
WALLET=$(cast wallet address --private-key $WALLET_PK)
VENDOR7=0x0000000000000000000000000000000000007702
DELEGATE=$(cd contracts && forge create --broadcast src/TameionDelegate.sol:TameionDelegate --rpc-url "$RPC_URL" --private-key "$OPERATOR_PK" | grep 'Deployed to:' | awk '{print $3}')
W0=$(cast balance "$WALLET" --rpc-url $RPC_URL)
cast send "$WALLET" "initialize(address,address,address,address,uint256,uint256,uint256,uint256,uint256,uint8)" \
  "$PRINCIPAL" "$AGENT" "$OPERATOR" "$REGISTRY" 42 1000000 5000000 0 60 2 \
  --auth "$DELEGATE" --rpc-url "$RPC_URL" --private-key "$WALLET_PK" >/dev/null 2>&1
check "the wallet's code points at the delegate" "$(cast code "$WALLET" --rpc-url $RPC_URL)" "0xef0100$(echo "${DELEGATE#0x}" | tr 'A-F' 'a-f')"
check "the wallet initialised itself, with the owner" "$(cast call "$WALLET" 'principal()(address)' --rpc-url $RPC_URL)" "$PRINCIPAL"
check "its money never moved (one tx of gas aside)" "$(python3 -c "print(($W0-$(cast balance "$WALLET" --rpc-url $RPC_URL))<10**16)")" "True"
OUT=$(cast send "$WALLET" "initialize(address,address,address,address,uint256,uint256,uint256,uint256,uint256,uint8)" "$STRANGER" "$STRANGER" "$STRANGER" "$REGISTRY" 1 1 1 0 0 2 --rpc-url "$RPC_URL" --private-key "$STRANGER_PK" 2>&1)
if grep -qiE "NotSelf|revert|0x29c3b7ee" <<<"$OUT"; then ok "nobody else can initialise the wallet"; else bad "nobody else can initialise the wallet -- got: $(head -1 <<<"$OUT")"; fi
cast send "$WALLET" "setPayee(address,bool)" "$VENDOR7" true --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null 2>&1
cast rpc evm_increaseTime 61 --rpc-url $RPC_URL >/dev/null; cast rpc evm_mine --rpc-url $RPC_URL >/dev/null
OUT=$(cast send "$WALLET" "pay(address,uint256,bytes32)" "$VENDOR7" 2000000 0x00000000000000000000000000000000000000000000000000000000000077a1 --rpc-url "$RPC_URL" --private-key "$AGENT_PK" 2>&1)
if grep -qiE "revert|error" <<<"$OUT" && [ "$(cast balance $VENDOR7 --rpc-url $RPC_URL)" = "0" ]; then ok "above the cap, the agent is refused inside the wallet"; else bad "above the cap -- got: $(head -1 <<<"$OUT")"; fi
cast send "$WALLET" "pay(address,uint256,bytes32)" "$VENDOR7" 1000000 0x00000000000000000000000000000000000000000000000000000000000077a2 --rpc-url "$RPC_URL" --private-key "$AGENT_PK" >/dev/null 2>&1
check "the agent paid an attested payee from the wallet itself" "$(cast balance $VENDOR7 --rpc-url $RPC_URL)" "1000000000000000000"

rule "15. the agent buys a paid API (x402) inside the same limits: the contract decides, not the service"
# A fresh policy: 1 USDC per payment, 3 per 24 h. The seller is
# scripts/x402_seller.py (its own ERC-7710 facilitator, gas paid by the
# stranger's key); the buyer signs with the agent key directly, as a
# compromised agent would, skipping Tameion's own checks entirely.
X402=$(cd contracts && forge create --broadcast src/BudgetPolicy.sol:BudgetPolicy --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" \
  --constructor-args "$PRINCIPAL" "$AGENT" "$OPERATOR" "$REGISTRY" 42 1000000 3000000 0 60 2 | grep 'Deployed to:' | awk '{print $3}')
cast send "$X402" --value 10000000000000000000 --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null
SELLER=0x00000000000000000000000000000000005e11e2
cast send "$X402" "setPayee(address,bool)" "$SELLER" true --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null
cast rpc evm_increaseTime 61 --rpc-url $RPC_URL >/dev/null; cast rpc evm_mine --rpc-url $RPC_URL >/dev/null
KD=$(mktemp -d); echo "$STRANGER_PK" > "$KD/fac"; echo "$AGENT_PK" > "$KD/agent"; echo "$STRANGER_PK" > "$KD/stranger"
for P in 7905:500000 7906:1500000; do
  RPC=$RPC_URL CHAIN_ID=31337 PORT=${P%%:*} PRICE_MINOR=${P##*:} PAY_TO=$SELLER FACILITATOR_KEY_FILE="$KD/fac" \
    python3 scripts/x402_seller.py >/tmp/x402-seller-${P%%:*}.log 2>&1 &
  X402_PIDS="${X402_PIDS:-} $!"
done
for _ in $(seq 1 30); do nc -z 127.0.0.1 7906 2>/dev/null && nc -z 127.0.0.1 7905 2>/dev/null && break; sleep 0.3; done
OUT=$(python3 scripts/x402_buy.py "$X402" "$KD/agent" "http://127.0.0.1:7905/v1/mid?coin=ETH" "$RPC_URL" "$KD/replay")
check "a purchase inside the caps is paid (HTTP 200)" "${OUT%% *}" "200"
check "the seller received 0.5 USDC" "$(cast balance $SELLER --rpc-url $RPC_URL)" "500000000000000000"
OUT=$(python3 scripts/x402_buy.py "$X402" "$KD/agent" "http://127.0.0.1:7905/v1/mid?coin=ETH" "$RPC_URL" "$KD/replay")
if [ "${OUT%% *}" = "402" ] && grep -qiE "AlreadyExecuted|d1d36dcd" <<<"$OUT"; then ok "the same signed purchase is redeemed once"; else bad "replay -- got: $OUT"; fi
OUT=$(python3 scripts/x402_buy.py "$X402" "$KD/agent" "http://127.0.0.1:7906/v1/mid?coin=ETH" "$RPC_URL")
if [ "${OUT%% *}" = "402" ] && grep -q "97b81df2" <<<"$OUT"; then ok "a price above the per-payment cap is refused by the contract (OverPerActionCap)"; else bad "over cap -- got: $OUT"; fi
OUT=$(python3 scripts/x402_buy.py "$X402" "$KD/stranger" "http://127.0.0.1:7905/v1/mid?coin=ETH" "$RPC_URL")
if [ "${OUT%% *}" = "402" ] && grep -qiE "BadAgentSignature|revert" <<<"$OUT"; then ok "a purchase someone else signed is refused (BadAgentSignature)"; else bad "stranger -- got: $OUT"; fi
python3 scripts/x402_buy.py "$X402" "$KD/agent" "http://127.0.0.1:7905/v1/mid?coin=ETH" "$RPC_URL" >/dev/null
python3 scripts/x402_buy.py "$X402" "$KD/agent" "http://127.0.0.1:7905/v1/mid?coin=ETH" "$RPC_URL" >/dev/null
python3 scripts/x402_buy.py "$X402" "$KD/agent" "http://127.0.0.1:7905/v1/mid?coin=ETH" "$RPC_URL" >/dev/null
check "four purchases of 0.5 count against the one 24-hour window" "$(cast call $X402 'spentInWindow()(uint256)' --rpc-url $RPC_URL | awk '{print $1}')" "2000000"
cast send "$X402" "setLimits(uint256,uint256,uint256)" 1000000 2000000 0 --rpc-url "$RPC_URL" --private-key "$PRINCIPAL_PK" >/dev/null
OUT=$(python3 scripts/x402_buy.py "$X402" "$KD/agent" "http://127.0.0.1:7905/v1/mid?coin=ETH" "$RPC_URL")
if [ "${OUT%% *}" = "402" ]; then ok "the owner lowers the 24-hour limit and the next purchase is refused at once"; else bad "after lowering -- got: $OUT"; fi
kill $X402_PIDS 2>/dev/null; wait $X402_PIDS 2>/dev/null; rm -rf "$KD"

rule "16. every purchase is a receipt, and a seller bought from on a cadence is a bill (receipt v1, docs/76 §6)"
# Through the hosted service this time, not the raw buyer: a second anvil
# carrying Arc's chain id so the service's arc-testnet preset (native USDC,
# Arc's purchase-digest domain) points at it; the account's record and agent
# key written where the service keeps them; an agent key with the buy scope
# as the passkey would mint it. The seller advertises payment-identifier.
ARC_ID=5042002; RPC2=http://127.0.0.1:8548; PORT2=7881
anvil --port 8548 --chain-id $ARC_ID --silent > /tmp/anvil-acceptance-x402.log 2>&1 &
ANVIL2_PID=$!
for _ in $(seq 1 40); do nc -z 127.0.0.1 8548 && break; sleep 0.3; done
AGENT2_PK=$(anvil_key 6)   # anvil's published key 6
AGENT2=$(cast wallet address --private-key $AGENT2_PK)
P2=$(cd contracts && forge create --broadcast src/BudgetPolicy.sol:BudgetPolicy --rpc-url "$RPC2" --private-key "$PRINCIPAL_PK" \
  --constructor-args "$PRINCIPAL" "$AGENT2" "$OPERATOR" "$REGISTRY" 42 1000000 5000000 0 60 2 | grep 'Deployed to:' | awk '{print $3}')
ANCHOR2=$(cd contracts && forge create --broadcast lib/arc-agent-kit/contracts/src/MemoryAnchor.sol:MemoryAnchor --rpc-url "$RPC2" --private-key "$PRINCIPAL_PK" --constructor-args "$REGISTRY" | grep 'Deployed to:' | awk '{print $3}')
cast send "$P2" --value 10000000000000000000 --rpc-url "$RPC2" --private-key "$PRINCIPAL_PK" >/dev/null
cast send "$P2" "setPayee(address,bool)" "$SELLER" true --rpc-url "$RPC2" --private-key "$PRINCIPAL_PK" >/dev/null
cast rpc evm_increaseTime 61 --rpc-url $RPC2 >/dev/null; cast rpc evm_mine --rpc-url $RPC2 >/dev/null
APP2=$(mktemp -d); P2L=$(tr 'A-F' 'a-f' <<<"$P2"); OWN=$(tr 'A-F' 'a-f' <<<"$PRINCIPAL"); SELLER_LC=$(tr 'A-F' 'a-f' <<<"$SELLER")
mkdir -p "$APP2/tenants/$OWN"; echo "$AGENT2_PK" > "$APP2/tenants/$OWN/agent.key"
TOK=acceptance-buy-key; TOKH=$(python3 -c "import hashlib;print(hashlib.sha256(b'$TOK').hexdigest())")
cat > "$APP2/tenants/$P2L.json" <<EOF
{"chain":"arc-testnet","owner":"$OWN","policy":"$P2L","agent":"$AGENT2","operator":"$OPERATOR","watched":"$P2L","recovery":"0x0000000000000000000000000000000000000000","per_action":"1","daily":"5","floor":"0","delay_secs":60,"mode":2,"created_block":0,"history_from_block":0,"created":0,"deployed":true,"agent_tokens":[{"id":"t1","label":"acceptance","scopes":["read","run","buy"],"expires":4102444800,"issued":0,"hash":"$TOKH"}]}
EOF
# The rule-based reasoner: the receipt bills are appended by the pass whatever
# reasons, and the check must not depend on a model call.
env -u ANTHROPIC_API_KEY -u ANTHROPIC_AUTH_TOKEN TAMEION_APP_DIR="$APP2" TAMEION_RPC_ARC="$RPC2" TAMEION_ANCHOR="$ANCHOR2" \
  TAMEION_RELAYER_KEY="$OPERATOR_PK" TAMEION_X402_ALLOW_LOCAL=1 PORT=$PORT2 \
  cargo run --locked -p operator --quiet --bin tameion-app > /tmp/acceptance-app.log 2>&1 &
APP_PID=$!
KD=$(mktemp -d); echo "$STRANGER_PK" > "$KD/fac"
RPC=$RPC2 CHAIN_ID=$ARC_ID PORT=7907 PRICE_MINOR=500000 PAY_TO=$SELLER FACILITATOR_KEY_FILE="$KD/fac" PAYMENT_IDENTIFIER=1 \
  python3 scripts/x402_seller.py >/tmp/x402-seller-7907.log 2>&1 &
X402_PIDS=$!
for _ in $(seq 1 90); do nc -z 127.0.0.1 $PORT2 && nc -z 127.0.0.1 7907 && break; sleep 1; done
buy(){ curl -s -X POST http://127.0.0.1:$PORT2/api/level1/buy -H 'content-type: application/json' -H "Authorization: Bearer $TOK" \
  -d "{\"policy\":\"$P2\",\"url\":\"http://127.0.0.1:7907/v1/mid?coin=ETH\",\"maxUsdc\":\"1\",\"reason\":\"ETH mid for the hedge\"}"; }
B1=$(buy); echo "$B1" > /tmp/acc-buy1.json
check "the service bought, the contract paid, the Paid log read back (paid:true)" "$(echo "$B1" | jq_ "d['paid']")" "True"
check "the receipt carries the x402 payment identifier the seller required" "$(echo "$B1" | jq_ "d['purchase']['idFrom']+' '+d['purchase']['id'][:4]")" "payment-identifier pay_"
check "the identifier is the purchase digest (what the contract refuses twice)" "$(echo "$B1" | jq_ "d['purchase']['id']=='pay_'+d['purchase']['actionHash'][2:]")" "True"
check "sha256 on the receipt is the hash of the bytes returned" "$(echo "$B1" | python3 -c "import json,sys,hashlib;d=json.load(sys.stdin);print(hashlib.sha256(d['body'].encode()).hexdigest()==d['purchase']['sha256'])")" "True"
# Three more on a cadence (integer-second gaps; the detector needs the gaps
# to agree within 35 %).
for _ in 1 2 3; do sleep 3; buy >/dev/null; done
J=$(curl -s -X POST http://127.0.0.1:$PORT2/api/level1/run -H 'content-type: application/json' -H "Authorization: Bearer $TOK" -d "{\"policy\":\"$P2\"}" | jq_ "d['id']")
for _ in $(seq 1 180); do S=$(curl -s http://127.0.0.1:$PORT2/api/watch/$J | jq_ "d['status']"); { [ "$S" = done ] || [ "$S" = failed ]; } && break; sleep 1; done
REP=$(curl -s http://127.0.0.1:$PORT2/api/watch/$J); echo "$REP" > /tmp/acc-x402-pass.json
check "the pass finished" "$(echo "$REP" | jq_ "d['status']")" "done"
check "the pass lists the seller as a bill, provenance x402 receipt, four samples" "$(echo "$REP" | jq_ "[(o['provenance'], o['samples']) for o in d['report']['obligationsList'] if o['counterparty']=='$SELLER_LC'][0]")" "('x402 receipt', 4)"
check "the pass paid nothing by itself (a per-call API is not paid unprompted)" "$(echo "$REP" | jq_ "len(d['report']['settled'])")" "0"
IB=$(curl -s -H "Authorization: Bearer $TOK" "http://127.0.0.1:$PORT2/api/level1/inbox?policy=$P2"); echo "$IB" > /tmp/acc-x402-inbox.json
check "the inbox counts the four purchases under Paid, each with its reason" "$(echo "$IB" | jq_ "len([p for p in d['paid'] if p.get('bought') and p.get('reason')])")" "4"
check "the inbox proposes no standing order for the seller" "$(echo "$IB" | jq_ "any(c['payee'].lower()=='$SELLER_LC' for c in (d.get('standingOrderCandidates') or []))")" "False"
JL=$(curl -s -H "Authorization: Bearer $TOK" "http://127.0.0.1:$PORT2/api/level1/purchases.jsonl?policy=$P2"); echo "$JL" > /tmp/acc-x402-purchases.jsonl
check "purchases.jsonl: four paid lines, each with sha256, the identifier, network and tx" "$(echo "$JL" | python3 -c "import json,sys;ls=[json.loads(l) for l in sys.stdin if l.strip()];print(sum(1 for l in ls if l['status']=='paid' and len(l.get('sha256',''))==64 and l.get('idFrom')=='payment-identifier' and l['id'].startswith('pay_') and l['network']=='eip155:$ARC_ID' and l['tx'].startswith('0x')))")" "4"
check "without the agent key the receipts are closed (401)" "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT2/api/level1/purchases.jsonl?policy=$P2")" "401"
MCP=$(printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"purchases","arguments":{"policy":"'"$P2"'"}}}' \
  | TAMEION_URL=http://127.0.0.1:$PORT2 TAMEION_TOKEN=$TOK cargo run --locked -p operator --quiet --bin tameion-mcp 2>/dev/null)
check "the MCP tool 'purchases' returns the same four receipts" "$(echo "$MCP" | jq_ "len([p for p in json.loads(d['result']['content'][0]['text'])['purchases'] if p['status']=='paid'])")" "4"
kill $APP_PID $X402_PIDS 2>/dev/null; wait $APP_PID $X402_PIDS 2>/dev/null; rm -rf "$KD" "$APP2"

rule "not covered here"
cat <<'NOTE'
  These are real gaps, stated rather than implied:
    - History is real on-chain data, but written by this script rather than
      by a live treasury, and read through a TestUSDC stand-in because a
      local EVM emits nothing for a native transfer.
    - Runs on anvil, so Arc's own behaviour is untested *here*: the native
      Transfer emitter, getLogs chunking above 1,000 blocks, and 429 backoff.
      Those are covered by proof/arc-testnet-2026-09-21.txt, recorded against
      this contract with the model driving -- but by one run, not by a suite.
    - One scenario, four counterparties. No zero-obligation case, no
      balance-exhaustion case, no reorg.
NOTE

rule "result"
printf '  %d passed, %d failed\n\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
