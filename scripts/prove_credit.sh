#!/usr/bin/env bash
# The credit line on Arc testnet: deploy, lock -> score -> draw -> repay -> draw -> slash.
# No external lender; testnet; not a lending protocol.
#
#   TAMEION_RELAYER_KEY="$(cat ~/.tameion/treasury.key)" POLICY=0x… [GAS_PRICE=22gwei TIP=2gwei] scripts/prove_credit.sh
#
# The relayer is the account, the scorer and the judge at once (one funded key);
# the forge tests hold the roles apart. POLICY is a BudgetPolicy the relayer is
# principal of: draws land there. Amounts are USDC minor units (six decimals);
# on Arc native wei = units x 1e12. The second draw exists so the slash has a
# debt to cover: after a full repay, min(debt, deposit) is 0 and slash refuses.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# rpc.testnet.arc.network stopped resolving (NXDOMAIN) on 2026-10-17; same chain here.
RPC_URL=${RPC_URL:-https://rpc.testnet.arc.io}
EXPLORER=https://explorer.testnet.arc.io
: "${TAMEION_RELAYER_KEY:?set TAMEION_RELAYER_KEY}"
: "${POLICY:?set POLICY to a BudgetPolicy the relayer is principal of}"

W=1000000000000
CAP=50000          # 0.050000 USDC: most the account may owe
MAX_AGE=86400      # a score older than a day counts as 0
DELAY=3600         # an unlock waits an hour
FLOAT=5000         # 0.005000 USDC sent as float (never a deposit)
LOCK=10000         # 0.010000 USDC deposit
SCORE=8000         # bps -> multiplier 15000 -> limit 0.015
DRAW=5000          # 0.005000
DRAW2=2000         # 0.002000, left unpaid for the slash
RULE=1             # mandate rule id named by the slash
EVIDENCE=$(cast keccak "tameion:credit-line:testnet-dummy-evidence:2026-10-18")
# Optional fixed fees (Arc's base fee is 20 gwei): the default max fee doubles the
# base, and the node checks gas limit x max fee against the balance up front.
FEES=()
[ -n "${GAS_PRICE:-}" ] && FEES=(--gas-price "$GAS_PRICE" --priority-gas-price "${TIP:-1gwei}")

usdc() { python3 -c "print(f'{int($1)/1e6:.6f}')"; }
rule() { printf '\n== %s ==\n' "$1"; }
send() { # send <label> <cast send args…>; prints the tx and fails on a revert
  local label=$1 out tx st; shift
  out=$(cast send "$@" ${FEES[@]+"${FEES[@]}"} --rpc-url "$RPC_URL" --private-key "$TAMEION_RELAYER_KEY" --json)
  tx=$(python3 -c "import sys,json;print(json.load(sys.stdin)['transactionHash'])" <<<"$out")
  st=$(python3 -c "import sys,json;print(int(json.load(sys.stdin)['status'],16))" <<<"$out")
  gas=$(python3 -c "import sys,json;print(int(json.load(sys.stdin)['gasUsed'],16))" <<<"$out")
  printf '  %-8s tx %s  status %s  gas %s\n           %s/tx/%s\n' "$label" "$tx" "$st" "$gas" "$EXPLORER" "$tx"
  [ "$st" = 1 ] || { echo "  $label reverted"; exit 1; }
}
state() {
  printf '  deposit %s  debt %s  limit %s  totalDeposits %s  float %s  vault %s USDC\n' \
    "$(usdc "$(cast call "$LINE" 'deposit(address)(uint256)' "$ME" --rpc-url "$RPC_URL" | cut -d' ' -f1)")" \
    "$(usdc "$(cast call "$LINE" 'debt(address)(uint256)' "$ME" --rpc-url "$RPC_URL" | cut -d' ' -f1)")" \
    "$(usdc "$(cast call "$LINE" 'limit(address)(uint256)' "$ME" --rpc-url "$RPC_URL" | cut -d' ' -f1)")" \
    "$(usdc "$(cast call "$LINE" 'totalDeposits()(uint256)' --rpc-url "$RPC_URL" | cut -d' ' -f1)")" \
    "$(usdc "$(cast call "$LINE" 'available()(uint256)' --rpc-url "$RPC_URL" | cut -d' ' -f1)")" \
    "$(python3 -c "print(f'{int($(cast balance "$LINE" --rpc-url "$RPC_URL"))/1e18:.6f}')")"
}

rule "chain"
[ "$(cast chain-id --rpc-url "$RPC_URL")" = 5042002 ] || { echo "not Arc testnet"; exit 1; }
ME=$(cast wallet address --private-key "$TAMEION_RELAYER_KEY")
BAL=$(cast balance "$ME" --rpc-url "$RPC_URL")
echo "  Arc testnet 5042002 via $RPC_URL"
echo "  relayer $ME holds $(python3 -c "print(f'{int($BAL)/1e18:.6f}')") USDC"
python3 -c "import sys; sys.exit(0 if int('$BAL') >= 50 * 10**15 else 1)" || { echo "  needs >= 0.050 USDC"; exit 1; }
[ "$(cast call "$POLICY" 'principal()(address)' --rpc-url "$RPC_URL")" = "$ME" ] || { echo "  $POLICY is not the relayer's policy"; exit 1; }
POLICY_BEFORE=$(cast balance "$POLICY" --rpc-url "$RPC_URL")

rule "deploy CreditLine (scorer = judge = $ME, cap $(usdc $CAP), maxAge ${MAX_AGE}s, unlock delay ${DELAY}s)"
cd "$ROOT/contracts"
# --force: a `forge lint` leaves abi-only artifacts that a plain build calls current (docs/60 P-6).
LINE=$(forge create --force --broadcast src/credit/CreditLine.sol:CreditLine \
  ${FEES[@]+"${FEES[@]}"} --rpc-url "$RPC_URL" --private-key "$TAMEION_RELAYER_KEY" \
  --constructor-args "$ME" "$ME" $CAP $MAX_AGE $DELAY | tee /dev/stderr | awk '/Deployed to:/{print $3}')
cd "$ROOT"
echo "  CreditLine $LINE"
echo "             $EXPLORER/address/$LINE"

rule "float $(usdc $FLOAT) (anyone may send; never a deposit)"
send float "$LINE" --value $((FLOAT * W))
rule "1. lock $(usdc $LOCK)"
send lock "$LINE" "lock()" --value $((LOCK * W))
state
rule "scorer sets $SCORE bps; account registers its policy $POLICY"
send score "$LINE" "setScore(address,uint256)" "$ME" $SCORE
send policy "$LINE" "setPolicy(address)" "$POLICY"
state
rule "2. draw $(usdc $DRAW) into the policy"
send draw "$LINE" "draw(uint256)" $DRAW
state
rule "3. repay $(usdc $DRAW)"
send repay "$LINE" "repay()" --value $((DRAW * W))
state
rule "draw $(usdc $DRAW2), left unpaid"
send draw2 "$LINE" "draw(uint256)" $DRAW2
state
rule "4. slash: evidence $EVIDENCE, rule $RULE"
send slash "$LINE" "slash(address,bytes32,uint8)" "$ME" "$EVIDENCE" $RULE
state
POLICY_AFTER=$(cast balance "$POLICY" --rpc-url "$RPC_URL")
echo "  policy $POLICY received $(python3 -c "print(f'{(int($POLICY_AFTER)-int($POLICY_BEFORE))/1e18:.6f}')") USDC from the two draws"

rule "what the line refuses (eth_call, nothing sent)"
for S in "NotJudge()" "OverLimit(uint256,uint256)" "NothingToSlash()"; do echo "  selector $(cast sig "$S")  $S"; done
refused() { local out; out=$("$@" 2>&1) && { echo "  NOT REFUSED: $out"; return 1; }; grep -oE '0x[0-9a-f]{8}[0-9a-f]*' <<<"$out" | head -1 | sed 's/^/  refused: /'; }
echo "  a stranger slashes:"
refused cast call "$LINE" "slash(address,bytes32,uint8)" "$ME" "$EVIDENCE" $RULE --from 0x000000000000000000000000000000000000dEaD --rpc-url "$RPC_URL"
echo "  the account draws 0.020000, past its limit:"
refused cast call "$LINE" "draw(uint256)" 20000 --from "$ME" --rpc-url "$RPC_URL"
echo "  the judge slashes again with nothing owed:"
refused cast call "$LINE" "slash(address,bytes32,uint8)" "$ME" "$EVIDENCE" $RULE --from "$ME" --rpc-url "$RPC_URL"

rule "done"
echo "  relayer now holds $(python3 -c "print(f'{int($(cast balance "$ME" --rpc-url "$RPC_URL"))/1e18:.6f}')") USDC"
