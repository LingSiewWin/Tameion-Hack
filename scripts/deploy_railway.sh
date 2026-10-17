#!/usr/bin/env bash
# The public copy on Railway (the founder's existing account; docs/115 §6). Same image, same secrets, same checks as
# scripts/deploy_fly.sh; only the host differs. Deploy with:
#
#   scripts/deploy_railway.sh            link (or make) the project and service, mount /data, set the variables, make the
#                                        domain, upload and build, then read /healthz and /api/level1/config back
#   scripts/deploy_railway.sh relayer    only read the relayer's address and funding (scripts/deploy_fly.sh relayer)
#
# Needs: `railway login` once (opens a browser); a Full Trial or a paid plan (a Limited Trial restricts outbound network,
# so the service could not reach the chain); the relayer funded; ~/.tameion/invite.word. Variables as in deploy_fly.sh
# (RELAYER_KEY_FILE, SKIP_FUNDING_CHECK=1); RAILWAY_PROJECT=<name or id> links an existing project instead of making one
# named tameion; RAILWAY_SERVICE names the service (default tameion-agents, part of the domain).
#
# Railway shows a variable's value in its dashboard unless it is sealed there by hand (Fly's secrets are write-only):
# that is why the relayer is the low-balance demo key. Passkeys bind to the domain: never change it after a judge opens
# an account. railway.json carries the health check and restart policy (config as code, retired 2026-12-01).
set -euo pipefail
cd "$(dirname "$0")/.."
K=~/.tameion
SERVICE="${RAILWAY_SERVICE:-tameion-agents}"
export RELAYER_KEY_FILE="${RELAYER_KEY_FILE:-$K/demo-relayer.key}"

if [ "${1:-}" = relayer ]; then exec scripts/deploy_fly.sh relayer; fi

command -v railway >/dev/null || { echo "the Railway CLI is not installed. The founder runs:  brew install railway"; exit 2; }
railway whoami >/dev/null 2>&1 || { echo "not logged in to Railway. The founder runs:  railway login   then this script again"; exit 2; }

if [ "${SKIP_FUNDING_CHECK:-}" != 1 ] && ! scripts/deploy_fly.sh relayer; then
  echo "not deploying with an unfunded relayer: every judge's account would fail at the first step"
  exit 3
fi
[ -s "$K/invite.word" ] || {
  echo "no invite word. The founder writes it:  printf '%s' '<word>' > ~/.tameion/invite.word && chmod 600 ~/.tameion/invite.word"
  exit 2
}
[ -s "$K/admin.token" ] || { (umask 077 && openssl rand -hex 32 >"$K/admin.token") && echo "made $K/admin.token (mode 600, not printed)"; }

# The project, once: .railway/ (git-ignored) remembers the link.
if ! railway status >/dev/null 2>&1; then
  if [ -n "${RAILWAY_PROJECT:-}" ]; then railway link --project "$RAILWAY_PROJECT"; else railway init --name tameion; fi
fi
# The service, once, then selected for every call below.
railway service list --json 2>/dev/null | grep -Eq "\"name\": *\"$SERVICE\"" || railway add --service "$SERVICE"
railway service "$SERVICE" >/dev/null
# The volume at /data (TAMEION_APP_DIR=/data/app, the book in /data/log: Dockerfile ENV), once.
# (`volume add` takes no --service: it uses the service linked just above.)
railway volume list --json 2>/dev/null | grep -q '"/data"' || railway volume add --mount-path /data

# Plain settings (fly.toml [env]) in one call; RAILWAY_RUN_UID=0 because the volume is mounted as root and the image's
# CMD chowns it before dropping to the service user.
railway variable set --service "$SERVICE" --skip-deploys \
  PORT=8080 TAMEION_APP_DIR=/data/app TAMEION_DEMO=1 \
  TAMEION_DEMO_VENDOR=0xE5E2d8dCC2038fd0A010A6e7845378b646e33854 TAMEION_REASONER=cloud-redacted \
  TAMEION_LEVEL1_WATCH_SECS=60 TAMEION_LEVEL1_EVERY_SECS=3600 \
  TAMEION_CONTACT_URL=https://discord.gg/rsVfYutFZg TAMEION_REPO_URL=https://github.com/lingsiewwin/Tameion-Hack \
  RAILWAY_RUN_UID=0 >/dev/null
# Secrets one at a time over stdin: the value never appears in a process's arguments or in this script's output.
secret() { printf '%s' "$(<"$2")" | railway variable set --service "$SERVICE" --skip-deploys "$1" --stdin >/dev/null; }
secret TAMEION_RELAYER_KEY "$RELAYER_KEY_FILE"
secret TAMEION_LEVEL1_INVITE "$K/invite.word"
secret TAMEION_APP_ADMIN_TOKEN "$K/admin.token"
if [ -s "$K/anthropic.key" ]; then secret ANTHROPIC_API_KEY "$K/anthropic.key"; fi
if [ -s "$K/rpc-arbitrum-sepolia.url" ]; then secret TAMEION_RPC_ARBITRUM_SEPOLIA "$K/rpc-arbitrum-sepolia.url"; fi
echo "variables set on $SERVICE: TAMEION_RELAYER_KEY TAMEION_LEVEL1_INVITE TAMEION_APP_ADMIN_TOKEN$([ -s "$K/anthropic.key" ] && echo ' ANTHROPIC_API_KEY')$([ -s "$K/rpc-arbitrum-sepolia.url" ] && echo ' TAMEION_RPC_ARBITRUM_SEPOLIA')"

# The Railway-made domain (one per service), on the image's port; TAMEION_PUBLIC_URL is what the pages and the agent
# card name, so it is set from the domain Railway returns.
DOMAIN="$(railway domain list --service "$SERVICE" 2>/dev/null | grep -oE '[a-z0-9.-]+\.up\.railway\.app' | head -1 || true)"
[ -n "$DOMAIN" ] || DOMAIN="$(railway domain --service "$SERVICE" --port 8080 2>/dev/null | grep -oE '[a-z0-9.-]+\.up\.railway\.app' | head -1 || true)"
[ -n "$DOMAIN" ] || { echo "no Railway domain came back; make one in the dashboard (service → Settings → Networking → Generate Domain) and rerun"; exit 2; }
URL="https://$DOMAIN"
railway variable set --service "$SERVICE" --skip-deploys TAMEION_PUBLIC_URL="$URL" >/dev/null

echo "uploading the tree (minus .gitignore and .railwayignore) and building the image on Railway (the Rust build takes 15–25 min)"
railway up --service "$SERVICE" --ci

for _ in $(seq 1 36); do
  if curl -fsS -m 10 "$URL/healthz" >/dev/null 2>&1; then break; fi
  sleep 10
done
echo "$URL"
echo "GET $URL/healthz -> $(curl -sS -m 10 -o /dev/null -w '%{http_code}' "$URL/healthz") $(curl -sS -m 10 "$URL/healthz" || true)"
curl -sS -m 10 "$URL/api/level1/config" | python3 -I -c '
import json, sys
c = json.load(sys.stdin)
print("operator (relayer):", c.get("operator"), "| invite needed:", c.get("inviteNeeded"), "| publicUrl:", c.get("publicUrl"))
if not c.get("inviteNeeded"):
    print("inviteNeeded is false: the variables did not reach this deployment. In the dashboard, deploy the staged variable changes, then `railway service redeploy`.")'
