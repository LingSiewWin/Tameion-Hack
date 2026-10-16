#!/usr/bin/env bash
# The most each key can lose, read from the chain and the venue. Nothing from
# our own database: every number below comes from a `cast call`, a `cast logs`
# or one public venue read, so a buyer can re-run it line by line (docs/46 #2).
#
#   scripts/max_loss.sh                      # live Arc policy in runtime/arc_policy.addr
#   POLICY=0x... RPC_URL=... scripts/max_loss.sh
#   HEDGE_USER=0x... scripts/max_loss.sh     # also the venue margin for that address
#
# Needs: cast (foundry), curl, python3. Read-only; no key.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RPC_URL=${RPC_URL:-https://rpc.testnet.arc.network}
POLICY=${POLICY:-$(cat "$ROOT/runtime/arc_policy.addr")}
FROM_BLOCK=${FROM_BLOCK:-64981000}          # just before the live policy was deployed (attestations at 64981196); override for another deployment
INFO=${INFO:-https://api.hyperliquid-testnet.xyz/info}

call() { cast call "$POLICY" "$1" --rpc-url "$RPC_URL"; }
u() { echo "$1" | awk '{printf "%.6f", $1/1e6}'; }   # 6-decimal USDC minor units

block=$(cast block-number --rpc-url "$RPC_URL")
principal=$(call 'principal()(address)')
operator=$(call 'operator()(address)')
agent=$(call 'agent()(address)')
per_action=$(call 'perActionCap()(uint256)' | awk '{print $1}')
daily=$(call 'dailyCap()(uint256)' | awk '{print $1}')
floor=$(call 'reserveFloor()(uint256)' | awk '{print $1}')
balance=$(call 'balanceUsdc()(uint256)' | awk '{print $1}')
spent=$(call 'spentInWindow()(uint256)' | awk '{print $1}')
remaining=$(call 'remainingAllowance()(uint256)' | awk '{print $1}')

# Attested payees: the only addresses `pay` can reach. Not enumerable from
# storage, so replay PayeeSet events and keep the last value per address.
# Fast path: the public explorer's log index (one request, Blockscout API).
# Exhaustive path (PAYEE_SOURCE=rpc): the same eth_getLogs the explorer indexed,
# paged in CHUNK blocks because the public RPC refuses ranges over a few
# thousand blocks and rate-limits; it takes minutes, and anyone can run it.
EXPLORER=${EXPLORER:-https://explorer.testnet.arc.io}
topic=$(cast keccak 'PayeeSet(address,bool)')
if [ "${PAYEE_SOURCE:-explorer}" = explorer ]; then
payees=$(python3 - "$EXPLORER" "$POLICY" "$topic" <<'PY'
import json,sys,urllib.request,urllib.parse
base,addr,topic=sys.argv[1:4]
url=f"{base}/api/v2/addresses/{addr}/logs"; last={}; params={}
while True:
    q=url+("?"+urllib.parse.urlencode(params) if params else "")
    r=urllib.request.Request(q,headers={"accept":"application/json","user-agent":"curl/8.0 tameion-max-loss"})
    d=json.load(urllib.request.urlopen(r,timeout=60))
    for ev in d.get("items",[]):
        t=ev.get("topics") or []
        if t and t[0] and t[0].lower()==topic.lower():
            payee="0x"+t[1][-40:]
            if payee not in last:           # newest first: first seen wins
                last[payee]=int(ev["data"],16)==1
    params=d.get("next_page_params") or {}
    if not params: break
print("\n".join(p for p,v in last.items() if v))
PY
)
else
payees=$(python3 - "$RPC_URL" "$POLICY" "$FROM_BLOCK" "$block" "${CHUNK:-4000}" "$topic" <<'PY'
import json,sys,urllib.request,urllib.error,time
rpc,addr,lo,hi,chunk,topic=sys.argv[1],sys.argv[2],int(sys.argv[3]),int(sys.argv[4]),int(sys.argv[5]),sys.argv[6]
def call(frm,to):
    body=json.dumps({"jsonrpc":"2.0","id":1,"method":"eth_getLogs","params":[{"address":addr,"topics":[topic],"fromBlock":hex(frm),"toBlock":hex(to)}]}).encode()
    r=urllib.request.Request(rpc,data=body,headers={"content-type":"application/json","user-agent":"curl/8.0 tameion-max-loss"})
    for attempt in range(12):
        try:
            time.sleep(0.25); return json.load(urllib.request.urlopen(r,timeout=60))
        except urllib.error.HTTPError as e:
            if e.code!=429: raise
            time.sleep(1.5*(attempt+1))
    raise SystemExit("eth_getLogs: rate limited 12 times")
last={}; frm=lo
while frm<=hi:
    to=min(frm+chunk-1,hi); res=call(frm,to)
    if "error" in res:
        if chunk<=500: raise SystemExit("eth_getLogs: "+str(res["error"]))
        chunk//=2; continue
    for ev in res["result"]: last["0x"+ev["topics"][1][-40:]]=int(ev["data"],16)==1
    frm=to+1
print("\n".join(p for p,v in last.items() if v))
PY
)
fi
n_payees=$(printf '%s' "$payees" | grep -c . || true)

# agent key: one rolling window, only to attested payees
agent_window=$(( daily < balance - floor ? daily : balance - floor ))
[ "$agent_window" -lt 0 ] && agent_window=0
# operator key: raise caps to the balance, zero the floor, swap the agent;
# cannot add a payee (setPayee/setPayees are principal-only)
operator_max=$balance

# docs/46 #3/#4 (policies deployed after 2026-10-13); older deployments have
# neither function, which the output says.
if mode_raw=$(call 'mode()(uint8)' 2>/dev/null); then
  case "$mode_raw" in 0) mode=Watch;; 1) mode=Propose;; *) mode=Autopilot;; esac
  delay=$(call 'delay()(uint256)' | awk '{print $1}')
  pend=$(call 'pending()(uint256,uint256,uint256,uint8,uint64,uint8)' | tr '\n' ' ')
  kinds=$(echo "$pend" | awk '{print $NF}')
  if [ "$kinds" = 0 ]; then pending_line="pending change   : none"; else
    pending_line="pending change   : kinds=$kinds (1 limits, 2 mode, 3 both) -> perActionCap/dailyCap/reserveFloor/mode/eta = $pend  (principal can cancelPending until eta)"; fi

  gov_lines="mode             : $mode   (pay works only in Autopilot; lowering is instant, raising waits the delay)
delay            : ${delay}s = $(awk "BEGIN{printf \"%.1f\", $delay/3600}") h before a raised cap, lowered floor, higher mode or new payee takes effect
$pending_line"
  windows=$(( (delay + 86399) / 86400 )); [ "$windows" -lt 1 ] && windows=1
  operator_note="<= dailyCap per 24 h until a raise applies = $(u "$daily") USDC x $windows window(s) = $(u $((daily * windows))) USDC (a raise waits ${delay}s and the principal can cancel it; agent rotation is instant but bound by the same caps). Cannot add a payee."
else
  gov_lines="mode / delay     : n/a -- this policy predates docs/46 #3/#4 (no mode(), no delay()); raises apply at once"
  operator_note="<= balance = $(u "$operator_max") USDC, still only to the same $n_payees payees (cannot add a payee); no delay on this deployment, stopped by one principal call (setOperator)."
fi
venue_line="venue wallet     : not queried (set HEDGE_USER=0x... to read the margin)"
if [ -n "${HEDGE_USER:-}" ]; then
  acct=$(curl -s "$INFO" -H 'content-type: application/json' \
    -d "{\"type\":\"clearinghouseState\",\"user\":\"$HEDGE_USER\"}" \
    | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["marginSummary"]["accountValue"])')
  venue_line="venue wallet     : <= the margin placed on the venue = $acct USDC (clearinghouseState.accountValue for $HEDGE_USER). \"Cannot withdraw\" is documented by third parties, not proved by us; see docs/46."
fi

cat <<EOF
# max loss, read from chain $(date -u +%FT%TZ)
policy           : $POLICY  (rpc $RPC_URL, block $block)
principal        : $principal
operator         : $operator
agent            : $agent
perActionCap     : $(u "$per_action") USDC
dailyCap         : $(u "$daily") USDC   (rolling 24 h; spent in window $(u "$spent"), remaining $(u "$remaining"))
reserveFloor     : $(u "$floor") USDC
balance          : $(u "$balance") USDC
$gov_lines
attested payees  : $n_payees
$(printf '%s\n' "$payees" | sed 's/^/                   /')

agent key        : <= min(dailyCap, balance - reserveFloor) = $(u "$agent_window") USDC per rolling 24 h, only to the $n_payees attested payees above, <= $(u "$per_action") per call; stopped by one operator call (setAgent).
operator key     : $operator_note
principal key    : everything ($(u "$balance") USDC). withdraw has no cap and no delay, by design (docs/14).
$venue_line
EOF
