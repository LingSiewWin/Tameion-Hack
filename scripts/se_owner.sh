#!/usr/bin/env bash
# The account's owner key lives in this Mac's Secure Enclave.
#
# The same PasskeyPrincipal + BudgetPolicyToken that prove_passkey.sh deploys
# for a phone's passkey, owned instead by a P-256 key the Enclave made and
# will not export (apps/desk-se). No contract change: the Enclave signs the
# same WebAuthn-shaped assertion a browser passkey does.
#
#   scripts/se_owner.sh arbsep key                # make the Enclave key (once; refuses to replace one)
#   scripts/se_owner.sh arbsep deploy             # owner (the Enclave key) + USDC policy
#   scripts/se_owner.sh arbsep approve <payee>    # the Enclave signs setPayee(payee); the relayer submits
#   scripts/se_owner.sh arbsep refuse             # a software key, a replay, a re-point, the relayer: all refused
#
# State: runtime/se/<net>.env (kept apart from runtime/passkey/, the phone's run).
# The relayer (~/.tameion/treasury.key) pays gas and holds no authority.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
mkdir -p runtime/se

NET=${1:-}; CMD=${2:-}
case "$NET" in
  arbsep)
    RPC_URL=${RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}
    CHAIN=421614
    USDC=0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d   # Circle's USDC on Arbitrum Sepolia
    EXPLORER=https://sepolia.arbiscan.io
    ;;
  # ponytail: Arbitrum Sepolia only; Arc needs the native-USDC deploy branch of prove_passkey.sh.
  *) sed -n '2,15p' "$0"; exit 2 ;;
esac
RELAYER_PK=$(cat ~/.tameion/treasury.key)
RELAYER=$(cast wallet address --private-key "$RELAYER_PK")
AGENT=$(cat ~/.tameion/agent.address)
REGISTRY=0x8004A818BFB912233c491871b3d84c89A494BD9e
STATE=runtime/se/$NET.env
PER_ACTION=${PER_ACTION:-2000000}; DAILY=${DAILY:-5000000}   # 2 and 5 USDC
DELAY=${DELAY:-60}; MODE=${MODE:-2}                          # testnet delay, as prove_passkey.sh
RECOVERY_DELAY=${RECOVERY_DELAY:-60}
EXECUTE="execute(address,uint256,bytes,(bytes32,bytes32,uint256,uint256,bytes,string))"

# The Enclave CLI, rebuilt when its source is newer.
SE=target/desk-se
[[ -x $SE && $SE -nt apps/desk-se/main.swift ]] || swiftc -O apps/desk-se/main.swift -o "$SE"

rule() { printf '\n== %s ==\n' "$1"; }
need_state() { [[ -s $STATE ]] || { echo "no deployment on $NET yet: $0 $NET deploy" >&2; exit 1; }
               # shellcheck disable=SC1090
               source "$STATE"; }
[[ $(cast chain-id --rpc-url "$RPC_URL") == "$CHAIN" ]] || { echo "$RPC_URL is not chain $CHAIN" >&2; exit 1; }

# execute() calldata from an assertion file. clientDataJSON goes in as bytes:
# ABI-encoded string and bytes are identical, and hex keeps cast's tuple
# parser away from the JSON's quotes and commas.
exec_data() {   # <calldata> <auth.json>
  local a=$2 client
  client=0x$(jq -j .clientDataJSON "$a" | xxd -p | tr -d '\n')
  echo "$(cast sig "$EXECUTE")$(cast abi-encode "f(address,uint256,bytes,(bytes32,bytes32,uint256,uint256,bytes,bytes))" \
    "$POLICY" 0 "$1" "($(jq -r .r "$a"),$(jq -r .s "$a"),$(jq -r .challengeIndex "$a"),$(jq -r .typeIndex "$a"),$(jq -r .authenticatorData "$a"),$client)" | cut -c3-)"
}

# A refusal is a revert at eth_call against live state: nothing mined, nothing
# spent, and the reason is the contract's own error, matched by selector.
ERRORS="BadPasskeySignature() NotPrincipal() NeedsMoreApprovals(uint8,uint8) CallFailed(bytes)"
why() {
  local out sel e
  if out=$("$@" 2>&1); then echo "  NOT REFUSED (call succeeded)"; return 1; fi
  for e in $ERRORS; do
    sel=$(cast sig "$e")
    grep -q "${sel#0x}" <<<"$out" && { echo "  refused: ${e%%(*} ($sel)"; return 0; }
  done
  echo "  refused, unrecognised: $(grep -oE 'execution reverted[^"]*|0x[0-9a-f]{8}' <<<"$out" | head -1)"
}

challenge_for() {   # <calldata> -> challenge for the owner's next approval
  cast call "$OWNER" "challengeFor(address,uint256,bytes,uint256)(bytes32)" "$POLICY" 0 "$1" \
    "$(cast call "$OWNER" 'nonce()(uint256)' --rpc-url "$RPC_URL")" --rpc-url "$RPC_URL"
}

case "$CMD" in
key)
  "$SE" create --no-presence   # unattended proof; a real owner key omits it (Touch ID)
  ;;

deploy)
  read -r QX QY <<<"$("$SE" pub | awk '{print $2}' | tr '\n' ' ')"
  AGENT_ID=$(cat "runtime/${NET}_agent_id")
  [[ $(cast call "$REGISTRY" "ownerOf(uint256)(address)" "$AGENT_ID" --rpc-url "$RPC_URL" | tr 'A-F' 'a-f') == $(tr 'A-F' 'a-f' <<<"$RELAYER") ]] \
    || { echo "ERC-8004 agent $AGENT_ID is not the relayer's" >&2; exit 1; }
  rule "deploy on chain $CHAIN: the Secure Enclave key owns the policy"
  echo "  Enclave public key  qx $QX"
  echo "                      qy $QY"
  echo "  relayer $RELAYER holds $(cast balance "$RELAYER" --rpc-url "$RPC_URL" -e) ETH (gas only, no authority)"
  START=$(cast block-number --rpc-url "$RPC_URL")
  OUT=$(cd contracts && forge script script/PasskeyPolicy.s.sol \
    --sig "deployToken(address,bytes32,bytes32,address,uint256,address,address,uint256,uint256,uint256,uint256,uint256,uint8)" \
    "$USDC" "$QX" "$QY" "$RELAYER" "$RECOVERY_DELAY" "$AGENT" "$RELAYER" "$AGENT_ID" "$PER_ACTION" "$DAILY" 0 "$DELAY" "$MODE" \
    --rpc-url "$RPC_URL" --private-key "$RELAYER_PK" --broadcast 2>&1) || { echo "$OUT" | tail -20 >&2; exit 1; }
  OWNER=$(awk '/^ +owner /{print $2}' <<<"$OUT"); POLICY=$(awk '/^ +policy /{print $2}' <<<"$OUT")
  [[ -n $OWNER && -n $POLICY ]] || { echo "$OUT" >&2; exit 1; }
  printf 'OWNER=%s\nPOLICY=%s\nSTART=%s\nQX=%s\nQY=%s\n' "$OWNER" "$POLICY" "$START" "$QX" "$QY" > "$STATE"
  jq -r '.receipts[] | "  tx \(.transactionHash)  creates \(.contractAddress)  gas \(.gasUsed)"' \
    "contracts/broadcast/PasskeyPolicy.s.sol/$CHAIN/deployToken-latest.json"
  echo "  PasskeyPrincipal  $OWNER   $EXPLORER/address/$OWNER"
  echo "  BudgetPolicyToken $POLICY   $EXPLORER/address/$POLICY"
  echo "  read back: owner.qx()        $(cast call "$OWNER" 'qx()(bytes32)' --rpc-url "$RPC_URL")"
  echo "             owner.qy()        $(cast call "$OWNER" 'qy()(bytes32)' --rpc-url "$RPC_URL")"
  echo "             policy.principal() $(cast call "$POLICY" 'principal()(address)' --rpc-url "$RPC_URL")"
  echo "             policy.agent()     $(cast call "$POLICY" 'agent()(address)' --rpc-url "$RPC_URL")"
  echo "             policy.operator()  $(cast call "$POLICY" 'operator()(address)' --rpc-url "$RPC_URL")"
  ;;

approve)
  need_state
  PAYEE=${3:?payee address}
  DATA=$(cast calldata "setPayee(address,bool)" "$PAYEE" true)
  N=$(cast call "$OWNER" 'nonce()(uint256)' --rpc-url "$RPC_URL")
  C=$(challenge_for "$DATA")
  rule "the Secure Enclave approves payee $PAYEE (approval #$N)"
  echo "  challenge = owner.challengeFor(policy, 0, setPayee($PAYEE,true), $N) = $C"
  t0=$(python3 -c 'import time; print(time.time())')
  "$SE" sign "$C" --unattended > "runtime/se/$NET-auth.json"
  echo "  signed in the Enclave in $(python3 -c "import time; print(round((time.time()-$t0)*1000))") ms (process start included)"
  jq -c '{r, s, authenticatorData}' "runtime/se/$NET-auth.json" | sed 's/^/  /'
  echo "  before: attestedPayee($PAYEE) = $(cast call "$POLICY" 'attestedPayee(address)(bool)' "$PAYEE" --rpc-url "$RPC_URL")"
  OUT=$(cast send "$OWNER" "$(exec_data "$DATA" "runtime/se/$NET-auth.json")" \
    --rpc-url "$RPC_URL" --private-key "$RELAYER_PK" --json)
  TX=$(jq -r .transactionHash <<<"$OUT")
  echo "  status $(jq -r .status <<<"$OUT")  gas $(cast to-dec "$(jq -r .gasUsed <<<"$OUT")")  block $(cast to-dec "$(jq -r .blockNumber <<<"$OUT")")"
  echo "  after:  attestedPayee($PAYEE) = $(cast call "$POLICY" 'attestedPayee(address)(bool)' "$PAYEE" --rpc-url "$RPC_URL")"
  echo "  owner.nonce() = $(cast call "$OWNER" 'nonce()(uint256)' --rpc-url "$RPC_URL")"
  echo "  tx $TX   $EXPLORER/tx/$TX"
  printf 'LAST_DATA=%s\n' "$DATA" > "runtime/se/$NET.last"
  cp "runtime/se/$NET-auth.json" "runtime/se/$NET-last-auth.json"
  ;;

refuse)
  need_state
  # shellcheck disable=SC1090
  source "runtime/se/$NET.last"
  OTHER=0x000000000000000000000000000000000000dEaD
  DATA=$(cast calldata "setPayee(address,bool)" "$OTHER" true)
  C=$(challenge_for "$DATA")
  rule "every other key and every other call, against live state (eth_call, nothing mined)"
  echo "  next approval's challenge for setPayee($OTHER): $C"
  "$SE" sign "$C" --unattended > "runtime/se/$NET-next-auth.json"
  # A software P-256 key made here, in memory, never written: the right
  # shape, the right challenge, the wrong key.
  /opt/homebrew/bin/python3 - "$C" > "runtime/se/$NET-soft-auth.json" <<'PY'
import base64, hashlib, json, sys
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature
N = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551
key = ec.generate_private_key(ec.SECP256R1())
b64 = base64.urlsafe_b64encode(bytes.fromhex(sys.argv[1][2:])).rstrip(b"=").decode()
client = json.dumps({"type": "webauthn.get", "challenge": b64, "origin": "http://localhost:7880", "crossOrigin": False}, separators=(",", ":"))
auth = hashlib.sha256(b"localhost").digest() + bytes([0x05]) + bytes(4)
r, s = decode_dss_signature(key.sign(auth + hashlib.sha256(client.encode()).digest(), ec.ECDSA(hashes.SHA256())))
s = min(s, N - s)
print(json.dumps({"r": "0x%064x" % r, "s": "0x%064x" % s, "challengeIndex": client.index('"challenge":"'),
                  "typeIndex": client.index('"type":"webauthn.get"'), "authenticatorData": "0x" + auth.hex(), "clientDataJSON": client}))
PY
  call() { cast call "$OWNER" "$1" --from "$RELAYER" --rpc-url "$RPC_URL"; }
  echo "1. a software P-256 key signs the same challenge, same WebAuthn shape"
  why call "$(exec_data "$DATA" "runtime/se/$NET-soft-auth.json")"
  echo "2. control: the Enclave signs that challenge (simulated, not sent)"
  if call "$(exec_data "$DATA" "runtime/se/$NET-next-auth.json")" >/dev/null 2>&1; then echo "  accepted (eth_call returns; not broadcast)"
  else echo "  UNEXPECTED: the Enclave's own signature was refused"; fi
  echo "3. the Enclave's signature for $OTHER, re-pointed at the relayer as payee"
  why call "$(exec_data "$(cast calldata "setPayee(address,bool)" "$RELAYER" true)" "runtime/se/$NET-next-auth.json")"
  echo "4. the approval already used on chain, submitted again"
  why call "$(exec_data "$LAST_DATA" "runtime/se/$NET-last-auth.json")"
  echo "5. the relayer, which pays the gas, calls setPayee itself"
  why cast call "$POLICY" "setPayee(address,bool)" "$OTHER" true --from "$RELAYER" --rpc-url "$RPC_URL"
  echo "6. the agent calls setPayee itself"
  why cast call "$POLICY" "setPayee(address,bool)" "$OTHER" true --from "$AGENT" --rpc-url "$RPC_URL"
  echo "  attestedPayee($OTHER) = $(cast call "$POLICY" 'attestedPayee(address)(bool)' "$OTHER" --rpc-url "$RPC_URL")"
  ;;

*) sed -n '2,15p' "$0"; exit 2 ;;
esac
