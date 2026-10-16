#!/usr/bin/env bash
# The desk through the service's API (docs/83 W5), as the console and the
# counterparties use it: the owner approves who may sign, the supplier and the
# vault sign their milestones, the owner enters a fabrication job, the offset
# follows the book on Hyperliquid testnet, a lender's kit is issued and checked
# by scripts/verify_desk.py, the buyer's sale closes the offset.
#
#   scripts/prove_desk_api.sh <policy> <device-token-file> [host]
#
# The service must run with TAMEION_DESK_VENUE (and HEDGE_LIVE=1 for real
# orders). Counterparty test keys: ~/.tameion/test-keys/desk-{supplier,vault,buyer}.key.
set -uo pipefail
cd "$(dirname "$0")/.."
POLICY=${1:?policy}; TOKEN=$(cat "${2:?device token file}"); HOST=${3:-http://127.0.0.1:7880}
# The route wraps the record ({"tenant": {...}}); a wrong chain makes every owner signature miss (docs/60 P-126).
CHAIN=$(curl -fsS "$HOST/api/level1/tenant/$POLICY" | python3 -c 'import sys,json; t=json.load(sys.stdin); t=t.get("tenant",t); print({"arbitrum-sepolia":421614,"arc-testnet":5042002}[t["chain"]])') \
  || { echo "cannot read the account's chain from $HOST" >&2; exit 1; }
KEYS=~/.tameion/test-keys
DEAL="GOLD-$(date +%s)"
addr() { cargo run -q -p operator --example desk_sign -- "$1" "$CHAIN" "$POLICY" '{"deal":"x","kind":"bought","goods":"gold","quantity":"1","usdc":"1","counterparty":"x","document":"x","at":0}' | python3 -c 'import sys,json; print(json.load(sys.stdin)["milestone"]["signer"])'; }
post() { curl -sS -X POST "$HOST$1" -H 'content-type: application/json' "${@:3}" -d "$2"; echo; }
owner() { post "$1" "$2" -H "x-tameion-device: $TOKEN"; }
# Approving a signer is a trust decision: the owner's passkey signs it (docs/89 M5). The
# test desk's owner key is this Mac's Secure Enclave (target/desk-se; made without Touch ID
# for unattended proofs, hence --unattended). Another owner: OWNER_SIGN="<command> <challenge>".
CHALLENGE=./target/debug/examples/desk_challenge
[ -x "$CHALLENGE" ] || cargo build -q -p operator --example desk_challenge
owner_auth() { # kind args… → "issued":…, "auth":…  (the owner's passkey over the desk challenge)
  local iss ch
  iss=$(date +%s)
  ch=$("$CHALLENGE" "$1" "$CHAIN" "$POLICY" "${@:2}" "$iss")
  printf '"issued":%s,"auth":%s' "$iss" "$(${OWNER_SIGN:-target/desk-se sign} "$ch" --unattended 2>/dev/null)"
}
approve() { # name address kinds (the role, docs/91 R7: what this counterparty may sign)
  local k; k=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1].split(",")))' "$3")
  post /api/desk/party "{\"policy\":\"$POLICY\",\"name\":\"$1\",\"address\":\"$2\",\"kinds\":$k,$(owner_auth party "$1" "$2" "$3")}"
}
milestone() { # kind qty usdc who doc keyfile|-
  local m; m=$(python3 -c 'import json,sys,time; k,q,u,w,d,deal=sys.argv[1:7]; print(json.dumps({"deal":deal,"kind":k,"goods":"gold","quantity":q,"usdc":(u if u!="-" else None),"counterparty":w,"document":d,"at":int(time.time())}))' "$1" "$2" "$3" "$4" "$5" "$DEAL")
  if [ "$6" = "-" ]; then
    post /api/desk/milestone "{\"policy\":\"$POLICY\",\"milestone\":$m,$(owner_auth step "$m")}"
  else
    local signed; signed=$(cargo run -q -p operator --example desk_sign -- "$6" "$CHAIN" "$POLICY" "$m")
    post /api/desk/milestone "$(python3 -c 'import json,sys; s=json.loads(sys.argv[1]); s["policy"]=sys.argv[2]; print(json.dumps(s))' "$signed" "$POLICY")"
  fi
}
echo "# desk API proof · account $POLICY (chain $CHAIN) · deal $DEAL · $HOST"
echo "## the book is the owner's: no token → refused; the owner's device → the book"
curl -sS "$HOST/api/desk?policy=$POLICY"; echo
curl -sS "$HOST/api/desk?policy=$POLICY" -H "x-tameion-device: $TOKEN" | python3 -c 'import sys,json; d=json.load(sys.stdin); print("deals", len(d.get("deals",[])), "parties", len(d.get("parties",[])), "venue", d.get("venueAccount"))'
echo "## the owner approves who may sign (a trust decision: the owner's passkey — here the Mac's Secure Enclave — signs each)"
PINS=()
for p in Supplier:supplier:bought Vault:vault:arrived,returned Buyer:buyer:sold; do
  IFS=: read -r NAME KEY KINDS <<<"$p"
  A=$(addr $KEYS/desk-$KEY.key)
  PINS+=(--party "$NAME=$A:$KINDS")
  approve "$NAME" "$A" "$KINDS" | cut -c1-160
done
echo "## an approval without the owner's passkey → refused"
post /api/desk/party "{\"policy\":\"$POLICY\",\"name\":\"Mallory\",\"address\":\"0x000000000000000000000000000000000000dEaD\"}"
echo "## the owner states who holds the venue's master key (docs/87 option A), and keeps no unhedged base for this run"
CTRL="the venue master key is held by the Tameion team on testnet; no lender holds it yet"
post /api/desk/control "{\"policy\":\"$POLICY\",\"text\":\"$CTRL\",$(owner_auth control "$CTRL")}"
post /api/desk/base "{\"policy\":\"$POLICY\",\"goods\":\"gold\",\"grams\":\"0\",$(owner_auth base gold 0)}"
echo "## 1. the supplier's priced purchase, signed (the service moves the offset by itself)"
MARK=$(curl -s -X POST https://api.hyperliquid-testnet.xyz/info -H 'content-type: application/json' -d '{"type":"allMids"}' | python3 -c 'import sys,json; print(json.load(sys.stdin)["PAXG"])')
COST=$(python3 -c "print(round(10/31.1034768*$MARK, 2))")
milestone bought 10.000 "$COST" Supplier "PO-$DEAL" $KEYS/desk-supplier.key
echo "## a purchase confirmation signed by the buyer's key but naming the Supplier → refused"
DEAL_SAVE=$DEAL; DEAL="$DEAL-X"; milestone bought 10.000 "$COST" Supplier "PO-FORGED" $KEYS/desk-buyer.key; DEAL=$DEAL_SAVE
echo "## an unsigned step without the owner's passkey → refused"
post /api/desk/milestone "{\"policy\":\"$POLICY\",\"milestone\":{\"deal\":\"$DEAL\",\"kind\":\"arrived\",\"goods\":\"gold\",\"quantity\":\"10\",\"counterparty\":\"Vault\",\"document\":\"FAKE\",\"at\":0}}"
echo "## 2. the vault's receipt, signed (gold flies into a vault; docs/84 §1b)"
milestone arrived 10.000 - Vault "VR-$DEAL" $KEYS/desk-vault.key
echo "## 3. the offset as the venue reports it (read only)"
curl -sS "$HOST/api/desk?policy=$POLICY" -H "x-tameion-device: $TOKEN" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("venue"))'
echo "## 4. the owner sends 2 g to fabrication (unsigned: the owner's own entry)"
milestone to_fabrication 2.000 - Workshop "JOB-$DEAL" -
echo "## 5. the lender's kit, root anchored now"
owner /api/desk/statement "{\"policy\":\"$POLICY\",\"deal\":\"$DEAL\"}" > "runtime/desk/api-$DEAL.lender.json"
python3 -c 'import json,sys; k=json.load(open(sys.argv[1])); s=k.get("signed",{}).get("statement",{}); print(k.get("error") or {x:s.get(x) for x in ("held","stage","eligible","ineligible","offsetShareBps","maxLossUsdc","eligibleUsdc","fundingUsdc","hedgeControl")}); print("anchor", k.get("anchor"))' "runtime/desk/api-$DEAL.lender.json"
SIGNER=$(curl -s "$HOST/api/level1/config" | python3 -c 'import sys,json; print(json.load(sys.stdin)["statementSigner"])')
VENUE=$(curl -sS "$HOST/api/desk?policy=$POLICY" -H "x-tameion-device: $TOKEN" | python3 -c 'import sys,json; print(json.load(sys.stdin)["venue"]["account"])')
echo "### the lender's check, pinned: statement key $SIGNER, venue $VENUE, ${#PINS[@]} party flags"
python3 scripts/verify_desk.py "runtime/desk/api-$DEAL.lender.json" --signer "$SIGNER" --desk "$POLICY" --venue "$VENUE" "${PINS[@]}"
echo "### the same kit, nothing pinned (must not say PASS)"
python3 scripts/verify_desk.py "runtime/desk/api-$DEAL.lender.json" | tail -1
echo "## 6. the fabrication job comes back, the buyer's sale is signed, the offset closes"
milestone returned 2.000 - Workshop "JOB-$DEAL-DONE" -
milestone sold 10.000 "$(python3 -c "print(round($COST*1.015, 2))")" Buyer "SO-$DEAL" $KEYS/desk-buyer.key
echo "## the statements this desk issued (owner only), newest first"
curl -sS "$HOST/api/desk/statements?policy=$POLICY" -H "x-tameion-device: $TOKEN" | python3 -c 'import sys,json; l=json.load(sys.stdin)["statements"]; print(len(l), "kits; newest:", l[0]["deal"], l[0]["maxLossUsdc"], l[0]["anchorTx"])'
echo "## the book now"
curl -sS "$HOST/api/desk?policy=$POLICY" -H "x-tameion-device: $TOKEN" | python3 -c 'import sys,json; d=json.load(sys.stdin); [print(x["id"], x["stage"], x["held"], [m["words"] for m in x["milestones"]]) for x in d["deals"]]; print("exposure", d["exposure"]); print("root", d["root"])'
