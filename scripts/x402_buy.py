#!/usr/bin/env python3
"""Buy one call of an x402 service by signing the purchase with an agent key
file directly -- no Tameion service in between (PLAN 5.54). It is what a
compromised agent could do with its key, and the account's contract still
decides: the acceptance suite and proof/x402-2026-10-14.txt use it to show
that the limits are the contract's, not the service's.

  x402_buy.py <policy> <agent-key-file> <url> [rpc] [replay-file]

Prints `<HTTP status> <decoded PAYMENT-RESPONSE or body>`. With a replay
file, the PAYMENT-SIGNATURE sent is saved there, or -- if the file exists --
sent again unchanged (a replay)."""
import base64, json, os, subprocess, sys, time, urllib.error, urllib.request

policy, keyfile, url = sys.argv[1:4]
rpc = sys.argv[4] if len(sys.argv) > 4 else "https://rpc.testnet.arc.io"
replay = sys.argv[5] if len(sys.argv) > 5 else None


def cast(*a):
    return subprocess.run(["cast", *a], capture_output=True, text=True, check=True).stdout.strip()


def send(header):
    req = urllib.request.Request(url, headers={"PAYMENT-SIGNATURE": header})
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            pr = r.headers.get("PAYMENT-RESPONSE")
            print(r.status, json.loads(base64.b64decode(pr)) if pr else r.read().decode())
    except urllib.error.HTTPError as e:
        pr = e.headers.get("PAYMENT-RESPONSE")
        print(e.code, json.loads(base64.b64decode(pr)) if pr else e.read().decode())


if replay and os.path.exists(replay):
    send(open(replay).read().strip())
    sys.exit(0)
try:
    urllib.request.urlopen(url, timeout=30)
    sys.exit("the service did not ask for payment")
except urllib.error.HTTPError as e:
    need = json.loads(base64.b64decode(e.headers["PAYMENT-REQUIRED"]))
offer = need["accepts"][0]
nonce = "0x" + os.urandom(32).hex()
deadline = int(time.time()) + int(offer.get("maxTimeoutSeconds", 60))
# The chain's clock may run ahead of this machine's (anvil after a time warp).
deadline = max(deadline, int(cast("block", "latest", "--field", "timestamp", "--rpc-url", rpc)) + 120)
digest = cast("call", policy, "purchaseDigest(address,uint256,bytes32,uint64)(bytes32)",
              offer["payTo"], offer["amount"], nonce, str(deadline), "--rpc-url", rpc)
sig = cast("wallet", "sign", "--no-hash", digest, "--private-key", open(keyfile).read().strip())
ctx = cast("abi-encode", "f(bytes32,uint64,bytes)", nonce, str(deadline), sig)
offer.setdefault("extra", {})["assetTransferMethod"] = "erc7710"
payload = {"x402Version": 2, "resource": need.get("resource"), "accepted": offer,
           "payload": {"delegationManager": policy, "permissionContext": ctx, "delegator": policy}}
header = base64.b64encode(json.dumps(payload).encode()).decode()
if replay:
    open(replay, "w").write(header)
send(header)
