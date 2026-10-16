#!/usr/bin/env python3
"""What the hosted service has actually done, read from its own files.

  /opt/homebrew/bin/python3 scripts/metrics.py [app_dir] [--inbox URL | --no-inbox]

app_dir defaults to runtime/app. Every account counted here was made by the
team while building and testing: there are no customers yet, and the output
says so. Agent payments, purchases, invoices, anchors and pass timings come
from the service's files; payments the owners made with their own passkey are
only on chain, so they are read through the running service's inbox
(GET /api/level1/inbox, read-only) for deployed accounts — skipped with
--no-inbox, and an account locked with agent keys is skipped and counted.
"""
import glob
import json
import os
import statistics
import sys
import urllib.request
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone

# The first pass run by the binary with the P-38 fix (job 6acf9e08-0000,
# 2026-10-14 23:21 local): before it, a first check read history 10,000
# blocks at a time.
P38_FIX = 1791991304
# The first pass after receipts were polled every 0.5 s instead of 7 s
# (P-43, job 6acfa7c5-0000).
P43_FIX = 1791993797


def usdc(s):
    """'0.150000 USDC' -> minor units."""
    try:
        w, _, f = str(s).replace("USDC", "").strip().partition(".")
        return int(w or 0) * 1_000_000 + int((f + "000000")[:6] or 0)
    except ValueError:
        return 0


def fmt(minor):
    return f"{minor / 1_000_000:.6f} USDC"


def lines(path):
    try:
        with open(path) as f:
            return [json.loads(l) for l in f if l.strip()]
    except (OSError, json.JSONDecodeError):
        return []


def when(ts):
    return datetime.fromtimestamp(ts, timezone.utc).strftime("%Y-%m-%d %H:%M UTC")


def main(argv):
    app = next((a for a in argv if not a.startswith("--")), "runtime/app")
    inbox_url = None if "--no-inbox" in argv else "http://127.0.0.1:7880"
    if "--inbox" in argv:
        inbox_url = argv[argv.index("--inbox") + 1]
    tdir = os.path.join(app, "tenants")

    tenants = []
    for f in glob.glob(os.path.join(tdir, "0x*.json")):
        if os.path.basename(f).count(".") != 1:
            continue  # *.history.json, *.preview.json are not accounts
        try:
            tenants.append(json.load(open(f)))
        except (OSError, json.JSONDecodeError):
            pass

    jobs = []
    for f in glob.glob(os.path.join(app, "jobs", "*.json")):
        try:
            j = json.load(open(f))
        except (OSError, json.JSONDecodeError):
            continue
        j["_written"] = int(os.path.getmtime(f))
        jobs.append(j)
    runs = [j for j in jobs if j.get("request", {}).get("level1") == "run"]
    watches = [j for j in jobs if "address" in j.get("request", {})]

    out = []
    p = out.append
    p("Tameion service metrics — read from its own files")
    p(f"generated {when(int(datetime.now(timezone.utc).timestamp()))} from {app}")
    p("ALL ACCOUNTS BELOW WERE MADE BY THE TEAM WHILE BUILDING AND TESTING. No customers yet.")
    p("")

    # Accounts
    p("ACCOUNTS")
    by_chain = Counter(t.get("chain") for t in tenants)
    for c, n in sorted(by_chain.items()):
        dep = sum(1 for t in tenants if t.get("chain") == c and t.get("deployed", True))
        p(f"  {c:18} {n:3} accounts  ({dep} on chain, {n - dep} reserved — address known, nothing on chain until money arrives)")
    kinds = Counter(t.get("kind", "hold") for t in tenants)
    p(f"  kinds: {dict(kinds)}; keyless: {sum(1 for t in tenants if t.get('keyless'))}; "
      f"with agent keys minted: {sum(1 for t in tenants if t.get('agent_tokens'))}; "
      f"bills added by hand: {sum(len(t.get('bills') or []) for t in tenants)}")
    p("")

    # Passes and the agent's own payments
    done = [j for j in runs if j.get("status") == "done" and j.get("report")]
    failed = [j for j in runs if j.get("status") == "failed"]
    agent_pay = [(j, s) for j in done for s in j["report"].get("settled") or []]
    escal = sum(len(j["report"].get("escalated") or []) for j in done)
    blocked = sum(len(j["report"].get("blocked") or []) for j in done)
    p("AGENT PASSES (check history, decide, pay inside the caps, anchor)")
    p(f"  passes: {len(runs)} ({len(done)} done, {len(failed)} failed, {len(runs) - len(done) - len(failed)} other)")
    p(f"  payments the agent made by itself: {len(agent_pay)}, {fmt(sum(usdc(s.get('amount')) for _, s in agent_pay))}")
    p(f"    of which early for an invoice discount: {sum(1 for _, s in agent_pay if s.get('early'))}")
    p(f"  bills it asked the owner about instead: {escal}; refused by the checks: {blocked}")
    p(f"  passes with an anchored record: {sum(1 for j in done if j['report'].get('anchorTx'))}")
    why = Counter((j.get("error") or "")[:70] for j in failed)
    if why:
        p("  why passes failed (most common first):")
        for w, n in why.most_common(6):
            p(f"    {n:3} × {w}")
    p("")

    # First checks: wall time of each account's first finished pass
    firsts = {}
    for j in sorted(done, key=lambda j: j["created"]):
        firsts.setdefault(j["request"].get("policy"), j)
    secs = lambda lo, hi: [f["_written"] - f["created"] for f in firsts.values() if lo <= f["created"] < hi]
    eras = (
        ("before the history fix (P-38)", secs(0, P38_FIX)),
        ("after P-38, before receipt fix (P-43)", secs(P38_FIX, P43_FIX)),
        ("after both", secs(P43_FIX, 1 << 62)),
    )
    p("FIRST CHECK OF AN ACCOUNT (job start to its record written, seconds;")
    p("  for a reserved account it also creates the account on chain)")
    for name, xs in eras:
        if xs:
            p(f"  {name:40} n={len(xs):2}  median {statistics.median(xs):6.0f}  max {max(xs):6.0f}")
        else:
            p(f"  {name:40} n= 0")
    created = {t.get("policy", "").lower(): t.get("created") for t in tenants}
    waits = [f["_written"] - created[k.lower()] for k, f in firsts.items()
             if k and created.get(k.lower())]
    if waits:
        p(f"  account created -> first check written: median {statistics.median(waits):.0f} s "
          f"(includes the owner's own wait before funding)")
    p("")

    # Read-only reports (Watch / the Start view's preview)
    wd = [j for j in watches if j.get("status") == "done"]
    p("READ-ONLY REPORTS (paste a wallet, see its bills; no key, nothing sent)")
    p(f"  reports: {len(watches)} ({len(wd)} done) on {dict(Counter(j['request'].get('chain') for j in watches))}")
    ms = [j["report"].get("elapsedMs") for j in wd if j.get("report") and j["report"].get("elapsedMs")]
    if ms:
        p(f"  time to answer: median {statistics.median(ms) / 1000:.1f} s, fastest {min(ms) / 1000:.1f} s, slowest {max(ms) / 1000:.1f} s")
    p("")

    # Purchases (x402) and invoices
    purchases = [r for f in glob.glob(os.path.join(tdir, "*.purchases.jsonl")) for r in lines(f)]
    ps = Counter(r.get("status") for r in purchases)
    paid_p = [r for r in purchases if r.get("status") == "paid"]
    p("PAID APIS THE AGENT BOUGHT (x402, inside the same caps)")
    p(f"  attempts: {len(purchases)} {dict(ps)}; paid {fmt(sum(int(r.get('minor') or 0) for r in paid_p))}")
    p("")
    inv = [r for f in glob.glob(os.path.join(tdir, "*.invoices.jsonl")) for r in lines(f)]
    arrived = [r for r in inv if r.get("status") == "received"]
    decided = Counter(r.get("status") for r in inv if r.get("status") != "received")
    p("INVOICES SIGNED BY VENDORS (EIP-712, signed by the address to be paid)")
    p(f"  received: {len(arrived)} (one-off {sum(1 for r in arrived if not r.get('everyDays'))}, "
      f"repeating {sum(1 for r in arrived if r.get('everyDays'))}); decisions: {dict(decided)}")
    p("")

    # Anchors
    anchors = [r for f in glob.glob(os.path.join(tdir, "*", "log", "anchors.jsonl")) for r in lines(f)]
    p("ANCHORED RECORDS (a root of every decision written on chain)")
    p(f"  roots anchored: {len(anchors)} across {len(glob.glob(os.path.join(tdir, '*', 'log', 'anchors.jsonl')))} accounts; "
      f"events under them: {sum(int(r.get('events') or 0) for r in anchors)}")
    p("")

    # Owners' own payments: on chain only, read through the inbox
    if inbox_url:
        def read(pol):
            try:
                with urllib.request.urlopen(f"{inbox_url}/api/level1/inbox?policy={pol}", timeout=120) as r:
                    return pol, json.load(r), None
            except Exception as e:  # locked accounts answer 401
                return pol, None, str(e)[:40]

        # Reserved accounts are skipped: nothing on chain, and their inbox
        # would start a preview read.
        pols = [t.get("policy") for t in tenants if t.get("deployed", True)]
        with ThreadPoolExecutor(6) as ex:
            got = list(ex.map(read, pols))
        mine, amount, n, skipped = 0, 0, 0, []
        for pol, i, err in got:
            if err:
                skipped.append(f"{pol[:10]}… ({err})")
                continue
            n += 1
            for x in i.get("paid") or []:
                if x.get("by") == "you":
                    mine += 1
                    amount += usdc(x.get("amount"))
        p("PAYMENTS THE OWNERS MADE WITH THEIR PASSKEY (read from chain through the inbox)")
        p(f"  accounts read: {n}; payments: {mine}, {fmt(amount)}")
        if skipped:
            p(f"  not read ({len(skipped)}): " + "; ".join(skipped))
    else:
        p("PAYMENTS THE OWNERS MADE WITH THEIR PASSKEY: not read (--no-inbox)")
    print("\n".join(out))


if __name__ == "__main__":
    # Self-check of the one parser money depends on.
    assert usdc("0.150000 USDC") == 150_000 and usdc("12 USDC") == 12_000_000 and usdc("0.1") == 100_000
    main(sys.argv[1:])
