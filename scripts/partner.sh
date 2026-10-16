#!/usr/bin/env bash
# PLAN 5.45: partners bring their clients (an accounting firm, a payments company).
#
#   scripts/partner.sh add <id> "<name>"   # prints the partner's key ONCE; only its sha256 is stored
#   scripts/partner.sh list
#
# The partner's link is  https://<host>/?partner=<id>  — accounts started from it are
# listed for the partner at  GET /api/partner/accounts  (Authorization: Bearer <key>).
# A partner sees which accounts it brought and how they are doing; never a key, never money.
set -euo pipefail
DIR=${TAMEION_APP_DIR:-runtime/app}
FILE="$DIR/partners.json"
mkdir -p "$DIR"
case "${1:-}" in
  add)
    ID=${2:?partner id (letters, digits, -)}; NAME=${3:?partner name}
    [[ "$ID" =~ ^[a-z0-9-]{2,32}$ ]] || { echo "id: 2–32 of a-z 0-9 -"; exit 1; }
    KEY=$(openssl rand -hex 24)
    python3 - "$FILE" "$ID" "$NAME" "$KEY" <<'PY'
import hashlib, json, os, sys, time
path, pid, name, key = sys.argv[1:5]
rows = json.load(open(path)) if os.path.exists(path) else []
if any(r["id"] == pid for r in rows):
    sys.exit(f"partner {pid} exists")
rows.append({"id": pid, "name": name, "token_hash": hashlib.sha256(key.encode()).hexdigest(), "created": int(time.time())})
json.dump(rows, open(path, "w"), indent=2)
os.chmod(path, 0o600)
PY
    echo "partner $ID added. Key (shown once; store it in the partner's secret manager):"
    echo "$KEY"
    echo "link: /?partner=$ID"
    ;;
  list)
    [ -f "$FILE" ] && python3 -c "import json; [print(r['id'], '·', r['name']) for r in json.load(open('$FILE'))]" || echo "no partners"
    ;;
  *) sed -n 2,9p "$0"; exit 1 ;;
esac
