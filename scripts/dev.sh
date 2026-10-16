#!/usr/bin/env bash
# One command for the whole local stack (the founder's rule: a manual step that
# is not a trust decision is a bug — that includes running the product).
#
#   scripts/dev.sh          build and start everything that is not running
#   scripts/dev.sh status   what is up, on which port
#   scripts/dev.sh stop     stop what this script started
#
# Starts, each only if its port is free:
#   :7880  tameion-app  the service: API, the account page, the agent's passes
#   :3000  the console  (apps/console, Next.js dev server) when it exists
#   :7898  the Secure Enclave bridge, when SE_BLOB names a desk's chip key, so /desk signs with this Mac's
#          chip (Touch ID on each signature; UNATTENDED=1 only for a key made without Touch ID, for proofs);
#          it signs only desk decisions it can put in words, unless RAW_SIGN=1
#          SE_BLOB=~/.tameion/desks/<name>.blob scripts/dev.sh
# HEDGE_LIVE=1 in the environment sends real testnet orders; which desk trades is chosen on /desk ("Connect it").
# Keys stay where they are (~/.tameion, mode 600); nothing here prints one.
set -euo pipefail
cd "$(dirname "$0")/.."
LOGS="${TMPDIR:-/tmp}/tameion-dev"
mkdir -p "$LOGS"

up() { lsof -iTCP:"$1" -sTCP:LISTEN -n -P >/dev/null 2>&1; }
say() { printf '  %-6s %s\n' "$1" "$2"; }

status() {
  for p in 7880 3000 7898; do
    if up "$p"; then say ":$p" "up"; else say ":$p" "down"; fi
  done
}

case "${1:-start}" in
status) status; exit 0 ;;
stop)
  for f in "$LOGS"/*.pid; do
    [ -e "$f" ] || continue
    kill "$(cat "$f")" 2>/dev/null || true
    rm -f "$f"
  done
  status; exit 0 ;;
start) ;;
*) echo "usage: scripts/dev.sh [start|status|stop]" >&2; exit 2 ;;
esac

if up 7880; then
  say ":7880" "already up (left as it is)"
else
  echo "building the service…"
  cargo build -q -p operator --bin tameion-app
  # The agent's gas on Arc and a pass on every transfer to a watched wallet,
  # as the hosted service runs (fly.toml); the local x402 sellers are allowed.
  TAMEION_LEVEL1_WATCH_SECS=60 TAMEION_X402_ALLOW_LOCAL=1 PORT=7880 \
    nohup ./target/debug/tameion-app >"$LOGS/service.log" 2>&1 &
  echo $! >"$LOGS/service.pid"
  for _ in $(seq 1 40); do up 7880 && break; sleep 0.25; done
  say ":7880" "started (log: $LOGS/service.log)"
fi

if [ -f apps/console/package.json ]; then
  if up 3000; then
    say ":3000" "already up (left as it is)"
  else
    [ -d apps/console/node_modules ] || (cd apps/console && npm install --silent)
    (cd apps/console && nohup npm run dev -- -p 3000 >"$LOGS/console.log" 2>&1 & echo $! >"$LOGS/console.pid")
    for _ in $(seq 1 120); do up 3000 && break; sleep 0.5; done
    say ":3000" "started (log: $LOGS/console.log)"
  fi
else
  say ":3000" "no console yet (apps/console)"
fi

if [ -n "${SE_BLOB:-}" ]; then
  if up 7898; then
    say ":7898" "already up (left as it is)"
  else
    [ -x target/desk-se ] || swiftc -O apps/desk-se/main.swift -o target/desk-se
    [ -x target/debug/examples/desk_challenge ] || cargo build -q -p operator --example desk_challenge
    # Only decisions it can put in words (docs/92 T5); RAW_SIGN=1 also signs the account page's other owner actions.
    CLEAR=--clear-only; [ "${RAW_SIGN:-}" = 1 ] && CLEAR=
    nohup python3 scripts/desk_se_bridge.py ${UNATTENDED:+--unattended} $CLEAR >"$LOGS/bridge.log" 2>&1 &
    echo $! >"$LOGS/bridge.pid"
    for _ in $(seq 1 20); do up 7898 && break; sleep 0.25; done
    if up 7898; then say ":7898" "started for $SE_BLOB (log: $LOGS/bridge.log)"; else say ":7898" "did not start: $(tail -1 "$LOGS/bridge.log")"; fi
  fi
fi

echo
echo "  the desk           http://localhost:7880/desk"
echo "  the account page   http://localhost:3000   (console; falls back to http://localhost:7880)"
echo "  the service        http://localhost:7880   (API; /classic is the first page)"
