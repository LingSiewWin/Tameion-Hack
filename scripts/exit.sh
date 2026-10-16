#!/usr/bin/env bash
# Exit kit: the recovery wallet takes the account's USDC out using only the chain.
#
# With Tameion's service and domain gone, the phone's passkey no longer signs
# for the account (a browser uses a passkey only on the domain that made it).
# The owner contract (PasskeyPrincipal) carries a `recovery` address from its
# constructor — by default the wallet pasted at Start (docs/60 P-76). That
# wallet proposes a replacement passkey, waits `recoveryDelay`, applies it,
# and the new passkey signs `withdraw`. The replacement is a plain P-256 key
# from openssl: OpenZeppelin's WebAuthn library omits the origin and rpIdHash
# checks (WebAuthn.sol, header), so an assertion built by hand verifies.
# Tools: cast, openssl, python3 (stdlib). Nothing here calls Tameion.
#
#   scripts/exit.sh key                      make the P-256 key at $EXIT_KEY
#   scripts/exit.sh propose                  recovery wallet proposes it as the passkey
#   scripts/exit.sh cancel                   recovery wallet drops its own proposal
#   scripts/exit.sh apply                    after recoveryDelay (anvil: moves the clock)
#   scripts/exit.sh withdraw <to> <units>    sign withdraw(to, units); any funded key submits
#   scripts/exit.sh demo                     all of it on a fresh anvil (:8547) — the proof
#
# Environment:
#   RPC_URL      JSON-RPC of the chain (default http://127.0.0.1:8547)
#   OWNER        PasskeyPrincipal address (console Settings shows it)
#   POLICY       BudgetPolicy address (the account)
#   RECOVERY_PK  key of the recovery wallet (an EOA; a Safe sends the same two calls)
#   SUBMIT_PK    any funded key, pays gas for apply/withdraw (default RECOVERY_PK)
#   EXIT_KEY     path of the P-256 key (default ~/.tameion/exit.key, mode 600)
#   USDC         token the policy holds, or "native" on Arc (default native)
#
# Contract facts used (contracts/src/PasskeyPrincipal.sol, BudgetPolicy.sol:863):
#   proposeRecovery(bytes32,bytes32)   msg.sender == recovery; eta = now + recoveryDelay
#   cancelRecovery()                   the passkey (through execute) or recovery
#   applyRecovery()                    anyone, once block.timestamp >= eta
#   execute(target,value,data,auth)    WebAuthn.verify(challengeFor(target,value,data,nonce), auth, qx, qy, UV=true)
#   withdraw(to, units)                onlyPrincipal; not under the agent's caps
set -euo pipefail
# Anvil's published test mnemonic: its keys are derived when the script runs, never written into the repository.
anvil_key() { cast wallet private-key --mnemonic "test test test test test test test test test test test junk" --mnemonic-index "$1"; }

CMD=${1:-}
RPC_URL=${RPC_URL:-http://127.0.0.1:8547}
EXIT_KEY=${EXIT_KEY:-$HOME/.tameion/exit.key}
USDC=${USDC:-native}
EXECUTE_SIG='execute(address,uint256,bytes,(bytes32,bytes32,uint256,uint256,bytes,string))'

say()  { printf '\n== %s ==\n' "$1"; }
need() { for v in "$@"; do [[ -n ${!v:-} ]] || { echo "set $v" >&2; exit 2; }; done; }
call() { cast call "$@" --rpc-url "$RPC_URL"; }
lc()   { tr 'A-F' 'a-f' <<<"$1"; }
send() { # args: pk, then cast send args; prints the tx hash
  local pk=$1; shift
  cast send "$@" --rpc-url "$RPC_URL" --private-key "$pk" --json \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["transactionHash"])'
}
balance() {
  if [[ $USDC == native ]]; then python3 -c "print($(cast balance "$1" --rpc-url "$RPC_URL") // 10**12)"
  else call "$USDC" "balanceOf(address)(uint256)" "$1" | awk '{print $1}'; fi
}
usdc() { python3 -c "print(f'{int(\"$1\")/1e6:.6f} USDC')"; }
pending_eta() { call "$OWNER" "pendingKey()(bytes32,bytes32,uint64)" | sed -n 3p | awk '{print $1}'; }
delay() { call "$OWNER" "recoveryDelay()(uint256)" | awk '{print $1}'; }
# A refusal is a revert at gas estimation: nothing mined, the error is the contract's own.
refusal() {
  local out
  if out=$("$@" 2>&1); then echo "  NOT REFUSED: $out"; return 1; fi
  echo "  refused: $(grep -oE 'TooEarly\([^)]*\)|BadPasskeySignature|NotRecovery|NothingPending|NotPrincipal' <<<"$out" | head -1)"
}

# qx, qy of $EXIT_KEY: the DER public key ends in 0x04 || X || Y.
pubkey() {
  local hex; hex=$(openssl ec -in "$EXIT_KEY" -pubout -outform DER 2>/dev/null | tail -c 65 | xxd -p -c 65)
  [[ ${hex:0:2} == 04 && ${#hex} -eq 130 ]] || { echo "$EXIT_KEY is not a P-256 key" >&2; exit 1; }
  QX=0x${hex:2:64}; QY=0x${hex:66:64}
}

cmd_key() {
  if [[ ! -s $EXIT_KEY ]]; then
    mkdir -p "$(dirname "$EXIT_KEY")"
    (umask 077; openssl ecparam -name prime256v1 -genkey -noout -out "$EXIT_KEY")
    echo "  new P-256 key at $EXIT_KEY (mode 600); after apply it is the account's only key"
  else
    echo "  using the key at $EXIT_KEY"
  fi
  pubkey; echo "  qx $QX"; echo "  qy $QY"
}

cmd_propose() {
  need OWNER RECOVERY_PK; pubkey
  local rec from; rec=$(call "$OWNER" "recovery()(address)"); from=$(cast wallet address --private-key "$RECOVERY_PK")
  [[ $(lc "$rec") == "$(lc "$from")" ]] || { echo "the contract's recovery is $rec, not $from" >&2; exit 1; }
  echo "  recovery() $rec · recoveryDelay() $(delay) s"
  echo "  tx $(send "$RECOVERY_PK" "$OWNER" "proposeRecovery(bytes32,bytes32)" "$QX" "$QY")"
  echo "  pendingKey().eta $(pending_eta)"
}

cmd_cancel() {
  need OWNER RECOVERY_PK
  echo "  tx $(send "$RECOVERY_PK" "$OWNER" "cancelRecovery()")"
  echo "  pendingKey().eta $(pending_eta)"
}

cmd_apply() {
  need OWNER; local pk=${SUBMIT_PK:-${RECOVERY_PK:?set SUBMIT_PK or RECOVERY_PK}}
  local eta now; eta=$(pending_eta); now=$(cast block latest -f timestamp --rpc-url "$RPC_URL")
  (( eta > 0 )) || { echo "nothing pending: run propose first" >&2; exit 1; }
  if (( now < eta )); then
    if [[ $(cast chain-id --rpc-url "$RPC_URL") == 31337 ]]; then
      echo "  anvil: moving the clock $(( eta - now )) s, past eta"
      cast rpc evm_increaseTime "$(( eta - now ))" --rpc-url "$RPC_URL" >/dev/null
      cast rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
    else
      echo "  too early: eta $eta, chain time $now, $(( eta - now )) s to go; run apply again then" >&2; exit 1
    fi
  fi
  echo "  tx $(send "$pk" "$OWNER" "applyRecovery()")"
  echo "  qx() $(call "$OWNER" "qx()(bytes32)")"
}

# Build the WebAuthn assertion for execute(POLICY, 0, withdraw(to, units)),
# sign it with $EXIT_KEY, and submit it from $SUBMIT_PK.
cmd_withdraw() {
  need OWNER POLICY; local to=${1:?to}; local units=${2:?units}
  local pk=${SUBMIT_PK:-${RECOVERY_PK:?set SUBMIT_PK or RECOVERY_PK}}
  local tmp; tmp=$(mktemp -d)
  local data n c
  data=$(cast calldata "withdraw(address,uint256)" "$to" "$units")
  n=$(call "$OWNER" "nonce()(uint256)")
  c=$(call "$OWNER" "challengeFor(address,uint256,bytes,uint256)(bytes32)" "$POLICY" 0 "$data" "$n")
  echo "  withdraw($to, $units) · nonce $n"
  echo "  challengeFor $c"
  # authenticatorData = rpIdHash (any 32 bytes; the library does not check it) || flags 0x05 (UP|UV) || counter.
  # clientDataJSON: "type" first, then "challenge" = base64url(challenge) without padding (Base64.encodeURL).
  # The library signs sha256(authenticatorData || sha256(clientDataJSON)); openssl dgst -sha256 does
  # that hash over the file we write, so the file holds authenticatorData || sha256(clientDataJSON).
  eval "$(python3 - "$c" "$tmp" <<'PY'
import base64, hashlib, sys
challenge = bytes.fromhex(sys.argv[1][2:]); tmp = sys.argv[2]
auth = hashlib.sha256(b"exit.invalid").digest() + b"\x05" + b"\x00\x00\x00\x01"
b64 = base64.urlsafe_b64encode(challenge).rstrip(b"=").decode()
cd = '{"type":"webauthn.get","challenge":"%s","origin":"https://exit.invalid"}' % b64
open(tmp + "/preimage", "wb").write(auth + hashlib.sha256(cd.encode()).digest())
print("AUTH=0x" + auth.hex()); print("CD='%s'" % cd)
print("TI=%d" % cd.index('"type"')); print("CI=%d" % cd.index('"challenge"'))
PY
)"
  openssl dgst -sha256 -sign "$EXIT_KEY" -out "$tmp/sig.der" "$tmp/preimage"
  # DER (r, s) -> 32-byte words; s in the low half (P256.verify refuses s > N/2).
  eval "$(python3 - "$tmp/sig.der" <<'PY'
import sys
N = 0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551
d = open(sys.argv[1], "rb").read()
assert d[0] == 0x30 and d[2] == 0x02, "not a DER ECDSA signature"
rl = d[3]; r = int.from_bytes(d[4:4 + rl], "big")
assert d[4 + rl] == 0x02; sl = d[5 + rl]; s = int.from_bytes(d[6 + rl:6 + rl + sl], "big")
if s > N // 2: s = N - s
print("R=0x%064x" % r); print("S=0x%064x" % s)
PY
)"
  rm -rf "$tmp"
  local before after tx
  before=$(balance "$to")
  echo "  $to holds $(usdc "$before") before"
  tx=$(send "$pk" "$OWNER" "$EXECUTE_SIG" "$POLICY" 0 "$data" "($R,$S,$CI,$TI,$AUTH,'$CD')")
  after=$(balance "$to")
  echo "  tx $tx (submitted by $(cast wallet address --private-key "$pk"))"
  echo "  $to holds $(usdc "$after") after · policy holds $(usdc "$(balance "$POLICY")")"
  (( after > before )) || { echo "the balance did not move" >&2; exit 1; }
}

# The proof: a fresh anvil, an account whose passkey is "lost", the recovery
# wallet (anvil account #2) takes the money out; a stranger (#3) pays the gas.
cmd_demo() {
  local root; root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  local port=8547; RPC_URL=http://127.0.0.1:$port
  local tmp; tmp=$(mktemp -d); EXIT_KEY=$tmp/exit.key
  # anvil's published development keys: test values, no funds anywhere real.
  local relayer_pk=$(anvil_key 0)
  local agent_pk=$(anvil_key 1)
  RECOVERY_PK=$(anvil_key 2)   # #2: the pasted wallet
  SUBMIT_PK=$(anvil_key 3)     # #3: a stranger with gas
  local payout=0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65                           # #4: where the money goes
  echo "exit kit on anvil (chain 31337, port $port) — $(date -u '+%Y-%m-%d %H:%M UTC')"
  echo "no Tameion service is running; every call below is cast/openssl/python3 against the node"
  # Not `local`: the EXIT trap runs after this function has returned. Its
  # output goes to /dev/null, or a `| tee` on this script never sees EOF.
  anvil --port $port --silent >/dev/null 2>&1 & ANVIL_PID=$!
  DEMO_TMP=$tmp
  trap 'kill $ANVIL_PID 2>/dev/null; rm -rf "$DEMO_TMP"' EXIT
  for _ in 1 2 3 4 5 6 7 8 9 10; do cast chain-id --rpc-url "$RPC_URL" >/dev/null 2>&1 && break; sleep 0.5; done

  say "1. deploy: test USDC, PasskeyPrincipal(recovery = anvil #2), BudgetPolicyToken"
  local phone=$tmp/phone.key   # stands in for the passkey on a phone that is now unusable
  (umask 077; openssl ecparam -name prime256v1 -genkey -noout -out "$phone")
  local pq; pq=$(openssl ec -in "$phone" -pubout -outform DER 2>/dev/null | tail -c 65 | xxd -p -c 65)
  local pqx=0x${pq:2:64} pqy=0x${pq:66:64}
  local recovery agent relayer; recovery=$(cast wallet address --private-key "$RECOVERY_PK")
  agent=$(cast wallet address --private-key "$agent_pk"); relayer=$(cast wallet address --private-key "$relayer_pk")
  cd "$root/contracts"
  USDC=$(forge create --broadcast test/BudgetPolicyToken.t.sol:TestUsdc --rpc-url "$RPC_URL" \
    --private-key "$relayer_pk" --constructor-args 6 | awk '/Deployed to:/{print $3}')
  # Same shape as scripts/prove_passkey.sh local: DELAY 48 h, MODE 2 (Autopilot), recovery delay 7 d.
  local out; out=$(forge script script/PasskeyPolicy.s.sol \
    --sig "deployToken(address,bytes32,bytes32,address,uint256,address,address,uint256,uint256,uint256,uint256,uint256,uint8)" \
    "$USDC" "$pqx" "$pqy" "$recovery" 604800 "$agent" "$relayer" 0 2000000 5000000 0 172800 2 \
    --rpc-url "$RPC_URL" --private-key "$relayer_pk" --broadcast 2>&1)
  cd "$root"
  OWNER=$(awk '/^ +owner /{print $2}' <<<"$out"); POLICY=$(awk '/^ +policy /{print $2}' <<<"$out")
  [[ -n $OWNER && -n $POLICY ]] || { echo "$out" >&2; exit 1; }
  cast send "$USDC" "mint(address,uint256)" "$POLICY" 20000000 --rpc-url "$RPC_URL" --private-key "$relayer_pk" >/dev/null
  echo "  USDC (test, 6 dp)  $USDC"
  echo "  owner              $OWNER  recovery() = $(call "$OWNER" 'recovery()(address)')  recoveryDelay() = $(delay) s"
  echo "  policy             $POLICY  principal() = $(call "$POLICY" 'principal()(address)')"
  echo "  policy holds $(usdc "$(balance "$POLICY")") · payout $payout holds $(usdc "$(balance "$payout")")"

  say "2. the key that replaces the passkey (openssl, no browser)"
  cmd_key

  say "3. the recovery wallet can propose, and can cancel its own proposal"
  cmd_propose
  cmd_cancel
  echo "  (eta 0: nothing pending; a proposal the owner did not want dies the same way, by the passkey through execute)"

  say "4. propose again; too early is refused; the new key cannot sign yet"
  cmd_propose
  refusal cast send "$OWNER" "applyRecovery()" --rpc-url "$RPC_URL" --private-key "$SUBMIT_PK"
  echo "  withdraw with the new key before apply:"
  ( cmd_withdraw "$payout" 1000000 2>&1 | grep -oE 'BadPasskeySignature' | sed 's/^/  refused: /' ) || true

  say "5. after the delay: apply (anyone), then withdraw signed by the new key, submitted by a stranger"
  cmd_apply
  cmd_withdraw "$payout" 20000000

  say "6. what the agent key still cannot do"
  refusal cast send "$POLICY" "withdraw(address,uint256)" "$payout" 1 --rpc-url "$RPC_URL" --private-key "$agent_pk"
  echo
  echo "done: $(usdc 20000000) left the account to $payout with no Tameion service, no browser, no passkey from a phone."
}

case "$CMD" in
  key) cmd_key ;;
  propose) cmd_propose ;;
  cancel) cmd_cancel ;;
  apply) cmd_apply ;;
  withdraw) shift; cmd_withdraw "$@" ;;
  demo) cmd_demo ;;
  *) sed -n '2,36p' "$0"; exit 2 ;;
esac
