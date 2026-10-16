#!/usr/bin/env python3
"""What the hedge rule as coded would have done over the last 90 days for real holders.

Research, not product (docs/109 section 4 and 9.3). Python stdlib only; public, keyless sources only.

Reproduce (from the repo root):

  S=scripts/research/hedge_backtest.py
  python3 $S selftest                                                       # the rule's port against crates/hedge/tests/safety.rs cases
  python3 $S holdings arbitrum runtime/research/treasury-scan/safes.json S  # today's balances of the 133 Safes (~5 min)
  T=runtime/research/exposure-census/addresses.tsv                          # S7's census (docs/109 K1/K2)
  python3 $S holdings base $T C; python3 $S holdings ethereum $T C           # its rows with nonstable_usd >= $1,000, per chain
  python3 $S prices S [C]                                                   # Hyperliquid candles, funding, meta, fees; DefiLlama 1 h paths
  python3 $S run S [C] > proof/hedge-backtest-2026-10-16.txt                # tables with ids only; raw per address in runtime/
  python3 $S crosscheck S 12 >> proof/hedge-backtest-2026-10-16.txt         # the 12 largest holders, valued again by Blockscout
  python3 $S windows >> proof/hedge-backtest-2026-10-16.txt                  # one-coin hedges over every 90-day window since 2024-09

Sources. Holdings: Multicall3 `balanceOf` / `getEthBalance` on the chain's public node, over every token on CoinGecko's
token list for the chain (Blockscout's Cloudflare rule refuses most clients, docs/60 P-164; exact balances). Prices now:
DefiLlama /prices/current at confidence >= 0.9, else Blockscout's exchange_rate (S7's header passes its Cloudflare rule),
else the token is dropped as unpriceable. Paths: Hyperliquid
mainnet `info` candleSnapshot 1 h for BTC, ETH, SOL, PAXG and fundingHistory (read only); other tokens: DefiLlama /chart
1 h (only where the token is worth >= $1,000 in some address; smaller ones are held at today's price). Raw data lands in
runtime/research/hedge-backtest/ (other people's addresses: not in the repository).

The rule, as crates/hedge/src/{policy,delta,safety}.rs and engine.rs::step_coin state it (HedgePolicy::default):
  target = -holding per coin (delta::target_hedge); trade only when |target - current| / |target| > 500 bps
  (delta::rebalance); every order passes safety::check on the whole account: leverage <= 2x, equity >= 4 x the venue's
  maintenance margin (tiered: notional / (2 x tier maxLeverage) less the tier deduction), equity judged 50 bps lower for
  an order that does not only reduce (engine.rs), an order over a limit shrunk to the largest safe size (floored at 8
  places), refused under 10 USDC; sizes floored to the market's szDecimals (hl::round_size).
  Margin = plain USDC from the address's stablecoins, at three levels, each capped at 50 % of them
  (HedgePolicy::max_margin_share_pct): min = the least the check accepts for the whole hedge (2x at entry); 1x = the
  hedged notional (policy.rs: "it never needs more than 1x"; the main table); max = the 50 % cap itself. An address
  with no stablecoins has no margin available and no hedge.
  Fills at the hourly close plus the venue's taker fee; funding paid or received every hour on the hourly close.
  Between trades nothing tops the margin up (the engine has no such step); the venue liquidates if equity falls under
  its maintenance margin at the hour's high.
Holdings are today's balances held unchanged for 90 days; ETH, BTC, SOL and gold wrappers (WETH, wstETH, WBTC, cbBTC,
XAUt0, ...) count as that coin at today's price ratio.
"""
import concurrent.futures as cf
import json
import math
import os
import statistics
import sys
import time
import urllib.request
from datetime import datetime, timezone
from decimal import ROUND_DOWN, Decimal

D = "runtime/research/hedge-backtest"
HL = "https://api.hyperliquid.xyz/info"
MC3 = "0xcA11bde05977b3631167028862bE2a173976CA11"  # Multicall3, same address on Arbitrum One, Base and Ethereum
SEL_BAL, SEL_ETH, SEL_AGG3 = "70a08231", "4d2301cc", "82ad56cb"  # balanceOf, getEthBalance, aggregate3 (cast sig)
CHAINS = {
    "arbitrum": {"rpc": "https://arb1.arbitrum.io/rpc", "cg": "arbitrum-one", "llama": "arbitrum"},
    "base": {"rpc": "https://base-rpc.publicnode.com", "cg": "base", "llama": "base"},
    "ethereum": {"rpc": "https://ethereum-rpc.publicnode.com", "cg": "ethereum", "llama": "ethereum"},
}
ALLOW = ("BTC", "ETH", "SOL", "PAXG")  # HedgePolicy::default().allowlist
POLICY = {"band_bps": 500, "max_leverage": 2, "k": 4, "min_notional": 10.0, "slippage_bps": 50, "margin_share": 0.50}
# docs/109 section 4 list, plus the bridged and wrapped spellings that hold the same dollar.
STABLES = {"USDC", "USDC.E", "USDBC", "USDT", "USDT0", "USD₮0", "DAI", "EURC", "USDE", "FRAX", "GHO", "PYUSD", "USDS"}
DAYS, MIN_NONSTABLE, STRESS, CHART_MIN_USD = 90, 1000.0, 0.30, 1000.0
UA = {"User-Agent": "curl/8.7.1", "Accept": "application/json"}  # arb1.arbitrum.io refuses urllib's default (docs/60 P-94)
BS_UA = {"User-Agent": "Mozilla/5.0 (compatible; tameion-research/0.1)", "Accept": "application/json"}  # Blockscout, P-164
BS_HOST = {"arbitrum": "arbitrum", "base": "base", "ethereum": "eth"}


def http(url, body=None, tries=6, ua=UA):
    data = json.dumps(body).encode() if body is not None else None
    hdr = dict(ua, **({"Content-Type": "application/json"} if data else {}))
    last = None
    for i in range(tries):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, data=data, headers=hdr), timeout=90) as r:
                return json.load(r)
        except Exception as e:  # noqa: BLE001 - public endpoints throttle; retry then fail loudly
            last = e
            time.sleep(2.0 * (i + 1))
    raise RuntimeError(f"{url}: {last}")


def rpc(chain, method, params, tries=6):
    for i in range(tries):
        r = http(CHAINS[chain]["rpc"], {"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
        if "error" not in r:
            return r["result"]
        time.sleep(2.0 * (i + 1))
    raise RuntimeError(f"{chain} {method}: {r['error']}")


def word(n):
    return n.to_bytes(32, "big")


def aggregate3(chain, calls):
    """Multicall3.aggregate3 with allowFailure on every call: [(target, calldata)] -> [bytes or None]."""
    tuples = []
    for target, data in calls:
        tuples.append(bytes(12) + bytes.fromhex(target[2:]) + word(1) + word(96) + word(len(data)) + data + bytes(-len(data) % 32))
    offs, o = [], 32 * len(tuples)
    for t in tuples:
        offs.append(o)
        o += len(t)
    data = bytes.fromhex(SEL_AGG3) + word(32) + word(len(tuples)) + b"".join(word(x) for x in offs) + b"".join(tuples)
    ret = bytes.fromhex(rpc(chain, "eth_call", [{"to": MC3, "data": "0x" + data.hex()}, "latest"])[2:])
    w = lambda p: int.from_bytes(ret[p:p + 32], "big")  # noqa: E731
    base = w(0)
    out = []
    for i in range(w(base)):
        t = base + 32 + w(base + 32 + 32 * i)
        b = t + w(t + 32)
        out.append(ret[b + 32:b + 32 + w(b)] if w(t) else None)
    if len(out) != len(calls):
        raise RuntimeError(f"aggregate3 returned {len(out)} results for {len(calls)} calls")
    return out


def read_addrs(src, chain):
    """[(address, set, S7's nonstable_usd)]: a JSON list of {hash} (docs/21 Safes), or S7's addresses.tsv rows on this chain
    with nonstable_usd >= $1,000 (columns set, chain, address, nonstable_usd; S7 writes the chain as base / eth)."""
    if src.endswith(".json"):
        return [(x["hash"], "", None) for x in json.load(open(src))]
    names = {"ethereum": {"ethereum", "eth"}}.get(chain, {chain})
    rows = [r.rstrip("\n").split("\t") for r in open(src) if r.strip() and not r.startswith("#")]
    head = [h.strip().lower() for h in rows[0]]
    col = {h: i for i, h in enumerate(head)}
    out, seen = [], set()
    for r in rows[1:]:
        if r[col["chain"]].strip().lower() not in names or float(r[col["nonstable_usd"]] or 0) < MIN_NONSTABLE:
            continue
        a = r[col["address"]].strip()
        if a.startswith("0x") and a.lower() not in seen:
            seen.add(a.lower())
            out.append((a, r[col["set"]].strip() if "set" in col else "", float(r[col["nonstable_usd"]])))
    return out


def llama_now(keys):
    out = {}
    for i in range(0, len(keys), 60):
        d = http("https://coins.llama.fi/prices/current/" + ",".join(keys[i:i + 60]))
        out.update({k.lower(): v for k, v in d.get("coins", {}).items()})
    return out


def cmd_holdings(chain, src, label):
    listed = read_addrs(src, chain)
    addrs = [a for a, _, _ in listed]
    toks = {t["address"].lower(): t for t in http(f"https://tokens.coingecko.com/{CHAINS[chain]['cg']}/all.json")["tokens"]}
    pairs = [(a, None) for a in addrs] + [(a, t) for t in toks for a in addrs]
    calls = [(MC3, bytes.fromhex(SEL_ETH) + bytes(12) + bytes.fromhex(a[2:])) if t is None
             else (t, bytes.fromhex(SEL_BAL) + bytes(12) + bytes.fromhex(a[2:])) for a, t in pairs]
    chunks = [range(i, min(i + 400, len(calls))) for i in range(0, len(calls), 400)]
    raw = [None] * len(calls)
    t0 = time.time()
    with cf.ThreadPoolExecutor(6) as ex:
        for rng, res in zip(chunks, ex.map(lambda r: aggregate3(chain, [calls[i] for i in r]), chunks)):
            for i, b in zip(rng, res):
                raw[i] = int.from_bytes(b[:32], "big") if b and len(b) >= 32 else 0
    print(f"{len(calls)} balance reads in {len(chunks)} multicalls, {time.time() - t0:.0f} s", file=sys.stderr)
    held = {}
    for (a, t), v in zip(pairs, raw):
        if v:
            held.setdefault(a, {})[t or "native"] = v
    lk = CHAINS[chain]["llama"]
    keys = sorted({f"{lk}:{t}" for h in held.values() for t in h if t != "native"}) + ["coingecko:ethereum"]
    px = llama_now(keys)
    path = f"{D}/holdings-{label}.json"
    kept = [h for h in json.load(open(path))["holders"] if h["chain"] != chain] if os.path.exists(path) else []
    out = []
    for n, (a, aset, s7usd) in enumerate(listed, len(kept) + 1):
        rows = []
        for t, v in held.get(a, {}).items():
            p = px.get("coingecko:ethereum") if t == "native" else px.get(f"{lk}:{t}")
            dec = 18 if t == "native" else int(toks[t].get("decimals") or 18)
            sym = "ETH" if t == "native" else toks[t]["symbol"]
            ok = bool(p) and p.get("confidence", 0) >= 0.9
            rows.append({"token": t, "symbol": sym, "qty": v / 10 ** dec, "price": p["price"] if ok else None,
                         "confidence": p.get("confidence") if p else None})
        if any(r["price"] is None for r in rows):  # DefiLlama has no confident price: take Blockscout's exchange_rate
            try:
                bal = http(f"https://{BS_HOST[chain]}.blockscout.com/api/v2/addresses/{a}/token-balances", tries=3, ua=BS_UA)
            except RuntimeError as e:  # best effort: the token stays unpriced and is counted as such
                print(f"blockscout fallback failed for {label}{n:03d}: {str(e)[-60:]}", file=sys.stderr)
                bal = []
            rate = {t["token"]["address_hash"].lower(): t["token"].get("exchange_rate") for t in bal}
            for r in rows:
                if r["price"] is None and rate.get(r["token"]):
                    r["price"], r["price_source"] = float(rate[r["token"]]), "blockscout"
        out.append({"id": f"{label}{n:03d}", "address": a, "chain": chain, "set": aset, "s7_nonstable_usd": s7usd,
                    "rows": rows})
    json.dump({"source": src, "at": int(time.time()), "holders": kept + out}, open(path, "w"))
    print(f"{path}: {len(out)} {chain} addresses ({len(toks)} listed tokens), {sum(1 for h in out if h['rows'])} with a balance",
          file=sys.stderr)


def classify(sym, price, marks, perps):
    """stable | hedge:<coin> | perp (a Hyperliquid perp exists, not on the allowlist) | noperp."""
    s = sym.upper()
    if s in STABLES or ("USD" in s and 0.95 <= price <= 1.25):
        return "stable"
    for coin, tag, lo, hi in (("ETH", "ETH", 0.9, 1.3), ("BTC", "BTC", 0.9, 1.1), ("SOL", "SOL", 0.9, 1.3),
                              ("PAXG", "PAXG", 0.9, 1.1), ("PAXG", "XAU", 0.9, 1.1)):
        if tag in s and lo <= price / marks[coin] <= hi:
            return "hedge:" + coin
    return "perp" if (s in perps or "K" + s in perps) else "noperp"


def positions(h, marks, perps):
    """One address: allowlisted coin quantities, the rest as (key, qty, price, class), stable dollars, unpriced count."""
    q, other, stables, unpriced = {}, [], 0.0, 0
    for r in h["rows"]:
        if r["price"] is None:
            unpriced += 1
            continue
        usd = r["qty"] * r["price"]
        c = classify(r["symbol"], r["price"], marks, perps)
        if c == "stable":
            stables += usd
        elif c.startswith("hedge:"):
            coin = c[6:]
            q[coin] = q.get(coin, 0.0) + usd / marks[coin]
        else:
            other.append((r["token"], r["qty"], r["price"], c, r["symbol"]))
    return q, other, stables, unpriced


def hl_pages(body_fn, start_ms, end_ms):
    out, t = [], start_ms
    while t < end_ms:
        rows = http(HL, body_fn(t))
        if not rows:
            break
        out += rows
        t = rows[-1]["time"] + 1
        if len(rows) < 500:
            break
        time.sleep(0.5)
    return out


def cmd_prices(labels):
    end = int(time.time()) // 3600 * 3600
    start = end - DAYS * 86400
    meta = http(HL, {"type": "meta"})
    ctx = http(HL, {"type": "metaAndAssetCtxs"})
    fees = http(HL, {"type": "userFees", "user": "0x0000000000000000000000000000000000000001"})
    marks = {u["name"]: float(c["markPx"]) for u, c in zip(ctx[0]["universe"], ctx[1])}
    candles = {c: http(HL, {"type": "candleSnapshot", "req": {"coin": c, "interval": "1h", "startTime": start * 1000,
                                                              "endTime": end * 1000}}) for c in ALLOW}
    funding = {c: hl_pages(lambda t, c=c: {"type": "fundingHistory", "coin": c, "startTime": t}, start * 1000, end * 1000)
               for c in ALLOW}
    json.dump({"start": start, "end": end, "meta": meta, "marks": marks, "taker": float(fees["userCrossRate"]),
               "candles": candles, "funding": funding}, open(f"{D}/prices.json", "w"))
    perps = {u["name"].upper() for u in meta["universe"] if not u.get("isDelisted")}
    exact = {u["name"].upper(): u["name"] for u in meta["universe"]}  # the venue's spelling (kPEPE), for its candles
    for label in labels:
        hold = json.load(open(f"{D}/holdings-{label}.json"))
        need, perp_sym = set(), {}
        for h in hold["holders"]:
            for tok, qty, px, cls, sym in positions(h, marks, perps)[1]:
                if qty * px >= CHART_MIN_USD:
                    need.add(f"{CHAINS[h['chain']]['llama']}:{tok}")
                    if cls == "perp":
                        perp_sym[f"{CHAINS[h['chain']]['llama']}:{tok}"] = exact.get(sym.upper()) or exact["K" + sym.upper()]
        charts = {}

        def chart(key):
            pts = []
            for s in range(start, end, 500 * 3600):
                span = min(500, (end - s) // 3600 + 1)
                d = http(f"https://coins.llama.fi/chart/{key}?start={s}&span={span}&period=1h")
                pts += [(p["timestamp"], p["price"]) for v in d.get("coins", {}).values() for p in v["prices"]]
            return key, sorted(set(pts))

        with cf.ThreadPoolExecutor(4) as ex:
            for key, pts in ex.map(chart, sorted(need)):
                charts[key] = pts
        for key, coin in perp_sym.items():  # the venue's own candles for the same token, to measure the basis
            hl_c = http(HL, {"type": "candleSnapshot", "req": {"coin": coin, "interval": "1h", "startTime": start * 1000,
                                                               "endTime": end * 1000}})
            charts["hl:" + key] = [coin, [(int(c["t"]), float(c["c"])) for c in hl_c]]
        json.dump(charts, open(f"{D}/charts-{label}.json", "w"))
        print(f"{label}: {len(need)} token paths from DefiLlama, {sum(1 for v in charts.values() if v)} non-empty", file=sys.stderr)


# ---- the rule (ports of crates/hedge/src/safety.rs and delta.rs; floats, sizes floored like the Rust) ----

def floor_dp(x, dp):
    return float(Decimal(repr(x)).quantize(Decimal(1).scaleb(-dp), rounding=ROUND_DOWN))


def tier_lev(tiers, notional, fallback):
    best = None
    for lb, lev in tiers:
        if lb <= notional and (best is None or lb > best[0]):
            best = (lb, lev)
    return max(best[1] if best else fallback, 1)


def venue_mm(tiers, notional, fallback):
    rate = lambda lev: 1.0 / (2 * max(lev, 1))  # noqa: E731
    mm, deduction, prev = notional * rate(fallback), 0.0, None
    for lb, lev in sorted(tiers):
        if lb > notional:
            break
        if prev is not None:
            deduction += lb * (rate(lev) - rate(prev))
        prev = lev
        mm = notional * rate(lev) - deduction
    return mm


def reduces_only(cur, delta):
    after = cur + delta
    return abs(after) < abs(cur) and (after == 0 or (after < 0) == (cur < 0))


def check(cur, delta, px, value, mm_used, total_notional, tiers, fallback, pol=POLICY):
    """safety::check -> ("allow" | "reduce" | "refuse", limits or reason)."""
    if px <= 0:
        return "refuse", "BadPrice"
    if value <= 0:
        return "refuse", "NoCollateral"
    after = cur + delta
    n_after = abs(after) * px
    two_l = 2 * tier_lev(tiers, n_after, fallback)
    k, max_lev = pol["k"], pol["max_leverage"]
    cur_n = abs(cur) * px
    o_n = max(total_notional - cur_n, 0.0)
    o_mm = max(mm_used - venue_mm(tiers, cur_n, fallback), 0.0)

    def limits(d, a, n):
        return {"size_delta": d, "size_after": a, "notional_after": n, "leverage_after": (o_n + n) / value,
                "required_equity": o_mm * k + n * k / two_l}

    lim = limits(delta, after, n_after)
    if after == 0:
        return "allow", lim
    if lim["leverage_after"] <= max_lev and value >= lim["required_equity"]:
        return ("refuse", "BelowMinNotional") if n_after < pol["min_notional"] else ("allow", lim)
    by_lev = max(value * max_lev - o_n, 0.0)
    by_buf = max(value - o_mm * k, 0.0) * two_l / k
    max_n, why = (by_lev, "Leverage") if by_lev <= by_buf else (by_buf, "MaintenanceBuffer")
    max_size = floor_dp(max_n / px, 8)
    red_n = max_size * px
    if red_n < pol["min_notional"]:
        return ("allow", lim) if reduces_only(cur, delta) else ("refuse", "BelowMinNotional")
    red_after = -max_size if after < 0 else max_size
    out = limits(red_after - cur, red_after, red_n)
    out["why"] = why
    return "reduce", out


class Market:
    def __init__(self, meta, coin):
        u = next(x for x in meta["universe"] if x["name"] == coin)
        tables = {i: t for i, t in meta.get("marginTables", [])}
        tid = u.get("marginTableId")
        self.szd, self.fallback = int(u["szDecimals"]), int(u["maxLeverage"])
        # The table meta.marginTables lists for this market (BTC 56, ETH 55, SOL 54, PAXG 51 today); a market whose id is not
        # listed is one tier at its own maxLeverage, which is what safety::maintenance_leverage falls back to.
        if tid in tables:
            self.tiers = [(float(t["lowerBound"]), int(t["maxLeverage"])) for t in tables[tid]["marginTiers"]]
        else:
            self.tiers = [(0.0, self.fallback)]


def simulate(q, v_path, close, high, fund, mk, stables, taker, mode="min", pol=POLICY):
    """Hourly run of the rule for one address. q: coin -> quantity; v_path: holding value per hour (all non-stable)."""
    K = len(v_path)
    coins = sorted((c for c in q if abs(floor_dp(q[c], mk[c].szd)) * close[c][0] >= pol["min_notional"]),
                   key=lambda c: -q[c] * close[c][0])

    def run(margin, hours):
        st = {"cash": margin, "pos": {c: 0.0 for c in coins}, "trades": 0, "rebal": 0, "fees": 0.0, "funding": 0.0,
              "breach_close": 0, "breach_high": 0, "episodes": 0, "lev_over": 0, "liquidations": 0, "min_health": None,
              "h_path": [], "reduced": 0, "refused": 0, "venue_rejected": 0}
        in_breach = False
        for k in range(hours):
            pos = st["pos"]
            for c in coins:  # funding on the position held into this hour
                f = -pos[c] * close[c][k] * fund[c][k]
                st["cash"] += f
                st["funding"] += f
            if any(pos.values()):
                eq_hi = st["cash"] + sum(pos[c] * (high[c][k] if pos[c] < 0 else close[c][k]) for c in coins)
                mm_hi = sum(venue_mm(mk[c].tiers, abs(pos[c]) * high[c][k], mk[c].fallback) for c in coins)
                if eq_hi < mm_hi:  # the venue's liquidation, at the hour's worst price for a short
                    for c in coins:
                        if pos[c]:
                            st["cash"] += pos[c] * high[c][k] - abs(pos[c]) * high[c][k] * taker
                            pos[c] = 0.0
                    st["liquidations"] += 1
                elif eq_hi < pol["k"] * mm_hi:
                    st["breach_high"] += 1
            for c in coins:  # engine.rs::step_coin, in holding order, each judged on the account after the last
                target = -q[c]
                gap = target - pos[c]
                if gap == 0 or (target != 0 and abs(gap) / abs(target) * 1e4 <= pol["band_bps"]):
                    continue
                delta = floor_dp(gap, mk[c].szd)
                if delta == 0:
                    continue
                px = close[c][k]
                value = st["cash"] + sum(pos[x] * close[x][k] for x in coins)
                notional = sum(abs(pos[x]) * close[x][k] for x in coins)
                mm = sum(venue_mm(mk[x].tiers, abs(pos[x]) * close[x][k], mk[x].fallback) for x in coins)
                if not reduces_only(pos[c], delta):
                    value -= abs(delta) * px * pol["slippage_bps"] / 1e4
                verdict, lim = check(pos[c], delta, px, value, mm, notional, mk[c].tiers, mk[c].fallback, pol)
                if verdict == "refuse":
                    st["refused"] += 1
                    continue
                send = floor_dp(lim["size_delta"], mk[c].szd)
                if send == 0:
                    continue
                if abs(send) * px < pol["min_notional"]:  # "Hyperliquid rejects orders under 10 USDC of notional" (policy.rs)
                    st["venue_rejected"] += 1
                    continue
                st["reduced"] += verdict == "reduce"
                fee = abs(send) * px * taker
                st["cash"] -= send * px + fee
                st["fees"] += fee
                pos[c] = round(pos[c] + send, mk[c].szd)
                st["trades"] += 1
                st["rebal"] += k > 0
            eq = st["cash"] + sum(pos[c] * close[c][k] for c in coins)
            mm = sum(venue_mm(mk[c].tiers, abs(pos[c]) * close[c][k], mk[c].fallback) for c in coins)
            notional = sum(abs(pos[c]) * close[c][k] for c in coins)
            if mm > 0:
                h = eq / (pol["k"] * mm)
                st["min_health"] = h if st["min_health"] is None else min(st["min_health"], h)
                if h < 1:
                    st["breach_close"] += 1
                    st["episodes"] += not in_breach
                in_breach = h < 1
                st["lev_over"] += eq <= 0 or notional / eq > pol["max_leverage"]
            else:
                in_breach = False
            st["h_path"].append(v_path[k] + eq - margin)
            st["eq"], st["mm"], st["notional"] = eq, mm, notional
        return st

    full = {c: floor_dp(-q[c], mk[c].szd) for c in coins}
    if not coins:
        need = 0.0
    else:
        lo, hi = 0.0, 2.0 * sum(abs(full[c]) * close[c][0] for c in coins) + 100
        for _ in range(60):
            mid = (lo + hi) / 2
            s = run(mid, 1)
            if all(s["pos"][c] == full[c] for c in coins):
                hi = mid
            else:
                lo = mid
        need = math.ceil(hi * 100) / 100
    cap = pol["margin_share"] * stables
    hedged_start = sum(abs(full[c]) * close[c][0] for c in coins)
    # min: the least the safety check accepts for the whole hedge (2x at entry). 1x: margin = the hedged notional, the
    # point policy.rs names ("it never needs more than 1x"). max: the share rule's ceiling. All capped at 50 % of stables.
    margin = {"min": min(need, cap), "1x": min(max(need, hedged_start), cap), "max": cap}[mode]
    st = run(margin, K)
    st.update(margin_needed=need, margin_posted=margin, coins=coins, hedged_start=hedged_start,
              short_end=sum(abs(st["pos"][c]) * close[c][K - 1] for c in coins))
    return st


def mdd(path):
    peak, worst_pct, worst_usd = -math.inf, 0.0, 0.0
    for v in path:
        peak = max(peak, v)
        if peak > 0:
            worst_pct = max(worst_pct, (peak - v) / peak)
            worst_usd = max(worst_usd, peak - v)
    return worst_pct, worst_usd


def series(P, coin, key):
    by_t = {c["t"]: float(c[key]) for c in P["candles"][coin]}
    return by_t


def grid(P):
    ts = sorted(int(c["t"]) for c in P["candles"]["ETH"] if int(c["t"]) < P["end"] * 1000)  # complete hours only
    close, high, fund, miss = {}, {}, {}, {}
    for coin in ALLOW:
        cl, hi = series(P, coin, "c"), series(P, coin, "h")
        last_c = last_h = None
        close[coin], high[coin] = [], []
        for t in ts:
            last_c = cl.get(t, last_c)
            last_h = hi.get(t, last_h if last_h is not None else last_c)
            close[coin].append(last_c)
            high[coin].append(last_h)
        by_hour = {round(int(f["time"]) / 3_600_000): float(f["fundingRate"]) for f in P["funding"][coin]}
        fund[coin] = [by_hour.get((t + 3_600_000) // 3_600_000, 0.0) for t in ts]
        miss[coin] = sum(1 for t in ts if (t + 3_600_000) // 3_600_000 not in by_hour)
    return ts, close, high, fund, miss


def token_path(pts, ts, now_px):
    if not pts:
        return None
    out, j, last = [], 0, pts[0][1]
    for t in ts:
        end = (t + 3_600_000) / 1000
        while j < len(pts) and pts[j][0] <= end:
            last = pts[j][1]
            j += 1
        out.append(last)
    return out


def pct(xs, p):
    xs = sorted(xs)
    if not xs:
        return float("nan")
    if len(xs) == 1:
        return xs[0]
    return statistics.quantiles(xs, n=100, method="inclusive")[p - 1] if 1 <= p <= 99 else xs[-1]


def backtest(label, P, ts, close, high, fund, mk, marks, perps):
    hold = json.load(open(f"{D}/holdings-{label}.json"))
    charts_file = f"{D}/charts-{label}.json"
    charts = json.load(open(charts_file)) if os.path.exists(charts_file) else {}
    K = len(ts)
    res = []
    for h in hold["holders"]:
        lk = CHAINS[h["chain"]]["llama"]
        q, other, stables, unpriced = positions(h, marks, perps)
        v_path = [sum(q[c] * close[c][k] for c in q) for k in range(K)]
        flat_usd = 0.0
        cls_usd = {"perp": 0.0, "noperp": 0.0}
        for tok, qty, px, cls, _ in other:
            cls_usd[cls] += qty * px
            path = token_path(charts.get(f"{lk}:{tok}"), ts, px)
            if path is None:
                flat_usd += qty * px
                path = [px] * K
            for k in range(K):
                v_path[k] += qty * path[k]
        hedgeable_now = sum(q[c] * close[c][K - 1] for c in q)
        nonstable_now = hedgeable_now + cls_usd["perp"] + cls_usd["noperp"]
        row = {"id": h["id"], "chain": h["chain"], "set": h.get("set", ""), "s7_nonstable_usd": h.get("s7_nonstable_usd"),
               "nonstable_usd": nonstable_now,
               "stables_usd": stables, "unpriced_tokens": unpriced, "hedgeable_usd": hedgeable_now,
               "perp_not_allowlisted_usd": cls_usd["perp"], "no_perp_usd": cls_usd["noperp"], "held_flat_usd": flat_usd,
               "coins": {c: q[c] for c in q}}
        if nonstable_now < MIN_NONSTABLE:
            row["skipped"] = "under $1,000 non-stable"
            res.append(row)
            continue
        mu_pct, mu_usd = mdd(v_path)
        row.update({"v_start": v_path[0], "v_end": v_path[-1], "unhedged_mdd_pct": mu_pct, "unhedged_mdd_usd": mu_usd,
                    "unhedged_pnl_usd": v_path[-1] - v_path[0], "stress_unhedged_usd": STRESS * v_path[-1],
                    "no_margin": stables <= 0 and any(q.values()), "m": {}})
        for mode in ("min", "1x", "max"):
            st = simulate(q, v_path, close, high, fund, mk, stables, P["taker"], mode)
            mh_pct, mh_usd = mdd(st["h_path"])
            rally_mm = sum(venue_mm(mk[c].tiers, abs(st["pos"][c]) * close[c][K - 1] * 1.3, mk[c].fallback)
                           for c in st["coins"])
            row["hedged_start_usd"], row["margin_needed_usd"] = st["hedged_start"], st["margin_needed"]
            row["m"][mode] = {
                "margin_posted_usd": st["margin_posted"], "hedged_mdd_pct": mh_pct, "hedged_mdd_usd": mh_usd,
                "hedged_pnl_usd": st["h_path"][-1] - st["h_path"][0], "short_end_usd": st["short_end"],
                "stress_hedged_usd": STRESS * (v_path[-1] - st["short_end"]),
                "entry_trades": st["trades"] - st["rebal"], "rebalances": st["rebal"], "reduced": st["reduced"],
                "refused": st["refused"], "venue_rejected": st["venue_rejected"],
                "breach_hours_close": st["breach_close"], "breach_hours_high": st["breach_high"],
                "breach_episodes": st["episodes"], "min_health": st["min_health"], "lev_over_2x_hours": st["lev_over"],
                "liquidations": st["liquidations"], "funding_usd": st["funding"], "fees_usd": st["fees"],
                "health_after_30pct_rally": ((st["eq"] - 0.30 * st["notional"]) / (POLICY["k"] * rally_mm))
                if rally_mm > 0 else None,
            }
        res.append(row)
    json.dump(res, open(f"{D}/result-{label}.json", "w"), indent=1)
    return hold, res


def fmt_usd(x):
    if x != x:
        return "n/a"
    a = abs(x)
    s = f"{a / 1e6:.2f}M" if a >= 1e6 else f"{a / 1e3:.1f}k" if a >= 1e3 else f"{a:.0f}"
    return ("-" if x < 0 else "") + "$" + s


def table(rows, pops):
    print(f"\n{'metric':40} {'n':>4} {'p10':>10} {'p50':>10} {'p90':>10}")
    for name, key, f, pop in rows:
        if key is None:
            print(name)
            continue
        xs = [x for x in (key(r) for r in pops[pop]) if x is not None]
        cells = [f(pct(xs, p)) if xs else "-" for p in (10, 50, 90)]
        print(f"{name:40} {len(xs):>4} {cells[0]:>10} {cells[1]:>10} {cells[2]:>10}")


def report(label, res, title, basis=()):
    run = [r for r in res if "skipped" not in r]
    hedged = [r for r in run if r["m"]["1x"]["entry_trades"] > 0]
    limited = [r for r in hedged if r["m"]["1x"]["reduced"]]
    print(f"\n## {label}: {title} — {len(res)} addresses; {len(run)} with >= $1,000 non-stable today")
    if not run:
        return
    for s_ in sorted({(r["set"], r["chain"]) for r in run if r["set"]}):
        g = [r for r in run if (r["set"], r["chain"]) == s_]
        u, hh = sum(r["stress_unhedged_usd"] for r in g), sum(r["m"]["1x"]["stress_hedged_usd"] for r in g)
        print(f"set {s_[0]} on {s_[1]}: {len(g)} addresses, p50 non-stable {fmt_usd(pct([r['nonstable_usd'] for r in g], 50))}, "
              f"{sum(r['hedgeable_usd'] for r in g) / sum(r['nonstable_usd'] for r in g):.0%} of it on the allowlist, "
              f"summed loss at a 30 % fall {fmt_usd(u)} unhedged -> {fmt_usd(hh)} hedged at 1x, no stablecoins: "
              f"{sum(1 for r in g if r['no_margin'])}")
    print(f"with ETH/BTC/SOL/PAXG (or a wrapper) worth >= $10: {sum(1 for r in run if r['hedged_start_usd'] > 0)} | "
          f"hedge placed: {len(hedged)} | no stablecoins, so no margin available: {sum(1 for r in run if r['no_margin'])} | "
          f"stablecoins too few for a first order: "
          f"{sum(1 for r in run if r['hedged_start_usd'] > 0 and not r['no_margin'] and r not in hedged)} | "
          f"only tokens off the allowlist: {sum(1 for r in run if r['hedged_start_usd'] == 0)}")
    print(f"margin-limited at 1x (50 % of stablecoins < the hedge's margin; the check shrank orders): {len(limited)}"
          + "".join(f"; {r['id']} {r['m']['1x']['short_end_usd'] / max(r['hedgeable_usd'], 1):.0%} offset at the end, "
                    f"{r['m']['1x']['rebalances']} fills + {r['m']['1x']['venue_rejected']} sub-$10 orders after entry"
                    for r in limited))
    s7 = [(r["nonstable_usd"], r["s7_nonstable_usd"]) for r in res if r.get("s7_nonstable_usd")]
    if s7:
        d = [abs(m_ - o) / o for m_, o in s7]
        print(f"against S7's own valuation of the same {len(s7)} addresses (Blockscout): median difference "
              f"{statistics.median(d):.1%}; under $1,000 by this script's count: {sum(1 for m_, _ in s7 if m_ < MIN_NONSTABLE)}")
    tot = lambda k: sum(r[k] for r in run)  # noqa: E731
    print(f"non-stable today across them: {fmt_usd(tot('nonstable_usd'))} = on the allowlist {fmt_usd(tot('hedgeable_usd'))}"
          f" + a Hyperliquid perp exists but not allowlisted {fmt_usd(tot('perp_not_allowlisted_usd'))} + no Hyperliquid perp "
          f"{fmt_usd(tot('no_perp_usd'))}; of which held at today's price for lack of a path {fmt_usd(tot('held_flat_usd'))}")
    pct0 = lambda x: f"{x:.0%}"  # noqa: E731
    pct1 = lambda x: f"{x:.1%}"  # noqa: E731
    num = lambda x: f"{x:.0f}"  # noqa: E731
    one = lambda k: (lambda r: r["m"]["1x"][k])  # noqa: E731
    print("\nMain table: margin posted = 1x the hedged notional (capped at 50 % of stablecoins). Rows above the line: every"
          " address with >= $1,000 non-stable.")
    table([
        ("non-stable holding today", lambda r: r["nonstable_usd"], fmt_usd, "run"),
        ("share of it on the allowlist", lambda r: r["hedgeable_usd"] / r["nonstable_usd"], pct0, "run"),
        ("max drawdown 90 d, unhedged", lambda r: r["unhedged_mdd_pct"], pct1, "run"),
        ("max drawdown 90 d, hedged", one("hedged_mdd_pct"), pct1, "run"),
        ("max drawdown $, unhedged", lambda r: r["unhedged_mdd_usd"], fmt_usd, "run"),
        ("max drawdown $, hedged", one("hedged_mdd_usd"), fmt_usd, "run"),
        ("loss at a 30 % fall, unhedged", lambda r: r["stress_unhedged_usd"], fmt_usd, "run"),
        ("loss at a 30 % fall, hedged", one("stress_hedged_usd"), fmt_usd, "run"),
        ("  hedged, as % of the holding", lambda r: r["m"]["1x"]["stress_hedged_usd"] / r["v_end"], pct1, "run"),
        ("90-day change, unhedged", lambda r: r["unhedged_pnl_usd"], fmt_usd, "run"),
        ("90-day change, hedged", one("hedged_pnl_usd"), fmt_usd, "run"),
        ("-- below: the addresses where a hedge was placed --", None, None, None),
        ("margin needed (least the rule accepts)", lambda r: r["margin_needed_usd"], fmt_usd, "hedged"),
        ("  as % of the address's stablecoins", lambda r: r["margin_needed_usd"] / r["stables_usd"], pct1, "hedged"),
        ("margin posted at 1x", one("margin_posted_usd"), fmt_usd, "hedged"),
        ("rebalances after entry (filled)", one("rebalances"), num, "hedged"),
        ("  orders the venue would reject (< $10)", one("venue_rejected"), num, "hedged"),
        ("hours equity < 4 x MM, hourly close", one("breach_hours_close"), num, "hedged"),
        ("hours equity < 4 x MM, hourly high", one("breach_hours_high"), num, "hedged"),
        ("hours over 2x leverage, unchecked", one("lev_over_2x_hours"), num, "hedged"),
        ("lowest health (equity / 4 x MM)", one("min_health"), lambda x: f"{x:.2f}", "hedged"),
        ("funding received (+) / paid (-)", one("funding_usd"), fmt_usd, "hedged"),
        ("taker fees", one("fees_usd"), fmt_usd, "hedged"),
        ("health after a further 30 % rally", one("health_after_30pct_rally"), lambda x: f"{x:.2f}", "hedged"),
    ], {"run": run, "hedged": hedged})
    print(f"\nMargin sensitivity, the same {len(hedged)} hedged addresses (min = the least the safety check accepts, 2x at"
          " entry; 1x = margin equal to the hedged notional; max = 50 % of stablecoins, the share rule's ceiling):")
    print(f"{'margin':6} {'p50 posted':>11} {'within 4xMM once':>17} {'liquidated':>11} {'limited':>8} {'p50 rebal':>10} "
          f"{'p50 hedged MDD':>15} {'p50 30% loss':>13} {'p50 90-d change':>16}")
    for mode in ("min", "1x", "max"):
        m = [r["m"][mode] for r in hedged]
        print(f"{mode:6} {fmt_usd(pct([x['margin_posted_usd'] for x in m], 50)):>11} "
              f"{sum(1 for x in m if x['breach_hours_close']):>17} {sum(1 for x in m if x['liquidations']):>11} "
              f"{sum(1 for x in m if x['reduced']):>8} "
              f"{pct([x['rebalances'] for x in m], 50):>10.0f} {pct([x['hedged_mdd_pct'] for x in m], 50):>15.1%} "
              f"{fmt_usd(pct([x['stress_hedged_usd'] for x in m], 50)):>13} "
              f"{fmt_usd(pct([x['hedged_pnl_usd'] for x in m], 50)):>16}")
    if basis:
        print("Off-allowlist tokens with a Hyperliquid perp: their DefiLlama spot path against the perp's own 1 h closes "
              "(the basis a hedge of them would carry): " + "; ".join(basis))
    agg_u = sum(r["stress_unhedged_usd"] for r in run)
    agg = {mode: sum(r["m"][mode]["stress_hedged_usd"] for r in run) for mode in ("min", "1x", "max")}
    print(f"\nSummed over the {len(run)} addresses (dollar-weighted), loss at a 30 % fall: unhedged {fmt_usd(agg_u)}; hedged "
          + "; ".join(f"{m_} {fmt_usd(v)} ({1 - v / agg_u:.0%} less)" for m_, v in agg.items()))
    print(f"What-if, not the rule: with every token that has a Hyperliquid perp (ARB, GMX, AAVE, ...) on the allowlist and fully"
          f" offset, the summed loss would be {fmt_usd(STRESS * sum(r['no_perp_usd'] for r in run))} (the no-perp part only).")
    print("\nPer address (ids only), margin 1x unless marked. $: non-stable today | on the allowlist | stables ; max drawdown"
          " unhedged / hedged ; loss at a 30 % fall unhedged / hedged ; margin needed (min) ; rebalances ; hours < 4xMM"
          " close ; liquidated at min margin? ; funding")
    for r in res:
        if "skipped" in r:
            print(f"{r['id']}  {fmt_usd(r['nonstable_usd']):>9}  skipped: {r['skipped']}")
            continue
        m, mn = r["m"]["1x"], r["m"]["min"]
        print(f"{r['id']}  {fmt_usd(r['nonstable_usd']):>9} {fmt_usd(r['hedgeable_usd']):>9} {fmt_usd(r['stables_usd']):>9} ; "
              f"{r['unhedged_mdd_pct']:6.1%} {m['hedged_mdd_pct']:6.1%} ; {fmt_usd(r['stress_unhedged_usd']):>9} "
              f"{fmt_usd(m['stress_hedged_usd']):>9} ; {fmt_usd(r['margin_needed_usd']):>9}"
              f"{' (no margin)' if r['no_margin'] else ''} ; {m['rebalances']:4d} ; {m['breach_hours_close']:4d} ; "
              f"{'liquidated' if mn['liquidations'] else '-':>10} ; {fmt_usd(m['funding_usd']):>8}")


def basis_lines(label, ts):
    path = f"{D}/charts-{label}.json"
    charts = json.load(open(path)) if os.path.exists(path) else {}
    out = []
    for key, (coin, hl_c) in sorted((k[3:], v) for k, v in charts.items() if k.startswith("hl:")):
        spot = token_path(charts.get(key), ts, None)
        perp = dict(hl_c)
        d = sorted(abs(spot[k] / perp[t] - 1) for k, t in enumerate(ts) if spot and t in perp and perp[t] > 0)
        if d:
            out.append(f"{coin} median {statistics.median(d):.1%}, p95 {d[int(len(d) * 0.95)]:.1%}")
    return out


def cmd_run(labels):
    P = json.load(open(f"{D}/prices.json"))
    ts, close, high, fund, miss = grid(P)
    mk = {c: Market(P["meta"], c) for c in ALLOW}
    perps = {u["name"].upper() for u in P["meta"]["universe"] if not u.get("isDelisted")}
    marks = P["marks"]
    iso = lambda t: datetime.fromtimestamp(t / 1000, timezone.utc).strftime("%Y-%m-%d %H:%MZ")  # noqa: E731
    now = datetime.now(timezone.utc).astimezone().strftime("%Y-%m-%d %H:%M %z")
    print(f"# Hedge backtest — what the hedge rule as coded would have done over the last {DAYS} days for real holders")
    print(f"# Run {now}. Script: scripts/research/hedge_backtest.py (stdlib; its header reproduces this file).")
    print("# Third-party addresses are not printed: ids only (S = docs/21 Safes, Arbitrum One; C = S7 census, Base + Ethereum).")
    print(f"# Window: {iso(ts[0] + 3_600_000)} .. {iso(ts[-1] + 3_600_000)}, {len(ts)} hourly closes, Hyperliquid mainnet "
          f"candleSnapshot 1h (read only); funding from fundingHistory; taker fee {P['taker']:.5f} (userFees, base tier).")
    print("# Rule: HedgePolicy::default() — short = holding per coin, band 500 bps, leverage <= 2x, equity >= 4 x tiered MM,")
    print("#   50 bps slippage haircut on non-reducing orders, 10 USDC minimum (position and order), allowlist BTC ETH SOL PAXG.")
    print("#   Margin at three levels, each capped at 50 % of the address's stablecoins: min (the least the check accepts, 2x")
    print("#   at entry), 1x (the hedged notional; the main table), max (the cap). No top-up between orders (the engine has none).")
    print("\n## The four markets over the window")
    for c in ALLOW:
        cl = close[c]
        f = sum(fund[c])
        print(f"{c:5} {cl[0]:>10.2f} -> {cl[-1]:>10.2f} ({cl[-1] / cl[0] - 1:+.1%}); highest close {max(cl):.2f} "
              f"({max(cl) / cl[0] - 1:+.1%} from start); worst fall from a peak {mdd(cl)[0]:.1%}; funding to a short "
              f"{f:+.2%} over {DAYS} d ({f * 365 / DAYS:+.1%}/yr); maintenance tiers {mk[c].tiers} szDecimals {mk[c].szd}; "
              f"hours without a funding row {miss[c]}")
    names = {"S": "Safes of docs/21 (Arbitrum One, top-4,000 USDC holders)", "C": "S7 census addresses (Base + Ethereum)"}
    for label in labels:
        _, res = backtest(label, P, ts, close, high, fund, mk, marks, perps)
        report(label, res, names.get(label, label), basis_lines(label, ts))


def cmd_crosscheck(label, n):
    """Value the n largest non-stable holders again from Blockscout v2 (its own balances and exchange_rate)."""
    P = json.load(open(f"{D}/prices.json"))
    perps = {u["name"].upper() for u in P["meta"]["universe"] if not u.get("isDelisted")}
    res = {r["id"]: r for r in json.load(open(f"{D}/result-{label}.json"))}
    hold = json.load(open(f"{D}/holdings-{label}.json"))["holders"]
    top = sorted(hold, key=lambda h: -res[h["id"]]["nonstable_usd"])[:int(n)]
    get = lambda u: http(u, ua=BS_UA)  # noqa: E731
    print(f"\n## Cross-check: the {n} largest non-stable holders of {label}, valued again by Blockscout v2")
    print("id    this script (multicall + DefiLlama)   Blockscout (balances + exchange_rate)   difference")
    diffs = []
    for h in top:
        b = f"https://{BS_HOST[h['chain']]}.blockscout.com/api/v2/addresses/{h['address']}"
        a = get(b)
        tot = int(a.get("coin_balance") or 0) / 1e18 * float(a.get("exchange_rate") or 0)
        for t in get(b + "/token-balances"):
            tk, rate = t["token"], t["token"].get("exchange_rate")
            if tk.get("type") != "ERC-20" or not rate:
                continue
            usd = int(t["value"]) / 10 ** int(tk.get("decimals") or 18) * float(rate)
            if classify(tk.get("symbol") or "", float(rate), P["marks"], perps) != "stable":
                tot += usd
        mine = res[h["id"]]["nonstable_usd"]
        diffs.append(abs(tot - mine) / max(mine, 1))
        print(f"{h['id']}  {fmt_usd(mine):>12}  {fmt_usd(tot):>40}  {(tot - mine) / max(mine, 1):+10.1%}")
        time.sleep(0.5)
    print(f"median absolute difference {statistics.median(diffs):.1%}")


def cmd_windows():
    """Is the 90-day window unusual? A one-coin hedge ($100k at entry) over every 90-day window starting each week, on
    Hyperliquid 4 h candles (the venue serves ~5,000 candles: 1 h reaches back ~7 months, 4 h ~25 months) and hourly funding
    summed per 4 h step. Margin min and 1x as in the main run; stablecoins ample, so the 50 % cap never binds."""
    end = int(time.time()) // 14400 * 14400
    start = end - 760 * 86400
    path = f"{D}/windows.json"
    if not os.path.exists(path):
        meta = http(HL, {"type": "meta"})
        candles = {c: http(HL, {"type": "candleSnapshot", "req": {"coin": c, "interval": "4h", "startTime": start * 1000,
                                                                  "endTime": end * 1000}}) for c in ALLOW}
        funding = {c: hl_pages(lambda t, c=c: {"type": "fundingHistory", "coin": c, "startTime": t}, start * 1000,
                               end * 1000) for c in ALLOW}
        json.dump({"end": end, "meta": meta, "candles": candles, "funding": funding, "taker": 0.00045}, open(path, "w"))
    W = json.load(open(path))
    print("\n## Window check: one-coin hedges over every 90-day window starting weekly (Hyperliquid 4 h candles)")
    print(f"{'coin':5} {'windows':>8} {'span':>23}  {'min: <4xMM':>11} {'min: liq.':>10}  {'1x: <4xMM':>10} {'1x: liq.':>9}  "
          f"{'1x lowest health p10/p50':>25}")
    for coin in ALLOW:
        cs = sorted((c for c in W["candles"][coin] if int(c["t"]) + 14_400_000 <= W["end"] * 1000), key=lambda c: c["t"])
        ts = [int(c["t"]) for c in cs]
        by4h = {}
        for f in W["funding"][coin]:
            hour = round(int(f["time"]) / 3_600_000) * 3_600_000  # paid at an hour boundary, a few ms late
            k = (hour - 1) // 14_400_000  # for the hour that ends there: the 4 h step holding it
            by4h[k] = by4h.get(k, 0.0) + float(f["fundingRate"])
        cl, hi = [float(c["c"]) for c in cs], [float(c["h"]) for c in cs]
        fu = [by4h.get(t // 14_400_000, 0.0) for t in ts]
        mk = {coin: Market(W["meta"], coin)}
        n90, step = 90 * 6, 7 * 6
        out = {"min": [], "1x": []}
        starts = range(0, len(ts) - n90 + 1, step)
        for i in starts:
            q = {coin: 100_000 / cl[i]}
            sl = slice(i, i + n90)
            v = [q[coin] * x for x in cl[sl]]
            for mode in out:
                st = simulate(q, v, {coin: cl[sl]}, {coin: hi[sl]}, {coin: fu[sl]}, mk, 1e12, W["taker"], mode)
                out[mode].append((st["breach_close"] + st["breach_high"] > 0 or st["liquidations"] > 0,
                                  st["liquidations"] > 0, st["min_health"]))
        n = len(out["min"])
        share = lambda xs, j: f"{sum(1 for x in xs if x[j]) / n:.0%}"  # noqa: E731
        h1 = [x[2] for x in out["1x"] if x[2] is not None]
        span = f"{datetime.fromtimestamp(ts[0] / 1000, timezone.utc):%Y-%m-%d}..{datetime.fromtimestamp(ts[-1] / 1000, timezone.utc):%Y-%m-%d}"
        print(f"{coin:5} {n:>8} {span:>23}  {share(out['min'], 0):>11} {share(out['min'], 1):>10}  {share(out['1x'], 0):>10} "
              f"{share(out['1x'], 1):>9}  {pct(h1, 10):>12.2f} / {pct(h1, 50):.2f}")


def selftest():
    tiers10 = [(0.0, 10), (3_000_000.0, 5)]
    assert tier_lev(tiers10, 1000, 50) == 10 and tier_lev(tiers10, 4_000_000, 50) == 5 and tier_lev([], 1, 0) == 1
    v, lim = check(0, -0.5, 3000, 2000, 0, 0, tiers10, 50)
    assert v == "allow" and lim["notional_after"] == 1500 and lim["leverage_after"] == 0.75 and lim["required_equity"] == 300
    v, lim = check(0, -2, 3000, 2000, 0, 0, tiers10, 50)
    assert v == "reduce" and lim["why"] == "Leverage" and 3999.9 < lim["notional_after"] <= 4000 and lim["size_after"] < 0
    v, lim = check(0, -0.5, 3000, 2000, 0, 0, tiers10, 50, dict(POLICY, k=100))
    assert v == "reduce" and lim["why"] == "MaintenanceBuffer" and 399.9 < lim["notional_after"] <= 400
    assert check(-1, 0.5, 3000, 1, 150, 3000, tiers10, 50)[1]["size_after"] == -0.5
    assert check(-1, -0.5, 3000, 1, 150, 3000, tiers10, 50) == ("refuse", "BelowMinNotional")
    v, lim = check(0, -0.5, 3000, 2000, 150, 3000, tiers10, 50)
    assert v == "reduce" and lim["why"] == "Leverage" and 999.9 < lim["notional_after"] <= 1000
    assert 799.9 < lim["required_equity"] <= 800
    assert check(-1, 1, 3000, 1, 150, 3000, tiers10, 50)[0] == "allow"  # closing to flat is always allowed
    # The venue's tiered maintenance is continuous at a tier boundary (deduction).
    assert abs(venue_mm(tiers10, 3_000_000, 50) - 150_000) < 1e-6 and abs(venue_mm(tiers10, 4_000_000, 50) - 250_000) < 1e-6
    # One coin, flat price: margin needed for a full 1-ETH short at 2000 = 2000 x (1/2 + 50 bps) = 1010.
    mk = {"ETH": type("M", (), {"szd": 4, "fallback": 25, "tiers": [(0.0, 25)]})()}
    st = simulate({"ETH": 1.0}, [2000.0] * 3, {"ETH": [2000.0] * 3}, {"ETH": [2000.0] * 3}, {"ETH": [0.0] * 3}, mk, 1e6, 0.0)
    assert abs(st["margin_needed"] - 1010.0) < 0.02 and st["pos"]["ETH"] == -1.0 and st["rebal"] == 0, st["margin_needed"]
    # A 45 % rally on that minimum margin: equity 1010 - 900 = 110 < 4 x MM (2900/50 x 4 = 232) -> a breach, no liquidation.
    px = [2000.0, 2900.0]
    st = simulate({"ETH": 1.0}, [2000.0, 2900.0], {"ETH": px}, {"ETH": px}, {"ETH": [0.0, 0.0]}, mk, 1e6, 0.0)
    assert st["breach_close"] == 1 and st["liquidations"] == 0 and st["h_path"][-1] - st["h_path"][0] < 1
    # Positive funding pays the short: 1 ETH x 2000 x 0.0001 per hour, two hours.
    st = simulate({"ETH": 1.0}, [2000.0] * 3, {"ETH": [2000.0] * 3}, {"ETH": [2000.0] * 3}, {"ETH": [0.0, 1e-4, 1e-4]}, mk, 1e6, 0.0)
    assert abs(st["funding"] - 0.4) < 1e-9
    # Margin-limited (50 % of 1,010 of stables): the check shrinks the entry to 0.495 ETH; the next passes ask for the rest,
    # are shrunk again to a sliver under $10, and the venue would refuse those orders.
    st = simulate({"ETH": 1.0}, [2000.0] * 3, {"ETH": [2000.0] * 3}, {"ETH": [2000.0] * 3}, {"ETH": [0.0] * 3}, mk, 1010.0, 0.0,
                  "1x")
    assert st["margin_posted"] == 505.0 and st["pos"]["ETH"] == -0.495 and st["venue_rejected"] == 2 and st["rebal"] == 0
    assert mdd([100, 120, 90, 130, 117]) == (0.25, 30)
    print("selftest: ok")


if __name__ == "__main__":
    a = sys.argv[1:]
    if not a or a[0] not in ("holdings", "prices", "run", "crosscheck", "windows", "selftest"):
        sys.exit(__doc__)
    os.makedirs(D, exist_ok=True)
    {"holdings": lambda: cmd_holdings(*a[1:]), "prices": lambda: cmd_prices(a[1:]), "run": lambda: cmd_run(a[1:]),
     "crosscheck": lambda: cmd_crosscheck(*a[1:]), "windows": cmd_windows, "selftest": selftest}[a[0]]()
