#!/usr/bin/env python3
"""Which physical-trade exposures can be offset on Hyperliquid today, measured.

Reads the public info API (no key): core perps and every HIP-3 dex, mainnet and
testnet, and prints 24 h notional volume, open interest and the funding rate for
the markets a trader of goods or currencies would hedge with. Stdlib only.

    python3 scripts/research/hl_markets.py > proof/hl-markets-$(date +%F).txt
"""

import json
import time
import urllib.request

NETS = {"mainnet": "https://api.hyperliquid.xyz/info", "testnet": "https://api.hyperliquid-testnet.xyz/info"}
# Physical goods and currencies, by the names markets use for them.
GOODS = ("PAXG", "GOLD", "SILVER", "PLATINUM", "PALLADIUM", "COPPER", "CL", "BRENTOIL", "NATGAS",
         "WHEAT", "CORN", "SOY", "COFFEE", "COCOA", "SUGAR", "COTTON", "ALUMINIUM", "URANIUM",
         "EUR", "JPY", "GBP", "CNH", "CNY", "AUD", "CAD", "CHF", "MYR", "SGD", "KRW", "INR")


def post(url, body):
    req = urllib.request.Request(url, json.dumps(body).encode(), {"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.load(r)


def rows(url, dex=None):
    body = {"type": "metaAndAssetCtxs"} | ({"dex": dex} if dex else {})
    meta, ctxs = post(url, body)
    for a, c in zip(meta["universe"], ctxs):
        if a.get("isDelisted"):
            continue
        name = a["name"]
        base = name.split(":")[-1]
        if base not in GOODS:
            continue
        mark = float(c.get("markPx") or 0)
        yield {
            "market": name,
            "dex": dex or "core",
            "vol24h": float(c.get("dayNtlVlm") or 0),
            "oi_usd": float(c.get("openInterest") or 0) * mark,
            "funding_h": float(c.get("funding") or 0),
            "mark": mark,
            "max_lev": a.get("maxLeverage"),
            "sz_decimals": a.get("szDecimals"),
        }


def main():
    print(f"# Hyperliquid markets for goods and currencies, read {time.strftime('%Y-%m-%d %H:%M %z')}")
    print("# source: POST /info metaAndAssetCtxs (core) and with dex=<name> (HIP-3), public API, no key")
    for net, url in NETS.items():
        dexs = [d["name"] for d in post(url, {"type": "perpDexs"}) if d]
        found = list(rows(url))
        for d in dexs:
            try:
                found += list(rows(url, d))
            except Exception as e:  # one broken dex must not hide the others
                print(f"# {net} dex {d}: {e}")
        found.sort(key=lambda r: -r["vol24h"])
        print(f"\n## {net}: {len(dexs)} HIP-3 dexs ({', '.join(dexs)}); {len(found)} goods/currency markets")
        print(f"{'market':22} {'dex':6} {'24h volume':>14} {'open interest':>14} {'funding/h':>11} {'mark':>12} lev")
        for r in found:
            print(f"{r['market']:22} {r['dex']:6} {r['vol24h']:14,.0f} {r['oi_usd']:14,.0f} "
                  f"{r['funding_h']:+11.6f} {r['mark']:12,.4f} {r['max_lev']}")


if __name__ == "__main__":
    main()
