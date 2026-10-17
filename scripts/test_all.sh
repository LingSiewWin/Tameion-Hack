#!/usr/bin/env bash
# Every offline test in the repository, in one command. No chain, no keys, no
# network beyond what the toolchains themselves fetch.
#
#   scripts/test_all.sh
#
# What this does not run, because each needs a chain or a browser:
# scripts/acceptance.sh (anvil, 38 checks), scripts/prove_*.sh (live chains),
# and the passkey page in a browser. Those leave their output in proof/.
# The console's static build (`next build`) is not run here; it takes a minute.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
failed=()

step() {
  local name=$1; shift
  printf '\n\033[1m== %s ==\033[0m\n' "$name"
  if "$@"; then echo "  ok"; else echo "  FAILED"; failed+=("$name"); fi
}

# Each suite runs in its own shell with pipefail, so a failing tool is not
# masked by the grep that trims its output.
run() { bash -o pipefail -c "$1"; }

step "contracts: forge test" run 'cd contracts && forge test 2>&1 | grep -E "Ran [0-9]+ test suites"'
step "contracts: forge fmt --check" run 'cd contracts && forge fmt --check >/dev/null'
step "contracts: forge lint (no findings)" run 'cd contracts && ! forge lint src 2>&1 | grep -qE "^(warning|note)"'
step "rust: cargo test --locked" run 'cargo test --locked --workspace --quiet 2>&1 | awk "/test result/{p+=\$4; f+=\$6} /panicked|FAILED/{bad=1} END {print \"  \" p \" passed, \" f \" failed\"; exit (f>0||bad)}"'
step "rust: clippy, warnings are errors" run 'cargo clippy --locked --workspace --all-targets --quiet -- -D warnings'
step "rust: formatting" run 'cargo fmt --all --check'
step "evidence desk: python unittest" run 'python3 -m unittest scripts/evidence/test_desk.py 2>&1 | tail -3'
step "lender verifier: python unittest (offline)" run 'python3 -m unittest scripts/test_verify_desk.py 2>&1 | tail -3'
# The audit pack (docs/117 §5), built offline from a real account's files and the chain's recorded answers; its ledger
# then goes to bean-check, a validator we did not write (`uv tool install beancount`).
step "audit pack: python unittest (offline)" run 'python3 -m unittest scripts/test_audit_pack.py 2>&1 | tail -3'
step "audit pack: bean-check on the fixture's ledger" run 'f=crates/operator/tests/fixtures/audit-pack; a="file://$PWD/$f/answers.json"; out=$(mktemp -d); trap "rm -rf \"$out\"" EXIT; python3 scripts/audit_pack.py "$f" 0xdaf055fff0ad86f112a5f7f9055eee4e5a9ad4c1 2026-10-16 2026-10-17 --rpc "$a" --rpc2 "$a" --gateway "$a" --block 320121361 --out "$out" >/dev/null || exit 1; if command -v bean-check >/dev/null; then bean-check "$out"/*/*/ledger.beancount && echo "  bean-check $(bean-check --version | head -1): exit 0"; else printf "  \033[31mskipped: bean-check not installed\033[0m\n"; fi'
step "passkey page: node --test" run 'node --test apps/classic/passkey/call.test.mjs 2>&1 | grep -E "^ℹ (pass|fail)"'
step "chip bridge: python unittest" run 'python3 -m unittest scripts/test_desk_se_bridge.py 2>&1 | tail -3'
step "key roles: python unittest" run 'python3 -I scripts/test_key_roles.py 2>&1 | tail -3'
# docs/117 §2: one role per key, from the code's defaults and runtime/app. Fails while one key holds two roles
# (today ~/.tameion/treasury.key: relayer, Hyperliquid master, HALT clearer, bill wallet, guardian) — a founder decision.
step "key roles: one role per key" run 'python3 -I scripts/key_roles.py runtime/app'
# The descriptor check validates against the ERC-7730 JSON schema: run it with a Python that has `jsonschema`
# (Homebrew's python3 can move ahead of the one it is installed for — docs/60).
step "clear-signing descriptors: ERC-7730" run 'py=$(for p in python3 python3.14 python3.13; do command -v "$p" >/dev/null && "$p" -c "import jsonschema" 2>/dev/null && { echo "$p"; break; }; done); [ -n "$py" ] || { echo "  no python with jsonschema (pip install jsonschema)"; exit 1; }; "$py" scripts/check_erc7730.py'
# The console (CLAUDE.md: strict TypeScript, ESLint clean) and its own logic tests;
# needs `npm ci` in apps/console once.
step "console: tsc, eslint, node --test" run 'cd apps/console && { [ -d node_modules ] || { echo "  run: (cd apps/console && npm ci)"; exit 1; }; } && npx --no-install tsc --noEmit && npx --no-install eslint src test && node --test test/*.mts 2>&1 | grep -E "^ℹ (pass|fail)"'
# The API contract, console side (docs/114 row 2): every field types.ts reads is in apps/classic/openapi.json, and every
# field it counts on is promised there. The service side (replies against openapi.json) is scripts/acceptance.sh rule 17.
step "console: types.ts against openapi.json" run 'node apps/console/scripts/check-types.mjs'
step "sdk: tsc, node --test" run 'cd apps/sdk && { [ -d node_modules ] || { echo "  run: (cd apps/sdk && npm ci)"; exit 1; }; } && npm test --silent 2>&1 | grep -E "^ℹ (pass|fail)"'

printf '\n'
if [ ${#failed[@]} -eq 0 ]; then
  echo "all suites passed"
else
  printf 'failed: %s\n' "${failed[@]}"
  exit 1
fi
