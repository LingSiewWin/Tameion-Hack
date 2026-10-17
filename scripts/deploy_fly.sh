#!/usr/bin/env bash
# The public copy of Tameion on Fly.io (testnet only), in one command (docs/115 §3, §6).
#
#   scripts/deploy_fly.sh           create the app if missing, set its secrets from files, deploy, check /healthz
#   scripts/deploy_fly.sh relayer   only make (if absent) and read the relayer key: its address and funding
#
# The founder does two things once, by hand: `brew install flyctl` and `fly auth login`. This script never logs in.
# Secrets are read from files under ~/.tameion by path and fed to `fly secrets import` on stdin: never printed,
# never on a command line, never in the image.
#   TAMEION_RELAYER_KEY           $RELAYER_KEY_FILE, default ~/.tameion/demo-relayer.key (made here if absent; only
#                                 its address is printed). The existing relayer instead:
#                                 RELAYER_KEY_FILE=~/.tameion/treasury.key scripts/deploy_fly.sh
#   TAMEION_LEVEL1_INVITE         ~/.tameion/invite.word, the word judges type (the founder writes it)
#   TAMEION_APP_ADMIN_TOKEN       ~/.tameion/admin.token (made here if absent)
#   ANTHROPIC_API_KEY             ~/.tameion/anthropic.key (optional: without it the rule decides)
#   TAMEION_RPC_ARBITRUM_SEPOLIA  ~/.tameion/rpc-arbitrum-sepolia.url (optional keyed RPC: without it the public node)
# FLY_APP overrides the app name in fly.toml, FLY_ORG picks the organisation for a new app, TAMEION_PUBLIC_URL
# overrides https://<app>.fly.dev.
# The relayer needs 0.02 ETH + 5 USDC on Arbitrum Sepolia (about 60 demo accounts at 0.05 USDC and ~0.0003 ETH
# of gas each); below that the script stops before deploying (SKIP_FUNDING_CHECK=1 deploys anyway).
set -euo pipefail
cd "$(dirname "$0")/.."
K="$HOME/.tameion"
RPC="https://sepolia-rollup.arbitrum.io/rpc"
USDC=0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d # Circle's USDC on Arbitrum Sepolia (crates/operator/src/watch.rs)
NEED_WEI=20000000000000000                       # 0.02 ETH
NEED_USDC=5000000                                # 5 USDC, 6 decimals
RELAYER_KEY_FILE="${RELAYER_KEY_FILE:-$K/demo-relayer.key}"

command -v cast >/dev/null || { echo "cast (Foundry) is needed to make the relayer key and read its balance"; exit 2; }

relayer() {
  if [ ! -s "$RELAYER_KEY_FILE" ]; then
    [ "$RELAYER_KEY_FILE" = "$K/demo-relayer.key" ] || { echo "no key at $RELAYER_KEY_FILE"; exit 2; }
    mkdir -p "$K"
    (umask 077 && cast wallet new --json | python3 -I -c '
import json, sys
w = json.load(sys.stdin)[0]
open(sys.argv[1], "w").write(w["private_key"] + "\n")
open(sys.argv[2], "w").write(w["address"] + "\n")' "$RELAYER_KEY_FILE" "$K/demo-relayer.address")
    chmod 600 "$RELAYER_KEY_FILE"
    echo "made a fresh demo relayer key: $RELAYER_KEY_FILE (mode 600, not printed)"
  fi
  local af="${RELAYER_KEY_FILE%.key}.address"
  if [ -s "$af" ]; then ADDR="$(tr -d '[:space:]' <"$af")"; else ADDR="$(cast wallet address --private-key "$(<"$RELAYER_KEY_FILE")")"; fi
  local wei usdc
  wei="$(cast balance "$ADDR" --rpc-url "$RPC")"
  usdc="$(cast call "$USDC" 'balanceOf(address)(uint256)' "$ADDR" --rpc-url "$RPC" | awk '{print $1}')"
  [ -n "$wei" ] && [ -n "$usdc" ] || { echo "relayer $ADDR: could not read its balance from $RPC"; return 1; }
  # Exit 0 when funded, 1 when not; the numbers are printed either way.
  python3 -I -c '
import sys
a, wei, usdc, nw, nu = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5])
print(f"relayer {a} on Arbitrum Sepolia: {wei / 1e18:.6f} ETH, {usdc / 1e6:.6f} USDC; needs {nw / 1e18:g} ETH + {nu / 1e6:g} USDC")
ok = wei >= nw and usdc >= nu
if not ok:
    print(f"fund it: send {max(nw - wei, 0) / 1e18:.6f} ETH and {max(nu - usdc, 0) / 1e6:.6f} USDC to {a} (Arbitrum Sepolia)")
sys.exit(0 if ok else 1)' "$ADDR" "$wei" "$usdc" "$NEED_WEI" "$NEED_USDC"
}

if [ "${1:-}" = relayer ]; then
  relayer || true
  exit 0
fi

FLY="$(command -v fly || command -v flyctl || true)"
[ -n "$FLY" ] || { echo "flyctl is not installed. The founder runs:  brew install flyctl  then  fly auth login"; exit 2; }
"$FLY" auth whoami >/dev/null 2>&1 || { echo "not logged in to Fly. The founder runs:  fly auth login   then this script again"; exit 2; }

APP="${FLY_APP:-$(sed -n 's/^app *= *"\(.*\)"/\1/p' fly.toml)}"
URL="${TAMEION_PUBLIC_URL:-https://$APP.fly.dev}"

if ! relayer && [ "${SKIP_FUNDING_CHECK:-}" != 1 ]; then
  echo "not deploying with an unfunded relayer: every judge's account would fail at the first step"
  exit 3
fi
[ -s "$K/invite.word" ] || {
  echo "no invite word. The founder writes it:  printf '%s' '<word>' > ~/.tameion/invite.word && chmod 600 ~/.tameion/invite.word"
  exit 2
}
[ -s "$K/admin.token" ] || { (umask 077 && openssl rand -hex 32 >"$K/admin.token") && echo "made $K/admin.token (mode 600, not printed)"; }

"$FLY" status -a "$APP" >/dev/null 2>&1 || "$FLY" apps create "$APP" ${FLY_ORG:+--org "$FLY_ORG"}

# printf is a shell builtin: the values pass through a pipe, never through a process's arguments.
secrets() {
  printf 'TAMEION_RELAYER_KEY=%s\n' "$(<"$RELAYER_KEY_FILE")"
  printf 'TAMEION_LEVEL1_INVITE=%s\n' "$(<"$K/invite.word")"
  printf 'TAMEION_APP_ADMIN_TOKEN=%s\n' "$(<"$K/admin.token")"
  if [ -s "$K/anthropic.key" ]; then printf 'ANTHROPIC_API_KEY=%s\n' "$(<"$K/anthropic.key")"; fi
  if [ -s "$K/rpc-arbitrum-sepolia.url" ]; then printf 'TAMEION_RPC_ARBITRUM_SEPOLIA=%s\n' "$(<"$K/rpc-arbitrum-sepolia.url")"; fi
}
secrets | "$FLY" secrets import --stage -a "$APP" >/dev/null
echo "secrets set on $APP: TAMEION_RELAYER_KEY TAMEION_LEVEL1_INVITE TAMEION_APP_ADMIN_TOKEN$([ -s "$K/anthropic.key" ] && echo ' ANTHROPIC_API_KEY')$([ -s "$K/rpc-arbitrum-sepolia.url" ] && echo ' TAMEION_RPC_ARBITRUM_SEPOLIA')"

echo "building on Fly's remote builder (no local Docker needed)"
"$FLY" deploy -a "$APP" --remote-only --ha=false --yes -e TAMEION_PUBLIC_URL="$URL"

for _ in $(seq 1 24); do
  if curl -fsS -m 10 "$URL/healthz" >/dev/null 2>&1; then break; fi
  sleep 5
done
echo "$URL"
echo "GET $URL/healthz -> $(curl -sS -m 10 -o /dev/null -w '%{http_code}' "$URL/healthz") $(curl -sS -m 10 "$URL/healthz" || true)"
curl -sS -m 10 "$URL/api/level1/config" | python3 -I -c '
import json, sys
c = json.load(sys.stdin)
print("operator (relayer):", c.get("operator"), "| invite needed:", c.get("inviteNeeded"), "| publicUrl:", c.get("publicUrl"))'
