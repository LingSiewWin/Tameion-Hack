#!/usr/bin/env bash
# Verify a hosted Level 1 tenant from any machine (docs/52, docs/46 #1):
#
#   scripts/verify_tenant.sh https://tameion.example 0x<policy>        # latest pass
#   scripts/verify_tenant.sh http://127.0.0.1:7880 0x<policy> 0x<root>  # one pass
#   TAMEION_TOKEN=<agent key with read> scripts/verify_tenant.sh ...    # a locked account, its events
#
# A locked account (its owner minted agent keys) serves an outsider each
# event's leaf hash, not the event (docs/60 P-212): without TAMEION_TOKEN the
# root is checked from those leaves, and the script says so. The key is sent
# in a header and never printed.
#
# Reads the tenant's anchored roots from the service, downloads the events
# behind one of them, recomputes the root from those events and compares it
# with rootOf() on the chain under the tenant's agent key. No key, no trust
# in the service's words: the events are the input, the chain is the judge.
set -euo pipefail
HOST=${1:?host, e.g. http://127.0.0.1:7880}
POLICY=${2:?policy address}
ROOT=${3:-}
ROOTDIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOTDIR"
# An account whose owner minted agent keys answers only to one with `read`
# (PLAN 5.31): TAMEION_TOKEN=<key> scripts/verify_tenant.sh ...
AUTH=()
[ -n "${TAMEION_TOKEN:-}" ] && AUTH=(-H "Authorization: Bearer $TAMEION_TOKEN")
records=$(curl -fsS ${AUTH[@]+"${AUTH[@]}"} "$HOST/api/level1/records?policy=$POLICY")
line=$(printf '%s' "$records" | python3 -c '
import json,sys
d=json.load(sys.stdin); rs=d["records"]; want=sys.argv[1].lower()
r=[x for x in rs if want and x["root"].lower()==want] if want else rs[-1:]
if not r: sys.exit("no such root for this policy" if want else "no anchored records yet")
print(json.dumps(r[-1]))' "$ROOT")
root=$(printf '%s' "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin)["root"])')
agent=$(printf '%s' "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin)["agent"])')
anchor=$(printf '%s' "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin)["anchor"])')
echo "policy  $POLICY"
echo "root    $root  (anchored in $(printf '%s' "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("anchorTx"))'))"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsS ${AUTH[@]+"${AUTH[@]}"} "$HOST/api/level1/record?policy=$POLICY&root=$root" -o "$tmp/$root.events.jsonl"
echo "events  $(wc -l < "$tmp/$root.events.jsonl" | tr -d ' ') lines downloaded"
if grep -q '"canonical"' "$tmp/$root.events.jsonl"; then
  how="from the events themselves"
else
  how="from the public leaf hashes only: this account is locked, and its events need TAMEION_TOKEN (a key with read)"
fi
echo "checked $how"
# Two independent RPCs (PLAN 5.18, docs/59 §3.12): the root must match on
# both, and both must be at the same height within a few blocks. A node that
# lies alone is caught; a service that lies is caught by either.
# The account's own chain picks the two default nodes (an Arbitrum Sepolia
# account checked against Arc's nodes finds no anchor contract and fails).
chain=$(printf '%s' "$records" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("chain") or "arc-testnet")')
case "$chain" in
  arbitrum-sepolia) d1=https://sepolia-rollup.arbitrum.io/rpc; d2=https://arbitrum-sepolia-rpc.publicnode.com ;;
  *) d1=https://rpc.testnet.arc.network; d2=https://explorer.testnet.arc.io/api/eth-rpc ;;
esac
echo "chain   $chain"
RPC_URL=${RPC_URL:-$d1}
RPC_URL_2=${RPC_URL_2:-$d2}
rc=0
for rpc in "$RPC_URL" "$RPC_URL_2"; do
  echo "-- against $rpc (block $(cast block-number --rpc-url "$rpc" 2>/dev/null || echo '?'))"
  # rootOf() holds only the latest root; an earlier one is checked by the
  # transaction the service says anchored it, read from the chain itself.
  ANCHOR_TX="$(printf '%s' "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("anchorTx") or "")')" \
  AUDIT_EVENTS="$tmp/$root.events.jsonl" AGENT="$agent" ANCHOR="$anchor" RPC_URL="$rpc" \
    cargo run --locked -q -p operator --example audit || { r=$?; [ $rc -eq 0 ] && rc=$r; }
done
if [ "$rc" -eq 0 ]; then echo "verify_tenant: MATCH on both RPCs, checked $how"; else echo "verify_tenant: a source disagreed or failed (exit $rc)"; fi
exit "$rc"
