#!/usr/bin/env bash
# What a stolen counterparty key can still do on a desk (docs/91 R3, R4, R6), run
# against the service: the offset moves by itself only while the total it moved
# without the owner stays inside the limit; past it, a device token is refused and
# only the owner's passkey releases the trade; a stopped counterparty books nothing.
#
#   TAMEION_DESK_AUTO_GRAMS=5 (on the service) scripts/prove_desk_limits.sh <policy> <device-token-file> [host]
#
# Trades about 6 g of gold each way on the desk's venue (Hyperliquid testnet) and
# leaves the offset where it found it. The owner's passkey is this Mac's Secure
# Enclave key (SE_BLOB, made without Touch ID for unattended proofs; OWNER_SIGN to
# use another signer). Counterparty test keys: ~/.tameion/test-keys/desk-{supplier,buyer}.key.
set -uo pipefail
cd "$(dirname "$0")/.."
POLICY=${1:?policy}; TOKEN=$(cat "${2:?device token file}"); HOST=${3:-http://127.0.0.1:7880}
CHAIN=$(curl -fsS "$HOST/api/level1/tenant/$POLICY" | python3 -c 'import sys,json; t=json.load(sys.stdin); t=t.get("tenant",t); print({"arbitrum-sepolia":421614,"arc-testnet":5042002}[t["chain"]])')
KEYS=~/.tameion/test-keys
RUN=$(date +%s)
CHALLENGE=./target/debug/examples/desk_challenge
SIGN=./target/debug/examples/desk_sign
[ -x "$CHALLENGE" ] && [ -x "$SIGN" ] || cargo build -q -p operator --example desk_challenge --example desk_sign
post() { curl -sS -X POST "$HOST$1" -H 'content-type: application/json' "${@:3}" -d "$2"; echo; }
owner_auth() { # kind args… → "issued":…, "auth":…
  local iss ch; iss=$(date +%s)
  ch=$("$CHALLENGE" "$1" "$CHAIN" "$POLICY" "${@:2}" "$iss")
  printf '"issued":%s,"auth":%s' "$iss" "$(${OWNER_SIGN:-target/desk-se sign} "$ch" --unattended 2>/dev/null)"
}
signed() { # deal kind grams usdc who keyfile → the service's reply, one line
  local m s; m=$(python3 -c 'import json,sys,time; d,k,q,u,w=sys.argv[1:6]; print(json.dumps({"deal":d,"kind":k,"goods":"gold","quantity":q,"usdc":u,"counterparty":w,"document":k.upper()+"-"+d,"at":int(time.time())}))' "$1" "$2" "$3" "$4" "$5")
  s=$("$SIGN" "$6" "$CHAIN" "$POLICY" "$m")
  post /api/desk/milestone "$(python3 -c 'import json,sys; s=json.loads(sys.argv[1]); s["policy"]=sys.argv[2]; print(json.dumps(s))' "$s" "$POLICY")" \
    | python3 -c 'import sys,json; d=json.load(sys.stdin); o=d.get("offset") or {}; print("  ", d.get("booked") or d.get("error"), "| offset:", o.get("waiting") or o.get("readBack") or o.get("error") or o)'
}
state() {
  curl -sS "$HOST/api/desk?policy=$POLICY" -H "x-tameion-device: $TOKEN" | python3 -c 'import sys,json; d=json.load(sys.stdin); e=d["exposure"][0]; v=d.get("venue") or {}; print("   held", e["held"], "| target", e["offsetTarget"], "| venue", v.get("position"), "| moved by itself since you last confirmed:", e.get("autoMoved"), "of", e.get("autoLimit"))'
}
target() { curl -sS "$HOST/api/desk?policy=$POLICY" -H "x-tameion-device: $TOKEN" | python3 -c 'import sys,json; print(json.load(sys.stdin)["exposure"][0]["offsetTarget"])'; }
tap() { # [passkey] → the owner's tap on "bring the offset in line"
  local extra=""
  [ "${1:-}" = passkey ] && extra=",$(owner_auth offset "$(target)")"
  post /api/desk/offset "{\"policy\":\"$POLICY\"$extra}" -H "x-tameion-device: $TOKEN" \
    | python3 -c 'import sys,json; d=json.load(sys.stdin); print("  ", d.get("error") or "readBack %s, by the owner: %s" % (d.get("readBack"), d.get("byOwner")))'
}
MARK=$(curl -s -X POST https://api.hyperliquid-testnet.xyz/info -H 'content-type: application/json' -d '{"type":"allMids"}' | python3 -c 'import sys,json; print(json.load(sys.stdin)["PAXG"])')
COST=$(python3 -c "print(round(3/31.1034768*$MARK, 2))")
echo "# desk limits · account $POLICY (chain $CHAIN) · run $RUN · PAXG $MARK · $(date '+%Y-%m-%d %H:%M:%S %z')"
state
echo "## R3. Two supplier-signed purchases of 3 g: the first moves by itself, the second would pass the 5 g the desk may move alone"
signed "L1-$RUN" bought 3.000 "$COST" Supplier $KEYS/desk-supplier.key
signed "L2-$RUN" bought 3.000 "$COST" Supplier $KEYS/desk-supplier.key
state
echo "## R6. The tap with the device token alone is refused past the limit; the owner's passkey releases it and resets the count"
tap
tap passkey
state
echo "## On the way out: each sale moves by itself while the total stays inside the limit; the tap, then the owner's passkey, close what is left"
signed "L1-$RUN" sold 3.000 "$COST" Buyer $KEYS/desk-buyer.key
signed "L2-$RUN" sold 3.000 "$COST" Buyer $KEYS/desk-buyer.key
tap
tap passkey
state
