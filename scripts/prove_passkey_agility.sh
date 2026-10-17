#!/usr/bin/env bash
# Signature agility on a live testnet (docs/117 §3). A fresh PasskeyPrincipal
# with a short delay, a Secp256k1Verifier and a P256WebAuthnVerifier; then
#   tx 1  the passkey proposes the secp256k1 verifier (execute -> proposeVerifier)
#   tx 2  after the delay, anyone applies it (the switch; the passkey alg retires)
#   tx 3  a valid passkey-signed execute is sent anyway: it reverts AlgNotActive
#   tx 4  the secp256k1 key approves the same no-op call through executeSigned
#
#   TAMEION_RELAYER_KEY="$(cat ~/.tameion/treasury.key)" scripts/prove_passkey_agility.sh arc|arbsep
#
# The relayer pays gas and holds no authority. The passkey is a software P-256
# key (~/.tameion/test-keys/agility-p256.key) producing the WebAuthn assertion a
# phone returns; the secp256k1 key is ~/.tameion/test-keys/agility-secp256k1.key.
# Both are made on first run (mode 600) and never printed.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NET=${1:-}
case "$NET" in
  arc)    RPC_URL=${RPC_URL:-https://rpc.testnet.arc.io}; CHAIN=5042002
          EXPLORER=https://explorer.testnet.arc.io; MIN_WEI=150000000000000000 ;;   # 0.15 USDC (native, 18 decimals)
  arbsep) RPC_URL=${RPC_URL:-https://sepolia-rollup.arbitrum.io/rpc}; CHAIN=421614
          EXPLORER=https://arbitrum-sepolia.blockscout.com; MIN_WEI=1000000000000000 ;;  # 0.001 ETH
  *) echo "usage: TAMEION_RELAYER_KEY=\"\$(cat ~/.tameion/treasury.key)\" $0 arc|arbsep" >&2; exit 2 ;;
esac
: "${TAMEION_RELAYER_KEY:?set TAMEION_RELAYER_KEY inside the command}"
DELAY=${DELAY:-60}
GAS3=${GAS3:-600000}   # tx 3 is sent with a fixed gas limit: estimation would refuse a call that reverts

KEYS=~/.tameion/test-keys
mk_key() {  # a random scalar in [1, n), written once, never printed
  [[ -s $1 ]] && return 0
  (umask 077; python3 -I -c 'import secrets, sys
n = int(sys.argv[1], 16); k = 0
while not 0 < k < n: k = secrets.randbelow(n)
print(hex(k))' "$2" > "$1")
}
mk_key "$KEYS/agility-p256.key" FFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551
mk_key "$KEYS/agility-secp256k1.key" FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
AGILITY_P256_KEY="$(cat "$KEYS/agility-p256.key")"; AGILITY_SECP_KEY="$(cat "$KEYS/agility-secp256k1.key")"
export AGILITY_P256_KEY AGILITY_SECP_KEY

cd "$ROOT/contracts"
forge build --quiet --force  # a prior `forge lint` leaves abi-only artifacts the cache calls current ("No contract bytecode")
log() { printf '%s\n' "$*"; }
script() { forge script script/PasskeyAgility.s.sol --sig "$@" --rpc-url "$RPC_URL" --json 2>/dev/null | jq -r 'select(.logs != null) | .logs[0]' | tail -1; }
receipt() {  # cast's JSON receipt -> one line, numbers in decimal, with the explorer link
  local r; r=$(cat)
  log "  tx $(jq -r .transactionHash <<<"$r")  status $(jq -r .status <<<"$r")  block $(cast to-dec "$(jq -r .blockNumber <<<"$r")")  gasUsed $(cast to-dec "$(jq -r .gasUsed <<<"$r")")"
  log "     $EXPLORER/tx/$(jq -r .transactionHash <<<"$r")"
}
send() { cast send --rpc-url "$RPC_URL" --private-key "$TAMEION_RELAYER_KEY" --json "$@"; }
create() {  # contract [constructor args…] -> address; logs the deploy tx
  local out; out=$(forge create "$1" --rpc-url "$RPC_URL" --private-key "$TAMEION_RELAYER_KEY" --broadcast --json ${2:+--constructor-args "${@:2}"})
  log "  $1  at $(jq -r .deployedTo <<<"$out")  tx $(jq -r .transactionHash <<<"$out")" >&2
  jq -r .deployedTo <<<"$out"
}

[[ $(cast chain-id --rpc-url "$RPC_URL") == "$CHAIN" ]] || { echo "RPC $RPC_URL is not chain $CHAIN" >&2; exit 1; }
RELAYER=$(cast wallet address --private-key "$TAMEION_RELAYER_KEY")
SIGNER=$(cast wallet address --private-key "$AGILITY_SECP_KEY")
BAL=$(cast balance "$RELAYER" --rpc-url "$RPC_URL")
log "== signature agility, $NET (chain $CHAIN), $(date '+%Y-%m-%d %H:%M:%S %z')"
log "rpc $RPC_URL   explorer $EXPLORER"
log "relayer $RELAYER balance $BAL wei   secp256k1 test signer $SIGNER"
python3 -I -c 'import sys; sys.exit(0 if int(sys.argv[1]) >= int(sys.argv[2]) else 1)' "$BAL" "$MIN_WEI" \
  || { echo "relayer holds $BAL wei, needs at least $MIN_WEI: fund $RELAYER first" >&2; exit 3; }
log "P-256 precompile probe (0x100, OpenZeppelin's Wycheproof vector): $(cast call 0x0000000000000000000000000000000000000100 0xbb5a52f42f9c9261ed4361f59422a1e30036e7c32b270c8807a419feca60502300000000000000000000000000000000000000000000000000000000000000050000000000000000000000000000000000000000000000000000000000000001a71af64de5126a4a4e02b7922d66ce9415ce88a4c9d25514d91082c8725ac9575d47723c8fbe580bb369fec9c2665d8e30a435b9932645482e7c9f11e872296b --rpc-url "$RPC_URL")"

log "-- deploy (delay ${DELAY}s; recovery = relayer)"
PK=$(script "passkey()")
QX=0x${PK:2:64}; QY=0x${PK:66:64}
OWNER=$(create src/PasskeyPrincipal.sol:PasskeyPrincipal "$QX" "$QY" "$RELAYER" "$DELAY")
SECPV=$(create src/verifiers/Secp256k1Verifier.sol:Secp256k1Verifier)
P256V=$(create src/verifiers/P256WebAuthnVerifier.sol:P256WebAuthnVerifier)
P256_ALG=$(cast call "$OWNER" "P256_WEBAUTHN()(bytes32)" --rpc-url "$RPC_URL")
SECP_ALG=$(cast keccak secp256k1)
log "  activeAlg $(cast call "$OWNER" "activeAlg()(bytes32)" --rpc-url "$RPC_URL")  (P256_WEBAUTHN = keccak256(\"p256-webauthn\") = $P256_ALG)"

log "-- tx 1: register (the passkey proposes Secp256k1Verifier for $SIGNER)"
send "$OWNER" "$(script "proposeCalldata(address,address,address)" "$OWNER" "$SECPV" "$SIGNER")" | receipt
ETA=$(cast call "$OWNER" "pendingAlg()(bytes32,address,uint64,bytes)" --rpc-url "$RPC_URL" | sed -n 3p | awk '{print $1}')
log "  pendingAlg eta $ETA"

log "-- tx 2: switch (anyone applies after the delay)"
until (( $(cast block latest -f timestamp --rpc-url "$RPC_URL") >= ETA )); do sleep 5; done
send "$OWNER" "applyVerifier()" | receipt
log "  activeAlg $(cast call "$OWNER" "activeAlg()(bytes32)" --rpc-url "$RPC_URL")  (keccak256(\"secp256k1\") = $SECP_ALG)"
log "  retired(P256_WEBAUTHN) $(cast call "$OWNER" "retired(bytes32)(bool)" "$P256_ALG" --rpc-url "$RPC_URL")"
log "  verifiers(secp256k1) $(cast call "$OWNER" "verifiers(bytes32)(address)" "$SECP_ALG" --rpc-url "$RPC_URL")"

log "-- tx 3: the old algorithm (a valid passkey assertion over the next challenge) must revert"
OLD=$(script "passkeyExecuteCalldata(address,address)" "$OWNER" "$RELAYER")
R3=$(send "$OWNER" "$OLD" --gas-limit "$GAS3" || true)
receipt <<<"$R3"
WHY=$(cast call "$OWNER" "$OLD" --from "$RELAYER" --rpc-url "$RPC_URL" 2>&1 || true)
DATA=$(grep -oE 'data: "0x[0-9a-fA-F]+' <<<"$WHY" | sed 's/data: "//' | head -1 || true)
log "  eth_call of the same calldata: ${WHY//$'\n'/ }"
log "  revert data $DATA = AlgNotActive(bytes32) [selector $(cast sig 'AlgNotActive(bytes32)')], activeAlg: $(cast decode-error "$DATA" --sig 'AlgNotActive(bytes32)' 2>&1 | tr '\n' ' ')"
TX3=$(jq -r .transactionHash <<<"$R3")
log "  explorer record of tx 3: $(curl -s -m 20 "$EXPLORER/api/v2/transactions/$TX3" | jq -c '{status, result, revert_reason}' 2>/dev/null || echo unavailable)"

log "-- tx 4: the new algorithm approves (secp256k1 executeSigned)"
send "$OWNER" "$(script "secpExecuteCalldata(address,address)" "$OWNER" "$RELAYER")" | receipt
log "  nonce $(cast call "$OWNER" "nonce()(uint256)" --rpc-url "$RPC_URL")"
log "owner $OWNER  secp256k1 verifier $SECPV  p256 verifier $P256V"
log "explorer: $EXPLORER/address/$OWNER"
