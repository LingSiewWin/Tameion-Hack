#!/usr/bin/env bash
# Check a signed weekly statement without taking the service's word for it
# (docs/76 §1d, §7 row 8). Python stdlib and cast only; no key.
#
#   curl -fsS "$HOST/api/level1/statement?policy=0x<policy>&week=2026-W42" > statement.json
#   scripts/verify_statement.sh statement.json [$HOST]            # HOST defaults to http://127.0.0.1:7880
#   TAMEION_TOKEN=<agent key with read> curl -H "Authorization: Bearer $TAMEION_TOKEN" ...   # a locked account
#
# Three checks, each printed on its own line; PASS only when all three hold:
#   a. linesHash  = keccak256(canonical JSON of `lines`), recomputed here
#                   (json.dumps(sort_keys=True, separators=(",",":"), ensure_ascii=False) | cast keccak)
#   b. signature  recovers, over the pinned EIP-712 domain and types, to the statement signer the
#                   service advertises at GET /api/level1/config  (cast wallet verify --data)
#   c. root       is on chain under the key the account itself names (agent(), or operator() on a
#                   keyless account), in the record contract pinned here for the account's chain, on
#                   two public nodes: rootOf() now, or -- for an earlier week -- the anchoring
#                   transaction's sender, target, status and calldata. The same rule as
#                   scripts/verify_tenant.sh and apps/console/src/lib/verify.ts.
# A flipped byte anywhere in the lines fails a; anywhere else in what was signed fails b.
# Writes nothing but stdout and a temp dir. Exit 0 on PASS, 1 on FAIL, 2 when a tool is missing.
set -uo pipefail
FILE=${1:?statement.json (fetched from /api/level1/statement)}
HOST=${2:-http://127.0.0.1:7880}
command -v cast >/dev/null 2>&1 || { echo "cast (foundry) is required"; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo "python3 is required"; exit 2; }
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
lower() { tr '[:upper:]' '[:lower:]'; }

# 1. Read the statement once. Every value that reaches the shell is checked
#    against a pattern first; the typed data is rebuilt from pinned types and
#    domain, so the file cannot choose what the signature is checked over.
fields=$(python3 -I - "$FILE" "$tmp" <<'EOF'
import json, re, sys
st = json.load(open(sys.argv[1]))
out = sys.argv[2]
HEX = re.compile(r"^0x[0-9a-fA-F]+$")
def want(v, pat=HEX):
    v = str(v)
    if not pat.match(v):
        sys.exit("verify_statement: unexpected value %r in %s" % (v, sys.argv[1]))
    return v
lines = st["lines"]
with open(out + "/lines.json", "w", encoding="utf-8") as f:
    f.write(json.dumps(lines, sort_keys=True, separators=(",", ":"), ensure_ascii=False))
chain_id = int(st["chain"]["chainId"])
typed = {
    "types": {
        "EIP712Domain": [{"name": "name", "type": "string"}, {"name": "version", "type": "string"}, {"name": "chainId", "type": "uint256"}],
        "Statement": [{"name": "policy", "type": "address"}, {"name": "chainId", "type": "uint256"}, {"name": "from", "type": "uint64"},
                      {"name": "to", "type": "uint64"}, {"name": "root", "type": "bytes32"}, {"name": "linesHash", "type": "bytes32"}],
    },
    "primaryType": "Statement",
    "domain": {"name": "Tameion Statement", "version": "1", "chainId": chain_id},
    "message": {"policy": want(st["policy"]), "chainId": chain_id, "from": int(st["week"]["from"]), "to": int(st["week"]["to"]),
                "root": want(st["record"]["root"]), "linesHash": want(st["record"]["linesHash"])},
}
with open(out + "/typed.json", "w") as f:
    json.dump(typed, f)
print("POLICY=%s" % want(st["policy"]))
print("CHAIN=%s" % want(st["chain"]["key"], re.compile(r"^[a-z0-9-]+$")))
print("CHAIN_ID=%d" % chain_id)
print("WEEK=%s" % want(st["week"]["label"], re.compile(r"^\d{4}-W\d{2}$")))
print("ROOT=%s" % want(st["record"]["root"]))
print("LINES_HASH=%s" % want(st["record"]["linesHash"]))
print("ANCHOR_TX=%s" % (want(st["record"]["anchorTx"]) if st["record"].get("anchorTx") else ""))
print("SIGNER=%s" % want(st["signature"]["signer"]))
print("SIG=%s" % want(st["signature"]["sig"]))
print("N=%d" % len(lines))
EOF
) || { echo "verify_statement: cannot read $FILE as a statement"; exit 1; }
eval "$fields"
echo "statement $WEEK for $POLICY on $CHAIN (chainId $CHAIN_ID): $N lines, root $ROOT"

# a. the lines hash
a=FAIL
got=$(cast keccak < "$tmp/lines.json")
[ "$(printf '%s' "$got" | lower)" = "$(printf '%s' "$LINES_HASH" | lower)" ] && a=ok
echo "a. linesHash recomputed $got"
echo "   statement claims     $LINES_HASH   → $([ $a = ok ] && echo MATCH || echo MISMATCH)"

# b. the signature, against the signer the service advertises (not the one the file names)
b=FAIL
advertised=$(curl -fsS -m 20 "$HOST/api/level1/config" 2>/dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("statementSigner") or "")' 2>/dev/null)
if [ -z "$advertised" ]; then
  echo "b. $HOST/api/level1/config advertises no statementSigner: cannot say who signed   → INVALID"
else
  if cast wallet verify --address "$advertised" --data --from-file "$tmp/typed.json" "$SIG" >/dev/null 2>&1; then b=ok; fi
  echo "b. EIP-712 signature by the advertised signer $advertised → $([ $b = ok ] && echo VALID || echo INVALID)   (the file names $SIGNER)"
fi

# c. the root on chain, two nodes, under the key the account names
c=FAIL
ZERO=0x0000000000000000000000000000000000000000000000000000000000000000
case "$CHAIN" in
  arc-testnet)      d1=https://rpc.testnet.arc.io;        d2=https://explorer.testnet.arc.io/api/eth-rpc;     ANCHOR=0xDAb65A5380BCC2Eebc3A3A91d0AD11Eec95994dB ;;
  arbitrum-sepolia) d1=https://sepolia-rollup.arbitrum.io/rpc; d2=https://arbitrum-sepolia-rpc.publicnode.com;     ANCHOR=0xe7C381AAFA077CC99359E8b297D2125efD76B8F5 ;;
  *) d1=; d2=; ANCHOR= ;;
esac
if [ -z "$ANCHOR" ]; then
  echo "c. no public nodes or record contract pinned here for chain $CHAIN   → FAIL"
elif [ "$(printf '%s' "$ROOT" | lower)" = "$ZERO" ]; then
  echo "c. the statement stands on no anchored record (root is zero): nothing on chain can vouch for it   → FAIL"
else
  RPC_URL=${RPC_URL:-$d1}
  RPC_URL_2=${RPC_URL_2:-$d2}
  SEL=$(cast sig 'anchor(bytes32)')
  okcount=0
  for rpc in "$RPC_URL" "$RPC_URL_2"; do
    echo "-- against $rpc (block $(cast block-number --rpc-url "$rpc" 2>/dev/null || echo '?'))"
    writer=$(cast call "$POLICY" 'agent()(address)' --rpc-url "$rpc" 2>/dev/null)
    who="the account's agent"
    if [ "$(printf '%s' "$writer" | lower)" = "0x000000000000000000000000000000000000dead" ]; then
      writer=$(cast call "$POLICY" 'operator()(address)' --rpc-url "$rpc" 2>/dev/null)
      who="the operator key this account names (it has no agent key)"
    fi
    if [ -z "$writer" ]; then echo "   the node did not answer agent() on $POLICY"; continue; fi
    latest=$(cast call "$ANCHOR" 'rootOf(address)(bytes32)' "$writer" --rpc-url "$rpc" 2>/dev/null)
    if [ "$(printf '%s' "$latest" | lower)" = "$(printf '%s' "$ROOT" | lower)" ]; then
      echo "   rootOf($writer) = $latest: the latest record $who put on chain   → MATCH"
      okcount=$((okcount + 1))
    elif [ -n "$ANCHOR_TX" ]; then
      # An earlier week: rootOf() holds only the latest root, so the record is
      # checked by the transaction that anchored it, read from this node.
      verdict=$( { cast tx "$ANCHOR_TX" --json --rpc-url "$rpc"; echo; cast receipt "$ANCHOR_TX" --json --rpc-url "$rpc"; } 2>/dev/null \
        | python3 -I -c '
import json, sys
raw = sys.stdin.read().strip().split("\n", 1)
tx, rc = json.loads(raw[0]), json.loads(raw[1])
ok = (rc.get("status") in ("0x1", 1, True)
      and str(tx.get("to", "")).lower() == sys.argv[1].lower()
      and str(tx.get("from", "")).lower() == sys.argv[2].lower()
      and str(tx.get("input", "")).lower() == (sys.argv[3] + sys.argv[4][2:]).lower())
print("ok" if ok else "no")' "$ANCHOR" "$writer" "$SEL" "$ROOT" 2>/dev/null)
      if [ "$verdict" = "ok" ]; then
        echo "   rootOf has moved on to $latest; $ANCHOR_TX was sent by $who to the record contract, succeeded and carries this root   → MATCH"
        okcount=$((okcount + 1))
      else
        echo "   rootOf = $latest and $ANCHOR_TX does not carry this root from $who   → MISMATCH"
      fi
    else
      echo "   rootOf = $latest, not this root, and the statement names no anchoring transaction   → MISMATCH"
    fi
  done
  [ "$okcount" -eq 2 ] && c=ok
  echo "c. root on chain under the account's own key, two nodes: $okcount of 2   → $([ $c = ok ] && echo MATCH || echo MISMATCH)"
fi

if [ $a = ok ] && [ $b = ok ] && [ $c = ok ]; then
  echo "verify_statement: PASS ($WEEK, $N lines, signer $advertised, root on chain)"
  exit 0
fi
echo "verify_statement: FAIL (lines $a, signature $b, chain $c)"
exit 1
