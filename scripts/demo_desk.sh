#!/usr/bin/env bash
# Put a test desk in the state the demo shows (docs/90), and take it out again. Testnet only.
#
#   scripts/demo_desk.sh open  [grams]   a supplier-signed purchase and a vault-signed receipt: the offset opens by itself
#   scripts/demo_desk.sh close           the buyer signs the sale of whatever the last `open` booked: the offset closes
#   scripts/demo_desk.sh status          held, offset and the exchange's position, read back
#
# Uses the test counterparties in ~/.tameion/test-keys/desk-{supplier,vault,buyer}.key (already approved on the desk) and
# the desk in DESK (default desk-gold-2329). Prints nothing secret. Needs the service with the desk's exchange connected
# (docs/88 §3) and HEDGE_LIVE=1 for real testnet orders.
set -euo pipefail
cd "$(dirname "$0")/.."
DESK=${DESK:-desk-gold-2329}
P=$(cat ~/.tameion/desks/$DESK.policy)
K=~/.tameion/test-keys
H=${TAMEION_URL:-http://127.0.0.1:7880}
STATE=runtime/desk/demo-$DESK.deal
SIGN=./target/debug/examples/desk_sign
[ -x "$SIGN" ] || cargo build -q -p operator --example desk_sign
CHAIN=$(curl -fsS "$H/api/level1/tenant/$P" | python3 -c 'import sys,json; t=json.load(sys.stdin); t=t.get("tenant",t); print({"arbitrum-sepolia":421614,"arc-testnet":5042002}[t["chain"]])')
mark() { curl -fsS -X POST https://api.hyperliquid-testnet.xyz/info -H 'content-type: application/json' -d '{"type":"allMids"}' | python3 -c 'import sys,json; print(json.load(sys.stdin)["PAXG"])'; }
usdc() { python3 -c "print(round($1/31.1034768*$(mark), 2))"; }
# deal kind grams usdc|- counterparty key
step() {
  local m s
  m=$(python3 -c 'import json,sys,time; d,k,q,u,w=sys.argv[1:6]; print(json.dumps({"deal":d,"kind":k,"goods":"gold","quantity":q,"usdc":(u if u!="-" else None),"counterparty":w,"document":k.upper()+"-"+d,"at":int(time.time())}))' "$1" "$2" "$3" "$4" "$5")
  s=$("$SIGN" "$6" "$CHAIN" "$P" "$m")
  curl -sS -X POST "$H/api/desk/milestone" -H 'content-type: application/json' \
    -d "$(python3 -c 'import json,sys; s=json.loads(sys.argv[1]); s["policy"]=sys.argv[2]; print(json.dumps(s))' "$s" "$P")" |
    python3 -c 'import sys,json; d=json.load(sys.stdin); o=d.get("offset") or {}; print("  ", d.get("booked") or d.get("error"), "| offset:", o.get("readBack") or o.get("waiting") or o.get("error") or "-")'
}
case "${1:-status}" in
  open)
    G=$(printf '%.3f' "${2:-10}")
    D=GOLD-$(date +%s)
    echo "$D $G" > "$STATE"
    echo "# $(date '+%Y-%m-%d %H:%M:%S %z') · $DESK · $D · $G g"
    step "$D" bought "$G" "$(usdc "$G")" Supplier "$K/desk-supplier.key"
    step "$D" arrived "$G" - Vault "$K/desk-vault.key"
    ;;
  close)
    read -r D G < "$STATE"
    echo "# $(date '+%Y-%m-%d %H:%M:%S %z') · $DESK · $D sold"
    step "$D" sold "$G" "$(usdc "$G")" Buyer "$K/desk-buyer.key"
    rm -f "$STATE"
    ;;
  status)
    curl -fsS "$H/api/desk?policy=$P" -H "x-tameion-device: $(cat ~/.tameion/desks/$DESK.token 2>/dev/null || cat $K/$DESK.token)" |
      python3 -c 'import sys,json; d=json.load(sys.stdin); e=(d.get("exposure") or [{}])[0]; v=d.get("venue") or {}; print("held", e.get("held"), "| offset target", e.get("offsetTarget"), "| exchange position", v.get("position"), v.get("error") or "")'
    ;;
  *) echo "usage: scripts/demo_desk.sh open [grams] | close | status" >&2; exit 2 ;;
esac
