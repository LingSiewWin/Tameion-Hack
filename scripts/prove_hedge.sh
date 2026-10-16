#!/usr/bin/env bash
# Proof runner for the hedge layer. Every mode writes its full output to
# proof/ so a claim in the docs can be checked against a real run.
#
#   scripts/prove_hedge.sh test       # cargo test --locked for ledger + hedge (pure core)
#   scripts/prove_hedge.sh readonly   # live testnet reads, no key, no orders
#   scripts/prove_hedge.sh dry        # full step against live prices, no orders
#   scripts/prove_hedge.sh unhalt "<reason>"  # clear HALT with a signed line in HALT.history (docs/46)
#   scripts/prove_hedge.sh nowithdraw # the API wallet tries to withdraw; the venue must refuse (docs/46)
#   scripts/prove_hedge.sh live       # one real hedge on testnet (needs funded
#                                     # account + API wallet key, see live_once.rs)
set -euo pipefail
cd "$(dirname "$0")/.."
mode="${1:-}"
stamp="$(date +%Y-%m-%d)"
mkdir -p proof runtime/hedge

case "$mode" in
  test)
    out="proof/hedge-tests-${stamp}.txt"
    { echo "# $(date -u +%FT%TZ) cargo test --locked --no-fail-fast -p ledger -p hedge"; cargo test --locked --no-fail-fast -p ledger -p hedge 2>&1; } | tee "$out"
    ;;
  readonly)
    out="proof/hedge-testnet-readonly-${stamp}.txt"
    { echo "# $(date -u +%FT%TZ) readonly_testnet"; cargo run --locked -q -p hedge --example readonly_testnet 2>&1; } | tee "$out"
    ;;
  dry)
    out="proof/hedge-testnet-dryrun-${stamp}.txt"
    { echo "# $(date -u +%FT%TZ) dry_run HEDGE_SPOT=${HEDGE_SPOT:-ETH=0.5}"; cargo run --locked -q -p hedge --example dry_run 2>&1; } | tee "$out"
    ;;
  probe)
    out="proof/hedge-testnet-probe-${stamp}.txt"
    { echo "# $(date -u +%FT%TZ) probe_positions (validates position parsing against a live, non-empty account)"; cargo run --locked -q -p hedge --example probe_positions 2>&1; } | tee "$out"
    ;;
  funding)
    out="proof/hedge-testnet-funding-${stamp}.txt"
    { echo "# $(date -u +%FT%TZ) probe_funding (books real user_funding payloads through book::funding)"; cargo run --locked -q -p hedge --example probe_funding 2>&1; } | tee "$out"
    ;;
  unhalt)
    out="proof/hedge-unhalt-${stamp}.txt"
    { echo "# $(date -u +%FT%TZ) unhalt: clear runtime/hedge/HALT with a signed, named reason (docs/46)"; HALT_REASON="${HALT_REASON:-${2:-}}" cargo run --locked -q -p hedge --example unhalt 2>&1; } | tee "$out"
    ;;
  nowithdraw)
    out="proof/hedge-testnet-nowithdraw-${stamp}.txt"
    { echo "# $(date -u +%FT%TZ) prove_no_withdraw: the API wallet signs a 1 USDC withdraw to the master's own address; the venue must refuse"; cargo run --locked -q -p hedge --example prove_no_withdraw 2>&1; } | tee "$out"
    ;;
  live)
    out="proof/hedge-testnet-live-${stamp}.txt"
    { echo "# $(date -u +%FT%TZ) live_once HEDGE_SPOT=${HEDGE_SPOT:-ETH=0.01}"; cargo run --locked -q -p hedge --example live_once 2>&1; } | tee "$out"
    ;;
  arc)
    # The operator pass on Arc testnet with the hedge step in front of it:
    # one anchored root covers the hedge verdict and the payments. Slow on
    # the public RPC (history scan); run it in the background if needed.
    out="proof/arc-hedge-${stamp}.txt"
    export HEDGE_SPOT="${HEDGE_SPOT:-ETH=0.5}"
    export HEDGE_USER="${HEDGE_USER:-0x6c1ce01BfE3f834A5Fbfa0a9081d5d976b819A25}"
    { echo "# $(date -u +%FT%TZ) prove_arc.sh run HEDGE_SPOT=$HEDGE_SPOT HEDGE_USER=$HEDGE_USER"; scripts/prove_arc.sh run 2>&1; } | tee "$out"
    ;;
  *)
    echo "usage: $0 {test|readonly|dry|probe|funding|unhalt|nowithdraw|live|arc}" >&2; exit 64 ;;
esac
echo "wrote $out"
