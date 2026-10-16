#!/usr/bin/env bash
# The agent's live feed beside the venue's own numbers (docs/106 T3).
#
# SECS seconds of GET /api/desk/live from a running service instance, next to
# Hyperliquid's own `clearinghouseState` read in the same seconds, while a small
# PAXG offset is open on the desk's venue account: opened and closed by the
# engine (the k = 4 / 2x rule, read back after each order, HALT on a mismatch)
# with the account's trade-only key. Nothing is left open: the close also runs
# on any exit after the open. Output lands in proof/.
#
#   PORT=7903 scripts/prove_agent_feed.sh
#
# Testnet only. Run between hh:01 and hh:58 UTC so the small position does not
# collect a funding payment on the desk's account.
set -euo pipefail
cd "$(dirname "$0")/.."
PORT="${PORT:-7903}"
POLICY="${POLICY:-0xcd8d71d97c3a946a14ac3fb92db4fb86e2974308}"
VENUE="${VENUE:-0x557e1e07652b75abaa667223b11704165fc94d09}"
KEY="${HEDGE_AGENT_KEY_FILE:-$HOME/.tameion/hedge-agent.key}"
TOKEN_FILE="${TOKEN_FILE:-$HOME/.tameion/test-keys/desk-gold-2329.token}"
HALT_DIR="${HEDGE_HALT_DIR:-runtime/seat-S3/hedge}"
SIZE="${SIZE:-0.003}"
SECS="${SECS:-60}"
OUT="${OUT:-proof/agent-feed-$(date +%F).txt}"
WORK="$(mktemp -d)"
TOKEN="$(tr -d '[:space:]' < "$TOKEN_FILE")"

cargo build -q -p hedge --example live_once
step() { # one engine pass holding $1 PAXG (the offset is the opposite short)
  HEDGE_AGENT_KEY_FILE="$KEY" TAMEION_TREASURY_ADDR="$VENUE" HEDGE_SPOT="PAXG=$1" \
    HEDGE_HALT_DIR="$HALT_DIR" HEDGE_COVER_USDC=full target/debug/examples/live_once
}
opened=0
close() {
  if [ "$opened" = 1 ]; then
    opened=0
    echo "## close: the engine flattens the offset (holding 0)"
    step 0 | grep -E '^(TRADED|  status|HALTED|action|verdict|## account after|AccountState)' || true
  fi
}
trap 'close; kill $(jobs -p) 2>/dev/null || true' EXIT
# Everything below also lands in $OUT (no subshell: the trap must see `opened`).
exec > >(tee "$OUT") 2>&1

echo "# $(date '+%Y-%m-%d %H:%M:%S %z') scripts/prove_agent_feed.sh — the feed (:$PORT) beside the venue's clearinghouseState"
echo "policy $POLICY  venue $VENUE  halt dir $HALT_DIR  size $SIZE PAXG  seconds $SECS"
echo
echo "## open: the engine offsets a holding of $SIZE PAXG (a short of $SIZE)"
opened=1
step "$SIZE" | grep -E '^(user:|TRADED|  status|HALTED|action|verdict)'
echo
# The feed and the venue, side by side, for SECS seconds.
curl -s -N -m "$((SECS + 3))" -H "x-tameion-device: $TOKEN" \
    "http://127.0.0.1:$PORT/api/desk/live?policy=$POLICY" > "$WORK/feed.txt" &
feed_pid=$!
python3 - "$VENUE" "$SECS" "$WORK/venue.jsonl" <<'PY'
import json, sys, time, urllib.request
venue, secs, out = sys.argv[1], int(sys.argv[2]), sys.argv[3]
body = json.dumps({"type": "clearinghouseState", "user": venue}).encode()
with open(out, "w") as f:
    end = time.time() + secs + 1
    while time.time() < end:
        time.sleep(1 - (time.time() % 1) + 0.3)  # 0.3 s into each second
        req = urllib.request.Request("https://api.hyperliquid-testnet.xyz/info", body, {"content-type": "application/json"})
        try:
            reply = json.load(urllib.request.urlopen(req, timeout=2))
        except Exception as e:  # recorded, not hidden
            reply = {"error": str(e)}
        f.write(json.dumps({"local": time.time(), "reply": reply}) + "\n")
PY
wait "$feed_pid" || true
echo "## the feed beside the venue, same second (venue numbers verbatim; mark = positionValue / |szi|; k = equity / maintenance, 2 dp toward zero)"
python3 - "$WORK/feed.txt" "$WORK/venue.jsonl" <<'PY'
import json, sys
from decimal import Decimal as D, ROUND_DOWN
feed = [json.loads(l[6:]) for l in open(sys.argv[1]) if l.startswith("data: ")]
venue = {}
for l in open(sys.argv[2]):
    r = json.loads(l)
    venue.setdefault(int(r["local"]), r["reply"])
def from_venue(v):
    mm = D(v["crossMaintenanceMarginUsed"])
    av = v["crossMarginSummary"]["accountValue"]
    pos = [{"coin": p["position"]["coin"], "size": p["position"]["szi"], "entry": p["position"]["entryPx"],
            "mark": format((D(p["position"]["positionValue"]) / abs(D(p["position"]["szi"]))).normalize(), "f"),
            "pnl": p["position"]["unrealizedPnl"]} for p in v["assetPositions"]]
    return {"accountValue": av, "marginUsed": v["crossMarginSummary"]["totalMarginUsed"],
            "maintenanceMarginUsed": v["crossMaintenanceMarginUsed"],
            "kRatio": None if mm == 0 else str((D(av) / mm).quantize(D("0.01"), rounding=ROUND_DOWN)),
            "positions": pos}
keys = ["accountValue", "marginUsed", "maintenanceMarginUsed", "kRatio", "positions"]
same = moved = missing = 0
print("second      feed.venueTime  venue.time      equal  accountValue(feed|venue)    pnl(feed|venue)       mark(feed|venue)   k(feed|venue)")
for ev in feed:
    v = venue.get(ev["at"])
    if v is None or "error" in v or ev.get("venueError"):
        missing += 1
        print(f"{ev['at']}  no pair (feed error: {ev.get('venueError')}, venue: {None if v is None else v.get('error')})")
        continue
    want = from_venue(v)
    got = {k: ev[k] for k in keys}
    eq = got == want
    same += eq
    moved += not eq
    fp = (ev["positions"] or [{}])[0]
    vp = (want["positions"] or [{}])[0]
    print(f"{ev['at']}  {ev['venueTime']}  {v['time']}  {'yes' if eq else 'NO '}    "
          f"{ev['accountValue']}|{want['accountValue']}  {fp.get('pnl')}|{vp.get('pnl')}  "
          f"{fp.get('mark')}|{vp.get('mark')}  {ev['kRatio']}|{want['kRatio']}")
    if not eq:
        diff = [k for k in keys if got[k] != want[k]]
        print(f"            differs in {diff}; the two reads are {int(ev['venueTime']) - int(v['time'])} ms apart in the venue's own time")
print()
print(f"SUMMARY: {len(feed)} feed events; {same} identical to the venue's own reply in the same second on every field;"
      f" {moved} differ; {missing} without a pair.")
ages = [ev["at"] * 1000 - ev["venueTime"] for ev in feed if ev.get("venueTime")]
if ages:
    print(f"age of the venue reading in each event, from the event's whole second: at most {max(ages)} to {max(ages) + 1000} ms"
          " (the service itself drops any reading older than 2,000 ms by its own clock)")
print("first event:", json.dumps(feed[0]) if feed else None)
PY
echo
close
echo
echo "## the feed after the close (our records: both orders with their read-back, the pass latency)"
# curl ends the stream itself after 3 s (exit 28); that is the point, not a failure.
(curl -s -N -m 3 -H "x-tameion-device: $TOKEN" "http://127.0.0.1:$PORT/api/desk/live?policy=$POLICY" || true) | grep '^data: ' | tail -1 | cut -c7-
echo
echo "## the venue's own reply after the close"
curl -s -m 5 -X POST https://api.hyperliquid-testnet.xyz/info -H 'content-type: application/json' \
    -d "{\"type\":\"clearinghouseState\",\"user\":\"$VENUE\"}"
echo
echo "saved: $OUT"
