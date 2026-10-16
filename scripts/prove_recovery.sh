#!/usr/bin/env bash
# The lost-Mac drill (docs/104 §C). An account whose only key is a passkey names two guardians with one passkey
# signature; the passkey is lost; the two guardians, each from its own wallet, name a new passkey; after the delay
# anyone applies it; the new key signs a trust action that lands, and the old key is refused.
#
#   scripts/prove_recovery.sh arbsep        # the whole drill on Arbitrum Sepolia; writes proof/recovery-drill-<date>.txt
#
# Keys (never printed, never in the tree):
#   guardians      ~/.tameion/test-keys/guardianA.key, guardianB.key (made with `cast wallet new`, mode 600); topped up
#                  to 0.002 ETH from the relayer when low
#   old, new key   software P-256 passkeys made for this run under ~/.tameion/test-keys/recovery/ (mode 600), signing in
#                  the WebAuthn shape the contract checks (as ~/.tameion/test-keys/soft_passkey.py does)
#   relayer        ~/.tameion/treasury.key: pays for the deploy and relays the passkey's calls. It holds no authority:
#                  it is never a guardian and the account names no recovery address (0x0), so nothing of ours can
#                  replace the key (docs/102 D10)
# The service (TAMEION_URL, default http://127.0.0.1:7880) is used only for what the Keys panel uses:
# /api/level1/prepare-guardians, /api/level1/challenge, /api/level1/execute — and every challenge it hands out is
# checked against the chain's own `challengeFor` before anything signs. The guardians and `applyRecovery` talk to
# the chain directly: recovery needs nothing of ours.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

[[ ${1:-} == arbsep ]] || { sed -n '2,20p' "$0"; exit 2; }
RPC_URL=${RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}
CHAIN=421614
CHAIN_KEY=arbitrum-sepolia
EXPLORER=https://sepolia.arbiscan.io
USDC=0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d   # Circle's USDC on Arbitrum Sepolia (prove_passkey.sh)
REGISTRY=0x8004A818BFB912233c491871b3d84c89A494BD9e
SVC=${TAMEION_URL:-http://127.0.0.1:7880}
PY=/opt/homebrew/bin/python3                       # has `cryptography` (docs/60 P-34)
RECOVERY_DELAY=${RECOVERY_DELAY:-60}               # testnet: one minute, so the drill fits a session
KEYS=$HOME/.tameion/test-keys
RUN=$(date +%s)
PROOF=${PROOF:-proof/recovery-drill-$(date +%Y-%m-%d).txt}
mkdir -p runtime/recovery "$KEYS/recovery"
chmod 700 "$KEYS/recovery"
exec > >(tee "$PROOF") 2>&1

RELAYER_PK=$(cat "$HOME/.tameion/treasury.key")
GA_PK=$(cat "$KEYS/guardianA.key")
GB_PK=$(cat "$KEYS/guardianB.key")
RELAYER=$(cast wallet address --private-key "$RELAYER_PK")
GA=$(cast wallet address --private-key "$GA_PK")
GB=$(cast wallet address --private-key "$GB_PK")
AGENT=$(cat "$HOME/.tameion/agent.address")
AGENT_ID=$(cat runtime/arbsep_agent_id)
EXECUTE="execute(address,uint256,bytes,(bytes32,bytes32,uint256,uint256,bytes,string))"
PAYEE=0x1111111111111111111111111111111111111111   # what the new key approves
OTHER=0x2222222222222222222222222222222222222222   # what the old key tries to approve

rule() { printf '\n== %s ==\n' "$1"; }
lc() { tr 'A-F' 'a-f' <<<"$1"; }
die() { echo "FAILED: $*"; exit 1; }
call() { cast call "$OWNER" "$@" --rpc-url "$RPC_URL"; }
eth() { cast balance "$1" --rpc-url "$RPC_URL" -e; }
link() { echo "$EXPLORER/tx/$1"; }
send() {   # <key> <to> <sig> [args…] -> tx hash, only if the receipt says success
  local pk=$1 out; shift
  out=$(cast send "$@" --rpc-url "$RPC_URL" --private-key "$pk" --json) || die "cast send $2: ${out:0:300}"
  [[ $(jq -r .status <<<"$out") == 0x1 ]] || die "transaction $(jq -r .transactionHash <<<"$out") reverted"
  jq -r .transactionHash <<<"$out"
}
post() {   # <path> <json> -> body (the service's own words on an error)
  curl -sS -m 300 -X POST "$SVC$1" -H 'content-type: application/json' -d "$2"
}
state() {
  local n i list=""
  n=$(call 'guardianCount()(uint256)' | awk '{print $1}')
  for ((i = 0; i < n; i++)); do list+="$(call 'guardians(uint256)(address)' "$i") "; done
  echo "  key 0 qx $(call 'qx()(bytes32)')"
  echo "        qy $(call 'qy()(bytes32)')"
  echo "  guardians [${list% }]  threshold $(call 'guardianThreshold()(uint8)')  epoch $(call 'recoveryEpoch()(uint64)' | awk '{print $1}')"
  echo "  recovery $(call 'recovery()(address)')  pending (qx, qy, eta) $(call 'pendingKey()(bytes32,bytes32,uint64)' | awk '{print $1}' | paste -s -d ' ' -)  nonce $(call 'nonce()(uint256)' | awk '{print $1}')"
}

# A refusal is a revert at eth_call against live state: nothing mined, nothing spent, and the reason is the
# contract's own error, matched by selector.
ERRORS="NotGuardian() AlreadyApproved() TooEarly(uint64) NotRecovery() BadPasskeySignature() NothingPending() NotPasskeyOrRecovery() BadGuardians()"
why() {
  local out sel e
  if out=$("$@" 2>&1); then echo "  NOT REFUSED (the call succeeded)"; return 1; fi
  for e in $ERRORS; do
    sel=$(cast sig "$e")
    grep -q "${sel#0x}" <<<"$out" && { echo "  refused: ${e%%(*} ($sel)"; return 0; }
  done
  echo "  refused, unrecognised: $(grep -oE 'execution reverted[^"]*|0x[0-9a-f]{8}' <<<"$out" | head -1)"
  return 1
}

keygen() {   # <pem> -> "qx qy" of a new P-256 key written mode 600 (the private key is never printed)
  "$PY" - "$1" <<'PY'
import os, sys
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ec
k = ec.generate_private_key(ec.SECP256R1())
fd = os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
os.write(fd, k.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
os.close(fd)
n = k.public_key().public_numbers()
print("0x%064x 0x%064x" % (n.x, n.y))
PY
}

sign() {   # <pem> <challenge> -> the assertion JSON a passkey returns (UP+UV flags, low s), as soft_passkey.py builds it
  "$PY" - "$1" "$2" <<'PY'
import base64, hashlib, json, sys
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature
N = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551
key = serialization.load_pem_private_key(open(sys.argv[1], "rb").read(), password=None)
ch = bytes.fromhex(sys.argv[2][2:])
client = json.dumps({"type": "webauthn.get", "challenge": base64.urlsafe_b64encode(ch).rstrip(b"=").decode(),
                     "origin": "http://localhost:7880", "crossOrigin": False}, separators=(",", ":"))
auth = hashlib.sha256(b"localhost").digest() + bytes([0x05]) + (0).to_bytes(4, "big")
r, s = decode_dss_signature(key.sign(auth + hashlib.sha256(client.encode()).digest(), ec.ECDSA(hashes.SHA256())))
s = min(s, N - s)
print(json.dumps({"r": "0x%064x" % r, "s": "0x%064x" % s, "challengeIndex": client.index('"challenge":"'),
                  "typeIndex": client.index('"type":"webauthn.get"'), "authenticatorData": "0x" + auth.hex(), "clientDataJSON": client}))
PY
}

challenge() {   # <target> <calldata> -> the chain's challenge for the owner's next approval
  call 'challengeFor(address,uint256,bytes,uint256)(bytes32)' "$1" 0 "$2" "$(call 'nonce()(uint256)' | awk '{print $1}')"
}

owner_call() {   # <pem> <target> <calldata> -> tx hash; through the service, as the Keys panel does
  local pem=$1 target=$2 data=$3 ch auth out
  ch=$(post /api/level1/challenge "{\"chain\":\"$CHAIN_KEY\",\"owner\":\"$OWNER\",\"target\":\"$target\",\"data\":\"$data\"}" | jq -r .challenge)
  [[ $(lc "$ch") == $(lc "$(challenge "$target" "$data")") ]] || die "the service's challenge $ch is not the chain's"
  auth=$(sign "$pem" "$ch")
  out=$(post /api/level1/execute "{\"chain\":\"$CHAIN_KEY\",\"owner\":\"$OWNER\",\"target\":\"$target\",\"data\":\"$data\",\"auth\":$auth,\"keyIndex\":0}")
  jq -e -r .tx <<<"$out" 2>/dev/null || die "the service did not relay it: $out"
}

exec_data() {   # <target> <calldata> <auth.json> -> execute() calldata; clientDataJSON as bytes (ABI-identical to string)
  local a=$3 client
  client=0x$(jq -j .clientDataJSON "$a" | xxd -p | tr -d '\n')
  echo "$(cast sig "$EXECUTE")$(cast abi-encode "f(address,uint256,bytes,(bytes32,bytes32,uint256,uint256,bytes,bytes))" \
    "$1" 0 "$2" "($(jq -r .r "$a"),$(jq -r .s "$a"),$(jq -r .challengeIndex "$a"),$(jq -r .typeIndex "$a"),$(jq -r .authenticatorData "$a"),$client)" | cut -c3-)"
}

# ---------------------------------------------------------------------------------------------------------------------
echo "# The lost-Mac drill — $(date '+%Y-%m-%d %H:%M:%S %z'), Arbitrum Sepolia ($CHAIN), scripts/prove_recovery.sh"
[[ $(cast chain-id --rpc-url "$RPC_URL") == "$CHAIN" ]] || die "$RPC_URL is not chain $CHAIN"
curl -fsS -m 5 "$SVC/healthz" >/dev/null || die "the service is not answering at $SVC"
[[ $(cast call "$REGISTRY" 'ownerOf(uint256)(address)' "$AGENT_ID" --rpc-url "$RPC_URL" | tr 'A-F' 'a-f') == $(lc "$RELAYER") ]] \
  || die "ERC-8004 agent $AGENT_ID is not the relayer's"
for g in "$GA" "$GB"; do [[ $(lc "$g") != $(lc "$RELAYER") ]] || die "a guardian is the relayer"; done

rule "0. who is who"
echo "  relayer    $RELAYER  $(eth "$RELAYER") ETH — pays gas, relays; no authority over the account"
echo "  guardian A $GA  (its own wallet; key file guardianA.key, never printed)"
echo "  guardian B $GB  (its own wallet; key file guardianB.key)"
echo "  service    $SVC  (prepare-guardians, challenge, execute — the Keys panel's routes)"
for pair in "A $GA" "B $GB"; do
  read -r name addr <<<"$pair"
  if python3 -c "import sys; sys.exit(0 if float('$(eth "$addr")') < 0.0015 else 1)"; then
    tx=$(send "$RELAYER_PK" "$addr" --value 0.002ether)
    echo "  funded guardian $name with 0.002 ETH from the relayer: tx $tx  $(link "$tx")"
  fi
  echo "  guardian $name holds $(eth "$addr") ETH for its own gas"
done

rule "1. the old passkey owns a new account (recovery address 0x0: no one of ours can replace the key)"
OLD_PEM=$KEYS/recovery/old-$RUN.pem
read -r OQX OQY <<<"$(keygen "$OLD_PEM")"
echo "  old key (this run's software passkey, $OLD_PEM)"
echo "    qx $OQX"
echo "    qy $OQY"
OUT=$(cd contracts && forge script script/PasskeyPolicy.s.sol \
  --sig "deployToken(address,bytes32,bytes32,address,uint256,address,address,uint256,uint256,uint256,uint256,uint256,uint8)" \
  "$USDC" "$OQX" "$OQY" 0x0000000000000000000000000000000000000000 "$RECOVERY_DELAY" "$AGENT" "$RELAYER" "$AGENT_ID" 2000000 5000000 0 60 1 \
  --rpc-url "$RPC_URL" --private-key "$RELAYER_PK" --broadcast 2>&1) || { echo "$OUT" | tail -20; die "deploy"; }
OWNER=$(awk '/^ +owner /{print $2}' <<<"$OUT"); POLICY=$(awk '/^ +policy /{print $2}' <<<"$OUT")
[[ -n $OWNER && -n $POLICY ]] || { echo "$OUT"; die "deploy printed no addresses"; }
jq -r '.receipts[] | "  tx \(.transactionHash)  creates \(.contractAddress)  gas \(.gasUsed)"' \
  "contracts/broadcast/PasskeyPolicy.s.sol/$CHAIN/deployToken-latest.json"
echo "  PasskeyPrincipal  $OWNER  $EXPLORER/address/$OWNER"
echo "  BudgetPolicyToken $POLICY  $EXPLORER/address/$POLICY  (principal $(cast call "$POLICY" 'principal()(address)' --rpc-url "$RPC_URL"))"
echo "  recoveryDelay $(call 'recoveryDelay()(uint256)' | awk '{print $1}') s"
printf 'OWNER=%s\nPOLICY=%s\nOLD_PEM=%s\n' "$OWNER" "$POLICY" "$OLD_PEM" > runtime/recovery/last.env
state

rule "2. before any guardian: nobody of ours can touch the key"
echo "guardian A approves a key before it is a guardian"
why cast call "$OWNER" 'approveRecovery(bytes32,bytes32)' "$OQX" "$OQY" --from "$GA" --rpc-url "$RPC_URL"
echo "the relayer proposes a key of its own (the old testnet recovery path)"
why cast call "$OWNER" 'proposeRecovery(bytes32,bytes32)' "$OQX" "$OQY" --from "$RELAYER" --rpc-url "$RPC_URL"

rule "3. one passkey signature names the two guardians, 2 of 2 (the Keys panel's tap)"
PREP=$(post /api/level1/prepare-guardians "{\"owner\":\"$OWNER\",\"guardians\":[\"$GA\",\"$GB\"],\"threshold\":2}")
echo "  prepare-guardians: $(jq -r .summary <<<"$PREP")"
[[ $(lc "$(jq -r .to <<<"$PREP")") == $(lc "$OWNER") ]] || die "prepared for another contract"
[[ $(jq -r .data <<<"$PREP") == $(cast calldata 'setGuardians(address[],uint8)' "[$GA,$GB]" 2) ]] || die "prepared calldata is not setGuardians([A,B],2)"
echo "  calldata checked against cast's own setGuardians([A,B],2): same"
TX=$(owner_call "$OLD_PEM" "$OWNER" "$(jq -r .data <<<"$PREP")")
echo "  setGuardians tx $TX  $(link "$TX")"
state
echo "the relayer is not a guardian: it approves a key"
why cast call "$OWNER" 'approveRecovery(bytes32,bytes32)' "$OQX" "$OQY" --from "$RELAYER" --rpc-url "$RPC_URL"

rule "4. the Mac is gone: the old key is lost (from here on nothing uses it but the last refusal)"
NEW_PEM=$KEYS/recovery/new-$RUN.pem
read -r NQX NQY <<<"$(keygen "$NEW_PEM")"
echo "  new key, made on the replacement device ($NEW_PEM)"
echo "    qx $NQX"
echo "    qy $NQY"
printf 'NEW_PEM=%s\n' "$NEW_PEM" >> runtime/recovery/last.env
T0=$(date +%s)
TX=$(send "$GA_PK" "$OWNER" 'approveRecovery(bytes32,bytes32)' "$NQX" "$NQY")
echo "  guardian A approveRecovery(new) tx $TX  $(link "$TX")"
BALLOT=$(call 'recoveryBallot(bytes32,bytes32)(bytes32)' "$NQX" "$NQY")
echo "  votes for the new key: $(call 'guardianApprovals(bytes32)(uint8)' "$BALLOT") of $(call 'guardianThreshold()(uint8)'); pending eta $(call 'pendingKey()(bytes32,bytes32,uint64)' | sed -n 3p | awk '{print $1}') (0 = nothing pending: one guardian alone changes nothing)"
echo "guardian A votes again"
why cast call "$OWNER" 'approveRecovery(bytes32,bytes32)' "$NQX" "$NQY" --from "$GA" --rpc-url "$RPC_URL"
TX=$(send "$GB_PK" "$OWNER" 'approveRecovery(bytes32,bytes32)' "$NQX" "$NQY")
echo "  guardian B approveRecovery(new) tx $TX  $(link "$TX")"
ETA=$(call 'pendingKey()(bytes32,bytes32,uint64)' | sed -n 3p | awk '{print $1}')
[[ $ETA -gt 0 ]] || die "two votes and nothing pending"
echo "  votes: $(call 'guardianApprovals(bytes32)(uint8)' "$BALLOT") of 2 → pending, applies at unix $ETA ($(date -r "$ETA" '+%H:%M:%S %z'))"
echo "applying before the delay ends"
why cast call "$OWNER" 'applyRecovery()' --from "$GB" --rpc-url "$RPC_URL"

rule "5. the delay passes (the window in which a holder of the old key could cancel); anyone applies"
while (( $(cast block latest --field timestamp --rpc-url "$RPC_URL") < ETA )); do sleep 5; done
TX=$(send "$GB_PK" "$OWNER" 'applyRecovery()')
T1=$(date +%s)
echo "  applyRecovery tx $TX  $(link "$TX")  (sent by guardian B; anyone may)"
state
[[ $(lc "$(call 'qx()(bytes32)')") == $(lc "$NQX") && $(lc "$(call 'qy()(bytes32)')") == $(lc "$NQY") ]] || die "key 0 is not the new key"
echo "  key 0 is the new key; the guardians stay; recovery took $((T1 - T0)) s from the first vote ($RECOVERY_DELAY s of it the delay)"

rule "6. the new key signs a trust action: approve payee $PAYEE on the policy"
echo "  attestedPayee before: $(cast call "$POLICY" 'attestedPayee(address)(bool)' "$PAYEE" --rpc-url "$RPC_URL")"
TX=$(owner_call "$NEW_PEM" "$POLICY" "$(cast calldata 'setPayee(address,bool)' "$PAYEE" true)")
echo "  setPayee tx $TX  $(link "$TX")"
echo "  attestedPayee after:  $(cast call "$POLICY" 'attestedPayee(address)(bool)' "$PAYEE" --rpc-url "$RPC_URL"); owner nonce $(call 'nonce()(uint256)' | awk '{print $1}')"

rule "7. the old key is refused (a thief who kept it gets nothing)"
DATA=$(cast calldata 'setPayee(address,bool)' "$OTHER" true)
CH=$(challenge "$POLICY" "$DATA")
sign "$OLD_PEM" "$CH" > runtime/recovery/old-auth.json
echo "on chain: the old key signs setPayee($OTHER) as the next approval, eth_call execute"
why cast call "$OWNER" "$(exec_data "$POLICY" "$DATA" runtime/recovery/old-auth.json)" --from "$RELAYER" --rpc-url "$RPC_URL"
sign "$NEW_PEM" "$CH" > runtime/recovery/new-auth.json
echo "control: the new key signs that same challenge (eth_call only, not sent)"
if cast call "$OWNER" "$(exec_data "$POLICY" "$DATA" runtime/recovery/new-auth.json)" --from "$RELAYER" --rpc-url "$RPC_URL" >/dev/null 2>&1; then
  echo "  accepted — the refusal above is about the key, nothing else"
else die "the new key's own signature was refused"; fi
echo "through the service: the old key's assertion sent to /api/level1/execute"
R=$(post /api/level1/execute "{\"chain\":\"$CHAIN_KEY\",\"owner\":\"$OWNER\",\"target\":\"$POLICY\",\"data\":\"$DATA\",\"auth\":$(cat runtime/recovery/old-auth.json),\"keyIndex\":0}")
jq -e .tx <<<"$R" >/dev/null 2>&1 && die "the service relayed the old key: $R"
echo "  refused by the service: $(jq -r .error <<<"$R")"
echo "  attestedPayee($OTHER) = $(cast call "$POLICY" 'attestedPayee(address)(bool)' "$OTHER" --rpc-url "$RPC_URL")"

rule "result"
echo "  PASS — two guardians replaced a lost passkey on chain with no help from us; the new key signs, the old one cannot."
echo "  relayer now $(eth "$RELAYER") ETH; guardian A $(eth "$GA"); guardian B $(eth "$GB")"
