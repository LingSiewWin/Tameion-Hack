#!/usr/bin/env python3
"""Let the console ask this Mac's Secure Enclave to sign (docs/88 §0b): a local
bridge on 127.0.0.1:7898 that answers POST /sign {"challenge": "0x…32 bytes"}
with the WebAuthn-shaped assertion `target/desk-se sign` produces.

    SE_BLOB=~/.tameion/desks/<name>.blob python3 scripts/desk_se_bridge.py [--unattended]

- Only the service's own pages may call it (Origin http://localhost:7880 or
  http://127.0.0.1:7880, or TAMEION_CONSOLE_ORIGIN); anything else is refused.
- A key made with Touch ID asks the person at this Mac on every signature: the
  prompt is the consent. A key made without it would sign silently, so the
  bridge refuses to serve one unless started with --unattended (proofs only).
- It signs 32-byte challenges and nothing else; the page computes each one
  itself (apps/console/src/lib/guard.ts), never the service.
- Clear-signing (docs/91 R18): a desk decision is sent as its fields
  ({"decision": {"kind": "party", "chainId", "desk", "issued", "name", "address", "kinds"}}; also
  base, control, offset, step). The bridge computes the challenge itself with the same Rust code the
  service verifies with (`desk_challenge`) and the Touch ID prompt says the decision in words, so a
  program that asks for a signature cannot show one thing and sign another. It returns the
  challenge it signed; the page refuses an assertion over any other. A raw challenge (the account's
  other owner actions) is signed with a prompt that says no more than that; `--clear-only` refuses it.
- Owner-contract calls (docs/104 §E2): {"decision": {"kind": "call", "chainId", "owner", "nonce", "target",
  "data"}} for the calls that decide who can replace the key — setGuardians, setRecovery, cancelRecovery on
  the owner contract itself. The bridge recomputes `PasskeyPrincipal.challengeFor` (with `cast`) and words the
  call from its calldata with whole addresses; any other call is refused. With `--unattended` every such call
  is refused (403): a key that asks nobody would let any local process name its own guardians and later
  replace the owner. `--allow-unattended-calls` lifts that for proof runs only; scripts/dev.sh never passes it.
Stdlib only (and Foundry's `cast` for keccak). The key never leaves the chip; this process never sees it.
"""

import json
import os
import re
import subprocess
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SE = os.path.join(ROOT, "target", "desk-se")
BLOB = os.environ.get("SE_BLOB", os.path.expanduser("~/.tameion/se-owner.blob"))
ORIGINS = {"http://localhost:7880", "http://127.0.0.1:7880"} | (
    {os.environ["TAMEION_CONSOLE_ORIGIN"]} if os.environ.get("TAMEION_CONSOLE_ORIGIN") else set())
UNATTENDED = "--unattended" in sys.argv
CLEAR_ONLY = "--clear-only" in sys.argv
ALLOW_UNATTENDED_CALLS = "--allow-unattended-calls" in sys.argv  # proof runs only; scripts/dev.sh never passes it
CALLS_NEED_PRESENCE = "guardian and recovery changes need a key that asks for Touch ID"


class Refused(ValueError):
    """A request this bridge will not sign whatever its fields say: answered 403, not 400."""
CHALLENGE = re.compile(r"^0x[0-9a-fA-F]{64}$")
DESK_CHALLENGE = os.path.join(ROOT, "target", "debug", "examples", "desk_challenge")
ADDRESS = re.compile(r"^0x[0-9a-fA-F]{40}$")
PORT = int(os.environ.get("DESK_SE_BRIDGE_PORT", "7898"))  # the page asks 7898; another port is for proofs only
# The owner-contract calls this bridge can put in words (selectors of PasskeyPrincipal).
OWNER_CALLS = {"e94e9e99": "setGuardians", "f0d85c89": "setRecovery", "0ba234d6": "cancelRecovery"}


def cast(*args):
    r = subprocess.run(["cast", *args], capture_output=True, text=True, timeout=30)
    if r.returncode != 0:
        raise ValueError((r.stderr or "cast failed").strip()[-200:])
    return r.stdout.strip()


def word_address(w):
    if w[:24] != "0" * 24:
        raise ValueError("an argument of the call is not an address")
    return "0x" + w[24:]


def owner_call(d):
    """(challenge, sentence) for a call on the account's own owner contract: `challengeFor` over the call's
    fields, the sentence read from the calldata itself — never from the caller's words."""
    # A key that asks nobody must not decide who can replace the owner (fix round 2 of docs/103 S1-1).
    if UNATTENDED and not ALLOW_UNATTENDED_CALLS:
        raise Refused(CALLS_NEED_PRESENCE)
    chain, nonce = int(d["chainId"]), int(d["nonce"])
    owner, target, data = str(d["owner"]), str(d["target"]), str(d["data"]).lower()
    if not (ADDRESS.match(owner) and ADDRESS.match(target)) or owner.lower() != target.lower():
        raise ValueError("this bridge signs only calls on the account's own owner contract")
    if chain < 0 or nonce < 0 or not re.fullmatch(r"0x[0-9a-f]{8}(?:[0-9a-f]{64})*", data):
        raise ValueError("a call is a selector and whole 32-byte words")
    fn, body = OWNER_CALLS.get(data[2:10]), data[10:]
    w = [body[i:i + 64] for i in range(0, len(body), 64)]
    if fn == "setGuardians":
        if len(w) < 3 or int(w[0], 16) != 64 or int(w[1], 16) > 255 or int(w[2], 16) != len(w) - 3:
            raise ValueError("not a setGuardians call this bridge can read")
        names, need = [word_address(x) for x in w[3:]], int(w[1], 16)
        say = (f"Name {len(names)} guardians of account {owner}: {', '.join(names)}; {need} of them together may replace its key after the delay"
               if names else f"Remove every guardian of account {owner}")
    elif fn == "setRecovery" and len(w) == 1:
        nxt = word_address(w[0])
        say = (f"Remove the address that may alone propose a new key for account {owner}" if int(nxt, 16) == 0
               else f"Let {nxt} alone propose a new key for account {owner}, after the delay")
    elif fn == "cancelRecovery" and not w:
        say = f"Cancel the waiting replacement of the key of account {owner}"
    else:
        raise ValueError("this bridge words only setGuardians, setRecovery and cancelRecovery on the owner contract")
    # PasskeyPrincipal.challengeFor: keccak256(abi.encode(chainid, this, n, target, value, keccak256(data))).
    enc = cast("abi-encode", "f(uint256,address,uint256,address,uint256,bytes32)", str(chain), owner, str(nonce), target, "0", cast("keccak", data))
    ch = cast("keccak", enc)
    if not CHALLENGE.match(ch):
        raise ValueError("the challenge could not be computed")
    if len(say) > 400:
        raise ValueError("this call is too long to show whole on the Touch ID prompt")
    return ch, say


def decide(d):
    """(challenge, sentence) for a desk decision, both from its fields, never from the caller's words."""
    if d.get("kind") == "call":
        return owner_call(d)
    kind, desk = d.get("kind"), str(d.get("desk", ""))
    chain, issued = str(int(d["chainId"])), str(int(d["issued"]))
    if not ADDRESS.match(desk):
        raise ValueError("desk must be the desk's account address")
    # Whole addresses and every field the challenge binds: a sentence that two different decisions share
    # would let a program show one and sign the other (docs/92 T5).
    on = f"on desk {desk}"
    if kind == "party":
        name, addr, kinds = str(d["name"]).strip(), str(d["address"]), d.get("kinds")
        if not ADDRESS.match(addr):
            raise ValueError("address must be an address")
        args = [name, addr] + ([",".join(map(str, kinds))] if kinds else [])
        what = ", ".join(k.replace("_", " ") for k in kinds) if kinds else "any step"
        say = f"Approve {name} ({addr}) to sign {what} {on}"
    elif kind == "base":
        args = [str(d["goods"]), str(d["grams"]).strip()]
        say = f"Keep {args[1]} g of {args[0]} unoffset {on}"
    elif kind == "control":
        args = [str(d["text"]).strip()]
        say = f"State {on}: {args[0]}"
    elif kind == "offset":
        args = [str(d["target"]).strip()]
        say = f"Move the offset to a {args[0]} oz short {on}"
    elif kind == "venue":
        args = [str(d["account"])]
        if not ADDRESS.match(args[0]):
            raise ValueError("account must be an address")
        say = f"Let the desk trade on exchange account {args[0]} {on}"
    elif kind == "step":
        m = d["milestone"]
        args = [json.dumps(m)]
        price = f"for {m['usdc']} USDC" if m.get("usdc") else "no price"
        say = (f"Enter yourself {on}: deal {m['deal']}, {str(m['kind']).replace('_', ' ')} {m['quantity']} g of {m['goods']}, "
               f"{price}, counterparty {m['counterparty']}, document {m['document']}, at {m['at']}")
    else:
        raise ValueError("a decision is party, base, control, offset, venue or step")
    r = subprocess.run([DESK_CHALLENGE, kind, chain, desk, *args, issued], capture_output=True, text=True, timeout=30)
    if r.returncode != 0 or not CHALLENGE.match(r.stdout.strip()):
        raise ValueError((r.stderr or "the challenge could not be computed").strip()[-200:])
    if len(say) > 400:  # cut, two decisions could read the same (docs/94 U7): refuse instead
        raise ValueError("this decision is too long to show whole on the Touch ID prompt; shorten its deal or document name")
    return r.stdout.strip(), say


def presence():
    try:
        return open(BLOB + ".presence").read().strip() == "1"
    except OSError:
        return False


class Bridge(BaseHTTPRequestHandler):
    def _head(self, code, origin=None, body=b""):
        self.send_response(code)
        if origin:
            self.send_header("Access-Control-Allow-Origin", origin)
            self.send_header("Access-Control-Allow-Headers", "content-type")
            self.send_header("Access-Control-Allow-Methods", "POST, OPTIONS")
            self.send_header("Vary", "Origin")
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _origin(self):
        o = self.headers.get("Origin", "")
        return o if o in ORIGINS else None

    def do_OPTIONS(self):
        o = self._origin()
        self._head(204 if o else 403, o)

    def do_POST(self):
        o = self._origin()
        if not o or self.path != "/sign":
            return self._head(403, o, json.dumps({"error": "only the service's own pages may ask this Mac to sign"}).encode())
        n = int(self.headers.get("content-length", "0") or 0)
        try:
            body = json.loads(self.rfile.read(min(n, 8192)) or b"{}")
            if body.get("decision"):
                ch, say = decide(body["decision"])
            elif CLEAR_ONLY:
                return self._head(403, o, json.dumps({"error": "this bridge signs only decisions it can put in words"}).encode())
            else:
                ch, say = body.get("challenge", ""), "Sign a Tameion request from this Mac (no details given)"
        except Refused as e:
            return self._head(403, o, json.dumps({"error": str(e)}).encode())
        except (ValueError, KeyError, TypeError) as e:
            return self._head(400, o, json.dumps({"error": str(e) or "a bad request"}).encode())
        if not CHALLENGE.match(ch):
            return self._head(400, o, json.dumps({"error": "a challenge is 32 bytes of hex"}).encode())
        args = [SE, "sign", ch, "--reason", say] + (["--unattended"] if UNATTENDED else [])
        r = subprocess.run(args, capture_output=True, text=True, env={**os.environ, "SE_BLOB": BLOB}, timeout=120)
        if r.returncode != 0:
            return self._head(400, o, json.dumps({"error": (r.stderr or "the chip did not sign").strip()[-300:]}).encode())
        print(f"desk-se bridge: signed {ch[:10]}… saying: {say}", flush=True)
        self._head(200, o, json.dumps({**json.loads(r.stdout), "challenge": ch, "reason": say}).encode())

    def log_message(self, fmt, *args):
        print("desk-se bridge:", fmt % args, flush=True)


if __name__ == "__main__":
    if not os.path.exists(SE):
        sys.exit("build the signer first: swiftc -O apps/desk-se/main.swift -o target/desk-se")
    if not os.path.exists(DESK_CHALLENGE):
        sys.exit("build the challenge tool first: cargo build -p operator --example desk_challenge")
    if not presence() and not UNATTENDED:
        sys.exit(f"{BLOB} was made without Touch ID: the bridge would sign without asking anyone. "
                 "Make an owner key with `desk-se create` (Touch ID), or pass --unattended for proofs only.")
    print(f"desk-se bridge on http://127.0.0.1:{PORT} for {BLOB} "
          f"({'Touch ID on every signature' if presence() else 'UNATTENDED: no person asked'}); origins {sorted(ORIGINS)}; "
          f"guardian and recovery calls {'REFUSED (unattended)' if UNATTENDED and not ALLOW_UNATTENDED_CALLS else 'signed when worded'}",
          flush=True)
    HTTPServer(("127.0.0.1", PORT), Bridge).serve_forever()
