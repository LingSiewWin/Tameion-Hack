#!/usr/bin/env bash
# One gold deal through the desk (docs/83 W6): signed milestones, the offset on
# Hyperliquid testnet following the book, a lender's statement, the root
# anchored on Arbitrum Sepolia. Output lands in proof/.
#
#   scripts/prove_desk.sh dry     # everything except orders (the anchor is real)
#   scripts/prove_desk.sh live    # real orders with the trade-only key
#
# HEDGE_USER defaults to the testnet account the trade-only key is approved for.
set -euo pipefail
cd "$(dirname "$0")/.."
mode="${1:-dry}"
export HEDGE_USER="${HEDGE_USER:-0x557E1E07652B75ABaA667223B11704165fC94d09}"
case "$mode" in
  dry)  unset HEDGE_LIVE ;;
  live) export HEDGE_LIVE=1 ;;
  *) echo "usage: $0 dry|live" >&2; exit 2 ;;
esac
out="proof/desk-gold-${mode}-$(date +%Y-%m-%d-%H%M).txt"
{ echo "# $(date '+%Y-%m-%d %H:%M:%S %z') scripts/prove_desk.sh $mode"; cargo run -q -p operator --example prove_desk 2>&1; } | tee "$out"
echo "saved: $out"
