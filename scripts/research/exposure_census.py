#!/usr/bin/env python3
"""Exposure census: in the USDC/x402 agentic world, who holds anything but USDC? (docs/109 §4 method, §6 K1-K3)

Research, not product. Python 3 stdlib only. The whole run, from the repo root, in one command (about an hour; it resumes
where it stopped; delete runtime/research/exposure-census/ to start a new 30-day window):

    /opt/homebrew/bin/python3 -I scripts/research/exposure_census.py all

Steps, each runnable alone (the slow ones resume): k1-logs (fixes the window, then runs crosscheck), k2-owners, sample,
code, holdings, balances, fullpop, hl, bazaar, report. `selftest` checks the pure parts offline and the event/selector constants against a node.
Raw data (other people's addresses) stays in runtime/research/exposure-census/ (addresses.tsv is the per-address table); the
summary, with no addresses, goes to proof/exposure-census-<date>.txt.

K1  every `AuthorizationUsed(authorizer, nonce)` log of USDC on Base in the last 30 days (EIP-3009, the settlement of x402's
    `exact` scheme) -> distinct authorizers by count; sample = top 300 by count + 300 drawn at random (seed 109) from the rest.
    Holdings per address from Blockscout: native ETH + ERC-20 at Blockscout's exchange_rate; stablecoins excluded by symbol;
    a token without an exchange rate is counted apart as "unpriced". K1 = share of the 600 with >= $1,000 non-stable.
K2  ERC-8004 IdentityRegistry on Base and Ethereum: ownerOf(agentId) for every id -> distinct owners; the same sample shape per
    chain; holdings on that chain. K2 = owners with >= $1,000 non-stable (in the samples: a lower bound) + a stratified estimate.
K3  every sampled address: Hyperliquid `clearinghouseState` (an open position) and `extraAgents` (an API wallet).
Context, not a K: every resource on Coinbase's x402 Bazaar and the assets it accepts (the earn side).

Endpoints, and why (docs/60 P-158, P-159): Base logs come from Tenderly's public gateway at 1,000 blocks per call, because
mainnet.base.org answers every eth_getLogs with 429 -32011 "request limit reached" and base-rpc.publicnode.com refuses ranges
older than its recent window ("Archive requests require a personal token"). Block times from mainnet.base.org. ownerOf goes
through Multicall3 (1,000 per eth_call) on mainnet.base.org and ethereum-rpc.publicnode.com, because publicnode rate-limits a
JSON-RPC batch call by call (P-160); eth_getCode in small batches within each node's limit.
"""
import collections, gzip, http.client, json, math, random, statistics, sys, threading, time
import urllib.error, urllib.parse, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "runtime" / "research" / "exposure-census"
COMMAND = "/opt/homebrew/bin/python3 -I scripts/research/exposure_census.py all"
# base.blockscout.com's Cloudflare rule challenges urllib's and curl's default User-Agent (P-159); this one is answered.
UA = "Mozilla/5.0 (compatible; tameion-research/0.1)"

USDC = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
AUTH_USED = "0x98de503528ee59b575ef0c0a2576a82497bfc029a5685b209e9ec333479b10a5"
REG = "0x8004a169fb4a3325136eb29fa0ceb6d2e539a432"  # IdentityRegistry, Ethereum + Base: github.com/erc-8004/erc-8004-contracts
REGISTERED = "0xca52e62c367d81bb2e328eb795f7c7ba24afb478408a26c0e201d155c449bc4a"
OWNER_OF = "0x6352211e"
MULTICALL3 = "0xca11bde05977b3631167028862be2a173976ca11"  # same address on Base and Ethereum
TRY_AGGREGATE = "0xbce38bd7"
GET_ETH_BALANCE = "0x4d2301cc"  # Multicall3's own getEthBalance(address)
BALANCE_OF = "0x70a08231"
SIGS = {"AuthorizationUsed(address,bytes32)": AUTH_USED, "Registered(uint256,string,address)": REGISTERED,
        "ownerOf(uint256)": OWNER_OF, "tryAggregate(bool,(address,bytes)[])": TRY_AGGREGATE,
        "getEthBalance(address)": GET_ETH_BALANCE, "balanceOf(address)": BALANCE_OF}
TOKENS_N = 8  # fullpop: the non-stable tokens most held in each chain's samples
STABLES = {"usdc", "usdbc", "usdt", "dai", "eurc", "usde", "frax", "gho", "pyusd", "usds"}  # docs/109 §4, fixed before the run
DAYS, BLOCKS_PER_DAY, CHUNK = 30, 43_200, 1_000  # Base makes one block every 2 s
TOP, RANDOM_N, SEED, MIN_USD = 300, 300, 109, 1_000.0
BURN = {"0x000000000000000000000000000000000000dead", "0x0000000000000000000000000000000000000000"}  # nobody holds these keys
WORKERS = {"base": 3, "eth": 2}  # parallel Blockscout readers per chain (one is latency-bound at ~0.2 address/s)
HL_SECONDS = 1.25  # clearinghouseState (weight 2) + extraAgents (weight 20) per address, under 1,200 weight a minute

BASE_BLOCKS = "https://mainnet.base.org"
BASE_LOGS = ["https://gateway.tenderly.co/public/base", "https://base.gateway.tenderly.co"]
# eth_getCode in JSON-RPC batches: (node, batch size) — mainnet.base.org takes 25 calls a second, publicnode throttles big batches
STATE = {"base": ("https://mainnet.base.org", 20), "eth": ("https://ethereum-rpc.publicnode.com", 100)}
CALLS = {"base": ["https://mainnet.base.org", "https://base-rpc.publicnode.com"],  # single eth_calls (Multicall3)
         "eth": ["https://ethereum-rpc.publicnode.com", "https://eth.drpc.org"]}
SCOUT = {"base": "https://base.blockscout.com/api/v2", "eth": "https://eth.blockscout.com/api/v2"}
HL = "https://api.hyperliquid.xyz/info"
BAZAAR = "https://api.cdp.coinbase.com/platform/v2/x402/discovery/resources"  # Coinbase's x402 discovery list, keyless
# Bazaar asset ids that carry no EIP-712 name: USDC (Solana mint, Algorand ASA), other USD stablecoins (USDT's Solana mint,
# Open USD on Base per Blockscout, RLUSD on XRPL as hex), and volatile assets (XRP, BTC, Nano, wrapped SOL)
USDC_IDS = {"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "31566704"}
USD_IDS = {"Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", "0xB2000000000000000000002fEb517dFeC7415344"}
VOLATILE_IDS = {"XRP", "BTC", "XNO", "So11111111111111111111111111111111111111112"}
FX_WORDS = ("peso", "real", "yen", "jpy", "mxn", "brl", "cop", "eur")
LOCK = threading.Lock()


# ---------------------------------------------------------------- HTTP, with every refusal written to incidents.log
def incident(host, kind, detail=""):
    with LOCK, open(DATA / "incidents.log", "a") as f:
        f.write(f"{time.strftime('%Y-%m-%dT%H:%M:%S')}\t{host}\t{kind}\t{' '.join(str(detail).split())[:160]}\n")


def fetch(url, body=None, timeout=120, tries=8):
    """GET (body None) or POST JSON -> (parsed JSON, headers). A JSON error body on HTTP 400 is returned, not retried;
    404 returns {"_status": 404}; 429, 5xx, timeouts and challenges back off exponentially."""
    host = urllib.parse.urlsplit(url).netloc
    headers = {"User-Agent": UA, "Accept": "application/json", "Accept-Encoding": "gzip"}
    data = None
    if body is not None:
        data, headers["Content-Type"] = json.dumps(body).encode(), "application/json"
    for i in range(tries):
        wait = min(2 ** i, 60)
        try:
            with urllib.request.urlopen(urllib.request.Request(url, data=data, headers=headers), timeout=timeout) as r:
                raw = r.read()
                if r.headers.get("Content-Encoding") == "gzip":
                    raw = gzip.decompress(raw)
                return json.loads(raw), r.headers
        except urllib.error.HTTPError as e:
            raw = e.read()
            if e.headers.get("Content-Encoding") == "gzip":
                raw = gzip.decompress(raw)
            if e.code == 404:
                return {"_status": 404}, e.headers
            if e.code == 400:
                try:
                    return json.loads(raw), e.headers
                except ValueError:
                    pass
            incident(host, f"HTTP {e.code}", raw[:160].decode(errors="replace"))
            reset = e.headers.get("x-ratelimit-reset")
            if e.code == 429 and reset and reset.isdigit():
                wait = max(wait, int(reset) / 1000 + 0.5)
        except (urllib.error.URLError, TimeoutError, ConnectionError, http.client.HTTPException, ValueError, OSError) as e:
            incident(host, type(e).__name__, e)
        time.sleep(wait)
    raise RuntimeError(f"{host}: no answer after {tries} tries ({url[:100]})")


class TooBig(Exception):
    pass


TOO_BIG = ("too large", "more than", "too many", "query returned", "response size", "max results", "exceeds max")


def rpc(urls, method, params):
    urls = [urls] if isinstance(urls, str) else urls
    for i in range(8):
        url = urls[i % len(urls)]
        d, _ = fetch(url, {"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
        if "error" not in d and "result" in d:
            return d["result"]
        msg = json.dumps(d.get("error", d))
        if any(s in msg.lower() for s in TOO_BIG) and "rate limit" not in msg.lower():
            raise TooBig(msg)
        incident(urllib.parse.urlsplit(url).netloc, f"rpc {(d.get('error') or {}).get('code')}", msg)
        time.sleep(min(2 ** i, 60))
    raise RuntimeError(f"{method}: no result after 8 tries")


def rpc_batch(url, calls):
    """[(method, params)] -> results in order; None where the call reverted (an agentId that does not exist)."""
    body = [{"jsonrpc": "2.0", "id": i, "method": m, "params": p} for i, (m, p) in enumerate(calls)]
    for i in range(8):
        d, _ = fetch(url, body)
        if isinstance(d, list):
            by_id = {x.get("id"): x for x in d}
            out = []
            for j in range(len(calls)):
                x = by_id.get(j, {})
                err = x.get("error") or {}
                if "result" in x:
                    out.append(x["result"])
                elif err.get("code") == 3 or "revert" in str(err.get("message", "")).lower():
                    out.append(None)
                else:
                    break
            if len(out) == len(calls):
                return out
        incident(urllib.parse.urlsplit(url).netloc, "batch incomplete", json.dumps(d)[:160])
        time.sleep(min(2 ** i, 60))
    raise RuntimeError("batch: no complete answer after 8 tries")


def scout(chain, path, tries=8):
    """Blockscout v2, paced by its own x-ratelimit headers."""
    d, h = fetch(SCOUT[chain] + path, timeout=150, tries=tries)
    rem, reset = h.get("x-ratelimit-remaining"), h.get("x-ratelimit-reset")
    if rem and reset and rem.lstrip("-").isdigit() and reset.isdigit() and int(rem) <= 3:
        time.sleep(int(reset) / 1000 + 0.5)
    return d


# ---------------------------------------------------------------- files
def jsonl(path):
    """Lines of a JSON-lines file; a line cut short by an interrupted run is skipped (its work is redone)."""
    if not path.exists():
        return []
    out = []
    for line in path.read_text().splitlines():
        try:
            out.append(json.loads(line))
        except ValueError:
            pass
    return out


def terminate(path):
    """End a JSON-lines file with a newline, so an append never joins a line an interrupted run cut short."""
    if path.exists() and path.stat().st_size and not path.read_bytes().endswith(b"\n"):
        with open(path, "a") as f:
            f.write("\n")


def load(name):
    return json.loads((DATA / name).read_text())


def save(name, obj):
    tmp = DATA / (name + ".tmp")
    tmp.write_text(json.dumps(obj))
    tmp.replace(DATA / name)


def window():
    p = DATA / "window.json"
    return json.loads(p.read_text()) if p.exists() else {}


# ---------------------------------------------------------------- K1: who paid by EIP-3009 authorization
def block_ts(n):
    return int(rpc(BASE_BLOCKS, "eth_getBlockByNumber", [hex(n), False])["timestamp"], 16)


def get_logs(frm, to):
    try:
        return rpc(BASE_LOGS, "eth_getLogs", [{"address": USDC, "topics": [AUTH_USED], "fromBlock": hex(frm), "toBlock": hex(to)}])
    except TooBig:
        if frm == to:
            raise
        mid = (frm + to) // 2
        return get_logs(frm, mid) + get_logs(mid + 1, to)


def k1_logs():
    w = window()
    if "k1" not in w:
        head = int(rpc(BASE_LOGS, "eth_blockNumber", []), 16) - 12  # the log node's own head, less a safe margin
        frm = head - DAYS * BLOCKS_PER_DAY + 1
        w["k1"] = {"from": frm, "to": head, "from_ts": block_ts(frm), "to_ts": block_ts(head)}
        save("window.json", w)
        crosscheck()
    k = w["k1"]
    path = DATA / "k1_chunks.jsonl"
    terminate(path)
    done = {c["from"] for c in jsonl(path)}
    todo = [s for s in range(k["from"], k["to"] + 1, CHUNK) if s not in done]
    print(f"K1: Base blocks {k['from']}..{k['to']}; {len(todo)} chunks of {CHUNK} to fetch", flush=True)

    def one(s):
        e = min(s + CHUNK - 1, k["to"])
        counts = collections.Counter()
        logs = get_logs(s, e)
        for log in logs:
            assert log["address"].lower() == USDC and log["topics"][0] == AUTH_USED, log
            counts["0x" + log["topics"][1][-40:]] += 1
        return {"from": s, "to": e, "n": len(logs), "counts": counts}

    with ThreadPoolExecutor(4) as ex, open(path, "a") as f:
        for n, r in enumerate(ex.map(one, todo), 1):
            f.write(json.dumps(r) + "\n")
            f.flush()
            if n % 100 == 0:
                print(f"K1: {n}/{len(todo)} chunks", flush=True)


def crosscheck():
    """The last 500 blocks of the K1 window read from a second node (publicnode serves only recent blocks, so this runs
    right after the window is fixed): the same logs, by (transaction, log index)?"""
    k = window()["k1"]
    a, b = k["to"] - 499, k["to"]
    q = [{"address": USDC, "topics": [AUTH_USED], "fromBlock": hex(a), "toBlock": hex(b)}]
    try:
        x, y = rpc("https://base-rpc.publicnode.com", "eth_getLogs", q), rpc(BASE_LOGS, "eth_getLogs", q)
        keys = lambda logs: sorted((log["transactionHash"], log["logIndex"]) for log in logs)
        out = {"blocks": [a, b], "publicnode": len(x), "tenderly": len(y), "identical": keys(x) == keys(y)}
    except (RuntimeError, TooBig) as e:
        out = {"blocks": [a, b], "skipped": str(e)[:120]}
    save("crosscheck.json", out)
    print(f"crosscheck: {out}", flush=True)


# ---------------------------------------------------------------- K2: who owns the registered agents
def word(n):
    return n.to_bytes(32, "big").hex()


def multicall(urls, calls):
    """[(target, 36-byte calldata)] in ONE eth_call through Multicall3 tryAggregate(false, calls) -> [(success, returnData)].
    publicnode rate-limits a JSON-RPC batch call by call (P-160); a multicall counts once."""
    n = len(calls)
    assert all(len(cd) == 2 + 72 for _, cd in calls), "every call here is a selector + one word"
    heads = "".join(word(n * 32 + i * 160) for i in range(n))  # each (address, bytes) tuple is 160 bytes
    tuples = "".join(t[2:].lower().rjust(64, "0") + word(64) + word(36) + cd[2:].ljust(128, "0") for t, cd in calls)
    data = TRY_AGGREGATE + word(0) + word(64) + word(n) + heads + tuples
    out = bytes.fromhex(rpc(urls, "eth_call", [{"to": MULTICALL3, "data": data}, "latest"])[2:])

    def at(pos):
        return int.from_bytes(out[pos:pos + 32], "big")

    assert at(at(0)) == n, "multicall answered a different number of results"
    base, res = at(0) + 32, []
    for k in range(n):
        t = base + at(base + 32 * k)  # (bool success, bytes returnData)
        ok, ret = at(t), t + at(t + 32)
        res.append((bool(ok), out[ret + 32:ret + 32 + at(ret)]))
    return res


def owners_of(urls, ids):
    """ownerOf(id) for many ids in one multicall -> {id: owner} for the ids that exist."""
    res = multicall(urls, [(REG, OWNER_OF + word(i)) for i in ids])
    return {i: "0x" + r[12:].hex() for i, (ok, r) in zip(ids, res) if ok and len(r) == 32}


def k2_owners():
    for chain in ("base", "eth"):
        if (DATA / f"k2_owners_{chain}.json").exists():
            continue
        urls = CALLS[chain]

        def exists(n):
            return bool(owners_of(urls, [n]))

        # ids are assigned 0, 1, 2, ... and never burned (IdentityRegistryUpgradeable.sol: `agentId = $._lastId++`, no burn path),
        # so the largest id whose ownerOf answers bounds the set; the scan below still runs 500 ids past it.
        hi = 1
        while exists(hi):
            hi *= 2
        lo = hi // 2
        while hi - lo > 1:
            mid = (lo + hi) // 2
            lo, hi = (mid, hi) if exists(mid) else (lo, mid)
        b0 = int(rpc(urls, "eth_blockNumber", []), 16)
        owners = {}
        for start in range(0, lo + 501, 1000):
            owners.update(owners_of(urls, list(range(start, start + 1000))))
        b1 = int(rpc(urls, "eth_blockNumber", []), 16)
        for n in random.Random(SEED).sample(sorted(owners), min(20, len(owners))):  # the multicall decoding, checked directly
            direct = rpc(urls, "eth_call", [{"to": REG, "data": OWNER_OF + word(n)}, "latest"])
            assert "0x" + direct[-40:].lower() == owners[n], (chain, n, direct, owners[n])
        newest = None  # cross-check only: the newest Registered event Blockscout shows
        try:
            items = scout(chain, f"/addresses/{REG}/logs", tries=3).get("items") or []
            ids = [int(x["topics"][1], 16) for x in items if x.get("topics") and x["topics"][0] == REGISTERED]
            newest = max(ids) if ids else None
        except RuntimeError as e:
            incident("blockscout", "cross-check skipped", e)
        save(f"k2_owners_{chain}.json", {"chain": chain, "last_id": lo, "blocks": [b0, b1], "agents": len(owners),
                                         "newest_registered_blockscout": newest, "owners": owners})
        print(f"K2 {chain}: {len(owners)} agents, {len(set(owners.values()))} distinct owners (blocks {b0}..{b1})", flush=True)


# ---------------------------------------------------------------- samples
def rank_and_draw(counter, seed=SEED, top_n=TOP, random_n=RANDOM_N):
    """Top `top_n` by count (ties by address), then `random_n` drawn with a fresh Random(seed) from the sorted rest."""
    ranked = sorted(counter.items(), key=lambda kv: (-kv[1], kv[0]))
    top = [a for a, _ in ranked[:top_n]]
    rest = sorted(a for a, _ in ranked[top_n:])
    return top, random.Random(seed).sample(rest, min(random_n, len(rest)))


def sample():
    k = window()["k1"]
    chunks = jsonl(DATA / "k1_chunks.jsonl")
    spans = sorted((c["from"], c["to"]) for c in chunks)
    expect = [(s, min(s + CHUNK - 1, k["to"])) for s in range(k["from"], k["to"] + 1, CHUNK)]
    assert sorted(set(spans)) == expect, "K1 chunks do not cover the window: run k1-logs again"
    counts, seen = collections.Counter(), set()
    nlogs = 0
    for c in chunks:
        if c["from"] in seen:  # a chunk written twice by an interrupted run counts once
            continue
        seen.add(c["from"])
        counts.update(c["counts"])
        nlogs += c["n"]
    top, rnd = rank_and_draw(counts)
    s = {"k1": {"logs": nlogs, "distinct": len(counts), "top": top, "random": rnd,
                "top_logs": sum(counts[a] for a in top), "counts": {a: counts[a] for a in top + rnd},
                "median_count": statistics.median(counts.values())}}
    for chain in ("base", "eth"):
        o = load(f"k2_owners_{chain}.json")
        oc = collections.Counter(o["owners"].values())
        burned = sum(oc.pop(a, 0) for a in BURN)  # agents sent to a burn address have no operator
        top, rnd = rank_and_draw(oc)
        s["k2_" + chain] = {"agents": o["agents"], "burned": burned, "distinct": len(oc), "top": top, "random": rnd,
                            "counts": {a: oc[a] for a in top + rnd}, "top_agents": sum(oc[a] for a in top)}
    save("sample.json", s)
    print(f"sample: K1 {len(s['k1']['top']) + len(s['k1']['random'])} of {s['k1']['distinct']}; "
          f"K2 base {s['k2_base']['distinct']} owners, eth {s['k2_eth']['distinct']} owners", flush=True)


def sampled(s):
    """[(set, chain, stratum, address)] for every sampled address."""
    out = [("k1", "base", st, a) for st in ("top", "random") for a in s["k1"][st]]
    for chain in ("base", "eth"):
        out += [("k2", chain, st, a) for st in ("top", "random") for a in s["k2_" + chain][st]]
    return out


def code_kind(hexcode):
    if hexcode in (None, "0x", "0x0"):
        return "eoa"
    if hexcode.startswith("0xef0100") and len(hexcode) == 2 + 46:
        return "7702"  # an EOA that delegated to code (EIP-7702)
    return "contract"


def code():
    s = load("sample.json")
    out = {}
    for chain in ("base", "eth"):
        addrs = sorted({a for _, c, _, a in sampled(s) if c == chain})
        url, size = STATE[chain]
        for i in range(0, len(addrs), size):
            part = addrs[i:i + size]
            for a, r in zip(part, rpc_batch(url, [("eth_getCode", [a, "latest"]) for a in part])):
                out[f"{chain}:{a}"] = code_kind(r)
            time.sleep(1)
    save("code.json", out)


# ---------------------------------------------------------------- holdings (Blockscout)
def compact(item):
    t = item.get("token") or {}
    return {"token": (t.get("address_hash") or t.get("address") or "").lower(), "symbol": t.get("symbol"), "name": t.get("name"),
            "decimals": t.get("decimals"), "rate": t.get("exchange_rate"), "type": t.get("type"),
            "reputation": t.get("reputation"), "value": item.get("value")}


def holdings():
    s = load("sample.json")
    path = DATA / "holdings.jsonl"
    terminate(path)
    done = {(r["chain"], r["addr"]) for r in jsonl(path)}
    jobs = {c: sorted({a for _, ch, _, a in sampled(s) if ch == c and (c, a) not in done}) for c in ("base", "eth")}
    print(f"holdings: {len(jobs['base'])} on Base, {len(jobs['eth'])} on Ethereum to read", flush=True)

    queues = {c: collections.deque(jobs[c]) for c in jobs}
    count = collections.Counter()

    def worker(chain):  # WORKERS[chain] threads share one queue; Blockscout's own headers pace them
        fails = 0
        while True:
            with LOCK:
                if not queues[chain]:
                    return
                a = queues[chain].popleft()
            # One Blockscout call per address: the token list. Native ETH comes from the chain (`balances`, eth_getBalance) and
            # contract-or-not from eth_getCode (`code`); `/addresses/{a}` adds nothing the chain does not give, and skipping it
            # halves the calls per address under Blockscout's ~150-calls-a-window limit. (The run of 10-16 read all Ethereum
            # records and the first 386 Base records with it too.)
            rec = {"chain": chain, "addr": a, "found": True, "coin_balance": None, "eth_rate": None, "is_contract": None,
                   "tokens": [], "error": None}
            try:
                toks = scout(chain, f"/addresses/{a}/token-balances", tries=2)  # a 524 takes ~125 s
                fails = 0
                rec["tokens"] = [compact(t) for t in toks if isinstance(t, dict)] if isinstance(toks, list) else []
                rec["blockscout_404"] = isinstance(toks, dict) and toks.get("_status") == 404
            except RuntimeError as e:
                fails += 1
                rec["error"] = str(e)[:120]
                if fails >= 3:
                    print(f"holdings {chain}: three addresses in a row failed; stopping, re-run `holdings` later", flush=True)
                    return
            with LOCK, open(path, "a") as f:
                f.write(json.dumps(rec) + "\n")
                count[chain] += 1
                if count[chain] % 100 == 0:
                    print(f"holdings {chain}: {count[chain]}/{len(jobs[chain])}", flush=True)

    ts = [threading.Thread(target=worker, args=(c,)) for c in ("base", "eth") for _ in range(WORKERS[c])]
    [t.start() for t in ts]
    [t.join() for t in ts]


def balances():
    """Where Blockscout returned no coin_balance (an address it never fetched a balance for), read eth_getBalance from the
    chain instead of assuming zero; the record keeps `coin_balance_src: rpc`."""
    path = DATA / "holdings.jsonl"
    recs = jsonl(path)
    for chain in ("base", "eth"):
        todo = [r for r in recs if r["chain"] == chain and r["found"] and r.get("coin_balance") is None]
        url, size = STATE[chain]
        for i in range(0, len(todo), size):
            part = todo[i:i + size]
            for r, bal in zip(part, rpc_batch(url, [("eth_getBalance", [r["addr"], "latest"]) for r in part])):
                r["coin_balance"], r["coin_balance_src"] = str(int(bal, 16)), "rpc"
            time.sleep(1)
        print(f"balances {chain}: {len(todo)} read from the chain", flush=True)
    tmp = DATA / "holdings.jsonl.tmp"
    tmp.write_text("".join(json.dumps(r) + "\n" for r in recs))
    tmp.replace(path)


def value(rec, eth_rate_fallback=0.0):
    """Non-stable USD value of one address on one chain: native ETH + priced ERC-20s, stablecoins (by symbol) excluded,
    tokens without an exchange rate counted as unpriced. `held` maps a token label to its USD value."""
    rate = float(rec.get("eth_rate") or eth_rate_fallback or 0)
    eth = int(rec.get("coin_balance") or 0) / 1e18 * rate
    held, unpriced, stable_like, flagged = {"ETH (native)": eth}, 0, 0.0, 0.0
    for t in rec.get("tokens") or []:
        if t.get("type") != "ERC-20" or not str(t.get("value") or "0").isdigit() or int(t["value"]) == 0:
            continue
        sym = (t.get("symbol") or "").strip()
        if sym.lower() in STABLES:
            continue
        if not t.get("rate") or not str(t.get("decimals") or "").isdigit():
            unpriced += 1
            continue
        v = int(t["value"]) / 10 ** int(t["decimals"]) * float(t["rate"])
        label = f"{sym} ({t.get('name') or '?'})"
        held[label] = held.get(label, 0.0) + v
        if 0.98 <= float(t["rate"]) <= 1.02:
            stable_like += v
        if t.get("reputation") not in (None, "ok"):  # Blockscout flags the token (spam, scam)
            flagged += v
    return {"nonstable": sum(held.values()), "eth": eth, "unpriced": unpriced, "held": held, "stable_like": stable_like,
            "flagged": flagged}


# ---------------------------------------------------------------- every payer and owner, not a sample (a lower bound)
def fullpop():
    """The nearest measurable thing to K2's count over ~116k owners (a 600-address sample cannot resolve 30 of them), and a
    population check on K1: for EVERY payer and EVERY agent owner, native ETH plus the TOKENS_N priced non-stable tokens most held
    in that chain's samples, read through Multicall3 (getEthBalance, balanceOf) and priced at Blockscout's exchange_rate. Other
    tokens are not counted, so each figure is a lower bound on non-stable value."""
    s = load("sample.json")
    hold = jsonl(DATA / "holdings.jsonl")
    allk1 = set()
    for c in jsonl(DATA / "k1_chunks.jsonl"):
        allk1.update(c["counts"])
    pops = {"k1": ("base", sorted(allk1))}
    for chain in ("base", "eth"):
        pops["k2_" + chain] = (chain, sorted(set(load(f"k2_owners_{chain}.json")["owners"].values()) - BURN))
    out, hits = {"tokens": {}, "pops": {}}, {}
    for chain in ("base", "eth"):
        holders, meta = collections.Counter(), {}
        for r in hold:
            if r["chain"] != chain or not r["found"] or r["error"]:
                continue
            for t in r["tokens"]:
                v = value({"tokens": [t]})["held"]  # {"ETH (native)": 0, label: usd} for a priced non-stable token
                if len(v) == 2 and max(v.values()) >= 1 and t.get("reputation") in (None, "ok"):
                    holders[t["token"]] += 1
                    meta[t["token"]] = {"symbol": t["symbol"], "decimals": int(t["decimals"]), "rate": float(t["rate"])}
        tokens = [a for a, _ in holders.most_common(TOKENS_N)]
        eth_rate = statistics.median([float(r["eth_rate"]) for r in hold if r["chain"] == chain and r.get("eth_rate")])
        out["tokens"][chain] = {"eth_rate": eth_rate, "tokens": [dict(meta[a], holders_in_sample=holders[a], token=a) for a in tokens]}
        urls = CALLS[chain]
        for key, (ch, addrs) in pops.items():
            if ch != chain:
                continue
            usd = collections.defaultdict(float)
            eth_usd = {}
            jobs = [(MULTICALL3, GET_ETH_BALANCE, 10 ** 18, eth_rate)] + [(a, BALANCE_OF, 10 ** meta[a]["decimals"], meta[a]["rate"]) for a in tokens]
            for target, sel, unit, rate in jobs:
                parts = [addrs[i:i + 1000] for i in range(0, len(addrs), 1000)]
                with ThreadPoolExecutor(4) as ex:
                    for part, res in zip(parts, ex.map(lambda pt: multicall(urls, [(target, sel + word(int(a, 16))) for a in pt]), parts)):
                        for a, (ok, r) in zip(part, res):
                            if ok and len(r) == 32:
                                v = int.from_bytes(r, "big") / unit * rate
                                usd[a] += v
                                if sel == GET_ETH_BALANCE:
                                    eth_usd[a] = v
                print(f"fullpop {key}: {target[:10]} done", flush=True)
            for a in random.Random(SEED).sample(addrs, min(10, len(addrs))):  # the multicall decoding, checked directly
                direct = int(rpc(urls, "eth_getBalance", [a, "latest"]), 16) / 10 ** 18 * eth_rate
                assert abs(direct - eth_usd.get(a, 0)) <= max(1.0, 0.01 * direct), (key, a, direct, eth_usd.get(a))  # seconds apart
            vals = [usd[a] for a in addrs]
            hits[key] = {a for a in addrs if usd[a] >= MIN_USD}
            out["pops"][key] = {"chain": chain, "n": len(addrs), "ge_1000": sum(1 for v in vals if v >= MIN_USD),
                                "ge_1000_eth_only": sum(1 for a in addrs if eth_usd.get(a, 0) >= MIN_USD),
                                "ge_100": sum(1 for v in vals if v >= 100), "zero": sum(1 for v in vals if v < 0.01),
                                "median": statistics.median(vals) if vals else 0,
                                "p99": sorted(vals)[int(0.99 * (len(vals) - 1))] if vals else 0, "sum": sum(vals)}
            with open(DATA / f"fullpop_{key}.tsv", "w") as f:
                f.write("address\tnonstable_usd_lower_bound\teth_usd\n")
                f.writelines(f"{a}\t{usd[a]:.2f}\t{eth_usd.get(a, 0):.2f}\n" for a in addrs)
    out["k2_union_ge_1000"] = len(hits["k2_base"] | hits["k2_eth"])
    out["k2_union_n"] = len(set(pops["k2_base"][1]) | set(pops["k2_eth"][1]))
    out["read_at"] = time.strftime("%Y-%m-%d %H:%M %z")
    save("fullpop.json", out)


# ---------------------------------------------------------------- K3: Hyperliquid
def hl():
    s = load("sample.json")
    path = DATA / "hl.jsonl"
    terminate(path)
    done = {r["addr"] for r in jsonl(path)}
    todo = sorted({a for _, _, _, a in sampled(s)} - done)
    print(f"hl: {len(todo)} addresses to check (~{len(todo) * HL_SECONDS / 60:.0f} min)", flush=True)
    for n, a in enumerate(todo, 1):
        t0 = time.time()
        try:
            cs, _ = fetch(HL, {"type": "clearinghouseState", "user": a})
            ea, _ = fetch(HL, {"type": "extraAgents", "user": a})
        except RuntimeError as e:  # not written: a re-run of `hl` retries it
            incident("api.hyperliquid.xyz", "address skipped", e)
            continue
        if not isinstance(cs, dict) or "assetPositions" not in cs or not isinstance(ea, list):
            incident("api.hyperliquid.xyz", "unexpected answer", f"{str(cs)[:80]} {str(ea)[:60]}")
            continue
        rec = {"addr": a, "positions": len(cs["assetPositions"]), "agents": len(ea),
               "account_value": float((cs.get("marginSummary") or {}).get("accountValue") or 0)}
        with LOCK, open(path, "a") as f:
            f.write(json.dumps(rec) + "\n")
        if n % 100 == 0:
            print(f"hl: {n}/{len(todo)}", flush=True)
        time.sleep(max(0.0, HL_SECONDS - (time.time() - t0)))


# ---------------------------------------------------------------- the earn side: what x402 sellers ask to be paid in
def bazaar():
    """Every resource on the x402 Bazaar with what it accepts (network, asset, scheme, amount, payTo). Not a K: context for
    "earns anything but USDC" (docs/109 §3 row 4)."""
    if (DATA / "bazaar.json").exists():
        return
    items, offset, total = {}, 0, None
    while total is None or offset < total:
        d, _ = fetch(f"{BAZAAR}?limit=1000&offset={offset}", timeout=180)
        page = d.get("items") or []
        total = (d.get("pagination") or {}).get("total", 0)
        for it in page:
            items[it.get("resource") or f"#{offset}"] = {
                "updated": it.get("lastUpdated"),
                "accepts": [{"network": a.get("network"), "asset": a.get("asset"), "scheme": a.get("scheme"),
                             "amount": a.get("amount"), "payTo": a.get("payTo"), "name": (a.get("extra") or {}).get("name")}
                            for a in it.get("accepts") or []]}
        if not page:
            break
        offset += len(page)
        print(f"bazaar: {offset}/{total}", flush=True)
    save("bazaar.json", {"total": total, "read_at": time.strftime("%Y-%m-%d %H:%M %z"), "items": items})


def asset_class(a):
    name, asset = (a.get("name") or "").strip().lower(), str(a.get("asset") or "")
    if "usd coin" in name or name in ("usdc", "gatewaywalletbatched") or asset in USDC_IDS or asset.lower() == "uusdc" \
            or asset.startswith("USDC:"):
        return "USDC"  # GatewayWalletBatched = USDC held in Circle Gateway, paid in batches; uusdc = USDC on Noble
    if any(w in name for w in FX_WORDS):
        return "non-USD stablecoin"
    if any(w in name for w in ("usd", "dollar", "stables")) or asset in USD_IDS or asset.startswith("524C555344"):
        return "other USD stablecoin"
    if name or asset in VOLATILE_IDS or asset.endswith(".sbtc-token"):
        return "volatile"
    return "unnamed asset id"


# ---------------------------------------------------------------- report
def wilson(k, n, z=1.96):
    if n == 0:
        return 0.0, 0.0
    p, d = k / n, 1 + z * z / n
    c, r = p + z * z / (2 * n), z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n))
    return max(0.0, (c - r) / d), min(1.0, (c + r) / d)


def utc(ts):
    return time.strftime("%Y-%m-%d %H:%M UTC", time.gmtime(ts))


def pct(k, n):
    return f"{k}/{n} = {100 * k / n:.1f}%" if n else f"{k}/0"


def usd(x):
    return f"${x:,.0f}"


def report():
    s, w = load("sample.json"), window()
    hold = {(r["chain"], r["addr"]): r for r in jsonl(DATA / "holdings.jsonl")}
    hlr = {r["addr"]: r for r in jsonl(DATA / "hl.jsonl")}
    kinds = load("code.json")
    rates = {c: statistics.median([float(r["eth_rate"]) for (ch, _), r in hold.items() if ch == c and r.get("eth_rate")] or [0])
             for c in ("base", "eth")}
    rows, vals = [], {}
    for st, chain, stratum, a in sampled(s):
        r = hold.get((chain, a))
        v = value(r, rates[chain]) if r and r["found"] and not r["error"] else None
        vals[(st, chain, a)] = v
        h = hlr.get(a, {})
        count = (s["k1"] if st == "k1" else s["k2_" + chain])["counts"][a]
        rows.append([st, chain, stratum, a, count, kinds.get(f"{chain}:{a}", "?"), "" if v is None else f"{v['nonstable']:.2f}",
                     "" if v is None else f"{v['eth']:.2f}", "" if v is None else v["unpriced"],
                     h.get("positions", ""), h.get("agents", ""), h.get("account_value", "")])
    with open(DATA / "addresses.tsv", "w") as f:
        f.write("set\tchain\tstratum\taddress\tauthorizations_or_agents\tcode\tnonstable_usd\teth_usd\tunpriced_tokens"
                "\thl_positions\thl_api_wallets\thl_account_value\n")
        for row in rows:
            f.write("\t".join(map(str, row)) + "\n")

    def group(st, chain, stratum=None):
        return [(a, vals[(st, chain, a)]) for x, c, sr, a in sampled(s) if x == st and c == chain and (stratum in (None, sr))]

    def hits(g, key="nonstable"):
        return sum(1 for _, v in g if v and v[key] >= MIN_USD)

    def ok(g):
        return [v for _, v in g if v]

    k1 = w["k1"]
    L = [f"Exposure census (docs/109 §6 K1-K3, seat S7). Written {time.strftime('%Y-%m-%d %H:%M %z')} by the command:",
         f"  {COMMAND}",
         "Per-address data (other people's addresses) stays in runtime/research/exposure-census/addresses.tsv; none are printed here.",
         "",
         "Endpoints",
         "  Base logs   Tenderly public gateway (gateway.tenderly.co/public/base, base.gateway.tenderly.co): eth_getLogs, 1,000 blocks",
         "              per call. mainnet.base.org answered every eth_getLogs 429 -32011; base-rpc.publicnode.com refused old ranges",
         "              (\"Archive requests require a personal token\") - docs/60 P-158. Block times: mainnet.base.org.",
         "  State       ownerOf through Multicall3 tryAggregate, 1,000 ids per eth_call (mainnet.base.org; ethereum-rpc.publicnode.com),",
         "              each decoding checked against 20 direct ownerOf calls; eth_getCode in batches of 20 (Base) / 100 (Ethereum)",
         "  Holdings    base.blockscout.com, eth.blockscout.com /api/v2/addresses/{a} and /token-balances (exchange_rate)",
         "  Venue       api.hyperliquid.xyz/info clearinghouseState + extraAgents",
         ""]
    g = group("k1", "base")
    gt, gr = group("k1", "base", "top"), group("k1", "base", "random")
    n_ok, h_all, h_top, h_rnd = len(ok(g)), hits(g), hits(gt), hits(gr)
    lo, hi = wilson(h_rnd, len(ok(gr)))
    N = s["k1"]["distinct"]
    rest = N - len(gt)
    est = (h_top + (h_rnd / len(ok(gr)) if ok(gr) else 0) * rest) / N if N else 0
    kc = collections.Counter(kinds.get(f"base:{a}", "?") for a, _ in g)
    unpriced_addrs = sum(1 for v in ok(g) if v["unpriced"])
    unpriced_pos = sum(v["unpriced"] for v in ok(g))
    k1_share = h_all / n_ok if n_ok else 0
    cc = load("crosscheck.json") if (DATA / "crosscheck.json").exists() else {"skipped": "not run"}
    L += ["K1 - who pays by EIP-3009 authorization on Base (x402 `exact` settles this way)",
          f"  window       blocks {k1['from']:,}..{k1['to']:,} = {utc(k1['from_ts'])} .. {utc(k1['to_ts'])} (30 days; K5 fallback not used)",
          f"  found        {s['k1']['logs']:,} AuthorizationUsed logs from {N:,} distinct authorizers (median {s['k1']['median_count']} each);",
          f"               the top {len(gt)} made {100 * s['k1']['top_logs'] / s['k1']['logs']:.1f}% of all authorizations",
          f"  second node  last 500 blocks of the window from publicnode vs Tenderly: "
          + (f"{cc['publicnode']:,} vs {cc['tenderly']:,} logs, identical by (tx, log index): {cc['identical']}" if "identical" in cc
             else f"skipped ({cc['skipped']})"),
          f"  sampled      {len(g)} ({len(gt)} top by count + {len(gr)} random, seed {SEED}); holdings read for {n_ok}",
          f"  >= $1,000 non-stable   {pct(h_all, n_ok)}   [top {pct(h_top, len(ok(gt)))}; random {pct(h_rnd, len(ok(gr)))},"
          f" 95% CI {100 * lo:.1f}-{100 * hi:.1f}%]",
          f"  K1 threshold <10%: {'TRIPS' if k1_share < 0.10 else 'does not trip'}",
          f"  population-weighted estimate (top stratum exact + random stratum for the other {rest:,}): {100 * est:.2f}%",
          f"  median non-stable value  {usd(statistics.median([v['nonstable'] for v in ok(g)]) if ok(g) else 0)}"
          f" (top {usd(statistics.median([v['nonstable'] for v in ok(gt)]) if ok(gt) else 0)},"
          f" random {usd(statistics.median([v['nonstable'] for v in ok(gr)]) if ok(gr) else 0)})",
          f"  payer accounts  EOA {kc['eoa']}, EIP-7702-delegated EOA {kc['7702']}, contract {kc['contract']}"
          f" (code size > 0: {kc['7702'] + kc['contract']})",
          f"  unpriced     {unpriced_addrs} sampled payers hold at least one token without an exchange rate ({unpriced_pos} positions)",
          f"  robustness   native ETH alone >= $1,000: {pct(hits(g, 'eth'), n_ok)};"
          f" without unlisted ~$1 tokens (rate 0.98-1.02): {pct(sum(1 for v in ok(g) if v['nonstable'] - v['stable_like'] >= MIN_USD), n_ok)};"
          f" without tokens Blockscout flags: {pct(sum(1 for v in ok(g) if v['nonstable'] - v['flagged'] >= MIN_USD), n_ok)}",
          ""]
    k2_hit_addrs, k2_lines, est_lo, est_hi = set(), [], 0.0, 0.0
    for chain, name in (("base", "Base"), ("eth", "Ethereum")):
        o = load(f"k2_owners_{chain}.json")
        g2, t2, r2 = group("k2", chain), group("k2", chain, "top"), group("k2", chain, "random")
        ht, hr = hits(t2), hits(r2)
        k2_hit_addrs |= {a for a, v in g2 if v and v["nonstable"] >= MIN_USD}
        rest2 = s["k2_" + chain]["distinct"] - len(t2)
        p = hr / len(ok(r2)) if ok(r2) else 0
        lo2, hi2 = wilson(hr, len(ok(r2)))
        kc2 = collections.Counter(kinds.get(f"{chain}:{a}", "?") for a, _ in g2)
        est_lo, est_hi = est_lo + ht + lo2 * rest2, est_hi + ht + hi2 * rest2
        k2_lines += [f"  {name:<9}  {o['agents']:,} agents (ownerOf over ids 0..{o['last_id'] + 500:,} at blocks {o['blocks'][0]:,}..{o['blocks'][1]:,};"
                     f" newest Registered on Blockscout: {o['newest_registered_blockscout']}), {s['k2_' + chain]['distinct']:,} distinct owners"
                     f" ({s['k2_' + chain]['burned']} agents sent to a burn address left out);"
                     f" top {len(t2)} hold {100 * s['k2_' + chain]['top_agents'] / o['agents']:.1f}% of agents",
                     f"             sampled {len(g2)} ({len(t2)} top by agents + {len(r2)} random), holdings read for {len(ok(g2))}:"
                     f" >= $1,000 non-stable top {pct(ht, len(ok(t2)))}, random {pct(hr, len(ok(r2)))};"
                     f" median {usd(statistics.median([v['nonstable'] for v in ok(g2)]) if ok(g2) else 0)}",
                     f"             estimate for all owners: {ht + p * rest2:,.0f} (95% CI {ht + lo2 * rest2:,.0f}-{ht + hi2 * rest2:,.0f});"
                     f" accounts EOA {kc2['eoa']}, 7702 {kc2['7702']}, contract {kc2['contract']};"
                     f" unpriced in {sum(1 for v in ok(g2) if v['unpriced'])}"]
    L += ["K2 - who owns the ERC-8004 agents (IdentityRegistry 0x8004A169...a432, verified in erc-8004/erc-8004-contracts README)"] + k2_lines
    lb = len(k2_hit_addrs)
    fp = load("fullpop.json") if (DATA / "fullpop.json").exists() else None
    if lb >= 30:
        verdict = f"does not trip: {lb} owners counted directly, a lower bound"
    elif fp and fp["k2_union_ge_1000"] >= 30:
        verdict = f"does not trip: {fp['k2_union_ge_1000']} owners counted over every owner (ETH + top tokens), a lower bound"
    elif fp:
        verdict = (f"TRIPS on the nearest measurable count, a lower bound: {fp['k2_union_ge_1000']} of {fp['k2_union_n']:,} owners"
                   f" hold >= $1,000 in ETH + the {TOKENS_N} most-held tokens; {lb} of the sampled owners with every token; an every-token"
                   f" count of every owner is not measurable on keyless endpoints (sample-based 95% range {est_lo:,.0f}-{est_hi:,.0f})")
    elif est_lo >= 30:
        verdict = f"does not trip on the estimate (95% lower bound {est_lo:,.0f}); {lb} counted directly"
    elif est_hi < 30:
        verdict = "TRIPS"
    else:
        verdict = f"undetermined: {lb} counted directly, estimate 95% {est_lo:,.0f}-{est_hi:,.0f}"
    L += [f"  K2 count in the samples (distinct addresses, each on its registration chain): {lb} - a lower bound for all owners",
          f"  K2 threshold <30 owners: {verdict}",
          ""]
    if fp:
        L += [f"Every payer and every agent owner, not a sample (nearest measurable thing to K2's count; a LOWER bound: native ETH",
              f"  + the {TOKENS_N} priced non-stable tokens most held in that chain's samples, via Multicall3; read {fp['read_at']})"]
        for chain in ("base", "eth"):
            t = fp["tokens"][chain]
            L.append(f"  {chain:<5} counted: ETH at ${t['eth_rate']:,.0f}, " + ", ".join(x["symbol"] for x in t["tokens"]))
        for key, name in (("k1", "K1 payers (Base)"), ("k2_base", "K2 owners (Base)"), ("k2_eth", "K2 owners (Ethereum)")):
            q = fp["pops"][key]
            L.append(f"  {name:<21} {q['n']:>7,}: >= $1,000 {q['ge_1000']:,} ({100 * q['ge_1000'] / q['n']:.2f}%; ETH alone"
                     f" {q['ge_1000_eth_only']:,}); >= $100 {q['ge_100']:,}; under $0.01 {q['zero']:,}; median {usd(q['median'])};"
                     f" 99th percentile {usd(q['p99'])}")
        L += [f"  K2 owners on either chain with >= $1,000: {fp['k2_union_ge_1000']} of {fp['k2_union_n']:,}", ""]
    all_addrs = sorted({a for _, _, _, a in sampled(s)})
    chk = [a for a in all_addrs if a in hlr]
    pos = {a for a in chk if hlr[a]["positions"]}
    agt = {a for a in chk if hlr[a]["agents"]}
    k1set = set(s["k1"]["top"] + s["k1"]["random"])
    k2set = {a for x, _, _, a in sampled(s) if x == "k2"}
    eith = pos | agt
    L += ["K3 - already on Hyperliquid (same address)",
          f"  checked      {len(chk)} of {len(all_addrs)} distinct sampled addresses (K1 sample + K2 samples)",
          f"  open position {len(pos)}; API wallet {len(agt)}; either {pct(len(eith), len(chk))}",
          f"  by set       K1 payers {pct(len(eith & k1set), len([a for a in chk if a in k1set]))};"
          f" K2 owners {pct(len(eith & k2set), len([a for a in chk if a in k2set]))}",
          f"  (secondary)  Hyperliquid account value > 0: {sum(1 for a in chk if hlr[a]['account_value'] > 0)}",
          f"  K3 reading: {'> 10% - they hedge with other tools' if len(chk) and len(eith) / len(chk) > 0.10 else ('0 - nobody hedges' if not eith else 'between 0 and 10%')}",
          ""]
    for title, keys in (("K1 sample", [("k1", "base")]), ("K2 samples", [("k2", "base"), ("k2", "eth")])):
        holders = collections.Counter()
        for st, ch in keys:
            for _, v in group(st, ch):
                for label, x in (v or {}).get("held", {}).items():
                    if x >= 1:
                        holders[label] += 1
        L.append(f"Top 10 non-stable tokens held (priced value >= $1), by number of holders - {title}")
        L += [f"  {n:>4}  {label}" for label, n in holders.most_common(10)]
        L.append("")
    if (DATA / "bazaar.json").exists():
        bz = load("bazaar.json")
        res = list(bz["items"].values())
        classes = [{asset_class(a) for a in r["accepts"]} for r in res]
        cls = collections.Counter(c for cs in classes for c in cs)
        pairs = collections.Counter((a["network"], a["name"] or a["asset"]) for r in res for a in r["accepts"])
        sellers = {(a["network"], str(a["payTo"]).lower()) for r in res for a in r["accepts"] if a.get("payTo")}
        L += [f"x402 Bazaar (not a K; the earn side): {len(res):,} resources listed (pagination total {bz['total']:,}), read {bz['read_at']},"
              f" {len(sellers):,} distinct (network, payTo) sellers",
              "  resources accepting at least one asset of each class: "
              + "; ".join(f"{k} {pct(v, len(res))}" for k, v in cls.most_common()),
              f"  resources that do not accept USDC at all: {sum(1 for cs in classes if 'USDC' not in cs):,};"
              f" that accept a volatile asset as one option: {sum(1 for cs in classes if 'volatile' in cs):,}",
              "  top 15 (network, asset) by accepts:"]
        L += [f"  {n:>6}  {net}  {name}" for (net, name), n in pairs.most_common(15)]
        L.append("")
    inc = collections.Counter()
    p = DATA / "incidents.log"
    if p.exists():
        for line in p.read_text().splitlines():
            parts = line.split("\t")
            if len(parts) >= 3:
                inc[(parts[1], parts[2])] += 1
    L.append("Rate-limit and error incidents during the run (host, kind, count; every one was retried)")
    L += [f"  {h}  {k}  {n}" for (h, k), n in sorted(inc.items())] or ["  none"]
    L.append(f"Holdings not read: {sum(1 for r in hold.values() if r['error'])} token lists failed, "
             f"{sum(1 for r in hold.values() if not r['found'] or r.get('blockscout_404'))} addresses unknown to Blockscout; ETH balance read from the chain where"
             f" Blockscout had none: {sum(1 for r in hold.values() if r.get('coin_balance_src') == 'rpc')} addresses,"
             f" {sum(1 for r in hold.values() if r.get('coin_balance_src') == 'rpc' and int(r['coin_balance']) > 0)} of them non-zero")
    out = ROOT / "proof" / f"exposure-census-{time.strftime('%Y-%m-%d')}.txt"
    out.write_text("\n".join(L) + "\n")
    print("\n".join(L))
    print(f"\nwrote {out.relative_to(ROOT)} and {(DATA / 'addresses.tsv').relative_to(ROOT)}")


# ---------------------------------------------------------------- checks
def selftest():
    rec = {"coin_balance": str(10 ** 18), "eth_rate": "2000", "tokens": [
        {"type": "ERC-20", "symbol": "USDC", "decimals": "6", "rate": "1", "value": "5000000000"},
        {"type": "ERC-20", "symbol": "usds", "decimals": "18", "rate": "1", "value": str(10 ** 21)},
        {"type": "ERC-20", "symbol": "SPAM", "decimals": "18", "rate": None, "value": str(10 ** 24)},
        {"type": "ERC-20", "symbol": "WETH", "name": "Wrapped Ether", "decimals": "18", "rate": "2000", "value": str(5 * 10 ** 17)},
        {"type": "ERC-20", "symbol": "USD+", "name": "x", "decimals": "6", "rate": "1.0", "value": "3000000", "reputation": "scam"},
        {"type": "ERC-721", "symbol": "AGENT", "decimals": None, "rate": None, "value": "1"}]}
    v = value(rec)
    assert abs(v["nonstable"] - 3003.0) < 1e-6 and v["unpriced"] == 1 and v["eth"] == 2000.0 and abs(v["stable_like"] - 3) < 1e-9, v
    assert abs(v["flagged"] - 3) < 1e-9, v
    assert value({"coin_balance": None, "eth_rate": None, "tokens": []}, 2000)["nonstable"] == 0
    c = collections.Counter({"0xa": 5, "0xb": 5, "0xc": 1, "0xd": 1, "0xe": 1, "0xf": 1})
    top, rnd = rank_and_draw(c, seed=SEED, top_n=2, random_n=2)
    assert top == ["0xa", "0xb"] and len(set(rnd)) == 2 and not set(rnd) & set(top)
    assert rank_and_draw(c, seed=SEED, top_n=2, random_n=2) == (top, rnd)
    assert rank_and_draw(c, top_n=2, random_n=10)[1] and len(rank_and_draw(c, top_n=2, random_n=10)[1]) == 4
    lo, hi = wilson(0, 10)
    assert lo == 0 and abs(hi - 0.2775) < 1e-3 and wilson(5, 10)[0] < 0.5 < wilson(5, 10)[1]
    assert [code_kind(x) for x in ("0x", None, "0xef0100" + "ab" * 20, "0x6080")] == ["eoa", "eoa", "7702", "contract"]
    assert [asset_class(a) for a in ({"name": "USD Coin"}, {"name": "GatewayWalletBatched"}, {"asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"},
            {"name": "USD\u20ae0"}, {"name": "World Liberty Financial USD"}, {"name": "Peso Mexicano"}, {"name": "Swole Doge"},
            {"asset": "XRP"}, {"asset": "524C555344000000000000000000000000000000"}, {"asset": "CCW67"})] == \
        ["USDC", "USDC", "USDC", "other USD stablecoin", "other USD stablecoin", "non-USD stablecoin", "volatile", "volatile",
         "other USD stablecoin", "unnamed asset id"]
    for sig, want in SIGS.items():  # the constants above, recomputed by a node
        got = rpc("https://base-rpc.publicnode.com", "web3_sha3", ["0x" + sig.encode().hex()])
        assert got.startswith(want), (sig, got, want)
    print("selftest ok")


def run_all():
    selftest()
    ts = [threading.Thread(target=f) for f in (k1_logs, k2_owners)]
    [t.start() for t in ts]
    [t.join() for t in ts]
    sample()
    code()
    ts = [threading.Thread(target=f) for f in (holdings, hl, bazaar)]
    [t.start() for t in ts]
    [t.join() for t in ts]
    balances()
    fullpop()
    report()


if __name__ == "__main__":
    DATA.mkdir(parents=True, exist_ok=True)
    steps = {"all": run_all, "k1-logs": k1_logs, "k2-owners": k2_owners, "sample": sample, "code": code,
             "holdings": holdings, "balances": balances, "fullpop": fullpop, "hl": hl, "bazaar": bazaar, "crosscheck": crosscheck,
             "report": report, "selftest": selftest}
    if len(sys.argv) != 2 or sys.argv[1] not in steps:
        sys.exit(__doc__)
    steps[sys.argv[1]]()
