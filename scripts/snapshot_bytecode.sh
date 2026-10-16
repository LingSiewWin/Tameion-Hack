#!/usr/bin/env bash
# Snapshot the compiled creation bytecode of the contracts the hosted service
# deploys (docs/52, Level 1), so `tameion-app` can deploy without forge on the
# host. Re-run after any change to contracts/src; the gate's forge build is
# the compiler, this just copies its output and records the version.
set -euo pipefail
cd "$(dirname "$0")/.."
(cd contracts && forge build --quiet --force)  # --force: a prior `forge lint` leaves abi-only artifacts the cache calls current
for n in PasskeyPrincipal BudgetPolicy BudgetPolicyPull BudgetPolicyToken TameionDelegate; do
  python3 - "$n" <<'PY'
import json, sys
n = sys.argv[1]
j = json.load(open(f"contracts/out/{n}.sol/{n}.json"))
open(f"contracts/bytecode/{n}.hex", "w").write(j["bytecode"]["object"].strip() + "\n")
md = j.get("metadata", {})
ver = md.get("compiler", {}).get("version", "?")
settings = md.get("settings", {})
print(f"{n}: {len(j['bytecode']['object'])//2} bytes, solc {ver}, optimizer {settings.get('optimizer')}, viaIR {settings.get('viaIR')}")
PY
done | tee contracts/bytecode/VERSION
echo "snapshot written to contracts/bytecode/ ($(date -u +%FT%TZ))" >> contracts/bytecode/VERSION
