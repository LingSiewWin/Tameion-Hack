#!/usr/bin/env bash
# Fund a tenant's policy from USDC the bill wallet holds in Circle Gateway on
# another chain (PLAN 5.15): the service prepares the burn intent, the bill
# wallet signs it (EIP-712, here with `cast` and a key file; on the account
# page the browser wallet signs the same data), the service sends it to
# Gateway and relays the mint on Arc to the policy.
#
#   scripts/gateway_fund.sh <policy> <USDC|all> [sourceDomain]   # default source 3 = Arbitrum Sepolia; "all" = the balance less Circle's fee cap
#
# Env: TAMEION_URL (default http://127.0.0.1:7880),
#      TAMEION_TREASURY_KEY_FILE (default ~/.tameion/treasury.key; must be the bill wallet).
# The key never leaves the file; nothing here prints it.
set -euo pipefail

POLICY=${1:?policy address}
AMOUNT=${2:?USDC amount, e.g. 0.5}
SOURCE=${3:-3}
URL=${TAMEION_URL:-http://127.0.0.1:7880}
KEY_FILE=${TAMEION_TREASURY_KEY_FILE:-$HOME/.tameion/treasury.key}
PK=$(cat "$KEY_FILE")
FROM=$(cast wallet address --private-key "$PK")
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "1. prepare: $AMOUNT USDC from Gateway domain $SOURCE (depositor $FROM) to policy $POLICY"
curl -sf -X POST "$URL/api/level1/prepare-fund" -H 'content-type: application/json' \
  -d "{\"policy\":\"$POLICY\",\"amount\":\"$AMOUNT\",\"all\":$([ "$AMOUNT" = all ] && echo true || echo false),\"sourceDomain\":$SOURCE,\"from\":\"$FROM\"}" > "$TMP/prep.json"
python3 - "$TMP/prep.json" "$TMP/typed.json" <<'EOF'
import json, sys
p = json.load(open(sys.argv[1]))
if "error" in p: sys.exit("   service: " + p["error"])
json.dump(p["typedData"], open(sys.argv[2], "w"))
print("   maxFee", p["typedData"]["message"]["maxFee"], "minor; maxBlockHeight", p["typedData"]["message"]["maxBlockHeight"])
print("   ", p["describe"])
EOF

echo "2. sign (EIP-712, bill wallet)"
SIG=$(cast wallet sign --data --from-file "$TMP/typed.json" --private-key "$PK")
echo "   signature ${SIG:0:18}…"

echo "3. transfer + mint on Arc"
python3 - "$TMP/typed.json" "$SIG" > "$TMP/body.json" <<'EOF'
import json, sys
t = json.load(open(sys.argv[1]))
print(json.dumps({"burnIntent": t["message"], "signature": sys.argv[2]}))
EOF
python3 - "$TMP/body.json" "$POLICY" > "$TMP/fund.json" <<'EOF'
import json, sys
b = json.load(open(sys.argv[1])); b["policy"] = sys.argv[2]
print(json.dumps(b))
EOF
curl -s -X POST "$URL/api/level1/fund" -H 'content-type: application/json' -d @"$TMP/fund.json" \
  | python3 -c 'import sys,json; r=json.load(sys.stdin); print(json.dumps(r, indent=2))'
