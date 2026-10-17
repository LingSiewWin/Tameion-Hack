#!/usr/bin/env python3
"""The audit pack for one account and one period: a ledger an accountant checks with bean-check, and the evidence behind
every line of it, arranged by the attributes of reliability in proposed ISA 500 (Revised) (docs/117 §5).

    python3 scripts/audit_pack.py <app_dir> <policy> <from YYYY-MM-DD> <to YYYY-MM-DD> \
        [--rpc URL --rpc2 URL] [--gateway URL] [--host URL [--statement FILE ...]] [--out DIR] [--from-block N]
        [--block N]

Writes <out>/<policy>/<from>_<to>/ (default out: audit-pack/ at the repository root, git-ignored: it holds account data):
  ledger.beancount   the opening balance, every funding and every payment of the period (UTC days, both ends included),
                     from the agent's anchored record and the chain, and a zero-tolerance `balance` line at the end of
                     <to> whose number is the chain's: balanceOf now, less the transfers since; and a second one,
                     the Circle Gateway unified balance of the account's bill wallet, as Circle reports it
  root.json          every record's root recomputed from its events, its anchoring transaction read on two nodes, rootOf now
  replay.txt         the recompute event by event, and an inclusion proof for every booked payment
  signatures.json    every payee-signed invoice re-verified (EIP-712); weekly statements through scripts/verify_statement.sh
  countersign.json   payments settled under an invoice the payee signed, over all payments of the period
  sources.json       the two nodes, chain id, block numbers, contracts, tools, and the SHA-256 of every file read
  README.md          the five attributes, which file covers each, and what it does not

A payment the chain shows and the agent's anchored record does not hold is booked from the chain with the `!` flag and
named in the README: the ledger stays complete and the gap stays visible. A payment the record holds and the chain does
not show, or shows with another payee, amount or action, is booked as the record says, so the balance line fails.

No key. JSON-RPC over stdlib urllib on two nodes, and Circle Gateway's public balance read (a node or Gateway URL may
be file://<answers.json> for an offline run, with --block; AUDIT_PACK_RECORD=<file> writes the answers a run received).
Keccak-256 is Python's hashlib when its OpenSSL has KECCAK-256, else `cast keccak`; signatures are checked with
`cast wallet verify`. Exit 0 when every check held, 1 when one failed (the pack is still written and says which), 2 when
a tool or an input is missing.
"""

import argparse
import datetime as dt
import glob
import hashlib
import json
import os
import re
import shutil
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from decimal import Decimal

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Per chain: the USDC contract, the record contract (MemoryAnchor) pinned here as in scripts/verify_statement.sh, never
# taken from the app dir, two public nodes, and how many blocks one eth_getLogs may span on them (operator watch.rs).
CHAINS = {
    "arbitrum-sepolia": dict(id=421_614, label="Arbitrum-Sepolia", usdc="0x75faf114eafb1bdbe2f0316df893fd58ce46aa4d",
                             anchor="0xe7c381aafa077cc99359e8b297d2125efd76b8f5", span=5_000_000,
                             rpcs=["https://sepolia-rollup.arbitrum.io/rpc",
                                   "https://arbitrum-sepolia-rpc.publicnode.com"]),
    "arc-testnet": dict(id=5_042_002, label="Arc-Testnet", usdc="0x3600000000000000000000000000000000000000",
                        anchor="0xdab65a5380bcc2eebc3a3a91d0ad11eec95994db", span=10_000,
                        rpcs=["https://rpc.testnet.arc.network", "https://explorer.testnet.arc.io/api/eth-rpc"]),
}
# Circle Gateway's balance read, no key (operator watch.rs gateway_holdings): the testnet deployment for testnet chains.
GATEWAY_TESTNET = "https://gateway-api-testnet.circle.com"
KINDS = {1: "OBSERVATION", 2: "DECISION", 3: "SETTLEMENT", 4: "ESCALATION", 5: "SKIPPED", 6: "HEDGE", 7: "PROPOSAL",
         8: "FLOWCHECK", 9: "RESERVATION", 10: "METERCHECK", 11: "PRICETABLE"}
CURRENCY = {1: "USDC", 2: "EURC", 3: "USD", 4: "EUR", 5: "gold-gram"}
ADDR = re.compile(r"^0x[0-9a-f]{40}$")
HASH = re.compile(r"^0x[0-9a-f]{64}$")
DEAD = "0x000000000000000000000000000000000000dead"


def cast(*args):
    r = subprocess.run(["cast", *args], capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip() or f"cast {args[0]} failed")
    return r.stdout.strip()


try:
    hashlib.new("KECCAK-256")
    KECCAK = f"Python hashlib KECCAK-256 ({ssl.OPENSSL_VERSION})"

    def keccak(b):
        return hashlib.new("KECCAK-256", b).digest()
except ValueError:
    KECCAK = "cast keccak (foundry)"

    def keccak(b):
        return bytes.fromhex(cast("keccak", "0x" + b.hex())[2:])


def hx(b):
    return "0x" + b.hex()


def sel(signature):
    return hx(keccak(signature.encode())[:4])


def word(addr):
    return addr.lower().removeprefix("0x").rjust(64, "0")


def usdc(minor):
    return f"{'-' if minor < 0 else ''}{abs(minor) // 10**6}.{abs(minor) % 10**6:06d}"


def day(ts):
    return dt.datetime.fromtimestamp(ts, dt.timezone.utc).strftime("%Y-%m-%d")


def short(addr):
    return f"{addr[:6]}…{addr[-4:]}"


def read_jsonl(path):
    if not os.path.exists(path):
        return []
    return [json.loads(x) for x in open(path, encoding="utf-8") if x.strip()]


# ---- the record: canonical bytes read back (mirror of ledger::log::Event::canonical and operator statement.rs) ----

class Reader:
    def __init__(self, b):
        self.b, self.i = b, 0

    def take(self, n):
        if self.i + n > len(self.b):
            raise ValueError("canonical bytes end early")
        self.i += n
        return self.b[self.i - n:self.i]

    def u8(self):
        return self.take(1)[0]

    def u64(self):
        return int.from_bytes(self.take(8), "big")

    def blob(self):
        return self.take(self.u64())

    def text(self):
        return self.blob().decode()

    def money(self):
        minor = int.from_bytes(self.take(16), "big", signed=True)
        return minor, CURRENCY.get(self.u8(), "?")


def decode(canonical):
    """seq, at, kind, and the fields of the three kinds that pay, refuse or ask; other kinds carry their kind only."""
    r = Reader(canonical)
    if (version := r.text()) != "air-log/v0.1":
        raise ValueError(f"event version {version!r} is not 'air-log/v0.1'")
    e = {"seq": r.u64(), "at": r.u64()}
    tag = r.u8()
    e["kind"] = KINDS.get(tag, f"TAG{tag}")
    # Decision: counterparty, amount, verdict, action, confidence, balance seen, weakest evidence, evidence, note.
    if tag == 2:
        e["payee"], e["amount"], e["verdict"] = r.text().lower(), r.money(), r.u8()
        e["action"], e["confidence"] = hx(r.blob()), r.u64() / 10_000
        r.money()
        if r.u8() == 1:
            r.u8()
        e["evidence"] = [r.text() for _ in range(r.u64())]
        e["note"] = r.text()
    elif tag == 3:  # Settlement: counterparty, cycle key, amount, tx, action hash, settlement key, note
        e["payee"], e["cycle"], e["amount"], e["tx"] = r.text().lower(), r.text(), r.money(), r.text().lower()
        e["action"], e["key"], e["note"] = hx(r.blob()), hx(r.blob()), r.text()
    elif tag == 4:  # Escalation: counterparty, amount, reason, note
        e["payee"], e["amount"], e["reason"], e["note"] = r.text().lower(), r.money(), r.text(), r.text()
    return e


def leaf(canonical):
    return keccak(b"\x00" + canonical)


def node(left, right):
    return keccak(b"\x01" + left + right)


def split(n):
    """RFC 6962: the largest power of two strictly below n (n > 1)."""
    return 1 << ((n - 1).bit_length() - 1)


def mth(leaves):
    if not leaves:
        return bytes(32)
    if len(leaves) == 1:
        return leaves[0]
    k = split(len(leaves))
    return node(mth(leaves[:k]), mth(leaves[k:]))


def path(leaves, i):
    """Inclusion path of leaf i, leaf to root: (sibling, the sibling is on the left)."""
    if len(leaves) <= 1:
        return []
    k = split(len(leaves))
    if i < k:
        return path(leaves[:k], i) + [(mth(leaves[k:]), False)]
    return path(leaves[k:], i - k) + [(mth(leaves[:k]), True)]


def walk(start, steps):
    acc = start
    for sibling, left in steps:
        acc = node(sibling, acc) if left else node(acc, sibling)
    return acc


def load_record(log_dir):
    """Every `<root>.events.jsonl` of the account: events decoded from their canonical bytes (the `kind` and `note`
    columns are not hashed and are not read), their leaves, the recomputed root, and whether the file name states it."""
    records = {}
    for f in sorted(glob.glob(os.path.join(log_dir, "0x*.events.jsonl"))):
        named = os.path.basename(f).removesuffix(".events.jsonl").lower()
        events = []
        for n, line in enumerate(open(f, encoding="utf-8"), 1):
            if not line.strip():
                continue
            v = json.loads(line)
            canonical = bytes.fromhex(v["canonical"].removeprefix("0x"))
            e = decode(canonical)
            if v.get("seq") != len(events) or e["seq"] != len(events):
                raise ValueError(f"{f}: line {n}: seq {v.get('seq')} out of order")
            e["leaf"], e["root"] = leaf(canonical), named
            events.append(e)
        root = hx(mth([e["leaf"] for e in events]))
        records[named] = {"file": f, "events": events, "root": root, "nameOk": root == named}
    return records


# ---- the chain ----

ANSWERS, RECORDED = {}, {}


def rpc(url, method, params):
    key = method + " " + json.dumps(params, sort_keys=True)
    if url.startswith("file://"):
        if url not in ANSWERS:
            ANSWERS[url] = json.load(open(url[len("file://"):]))
        if key not in ANSWERS[url]:
            raise RuntimeError(f"{url} holds no answer for {key}")
        return ANSWERS[url][key]
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    for attempt in range(4):
        try:
            req = urllib.request.Request(url, body, {"Content-Type": "application/json",
                                                     "User-Agent": "tameion-audit-pack/1"})
            with urllib.request.urlopen(req, timeout=30) as r:
                reply = json.load(r)
            break
        except (urllib.error.URLError, TimeoutError) as e:  # public nodes throttle: back off and ask again
            if attempt == 3:
                raise RuntimeError(f"{url}: {method}: {e}") from e
            time.sleep(1 + attempt)
    if "error" in reply:
        raise RuntimeError(f"{url}: {method}: {reply['error']}")
    RECORDED[key] = reply["result"]
    return reply["result"]


def call(url, to, data, block):
    return rpc(url, "eth_call", [{"to": to, "data": data}, hex(block)])


def gateway(url, wallet):
    """The USDC `wallet` holds in Circle Gateway on every domain: POST {url}/v1/balances, the body watch.rs sends.
    Returns (total minor units, [{domain, balance}])."""
    key = f"gateway {wallet}"
    if url.startswith("file://"):
        if url not in ANSWERS:
            ANSWERS[url] = json.load(open(url[len("file://"):]))
        reply = ANSWERS[url][key]
    else:
        body = json.dumps({"token": "USDC", "sources": [{"depositor": wallet}]}).encode()
        req = urllib.request.Request(f"{url.rstrip('/')}/v1/balances", body, {
            "Content-Type": "application/json", "User-Agent": "tameion-audit-pack/1"})
        with urllib.request.urlopen(req, timeout=30) as r:
            reply = json.load(r)
        RECORDED[key] = reply
    rows, total = [], 0
    for b in reply["balances"]:
        d = Decimal(str(b["balance"]))
        if d != d.quantize(Decimal("0.000001")) or d < 0:
            raise RuntimeError(f"Gateway reported a balance USDC cannot hold: {b['balance']!r}")
        if d:
            rows.append({"domain": b.get("domain"), "balance": str(d)})
            total += int(d * 10**6)
    return total, rows


def both(urls, fn, what):
    """The same read on both nodes; a disagreement is an error, never averaged away."""
    answers = [fn(u) for u in urls]
    if answers[0] != answers[1]:
        raise RuntimeError(f"the two nodes disagree on {what}: {answers[0]} vs {answers[1]}")
    return answers[0]


def classify(receipt, policy, move):
    """Who moved the money, by the policy's own event in the same transaction: Paid (the agent, inside the caps),
    SettledByPrincipal (the owner's passkey), Withdrawn (the owner), or none. Returns (who, action hash)."""
    names = {hx(keccak(s.encode())): n for s, n in (
        ("Paid(address,uint256,bytes32)", "agent"), ("SettledByPrincipal(address,uint256,bytes32)", "owner"),
        ("Withdrawn(address,uint256)", "withdrawal"))}
    for lg in receipt["logs"]:
        t = [x.lower() for x in lg["topics"]]
        if (lg["address"].lower() == policy and len(t) > 1 and t[0] in names and "0x" + t[1][-40:] == move["to"]
                and int(lg["data"][:66], 16) == move["minor"]):
            return names[t[0]], (t[2] if len(t) > 2 else None)
    return "unknown", None


def read_chain(urls, c, policy, start_block, at=None):
    """Every USDC transfer into and out of the account since `start_block` (the same set from both nodes), its block
    time, the policy's own event in each outgoing one, and balanceOf, agent() and rootOf at one block read on both."""
    for u in urls:
        if int(rpc(u, "eth_chainId", []), 16) != c["id"]:
            raise RuntimeError(f"{u} is not chain {c['id']}")
    heads = [int(rpc(u, "eth_blockNumber", []), 16) for u in urls]
    at = at or min(heads)
    transfer, me = hx(keccak(b"Transfer(address,address,uint256)")), "0x" + word(policy)

    def span(u, topics, a, b):
        # Each node caps the range one eth_getLogs may cover (publicnode: 50,000 blocks): a refused range is split.
        try:
            return rpc(u, "eth_getLogs", [{"address": c["usdc"], "topics": topics, "fromBlock": hex(a),
                                           "toBlock": hex(b)}])
        except RuntimeError:
            if b - a < 1_000:
                raise
            return span(u, topics, a, (a + b) // 2) + span(u, topics, (a + b) // 2 + 1, b)

    def logs(u, topics):
        found = []
        for a in range(start_block, at + 1, c["span"]):
            found += span(u, topics, a, min(a + c["span"] - 1, at))
        return sorted((x["transactionHash"].lower(), int(x["logIndex"], 16), int(x["blockNumber"], 16),
                       x["blockHash"].lower(), x["data"].lower(), tuple(t.lower() for t in x["topics"])) for x in found)

    moves, times = [], {}
    for direction, topics in (("out", [transfer, me]), ("in", [transfer, None, me])):
        for tx, index, block, _, data, tps in both(urls, lambda u, t=topics: logs(u, t), f"USDC transfers {direction}"):
            frm, to = "0x" + tps[1][-40:], "0x" + tps[2][-40:]
            if frm == to:
                continue
            if block not in times:
                times[block] = int(rpc(urls[0], "eth_getBlockByNumber", [hex(block), False])["timestamp"], 16)
            m = {"dir": direction, "tx": tx, "logIndex": index, "block": block, "ts": times[block], "from": frm,
                 "to": to, "minor": int(data, 16)}
            if direction == "out":
                m["by"], m["action"] = classify(rpc(urls[0], "eth_getTransactionReceipt", [tx]), policy, m)
            moves.append(m)
    moves.sort(key=lambda m: (m["block"], m["logIndex"]))
    balance = both(urls, lambda u: int(call(u, c["usdc"], sel("balanceOf(address)") + word(policy), at), 16),
                   "the account's USDC balance")
    writer, who = both(urls, lambda u: "0x" + call(u, policy, sel("agent()"), at)[-40:].lower(), "agent()"), "agent"
    if writer == DEAD:  # an account with no agent key anchors with its operator key
        writer, who = both(urls, lambda u: "0x" + call(u, policy, sel("operator()"), at)[-40:].lower(),
                           "operator()"), "operator"
    root_of = both(urls, lambda u: call(u, c["anchor"], sel("rootOf(address)") + word(writer), at).lower(), "rootOf")
    return {"heads": heads, "at": at, "moves": moves, "balance": balance, "writer": writer, "writerIs": who,
            "rootOf": root_of}


def anchor_check(urls, c, writer, root, tx):
    """The anchoring transaction on each node: succeeded, sent by the account's key, to the pinned record contract,
    carrying exactly anchor(root)."""
    want = sel("anchor(bytes32)") + root[2:]
    out = []
    for u in urls:
        t = rpc(u, "eth_getTransactionByHash", [tx]) or {}
        rc = rpc(u, "eth_getTransactionReceipt", [tx]) or {}
        ok = (rc.get("status") == "0x1" and str(t.get("to", "")).lower() == c["anchor"]
              and str(t.get("from", "")).lower() == writer and str(t.get("input", "")).lower() == want)
        out.append({"node": u, "block": int(rc["blockNumber"], 16) if rc.get("blockNumber") else None,
                    "verdict": "MATCH" if ok else "MISMATCH"})
    return out


# ---- signatures ----

def invoice_id(payee, ref):
    """The invoice's id as operator invoice.rs computes it: keccak256 of ("tameion.invoice", payee, ref) ABI-encoded as
    ONE tuple, alloy's `SolValue::abi_encode` on a tuple with a dynamic member, so a 0x20 offset word comes first. Its
    doc comment's Solidity form, abi.encode("tameion.invoice", payee, ref), has no such word and gives another hash."""
    s = b"tameion.invoice"
    enc = (32).to_bytes(32, "big") + (96).to_bytes(32, "big") + bytes.fromhex(word(payee)) + bytes.fromhex(ref[2:])
    return hx(keccak(enc + len(s).to_bytes(32, "big") + s.ljust(32, b"\0")))


def invoice_typed(inv, chain_id):
    """The EIP-712 typed data the payee signed (operator invoice.rs: version 2 when the body names `from` or
    `domain`), rebuilt here from the stored fields so the file cannot choose what is verified."""
    v2 = "from" in inv or "domain" in inv
    fields = [("payee", "address"), ("policy", "address"), ("amount", "uint256"), ("dueAt", "uint64"),
              ("everyDays", "uint32"), ("discountBps", "uint16"), ("earlyDays", "uint16"), ("ref", "bytes32"),
              ("issuedAt", "uint64")] + ([("from", "string"), ("domain", "string")] if v2 else [])
    return {"types": {"EIP712Domain": [{"name": "name", "type": "string"}, {"name": "version", "type": "string"},
                                       {"name": "chainId", "type": "uint256"},
                                       {"name": "verifyingContract", "type": "address"}],
                      "Invoice": [{"name": n, "type": t} for n, t in fields]},
            "primaryType": "Invoice",
            "domain": {"name": "Tameion Invoice", "version": "2" if v2 else "1", "chainId": chain_id,
                       "verifyingContract": inv["policy"]},
            "message": {n: int(inv[n]) if t.startswith("uint") else inv.get(n, "") for n, t in fields}}


def verify_typed(typed, signer, sig):
    if not (ADDR.match(signer) and re.match(r"^0x[0-9a-fA-F]{130}$", sig)):
        return False
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(typed, f)
    try:
        return subprocess.run(["cast", "wallet", "verify", "--address", signer, "--data", "--from-file", f.name, sig],
                              capture_output=True, text=True, timeout=60).returncode == 0
    finally:
        os.remove(f.name)


def iso_weeks(first, last):
    out, d = [], first
    while d <= last:
        y, w, _ = d.isocalendar()
        if f"{y}-W{w:02d}" not in out:
            out.append(f"{y}-W{w:02d}")
        d += dt.timedelta(days=1)
    return out


def statements(args, pack, policy, urls):
    """Weekly statements, fetched from --host for each ISO week of the period or given with --statement, each checked
    by scripts/verify_statement.sh: the lines hash, the signature against the signer the host advertises, the root on
    chain, on the same two nodes."""
    if not args.host:
        return [], "no --host given: weekly statements not checked"
    os.makedirs(os.path.join(pack, "statements"), exist_ok=True)
    files, notes = [], []
    for i, src in enumerate(args.statement or []):
        files.append(os.path.join(pack, "statements", f"given-{i + 1}.json"))
        shutil.copyfile(src, files[-1])
    for label in [] if args.statement else iso_weeks(args.first, args.last):
        url = f"{args.host.rstrip('/')}/api/level1/statement?policy={policy}&week={label}"
        try:
            with urllib.request.urlopen(url, timeout=60) as r:
                body = r.read()
        except urllib.error.HTTPError as e:
            notes.append({"week": label, "verdict": "not served",
                          "note": f"HTTP {e.code}: the service answers this account only to an agent key with read; "
                                  "fetch the week with yours and pass --statement"})
            continue
        files.append(os.path.join(pack, "statements", f"{label}.json"))
        open(files[-1], "wb").write(body)
    env = dict(os.environ, RPC_URL=urls[0], RPC_URL_2=urls[1])
    for f in files:
        r = subprocess.run(["bash", os.path.join(ROOT, "scripts", "verify_statement.sh"), f, args.host],
                           capture_output=True, text=True, timeout=300, env=env)
        st = json.load(open(f))
        paid = [x for x in st.get("lines", []) if x.get("kind") == "paid" and x.get("tx")]
        notes.append({"week": st.get("week", {}).get("label"), "file": os.path.relpath(f, pack),
                      "verdict": "PASS" if r.returncode == 0 else "FAIL", "output": r.stdout.strip().splitlines(),
                      "paid": {str(x["tx"]).lower(): x.get("at") for x in paid}})
    return notes, None


# ---- the pack ----

def bean_str(s):
    """A beancount string: one line, backslashes and quotes escaped (runs of spaces fold to one)."""
    return '"' + " ".join(str(s).split()).replace("\\", "\\\\").replace('"', '\\"') + '"'


def account_part(name):
    s = re.sub(r"[^A-Za-z0-9]+", "-", name).strip("-")
    return s[:1].upper() + s[1:]


def origin(cycle):
    if cycle.startswith("x402:"):
        return "x402 purchase by the agent"
    if cycle.startswith("pay:"):
        return "Direct payment by the agent"
    return "Bill paid by the agent"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("app_dir")
    ap.add_argument("policy")
    ap.add_argument("start", metavar="from")
    ap.add_argument("end", metavar="to")
    ap.add_argument("--rpc")
    ap.add_argument("--rpc2")
    ap.add_argument("--gateway", default=GATEWAY_TESTNET, help="Circle Gateway API (default: the testnet one)")
    ap.add_argument("--host", help="the service: its weekly statements and the statement signer it advertises")
    ap.add_argument("--statement", action="append", help="a statement fetched by hand (needs --host for its signer)")
    ap.add_argument("--out", default=os.path.join(ROOT, "audit-pack"))
    ap.add_argument("--from-block", type=int, help="first block to read transfers from (default: the deployment's)")
    ap.add_argument("--block", type=int, help="read the chain at this block (an archive node, or an earlier run's "
                                              "answers file); default: the lower of the two nodes' heads")
    args = ap.parse_args(argv)
    if shutil.which("cast") is None:
        print("cast (foundry) is required: curl -L https://foundry.paradigm.xyz | bash && foundryup")
        return 2
    if keccak(b"").hex() != "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470":
        print(f"{KECCAK} does not compute Keccak-256")
        return 2
    policy = args.policy.lower()
    try:
        args.first, args.last = dt.date.fromisoformat(args.start), dt.date.fromisoformat(args.end)
    except ValueError:
        print("from and to are dates, YYYY-MM-DD")
        return 2
    tenants = os.path.join(args.app_dir, "tenants")
    tenant_file = os.path.join(tenants, f"{policy}.json")
    if not ADDR.match(policy) or args.last < args.first or not os.path.exists(tenant_file):
        print(f"need a policy address with {tenant_file}, and from <= to")
        return 2
    if args.statement and not args.host:
        print("--statement needs --host: a statement is checked against the signer the service advertises")
        return 2
    tenant = json.load(open(tenant_file))
    c = CHAINS.get(tenant.get("chain"))
    if c is None:
        print(f"no chain preset for {tenant.get('chain')!r}")
        return 2
    urls = [args.rpc or c["rpcs"][0], args.rpc2 or c["rpcs"][1]]
    t0 = int(dt.datetime.combine(args.first, dt.time(), dt.timezone.utc).timestamp())
    t1 = int(dt.datetime.combine(args.last + dt.timedelta(days=1), dt.time(), dt.timezone.utc).timestamp())
    pack = os.path.join(args.out, policy, f"{args.start}_{args.end}")
    os.makedirs(pack, exist_ok=True)
    fails = []

    # 1. The agent's record, from its canonical bytes.
    log_dir = os.path.join(tenants, policy, "log")
    records = load_record(log_dir)
    anchors = read_jsonl(os.path.join(log_dir, "anchors.jsonl"))
    events = [e for r in records.values() for e in r["events"]]

    # 2. The chain, on two nodes.
    # Transfers are read from the earlier of the reserve block the service wrote down and the deployment's block: the
    # service funds the reserved address before it deploys there. Too late a start fails the balanceOf check below.
    start = args.from_block
    if start is None:
        firsts = [int(tenant["created_block"])] if tenant.get("created_block") else []
        if HASH.match(str(tenant.get("policy_tx", "")).lower()):
            firsts.append(int(rpc(urls[0], "eth_getTransactionReceipt", [tenant["policy_tx"]])["blockNumber"], 16))
        start = min(firsts, default=0)
    ch = read_chain(urls, c, policy, start, args.block)
    moves = ch["moves"]

    def flow(ms):
        return sum(m["minor"] if m["dir"] == "in" else -m["minor"] for m in ms)

    net, later = flow(moves), flow(m for m in moves if m["ts"] >= t1)
    opening, closing = flow(m for m in moves if m["ts"] < t0), ch["balance"] - later
    if net != ch["balance"]:
        fails.append(f"the transfers since block {start} add up to {usdc(net)} USDC and balanceOf says "
                     f"{usdc(ch['balance'])}: USDC reached the account before that block (--from-block), or a node "
                     "left a transfer out")

    # 2b. The second witness: the Circle Gateway unified balance of the bill wallet, the account's first wallet as the
    # service picks it (operator level1::history_sources: its wallets, then the watched address, never 0x0).
    wallets = [str(w).lower() for w in list(tenant.get("wallets") or []) + [tenant.get("watched") or ""]]
    bill = next((w for w in wallets if ADDR.match(w) and int(w, 16)), None)
    gw = {"present": False, "note": "this account names no bill wallet, so there is no Gateway balance to read"}
    if bill:
        read_at = int(time.time())
        gw_total, gw_rows = gateway(args.gateway, bill)
        gw = {"present": True, "alg": "balance-read", "keyId": f"gateway:{bill}", "sig": None, "at": read_at,
              "balance": usdc(gw_total), "domains": gw_rows, "source": f"{args.gateway.rstrip('/')}/v1/balances"}

    # 3. Each settlement the record holds, matched to the transfer and the policy's Paid event in its transaction; the
    # event carries the settlement key (the contract's `executed` key), which for a bill is not the action hash.
    outs = {}
    for m in moves:
        if m["dir"] == "out":
            outs.setdefault(m["tx"], []).append(m)
    booked = []
    for s in (e for e in events if e["kind"] == "SETTLEMENT"):
        free = [m for m in outs.get(s["tx"], []) if "match" not in m]
        hit = next((m for m in free if m["to"] == s["payee"] and m["minor"] == s["amount"][0]
                    and m["by"] == "agent" and m["action"] == s["key"]), None)
        if hit:
            hit["match"], s["move"] = s, hit
        if not t0 <= (hit["ts"] if hit else s["at"]) < t1:
            continue
        if not hit:
            s["problem"] = ("it names no transaction" if not s["tx"] else
                            "the chain shows no USDC leaving the account in that transaction" if s["tx"] not in outs
                            else "that transfer is already booked for another line of the record" if not free
                            else "the chain shows another payee, amount or payer in that transaction")
            fails.append(f"the record says paid {usdc(s['amount'][0])} {s['amount'][1]} to {s['payee']} "
                         f"(evt:{s['seq']} under {s['root']}), but {s['problem']}")
        booked.append(((hit["ts"], hit["block"], hit["logIndex"]) if hit else (s["at"], 0, 0), "record", s))
    for m in moves:
        if t0 <= m["ts"] < t1 and "match" not in m:
            booked.append(((m["ts"], m["block"], m["logIndex"]), "chain", m))
    booked.sort(key=lambda b: b[0])

    # 4. The records the period needs, those holding a booked payment or anchored in the period: each recomputed (in
    # load_record) and its anchoring transaction read on both nodes.
    for root, rec in records.items():
        rec["anchor"] = next((a for a in anchors if str(a.get("root", "")).lower() == root and a.get("anchorTx")), None)
    scope = {x["root"] for _, src, x in booked if src == "record"} | {
        r for r, rec in records.items() if rec["anchor"] and t0 <= int(rec["anchor"].get("at") or 0) < t1}
    status = {}
    for root in scope:
        rec = records[root]
        if not rec["nameOk"]:
            status[root] = "MISMATCH"
        elif rec["anchor"] is None:
            status[root] = "NOT ANCHORED"
        else:
            rec["onChain"] = anchor_check(urls, c, ch["writer"], root, str(rec["anchor"]["anchorTx"]).lower())
            status[root] = "MATCH" if all(x["verdict"] == "MATCH" for x in rec["onChain"]) else "MISMATCH"
    for _, src, x in booked:
        if src == "record" and "move" in x and status[x["root"]] != "MATCH":
            x["problem"] = ("its record is not anchored on chain" if status[x["root"]] == "NOT ANCHORED" else
                            "its record does not match the root anchored on chain")

    # 5. Names: the owner's contacts, then the name a payee signed into its invoice (its claim), else the address.
    invoices = read_jsonl(os.path.join(tenants, f"{policy}.invoices.jsonl"))
    names = {}
    for inv in invoices:
        if inv.get("from"):
            names.setdefault(str(inv["payee"]).lower(), (inv["from"], "the payee's signed invoice (its own claim)"))
    for ct in tenant.get("contacts") or []:
        if ct.get("name") and ct.get("address"):
            names[ct["address"].lower()] = (ct["name"], "the owner's contact list")
    accounts = {}

    def expense(addr):
        if addr not in accounts:
            label = (account_part(names[addr][0]) if addr in names else "") or f"{addr[:8]}-{addr[-6:]}"
            if f"Expenses:Payees:{label}" in accounts.values():
                label = f"{label}-{addr[2:8]}"
            accounts[addr] = f"Expenses:Payees:{label}"
        return accounts[addr]

    def display(addr):
        return names[addr][0] if addr in names else short(addr)

    # 6. Invoices: the payee's signature, the id recomputed, the agent's decision, and the record line holding it.
    decisions = {d.get("id"): d.get("decision") or {} for d in
                 read_jsonl(os.path.join(tenants, f"{policy}.invoice-decisions.jsonl"))}
    valid, sig_invoices = {}, []
    for inv in invoices:
        payee, inv_id = str(inv["payee"]).lower(), invoice_id(str(inv["payee"]).lower(), str(inv["ref"]).lower())
        ok = verify_typed(invoice_typed(inv, c["id"]), payee, str(inv.get("signature", "")))
        if ok:
            valid[inv_id] = inv
        else:
            fails.append(f"invoice {inv_id}: the signature does not recover to the payee {payee}")
        rec = next((e for e in events if inv_id in e.get("note", "")), None)
        d = decisions.get(inv_id)
        sig_invoices.append({"id": inv_id, "storedIdMatches": str(inv.get("id", "")).lower() == inv_id,
                             "payee": payee, "amount": usdc(int(inv["amount"])), "issuedAt": inv.get("issuedAt"),
                             "dueAt": inv.get("dueAt"), "version": 2 if ("from" in inv or "domain" in inv) else 1,
                             "signature": "valid" if ok else "INVALID", "checkedWith": "cast wallet verify, EIP-712",
                             "decision": {k: d.get(k) for k in ("action", "basis", "by", "model", "why")} if d else None,
                             "inRecord": f"evt:{rec['seq']} under {rec['root']}" if rec else None})

    # 7. The ledger.
    local = {str(p.get("tx", "")).lower(): p for p in read_jsonl(os.path.join(tenants, f"{policy}.payments.jsonl"))
             + read_jsonl(os.path.join(tenants, f"{policy}.purchases.jsonl")) if p.get("tx")}
    asks = sorted((e for e in events if e["kind"] == "ESCALATION"), key=lambda e: e["at"], reverse=True)
    asset = f"Assets:USDC:{c['label']}:{policy[:8]}"
    entries, payments = [], []  # entries: (date, flag, payee, narration, meta, debit account, minor, credit account)
    for (ts, _, _), source, x in booked:
        if source == "record":
            payee, minor, action, by, tx = x["payee"], x["amount"][0], x["key"], "agent", x["tx"]
            meta = {"event": f"evt:{x['seq']}", "root": x["root"], "receipt": hx(x["leaf"]), "action": x["action"],
                    "settlementKey": action, "tx": tx, "why": x["note"],
                    "source": "the agent's anchored record, matched on chain"}
            if "problem" in x:
                meta["source"] = f"the agent's record; {x['problem']}"
            entries.append((day(ts), "!" if "problem" in x else "*", display(payee), origin(x["cycle"]), meta,
                            expense(payee), minor, asset))
        elif x["dir"] == "in":
            entries.append((day(ts), "*", short(x["from"]), "Funding", {"tx": x["tx"], "from": x["from"],
                            "source": "chain: the USDC transfer into the account"}, asset, x["minor"], "Income:Funding"))
            continue
        else:
            payee, minor, action, by, tx = x["to"], x["minor"], x["action"], x["by"], x["tx"]
            event = {"agent": "Paid", "owner": "SettledByPrincipal", "withdrawal": "Withdrawn"}.get(by)
            meta = {"tx": tx, "settlementKey": action, "source": f"chain: the USDC transfer and the policy's {event} event"
                    if event else "chain: the USDC transfer; the policy emitted no payment event"}
            if by == "withdrawal":
                entries.append((day(ts), "*", short(payee), "Withdrawn by the owner", meta, "Equity:Withdrawals",
                                minor, asset))
                continue
            if by == "owner":
                ask = next((e for e in asks if e["payee"] == payee and e["amount"][0] == minor and e["at"] <= ts), None)
                if ask:
                    meta["asked"], meta["askedIn"] = ask["reason"], f"evt:{ask['seq']} under {ask['root']}"
                entries.append((day(ts), "*", display(payee), "Paid by the owner with the passkey", meta,
                                expense(payee), minor, asset))
            else:
                if tx in local:
                    meta["why"] = local[tx].get("why") or local[tx].get("reason") or ""
                    meta["source"] += "; in this app dir's payments, not yet under an anchored root"
                else:
                    meta["source"] += "; in no file of this app dir"
                entries.append((day(ts), "!", display(payee), "Paid by the agent key, not in this account's anchored "
                                "record" if by == "agent" else "USDC left the account with no policy event", meta,
                                expense(payee), minor, asset))
        inv = valid.get(action)
        if inv and str(inv["payee"]).lower() == payee and int(inv["amount"]) == minor:
            entries[-1][4]["invoice"] = action
        signed_by_payee = entries[-1][4].get("invoice")
        slots = {"agent": None,
                 "counterparty": {"alg": "secp256k1-eip712", "keyId": payee, "sig": valid[action]["signature"],
                                  "at": valid[action].get("issuedAt")} if signed_by_payee else None,
                 "witness": {"alg": "chain-receipt", "keyId": f"chain:{c['id']}", "sig": None, "ref": tx, "at": ts}
                 if source == "chain" or "move" in x else None}
        filled = sum(1 for v in slots.values() if v)
        payments.append({"date": day(ts), "payee": payee, "name": display(payee), "amount": usdc(minor),
                         "minor": minor, "by": by, "inRecord": source == "record", "tx": tx,
                         "invoice": signed_by_payee, "slots": slots,
                         "signedness": {0: "unsigned", 1: "Single", 2: "Double", 3: "Triple"}[filled]})
    title = tenant.get("name") or policy
    out = [f"; Audit pack ledger: {title} ({policy}) on {c['label']}, {args.start} to {args.end} (UTC days, both "
           "included).", f"; Chain reads at block {ch['at']} on two nodes (sources.json). Hashes are metadata; postings "
           "carry names and amounts.", "", f"option \"title\" {bean_str(title + ': agent account')}",
           'option "operating_currency" "USDC"', "", f"{args.start} commodity USDC", f"{args.start} open {asset} USDC",
           f"  policy: {bean_str(policy)}"]
    opened = {asset}
    for acct in ["Equity:Opening-Balances", "Income:Funding"] + sorted({e[5] for e in entries} | {e[7] for e in entries}):
        if acct not in opened:
            opened.add(acct)
            out.append(f"{args.start} open {acct} USDC")
            addr = next((a for a, n in accounts.items() if n == acct), None)
            if addr:
                out += [f"  address: {bean_str(addr)}",
                        f"  nameFrom: {bean_str(names[addr][1] if addr in names else 'the address')}"]
    out.append("")
    if opening:
        out += [f"{args.start} * \"Opening balance\" \"USDC held when the period began, from the chain's transfers since "
                "the account was deployed\"", '  source: "chain"', f"  {asset}  {usdc(opening)} USDC",
                "  Equity:Opening-Balances", ""]
    for d, flag, payee, narration, meta, debit, minor, credit in entries:
        out.append(f"{d} {flag} {bean_str(payee)} {bean_str(narration)}")
        out += [f"  {k}: {bean_str(v)}" for k, v in meta.items() if v not in (None, "")]
        out += [f"  {debit}  {usdc(minor)} USDC", f"  {credit}", ""]
    close_note = f"chain: balanceOf at block {ch['at']}, less the transfers after the period"
    out += [f"{(args.last + dt.timedelta(days=1)).isoformat()} balance {asset}  {usdc(closing)} ~ 0 USDC",
            f"  source: {bean_str(close_note)}", ""]
    if gw["present"]:
        # A read, not a reconciliation: the pack holds no Gateway history, so this line is Circle's number booked as
        # read and asserted the day after; it puts the operator's USDC at Circle beside the account's.
        read_day = dt.date.fromisoformat(day(gw["at"]))
        gw_asset = f"Assets:USDC:Gateway:{bill[:8]}"
        out += [f"{min(read_day, args.first).isoformat()} open {gw_asset} USDC", f"  wallet: {bean_str(bill)}"]
        if gw_total:
            out += [f"{read_day.isoformat()} * \"Circle Gateway\" \"Unified balance of the bill wallet, as Circle "
                    "reported it\"", f"  witness: {bean_str(gw['alg'] + ' ' + gw['keyId'])}",
                    f"  source: {bean_str(gw['source'] + ' at ' + day(gw['at']))}",
                    f"  {gw_asset}  {usdc(gw_total)} USDC", "  Equity:Opening-Balances", ""]
        out += [f"{(read_day + dt.timedelta(days=1)).isoformat()} balance {gw_asset}  {usdc(gw_total)} ~ 0 USDC",
                f"  source: {bean_str('Circle Gateway, read; not reconciled to any history')}"]
    ledger = os.path.join(pack, "ledger.beancount")
    open(ledger, "w", encoding="utf-8").write("\n".join(out) + "\n")

    # 8. root.json and replay.txt: the records in scope, and rootOf now.
    latest = next((str(a["root"]).lower() for a in reversed(anchors) if a.get("anchorTx")), None)
    now = {"rootOf": ch["rootOf"], "key": ch["writer"], "keyIs": ch["writerIs"], "block": ch["at"],
           "latestAnchoredHere": latest, "verdict": "MATCH" if ch["rootOf"] == latest else
           "DIFFERS: the latest root on chain under this key was not anchored from this app dir"}
    replay = ["# replay: every record's root recomputed from the canonical bytes of its events",
              f"# keccak: {KECCAK}; leaf = keccak(0x00 || canonical), node = keccak(0x01 || left || right), split at "
              "the largest power of two below n (RFC 6962 with Keccak-256)",
              f"# record contract {c['anchor']} (pinned here), {ch['writerIs']} key {ch['writer']} (read on chain at "
              f"block {ch['at']})", ""]
    roots = []
    booked_ids = {id(x) for _, src, x in booked if src == "record"}
    for root in sorted(scope, key=lambda r: (int((records[r]["anchor"] or {}).get("at") or 0), r)):
        rec = records[root]
        entry = {"root": root, "file": os.path.basename(rec["file"]), "events": len(rec["events"]),
                 "recomputed": rec["root"], "fileNameMatches": rec["nameOk"],
                 "anchoredAt": (rec["anchor"] or {}).get("at"), "anchorTx": (rec["anchor"] or {}).get("anchorTx"),
                 "anchorTxOnChain": rec.get("onChain", []), "verdict": status[root]}
        if status[root] == "MISMATCH":
            fails.append(f"record {root}: " + ("its events recompute to another root" if not rec["nameOk"] else
                                               "no transaction from the account's key anchored it"))
        roots.append(entry)
        replay.append(f"## record {root} ({len(rec['events'])} events)")
        replay += [f"[{e['seq']:04}] {e['kind']:<11} {' '.join(e.get('note', '').split())}".rstrip()
                   for e in rec["events"]]
        replay.append(f"recomputed root : {rec['root']}   file name {'MATCH' if rec['nameOk'] else 'MISMATCH'}")
        replay += [f"anchor tx       : {entry['anchorTx']} on {x['node']} (block {x['block']})   {x['verdict']}"
                   for x in entry["anchorTxOnChain"]] or ["anchor tx       : none in anchors.jsonl   NOT ANCHORED"]
        leaves = [e["leaf"] for e in rec["events"]]
        for e in rec["events"]:
            if id(e) in booked_ids:
                steps = path(leaves, e["seq"])
                ok = hx(walk(e["leaf"], steps)) == rec["root"]
                replay.append(f"inclusion [{e['seq']:04}] leaf {hx(e['leaf'])} path {len(steps)} steps   "
                              f"{'VERIFIED' if ok else 'FAILED'}")
                replay += [f"  {i}: {'left ' if left else 'right'} {hx(sib)}" for i, (sib, left) in enumerate(steps)]
        replay.append("")
    replay.append(f"rootOf({ch['writer']}) at block {ch['at']}, both nodes: {ch['rootOf']}   {now['verdict']}")
    open(os.path.join(pack, "replay.txt"), "w", encoding="utf-8").write("\n".join(replay) + "\n")
    json.dump({"policy": policy, "chain": tenant["chain"], "recordContract": c["anchor"], "records": roots, "now": now,
               "writtenNeverAnchored": sorted(r for r, rec in records.items() if rec["anchor"] is None)},
              open(os.path.join(pack, "root.json"), "w"), indent=1)

    # 9. Statements, countersignatures, sources.
    sts, st_note = statements(args, pack, policy, urls)
    in_ledger = {p["tx"] for p in payments}
    for st in sts:
        if "paid" not in st:
            continue
        st["inStatementNotInLedger"] = sorted(tx for tx, at in st.pop("paid").items()
                                              if t0 <= int(at or 0) < t1 and tx not in in_ledger)
        if st["verdict"] != "PASS" or st["inStatementNotInLedger"]:
            fails.append(f"statement {st['week']}: {st['verdict']}, {len(st['inStatementNotInLedger'])} paid lines the "
                         "ledger does not hold")
    json.dump({"invoices": sig_invoices, "statements": sts, "statementsNote": st_note,
               "records": [{"root": r["root"], "anchoredBy": ch["writer"], "verdict": r["verdict"]} for r in roots]},
              open(os.path.join(pack, "signatures.json"), "w"), indent=1)
    signed = [p for p in payments if p["invoice"]]
    json.dump({"rule": "a payment counts when it was settled under the id of an invoice the payee signed (EIP-712, "
                       "re-verified here) for the same payee and amount; a withdrawal is not a payment; a recurring "
                       "invoice's later cycles settle under the bill's action, not the invoice's, and do not count",
               "slots": "per payment, the shape of ledger::symbolon (docs/117 addendum): counterparty = the payee's "
                        "signed invoice, witness = the transfer read back from the chain; the agent slot stays empty "
                        "until records carry the agent's own signature",
               "paymentsInPeriod": len(payments), "withPayeeSignedInvoice": len(signed),
               "ratio": f"{len(signed)}/{len(payments)}",
               "share": round(len(signed) / len(payments), 4) if payments else None, "payments": payments},
              open(os.path.join(pack, "countersign.json"), "w"), indent=1)
    read = [tenant_file] + [r["file"] for r in records.values()] + [
        p for p in [os.path.join(log_dir, "anchors.jsonl")] + [os.path.join(tenants, f"{policy}.{k}") for k in (
            "invoices.jsonl", "invoice-decisions.jsonl", "payments.jsonl", "purchases.jsonl")] if os.path.exists(p)]
    tools = {"python": sys.version.split()[0], "keccak": KECCAK, "cast": cast("--version").splitlines()[0]}
    bean, checked = shutil.which("bean-check"), None
    if bean:
        r = subprocess.run([bean, ledger], capture_output=True, text=True, timeout=300)
        ver = subprocess.run([bean, "--version"], capture_output=True, text=True).stdout.strip().splitlines()
        tools["bean-check"] = ver[0] if ver else "bean-check"
        checked = {"exit": r.returncode, "output": (r.stdout + r.stderr).strip()}
        if r.returncode != 0:
            fails.append("bean-check: " + checked["output"].splitlines()[0])
    json.dump({"builtAt": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
               "chain": {"key": tenant["chain"], "chainId": c["id"], "usdc": c["usdc"], "recordContract": c["anchor"]},
               "nodes": [{"url": u, "head": h} for u, h in zip(urls, ch["heads"])], "readAtBlock": ch["at"],
               "transfersFromBlock": start, "account": {"policy": policy, ch["writerIs"]: ch["writer"]},
               "balance": {"balanceOf": usdc(ch["balance"]), "transfersAfterPeriod": usdc(later),
                           "closing": usdc(closing), "opening": usdc(opening),
                           "transfersAddUpToBalanceOf": net == ch["balance"]},
               "period": {"from": args.start, "to": args.end, "fromUnix": t0, "toUnixExclusive": t1},
               "files": [{"path": os.path.relpath(p, args.app_dir),
                          "sha256": hashlib.sha256(open(p, "rb").read()).hexdigest()} for p in read],
               "witnesses": [{"alg": "balance-read", "keyId": f"chain:{c['id']}", "sig": None, "block": ch["at"],
                              "balance": usdc(ch["balance"]), "account": policy}, gw],
               "tools": tools, "beanCheck": checked, "vendorUsageReport": "not present in this app dir"},
              open(os.path.join(pack, "sources.json"), "w"), indent=1)

    # 10. README: the five attributes.
    in_record = [p for p in payments if p["inRecord"]]
    by_owner = [p for p in payments if p["by"] == "owner"]
    flagged = [p for p in payments if not p["inRecord"] and p["by"] != "owner"]
    period = [e for e in events if t0 <= e["at"] < t1]
    roots_ok = sum(1 for r in roots if r["verdict"] == "MATCH")
    if checked is None:
        bean_line = "not run here: `uv tool install beancount`, then `bean-check ledger.beancount`"
    elif checked["exit"] == 0:
        bean_line = f"passed ({tools['bean-check']})"
    else:
        bean_line = "FAILED: " + checked["output"].splitlines()[0]
    week_line = "; ".join(f"{s.get('week')}: {s['verdict']}" for s in sts) or st_note
    readme = README.format(
        name=title, chain=c["label"], start=args.start, end=args.end, n=len(payments),
        total=usdc(sum(p["minor"] for p in payments)), rec=len(in_record), owner=len(by_owner), flagged=len(flagged),
        flaglines="".join(f"\n    - {p['date']} {p['name']} {p['amount']} USDC, paid by the {p['by']} key"
                          for p in flagged),
        funded=usdc(sum(m["minor"] for m in moves if m["dir"] == "in" and t0 <= m["ts"] < t1)),
        opening=usdc(opening), closing=usdc(closing), bean=bean_line, roots=len(roots), rootsok=roots_ok,
        latest="the latest one anchored from this app dir" if now["verdict"] == "MATCH" else
        "not the latest one anchored from this app dir (another copy of the service may have anchored since)",
        inv=len(sig_invoices),
        invok=sum(1 for i in sig_invoices if i["signature"] == "valid"), ratio=f"{len(signed)}/{len(payments)}",
        sts=week_line, paid=sum(1 for e in period if e["kind"] == "SETTLEMENT"),
        refused=sum(1 for e in period if e["kind"] == "DECISION" and e["verdict"] == 3),
        asked=sum(1 for e in period if e["kind"] == "ESCALATION"),
        gateway=(f"the bill wallet {short(bill)} holds {gw['balance']} USDC at Circle "
                 + (f"on {len(gw['domains'])} chains" if gw["domains"] else "(every chain read zero)")
                 + f", read {dt.datetime.fromtimestamp(gw['at'], dt.timezone.utc):%Y-%m-%d %H:%M} UTC; booked as read, "
                 "not reconciled to any history" if gw["present"] else "absent: this account names no bill wallet"),
        gwcol=("the bill wallet's Circle Gateway unified balance, as Circle reports it (the second balance line)"
               if gw["present"] else "Circle Gateway: absent (this account names no bill wallet)"),
        verdict="every check held" if not fails else "FAILED: " + "; ".join(fails))
    open(os.path.join(pack, "README.md"), "w", encoding="utf-8").write(readme)
    if os.environ.get("AUDIT_PACK_RECORD"):
        json.dump(RECORDED, open(os.environ["AUDIT_PACK_RECORD"], "w"), indent=0, sort_keys=True)

    print(f"audit pack: {pack}")
    print(f"  ledger     {len(payments)} payments, {usdc(sum(p['minor'] for p in payments))} USDC; opening "
          f"{usdc(opening)}, closing {usdc(closing)} (the chain's, block {ch['at']})")
    print(f"  payments   {len(in_record)} in the agent's anchored record, {len(by_owner)} by the owner's passkey, "
          f"{len(flagged)} flagged ! (on chain, not in the record)")
    print(f"  records    {roots_ok}/{len(roots)} recomputed and anchored by the account's key on both nodes; rootOf "
          f"now: {now['verdict'].split(':')[0]}")
    print(f"  invoices   {sum(1 for i in sig_invoices if i['signature'] == 'valid')}/{len(sig_invoices)} payee "
          f"signatures valid; payments countersigned {len(signed)}/{len(payments)}")
    print(f"  statements {week_line}")
    print("  gateway    " + (f"{gw['balance']} USDC at Circle for the bill wallet {bill} (second balance line)"
                             if gw["present"] else gw["note"]))
    print(f"  bean-check {bean_line}")
    for f in fails:
        print(f"FAIL: {f}")
    if fails:
        return 1
    print("audit pack: every check held")
    return 0


README = """# Audit pack: {name}, {start} to {end}

Account on {chain}. Result: {verdict}.

## This period

- **Payments:** {n}, {total} USDC in all.
  - In the agent's anchored record, matching the chain: {rec}.
  - Paid by the owner with the passkey: {owner}. These are outside the agent's record by design; where the agent had
    asked for one, its ask is quoted beside it.
  - On the chain and not in this account's anchored record, booked from the chain with the `!` flag: {flagged}.{flaglines}
- **Funding in the period:** {funded} USDC. **Opening:** {opening} USDC. **Closing:** {closing} USDC, the chain's number
  (balanceOf at the block named in `sources.json`, less the transfers after the period).
- **Circle Gateway (second witness):** {gateway}.
- **bean-check:** {bean}. The closing balance is asserted with zero tolerance (`~ 0`): one millionth of a USDC off fails.
- **Records:** {rootsok} of {roots} recomputed from their events and found on chain, anchored by the account's own key,
  on two nodes. The root the chain holds now under that key: {latest}.
- **Invoices:** {inv} signed by payees, {invok} signatures valid. Payments settled under a payee-signed invoice: {ratio}.
- **Weekly statements:** {sts}.
- **The record beside the payments:** {paid} paid, {refused} refused, {asked} handed to the owner in this period.

## The five attributes

The columns are the attributes of reliability that proposed ISA 500 (Revised) lists in paragraph A4 (Exposure Draft
ED-500 (2026), IAASB, August 2026, comments due 15 December 2026), in its own words. Our earlier notes said "bias"; the
draft says **absence of bias**, and so does this pack. The draft says the attributes are not a checklist (A28), and that
for information produced by the entity, accuracy and completeness are presumed to be of significance (A43, A45).

| Attribute | File | What it covers | What it does not |
|---|---|---|---|
| Accuracy: "The information is free from error in its reflection of the underlying conditions, events, circumstances, actions or inactions, including reflecting the appropriate time period or point in time attributable to the conditions or events." | `ledger.beancount`, `replay.txt` | Each payment's payee and amount are read from the event bytes the anchored root commits to, and equal the USDC transfer and the policy's own event in the same transaction; each is dated by its block. bean-check ties the ledger to the chain's balance at the end of the period. | Whether the price was right for what was bought, or whether the vendor delivered it. |
| Completeness: "The information reflects all of the underlying conditions, events, circumstances, actions or inactions." | `ledger.beancount`, `root.json` | Every USDC movement of the account in the period, from the transfer logs of two nodes reconciled to balanceOf; every record's root recomputed and found on chain, so nothing was removed from a record after it was anchored. What the chain shows and the record does not is flagged `!` and listed above. Also {gwcol}. | USDC the operator holds in other wallets or on other chains; what moved in and out of Gateway (its line is Circle's number, not reconciled); bills the agent never saw; records written and never anchored (listed in `root.json`, not proved). |
| Authenticity: "The information has been generated by or provided by a source authorized to do so, and the information has not been altered in a manner that makes it inadequate for the audit." | `signatures.json`, `root.json` | Each invoice is signed by the key of the address the money goes to (EIP-712, re-verified); each record was anchored by the account's own key (the sender of the anchoring transaction, on two nodes); each weekly statement is signed by the statement key the service advertises. | That an address belongs to the company the owner believes it does; that no key was stolen (a stolen key signs validly). |
| Absence of bias: "The information is free from intentional and unintentional bias in its reflection of the underlying conditions, events, circumstances, actions or inactions." | `ledger.beancount` (`why`, `asked`), this README | The agent's reasons are quoted as written beside each payment and never executed; the record's refusals and asks are counted beside its payments, so the population is not only the payments that went out. | Proof. Whether the model's reasons are free of bias is the auditor's judgment. |
| Credibility: "The source has the competence and capability to generate the information to a required standard, and the source can be trusted." | `sources.json`, `countersign.json` | Who produced each piece: two public node operators, the chain, the payee's own signature, the service's record and statement key; the tool versions; the share of payments the counterparty itself signed for. | Proof. Whether a source can be trusted, and whether the two nodes are independent of each other, is the auditor's judgment. |

## Check it yourself

No key is needed. `bean-check ledger.beancount` (beancount v3) checks the arithmetic and the closing balance. To rebuild
the pack from a clone with the same app dir: `python3 scripts/audit_pack.py <app_dir> <policy> {start} {end}`. A record
can also be replayed by the service's own code: `AUDIT_EVENTS=<events file> AGENT=<key> ANCHOR=<record contract>
RPC_URL=<node> cargo run -p operator --example audit`.

What no file here proves: that the agent saw every bill it was owed, or that a payee is who the owner believes it is.
"""

if __name__ == "__main__":
    sys.exit(main())
