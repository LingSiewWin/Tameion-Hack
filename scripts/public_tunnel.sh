#!/usr/bin/env bash
# The fallback public link (docs/115 §3): a Cloudflare quick tunnel to the service on this Mac.
#
#   scripts/public_tunnel.sh [port]    default 7880; prints the https URL, runs until Ctrl-C or `kill <pid>`
#
# Sets nothing else: no environment, no restart, no account. A quick tunnel gets a new name on every start and
# passkeys bind to the name, so an account opened through one URL opens only through that URL. For a fixed name:
# scripts/deploy_fly.sh, or a named tunnel after `cloudflared tunnel login` (the founder's call).
# Refuses when the service opens accounts without an invite word (its relayer would pay for anyone's accounts):
# restart it with TAMEION_LEVEL1_INVITE first; TUNNEL_OPEN_OK=1 overrides, for a short check only.
set -euo pipefail
PORT="${1:-7880}"
cfg="$(curl -fsS -m 5 "http://127.0.0.1:$PORT/api/level1/config")" || { echo "nothing answers on :$PORT"; exit 2; }
if ! grep -q '"inviteNeeded":true' <<<"$cfg" && [ "${TUNNEL_OPEN_OK:-}" != 1 ]; then
  echo "the service on :$PORT opens accounts without an invite word: restart it with TAMEION_LEVEL1_INVITE=<word> first (TUNNEL_OPEN_OK=1 for a short check)"
  exit 3
fi
command -v cloudflared >/dev/null || { echo "cloudflared is not installed. The founder runs:  brew install cloudflared"; exit 2; }
log="$(mktemp -t tameion-tunnel)"
cloudflared tunnel --no-autoupdate --url "http://127.0.0.1:$PORT" >"$log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true' EXIT INT TERM
url=""
for _ in $(seq 1 30); do
  url="$(grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' "$log" | head -1 || true)"
  [ -n "$url" ] && break
  kill -0 "$pid" 2>/dev/null || { cat "$log"; exit 1; }
  sleep 1
done
[ -n "$url" ] || { echo "no URL from cloudflared after 30 s; its log: $log"; exit 1; }
echo "$url   (cloudflared pid $pid, log $log; Ctrl-C or kill $$ stops it)"
wait "$pid"
