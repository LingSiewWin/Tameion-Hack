#!/usr/bin/env bash
# Open a desk on this Mac in one command (docs/88): the owner key made inside the
# Secure Enclave (Touch ID on every signature), an account reserved for it on the
# chain (nothing on chain, no gas, until money arrives), the owner's device token
# taken with one chip signature, and the line to start the service with.
#
#   scripts/desk_init.sh <name> [chain] [--no-presence]
#
#   name    3–32 lowercase letters, numbers or dashes; the desk's handle
#   chain   arbitrum-sepolia (default) or arc-testnet
#   --no-presence   only for unattended proofs: the key asks for no person, and its
#                   signatures then claim a user check that did not happen
#
# Writes ~/.tameion/desks/<name>.{blob,token,policy} and <name>-console.js (mode 600).
# Prints nothing secret. Needs the service running (docs/88 §3), cast and swiftc.
set -euo pipefail
cd "$(dirname "$0")/.."
NAME=${1:?usage: scripts/desk_init.sh <name> [chain] [--no-presence]}
CHAIN=arbitrum-sepolia
PRESENCE=()
UNATTENDED=()
for a in "${@:2}"; do
  case "$a" in
    --no-presence) PRESENCE=(--no-presence); UNATTENDED=(--unattended) ;;
    arbitrum-sepolia | arc-testnet) CHAIN=$a ;;
    *) echo "unknown argument: $a" >&2; exit 2 ;;
  esac
done
HOST=${TAMEION_URL:-http://127.0.0.1:7880}
DIR=$HOME/.tameion/desks
mkdir -p "$DIR" && chmod 700 "$DIR"
umask 077
export SE_BLOB="$DIR/$NAME.blob"
SE=target/desk-se
[[ -x $SE && $SE -nt apps/desk-se/main.swift ]] || swiftc -O apps/desk-se/main.swift -o "$SE"
curl -fsS "$HOST/healthz" >/dev/null || { echo "the service is not running at $HOST (docs/88 §3)" >&2; exit 1; }
json() { python3 -c "import sys,json; d=json.load(sys.stdin); print($1)"; }
hexs() { printf '%s' "$1" | xxd -p | tr -d '\n'; }

echo "1. the owner key, inside this Mac's Secure Enclave"
if [[ -f $SE_BLOB ]]; then
  echo "   reusing $SE_BLOB"
else
  "$SE" create "${PRESENCE[@]}" >/dev/null
  echo "   made $SE_BLOB ($([[ ${#PRESENCE[@]} -eq 0 ]] && echo 'Touch ID on every signature' || echo 'NO Touch ID: unattended proofs only'))"
fi
read -r QX QY <<<"$("$SE" pub | awk '{print $2}' | tr '\n' ' ')"

echo "2. the account, reserved on $CHAIN (nothing on chain until money arrives)"
R=$(curl -sS -X POST "$HOST/api/level1/deploy" -H 'content-type: application/json' \
  -d "{\"chain\":\"$CHAIN\",\"qx\":\"$QX\",\"qy\":\"$QY\",\"name\":\"$NAME\",\"lazy\":true}")
POLICY=$(echo "$R" | json 'd.get("tenant",{}).get("policy") or ""')
[ -n "$POLICY" ] || { echo "   could not open it: $(echo "$R" | json 'd.get("error")')" >&2; exit 1; }
echo "   account $POLICY"
TOKEN=$(echo "$R" | json 'd.get("reserveToken") or ""')
CHAIN_ID=$(curl -fsS "$HOST/api/level1/config" | json "[c['chainId'] for c in d['chains'] if c['key']=='$CHAIN'][0]")

echo "3. the owner's device token"
if [ -z "$TOKEN" ]; then
  # The account was already reserved by this key: one chip signature takes a new token.
  ISS=$(date +%s)
  CH=$(cast keccak "$(cast concat-hex 0x"$(hexs tameion.reserve-token.v1)" "$(printf '0x%016x' "$CHAIN_ID")" "$POLICY" "$(printf '0x%016x' "$ISS")")")
  AUTH=$("$SE" sign "$CH" "${UNATTENDED[@]}")
  TOKEN=$(curl -sS -X POST "$HOST/api/level1/reserve-token" -H 'content-type: application/json' \
    -d "{\"policy\":\"$POLICY\",\"issued\":$ISS,\"auth\":$AUTH}" | json 'd.get("reserveToken") or ""')
  [ -n "$TOKEN" ] || { echo "   the service did not issue a token" >&2; exit 1; }
  echo "   taken with one chip signature"
else
  echo "   issued with the reservation"
fi
printf '%s\n' "$TOKEN" >"$DIR/$NAME.token"
printf '%s\n' "$POLICY" >"$DIR/$NAME.policy"

echo "4. opening it in a browser"
curl -fsS "$HOST/api/level1/tenant/$POLICY" | python3 -c '
import json, sys
t = json.load(sys.stdin)
t = t.get("tenant", t)  # the route wraps the record: {"tenant": {...}}
policy, token, qx, qy = sys.argv[1:5]
pub = {"qx": qx, "qy": qy, "credentialId": "secure-enclave"}
sets = {"tameion.tenant": json.dumps(t), "tameion.tenants": json.dumps([t]), "tameion.pub": json.dumps(pub),
        "tameion.reserve." + policy.lower(): token}
print("// Paste into the browser console on the service, then reload and open /desk. Testnet only.")
print("(() => {" + " ".join(f"localStorage.setItem({json.dumps(k)}, {json.dumps(v)});" for k, v in sets.items()) + " return \"ok\"; })();")
' "$POLICY" "$TOKEN" "$QX" "$QY" >"$DIR/$NAME-console.js"
echo "   $DIR/$NAME-console.js"

cat <<EOF

Desk "$NAME" is open: account $POLICY on $CHAIN.
To let it offset on Hyperliquid testnet: open /desk and tap "Connect it" (the account that approved this Mac's trade-only key,
your passkey signs), with the service started without TAMEION_DESK_POLICY. Or restart the service bound to it (one desk per service):
  TAMEION_DESK_POLICY=$POLICY TAMEION_DESK_VENUE=<your Hyperliquid testnet account> HEDGE_LIVE=1 \\
    PORT=7880 nohup ./target/debug/tameion-app > runtime/tameion-app.log 2>&1 &
  (the trade-only key that account approved goes in ~/.tameion/hedge-agent.key; docs/88 §2)
Then approve who may sign on /desk (your Touch ID signs each), or with
  SE_BLOB=$SE_BLOB scripts/prove_desk_api.sh $POLICY $DIR/$NAME.token   (a full test run: trades on the venue)
EOF
