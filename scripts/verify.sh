#!/usr/bin/env bash
# The five-minute check a finance lead or auditor runs themselves (docs/46 #1).
# No key. Four steps, each re-runnable on its own:
#
#   1. the limits and the payee list, read off the contract      (scripts/max_loss.sh)
#   2. the anchored decision log recomputed from the events on disk and
#      compared with rootOf() on chain                            (operator example `audit`)
#   3. the hedge books re-posted from the journal and set beside the venue's
#      own balances                                               (hedge example `audit_books`)
#   4. the clear-signing page: the two files' SHA-256, and how to recompute a
#      challenge on a second machine before signing it
#
#   scripts/verify.sh                 # all four
#   scripts/verify.sh 2               # one step
#   HEDGE_USER=0x... scripts/verify.sh
#   TAMEION_LOG_DIR=runtime/app/tenants/<policy>/log scripts/verify.sh 2   # one hosted tenant's record (docs/52), on the host
#   scripts/verify_tenant.sh <host> <policy>                                 # the same from any machine, through the service
#
# Writes nothing but stdout. Exit code is the first failing step's.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export RPC_URL=${RPC_URL:-https://rpc.testnet.arc.network}
steps=${1:-1 2 3 4}
rc=0

banner() { printf '\n== %s ==\n' "$1"; }

for s in $steps; do
  case "$s" in
    1)
      banner "1/4 limits, keys and payees, read off the contract"
      scripts/max_loss.sh || { r=$?; [ $rc -eq 0 ] && rc=$r; }
      ;;
    2)
      banner "2/4 decision log: recompute the root from runtime/log and compare with the chain, on two RPCs"
      # PLAN 5.18 (docs/59 §3.12): the second source is Blockscout's own node.
      for rpc in "$RPC_URL" "${RPC_URL_2:-https://explorer.testnet.arc.io/api/eth-rpc}"; do
        echo "-- against $rpc (block $(cast block-number --rpc-url "$rpc" 2>/dev/null || echo '?'))"
        RPC_URL="$rpc" cargo run --locked -q -p operator --example audit || { r=$?; [ $rc -eq 0 ] && rc=$r; }
      done
      ;;
    3)
      banner "3/4 hedge books: re-post runtime/hedge/journal.jsonl and compare with the venue"
      if [ -z "${HEDGE_USER:-}${TAMEION_TREASURY_ADDR:-}" ]; then
        echo "skipped: set HEDGE_USER=0x... (the venue account the books are kept against)"
      else
        cargo run --locked -q -p hedge --example audit_books || { r=$?; if [ $r -eq 3 ]; then echo "(no hedge books on disk yet: nothing to reconcile, not a failure)"; else [ $rc -eq 0 ] && rc=$r; fi; }
      fi
      ;;
    4)
      banner "4/4 clear-signing page"
      echo "The page loads one same-origin script and makes no network call (apps/classic/passkey/index.html, call.js)."
      echo "Hashes of what you are served, to compare with this repo:"
      shasum -a 256 apps/classic/passkey/index.html apps/classic/passkey/call.js | sed 's/^/  /'
      echo "Before Touch ID, recompute the challenge the page shows on another machine:"
      echo "  cast call <PasskeyPrincipal> 'challengeFor(address,uint256,bytes,uint256)(bytes32)' <target> <value> <calldata> <nonce> --rpc-url $RPC_URL"
      echo "  (the page prints target, value, calldata and nonce in words; the two 32-byte values must be identical)"
      if [ -f apps/classic/passkey/call.test.mjs ] && command -v node >/dev/null 2>&1; then
        echo "The page's own hash routine against pinned Keccak vectors:"
        (cd apps/classic/passkey && node --test call.test.mjs 2>&1 | grep -E 'pass|fail' | sed 's/^/  /') || true
      fi
      ;;
    *) echo "unknown step $s" >&2; exit 64 ;;
  esac
done
echo
if [ "$rc" -eq 0 ]; then echo "verify: all requested steps passed"; else echo "verify: a step failed (exit $rc)"; fi
exit "$rc"
