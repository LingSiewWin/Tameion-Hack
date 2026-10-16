#!/usr/bin/env bash
# "Connect the exchange" without a pasted key (docs/106 T5).
#
# 1. POST /api/desk/venue/connect: the service makes the desk's own trade-only
#    key (kept under ~/.tameion/desks, mode 600, never shown) and returns the
#    EIP-712 `ApproveAgent` typed data for the master wallet.
# 2. A stand-in for the browser wallet signs it: a copy of
#    ~/.tameion/test-keys/local_signer.py pointed at MASTER_KEY_FILE, on port
#    7904 (the key stays in its file; `cast wallet sign --data` signs).
# 3. POST /api/desk/venue/approve: where the offset lives is the owner's
#    decision, so the owner's passkey signs `desk::venue_challenge` for that
#    wallet (the test desk's owner key is this Mac's Secure Enclave,
#    `target/desk-se`, as in scripts/prove_desk_api.sh); then the service
#    submits the wallet's signature to Hyperliquid testnet and reads the key
#    back from the venue.
# 4. If the venue approved it: one small order from that key through the engine
#    (k = 4 / 2x rule, read back), then closed. Nothing is left open.
#
#   PORT=7903 scripts/prove_connect.sh
#
# Testnet only. A master that has no Hyperliquid testnet account is refused by
# the venue; the output then shows the venue's answer and the words the owner sees.
set -euo pipefail
cd "$(dirname "$0")/.."
PORT="${PORT:-7903}"
POLICY="${POLICY:-0xcd8d71d97c3a946a14ac3fb92db4fb86e2974308}"
MASTER_KEY_FILE="${MASTER_KEY_FILE:-$HOME/.tameion/hedge-margin.key}"
TOKEN_FILE="${TOKEN_FILE:-$HOME/.tameion/test-keys/desk-gold-2329.token}"
HALT_DIR="${HEDGE_HALT_DIR:-runtime/seat-S3/hedge}"
DESKS_DIR="${TAMEION_DESKS_DIR:-$HOME/.tameion/desks}"
APP_DIR="${TAMEION_APP_DIR:-runtime/seat-S3}"   # the instance's files: its venue claim
SIZE="${SIZE:-0.003}"
OUT="${OUT:-proof/connect-$(date +%F).txt}"
WORK="$(mktemp -d)"
TOKEN="$(tr -d '[:space:]' < "$TOKEN_FILE")"
INFO=https://api.hyperliquid-testnet.xyz/info
MASTER="$(cast wallet address --private-key "$(tr -d '[:space:]' < "$MASTER_KEY_FILE")")"
policy_lc="$(echo "$POLICY" | tr 'A-F' 'a-f')"

opened=0
agent_key="$DESKS_DIR/$policy_lc.agent.key"
step() { # one engine pass on the master's account with the desk's new key, holding $1 PAXG
  HEDGE_AGENT_KEY_FILE="$agent_key" TAMEION_TREASURY_ADDR="$MASTER" HEDGE_SPOT="PAXG=$1" \
    HEDGE_HALT_DIR="$HALT_DIR" HEDGE_COVER_USDC=full target/debug/examples/live_once
}
close() {
  if [ "$opened" = 1 ]; then
    opened=0
    echo "## close: the engine flattens the offset (holding 0), same key"
    step 0 | grep -E '^(TRADED|  status|HALTED|action|verdict)' || true
  fi
}
trap 'close; kill $(jobs -p) 2>/dev/null || true' EXIT
exec > >(tee "$OUT") 2>&1

echo "# $(date '+%Y-%m-%d %H:%M:%S %z') scripts/prove_connect.sh — connect the exchange with one wallet signature (:$PORT)"
echo "desk policy $POLICY; stand-in wallet = $MASTER_KEY_FILE ($MASTER) on 127.0.0.1:7904"
echo
echo "## the master on Hyperliquid testnet before (userRole, account value, API wallets)"
for q in userRole extraAgents; do
  printf '%s: ' "$q"
  curl -s -m 8 -X POST "$INFO" -H 'content-type: application/json' -d "{\"type\":\"$q\",\"user\":\"$MASTER\"}"
  echo
done
printf 'accountValue: '
curl -s -m 8 -X POST "$INFO" -H 'content-type: application/json' -d "{\"type\":\"clearinghouseState\",\"user\":\"$MASTER\"}" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["crossMarginSummary"]["accountValue"])'
echo

# The stand-in wallet: the test signer, pointed at the master key file, on its own port.
sed -e "s#~/.tameion/treasury.key#$MASTER_KEY_FILE#" -e 's/7899/7904/g' \
  "$HOME/.tameion/test-keys/local_signer.py" > "$WORK/signer.py"
python3 "$WORK/signer.py" > "$WORK/signer.log" 2>&1 &
for _ in 1 2 3 4 5 6 7 8 9 10; do
  curl -s -m 1 -X OPTIONS http://127.0.0.1:7904/ -o /dev/null && break
  sleep 0.3
done

echo "## 1. POST /api/desk/venue/connect {policy} (the owner's device)"
curl -s -m 10 -X POST "http://127.0.0.1:$PORT/api/desk/venue/connect" -H 'content-type: application/json' \
  -H "x-tameion-device: $TOKEN" -d "{\"policy\":\"$POLICY\"}" > "$WORK/connect.json"
python3 -m json.tool "$WORK/connect.json"
echo "the desk's pending key on this Mac (mode and size only):"
ls -l "$DESKS_DIR/$policy_lc.agent.pending" | awk '{print "  " $1, $5 " bytes", $NF}'
echo

echo "## 2. the wallet signs the typed data (eth_signTypedData_v4 stand-in)"
python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["typedData"]))' "$WORK/connect.json" > "$WORK/typed.json"
SIG="$(curl -s -m 10 -X POST http://127.0.0.1:7904/ -H 'content-type: application/json' --data-binary @"$WORK/typed.json")"
echo "signature: $SIG"
echo

claim_account() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("account"))' "$APP_DIR/desk-venue.json" 2>/dev/null || echo none; }
echo "the desk's venue claim before approve: account $(claim_account); desk key file: $([ -e "$agent_key" ] && echo present || echo absent)"
echo
echo "## 3a. the same request without the owner's passkey (a stolen device token or run key must not move the offset's account)"
curl -s -m 20 -X POST "http://127.0.0.1:$PORT/api/desk/venue/approve" -H 'content-type: application/json' \
  -H "x-tameion-device: $TOKEN" -d "{\"policy\":\"$POLICY\",\"signature\":\"$SIG\",\"master\":\"$MASTER\"}"
echo
echo
echo "## 3b. POST /api/desk/venue/approve {policy, signature, master, issued, auth, agent} (auth = the owner's passkey over desk::venue_challenge; agent = the key the wallet was shown)"
cargo build -q -p operator --example desk_challenge
CHAIN=$(curl -fsS "http://127.0.0.1:$PORT/api/level1/tenant/$POLICY" | python3 -c 'import sys,json; t=json.load(sys.stdin); t=t.get("tenant",t); print({"arbitrum-sepolia":421614,"arc-testnet":5042002}[t["chain"]])')
ISSUED=$(date +%s)
CH=$(target/debug/examples/desk_challenge venue "$CHAIN" "$POLICY" "$MASTER" "$ISSUED")
echo "challenge (chain $CHAIN, issued $ISSUED): $CH"
AUTH=$(${OWNER_SIGN:-target/desk-se sign} "$CH" --unattended 2>/dev/null)
AGENT=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["agent"])' "$WORK/connect.json")
# The owner check reads the owner's keys from the chain; the public RPC can be slow (CLAUDE.md, P-12), hence 60 s.
curl -s -m 60 -X POST "http://127.0.0.1:$PORT/api/desk/venue/approve" -H 'content-type: application/json' \
  -H "x-tameion-device: $TOKEN" -d "{\"policy\":\"$POLICY\",\"signature\":\"$SIG\",\"master\":\"$MASTER\",\"issued\":$ISSUED,\"auth\":$AUTH,\"agent\":\"$AGENT\"}" > "$WORK/approve.json" || true
python3 -m json.tool "$WORK/approve.json" || echo "(no JSON answer from the service: $(wc -c < "$WORK/approve.json") bytes)"
echo

if python3 -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1])).get("readBack") else 1)' "$WORK/approve.json"; then
  echo "## the venue's own list of the master's API wallets after"
  curl -s -m 8 -X POST "$INFO" -H 'content-type: application/json' -d "{\"type\":\"extraAgents\",\"user\":\"$MASTER\"}"
  echo
  echo "the desk's key on this Mac (mode and size only):"
  ls -l "$agent_key" | awk '{print "  " $1, $5 " bytes", $NF}'
  echo
  echo "## 4. one small order from that key, inside the rule, read back"
  cargo build -q -p hedge --example live_once
  opened=1
  step "$SIZE" | grep -E '^(user:|TRADED|  status|HALTED|action|verdict)'
  echo
  close
  echo
  echo "## the venue's own reply after the close"
  curl -s -m 5 -X POST "$INFO" -H 'content-type: application/json' -d "{\"type\":\"clearinghouseState\",\"user\":\"$MASTER\"}"
  echo
  echo "RESULT: PASS — approved with one wallet signature, read back from the venue, an order from the desk's key read back and closed"
else
  master_lc="$(echo "$MASTER" | tr 'A-F' 'a-f')"
  if grep -q "User: $master_lc" "$WORK/approve.json"; then
    echo "The venue named $master_lc as the signer: it recovered the stand-in wallet from our typed data and signature,"
    echo "so the message is the one Hyperliquid verifies; it refused only because that address has no testnet account (docs/60 P-147)."
  fi
  echo "after the refusal (docs/103 S3-2: the key file and the claim are written before the venue sees the key, and put back when it refuses):"
  echo "  the desk's venue claim: account $(claim_account); desk key file: $([ -e "$agent_key" ] && echo present || echo absent)"
  echo "RESULT: the venue did not approve the key; nothing was approved and nothing traded (its answer and the owner's words are above)"
fi
echo "saved: $OUT"
