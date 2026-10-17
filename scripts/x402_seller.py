#!/usr/bin/env python3
"""A paid API, for proving that a Tameion account buys inside its limits
(PLAN 5.54). It speaks x402 v2 over HTTP (`PAYMENT-REQUIRED`,
`PAYMENT-SIGNATURE`, `PAYMENT-RESPONSE`) and is its own facilitator for the
`exact` scheme's ERC-7710 method: it dry-runs `redeemDelegations` on the
buyer's account, then sends it and pays the gas. It is written apart from the
Rust buyer on purpose -- another language, the spec as the only contract
between them.

  PAY_TO=0x...  FACILITATOR_KEY_FILE=~/.tameion/test-keys/x402_seller.key \\
  python3 scripts/x402_seller.py            # http://127.0.0.1:7902/v1/mid?coin=ETH

The resource is real: Hyperliquid's public mid price for one coin, the number
the Protection card (PLAN 5.49) already reads. Price: PRICE_MINOR (default
1000 = 0.001 USDC). Chain: Arc testnet unless RPC / CHAIN_ID / USDC say so.
Test-only: the facilitator key is read from a file and handed to `cast`.
ANSWER_FAILED=1 makes it a dishonest seller: it settles, then answers that
it did not (the buyer must believe the chain, not the seller).
PAYMENT_IDENTIFIER=1 advertises the x402 `payment-identifier` extension as
required (specs/extensions/payment_identifier.md) and refuses a payment
without a well-formed `extensions["payment-identifier"].info.id`."""
import base64, json, os, re, subprocess, sys, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

RPC = os.environ.get("RPC", "https://rpc.testnet.arc.io")
CHAIN_ID = int(os.environ.get("CHAIN_ID", "5042002"))
USDC = os.environ.get("USDC", "0x3600000000000000000000000000000000000000")
PAY_TO = os.environ["PAY_TO"]
PRICE = int(os.environ.get("PRICE_MINOR", "1000"))
KEY_FILE = os.path.expanduser(os.environ["FACILITATOR_KEY_FILE"])
PORT = int(os.environ.get("PORT", "7902"))
NETWORK = f"eip155:{CHAIN_ID}"
WANT_ID = os.environ.get("PAYMENT_IDENTIFIER") == "1"
# The spec's id rules: 16-128 chars of [A-Za-z0-9_-].
ID_OK = re.compile(r"^[A-Za-z0-9_-]{16,128}$")
PAYMENT_IDENTIFIER = {
    "info": {"required": True},
    "schema": {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {"required": {"type": "boolean"}, "id": {"type": "string", "minLength": 16, "maxLength": 128}},
        "required": ["required"],
    },
}


def b64(obj):
    return base64.b64encode(json.dumps(obj, separators=(",", ":")).encode()).decode()


def cast(*args):
    r = subprocess.run(["cast", *args], capture_output=True, text=True, timeout=120)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip().splitlines()[-1] if (r.stderr or r.stdout).strip() else "cast failed")
    return r.stdout.strip()


def requirement(url):
    return {
        "scheme": "exact",
        "network": NETWORK,
        "amount": str(PRICE),
        "asset": USDC,
        "payTo": PAY_TO,
        "maxTimeoutSeconds": 120,
        "extra": {"name": "USDC", "version": "2"},
    }


def resource(url):
    return {"url": url, "description": "Hyperliquid mid price for one coin", "mimeType": "application/json"}


def mid(coin):
    """The resource itself; None when Hyperliquid cannot be reached (the
    payment has settled by then, so the buyer still gets an answer)."""
    try:
        return _mid(coin)
    except Exception:
        return None


def _mid(coin):
    req = urllib.request.Request(
        "https://api.hyperliquid.xyz/info",
        data=json.dumps({"type": "allMids"}).encode(),
        headers={"content-type": "application/json", "user-agent": "tameion-x402-seller"},
    )
    with urllib.request.urlopen(req, timeout=15) as r:
        mids = json.loads(r.read())
    return mids.get(coin.upper())


def settle_erc7710(accepted, payload):
    """Verify by simulation, then settle: the spec's Phase 3 and 4."""
    manager = payload["delegationManager"]
    if manager.lower() != payload["delegator"].lower():
        raise ValueError("this facilitator redeems only on the paying account itself")
    transfer = cast("calldata", "transfer(address,uint256)", accepted["payTo"], accepted["amount"])
    execution = "0x" + accepted["asset"][2:].lower() + "00" * 32 + transfer[2:]
    data = cast(
        "calldata",
        "redeemDelegations(bytes[],bytes32[],bytes[])",
        f"[{payload['permissionContext']}]",
        "[0x" + "00" * 32 + "]",
        f"[{execution}]",
    )
    me = cast("wallet", "address", "--private-key", open(KEY_FILE).read().strip())
    cast("call", manager, data, "--from", me, "--rpc-url", RPC)  # dry run: reverts say why
    # The spec's gas warning: a buyer's contract could burn the facilitator's
    # gas. Estimate, refuse above a ceiling, send with the estimate + 20 %.
    gas = int(cast("estimate", manager, data, "--from", me, "--rpc-url", RPC))
    if gas > 400_000:
        raise RuntimeError(f"redeem would use {gas} gas; this facilitator pays at most 400000")
    out = json.loads(cast("send", manager, data, "--rpc-url", RPC, "--private-key", open(KEY_FILE).read().strip(), "--gas-limit", str(gas * 6 // 5), "--json"))
    if out.get("status") not in ("0x1", 1):
        raise RuntimeError(f"redeem reverted in {out.get('transactionHash')}")
    return out["transactionHash"]


class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *a):
        sys.stderr.write("seller: " + (fmt % a) + "\n")

    def reply(self, code, body, headers=None):
        raw = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.send_header("content-length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        path, _, query = self.path.partition("?")
        if path != "/v1/mid":
            return self.reply(404, {"error": "not found"})
        coin = dict(kv.split("=", 1) for kv in query.split("&") if "=" in kv).get("coin", "ETH")
        url = f"http://{self.headers.get('host')}{self.path}"
        need = requirement(url)
        sig = self.headers.get("payment-signature")
        if not sig:
            required = {"x402Version": 2, "error": "PAYMENT-SIGNATURE header is required",
                        "resource": resource(url), "accepts": [need]}
            if WANT_ID:
                required["extensions"] = {"payment-identifier": PAYMENT_IDENTIFIER}
            return self.reply(402, {}, {"PAYMENT-REQUIRED": b64(required)})
        try:
            p = json.loads(base64.b64decode(sig))
            if WANT_ID:
                pid = ((p.get("extensions") or {}).get("payment-identifier") or {}).get("info", {}).get("id")
                if not isinstance(pid, str) or not ID_OK.match(pid):
                    # The spec: required and missing -> 400.
                    return self.reply(400, {"error": "payment-identifier id is required (16-128 chars of [A-Za-z0-9_-])"})
                sys.stderr.write(f"seller: payment-identifier {pid}\n")
            acc = p["accepted"]
            for k in ("scheme", "network", "amount", "payTo"):
                if str(acc.get(k)).lower() != str(need[k]).lower():
                    raise ValueError(f"accepted.{k} does not match the requirement")
            if acc["asset"].lower() != USDC.lower():
                raise ValueError("accepted.asset does not match")
            method = (acc.get("extra") or {}).get("assetTransferMethod")
            if method != "erc7710":
                raise ValueError(f"this seller settles erc7710 only, got {method!r}")
            tx = settle_erc7710(acc, p["payload"])
            if os.environ.get("ANSWER_FAILED") == "1":
                # Test-only: a dishonest seller that took the money and says it did not.
                raise RuntimeError("settlement failed (said after settling " + tx + ")")
        except Exception as e:  # the spec's failure shape: 402 + PAYMENT-RESPONSE
            return self.reply(402, {}, {"PAYMENT-RESPONSE": b64({
                "success": False, "errorReason": str(e)[:300], "transaction": "", "network": NETWORK,
                "payer": (locals().get("p") or {}).get("payload", {}).get("delegator", "")})})
        price = mid(coin)
        return self.reply(200, {"coin": coin.upper(), "mid": price, "source": "hyperliquid allMids"},
                          {"PAYMENT-RESPONSE": b64({"success": True, "transaction": tx, "network": NETWORK,
                                                    "payer": p["payload"]["delegator"]})})


if __name__ == "__main__":
    print(f"x402 seller on http://127.0.0.1:{PORT}/v1/mid  price {PRICE} minor -> {PAY_TO} on {NETWORK}")
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
