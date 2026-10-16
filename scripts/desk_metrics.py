#!/usr/bin/env python3
"""What the desks on this Mac have done, from their own files (runtime/app/tenants/*/desk.jsonl and statements/).

    python3 scripts/desk_metrics.py

Counts only what is on disk: deals, steps (signed by a counterparty or entered by the trader), grams bought and sold, offset
moves (by itself, or released by the owner's passkey), statements issued and the distinct on-chain anchors behind them. Stdlib only."""
import collections
import glob
import json
import os
from decimal import Decimal

ROOT = os.path.join(os.path.dirname(__file__), "..", "runtime", "app", "tenants")
tot = collections.Counter()
for book in sorted(glob.glob(os.path.join(ROOT, "*", "desk.jsonl"))):
    desk = os.path.basename(os.path.dirname(book))
    c, grams, deals = collections.Counter(), collections.Counter(), set()
    for line in open(book):
        e = json.loads(line)
        if e.get("type") == "milestone":
            m = e["milestone"]
            deals.add(m["deal"])
            c["steps"] += 1
            c["signed" if e.get("signature") or m.get("signer") else "unsigned"] += 1
            if m["kind"] in ("bought", "sold"):
                grams[m["kind"]] += Decimal(m["quantity"])
        elif e.get("type") == "moved":
            # `auto`: moved by itself inside the owner's limit; otherwise released by the owner's passkey (docs/99 W11).
            c["offset moves, by itself" if e.get("auto") else "offset moves, owner released"] += 1
    kits = glob.glob(os.path.join(os.path.dirname(book), "statements", "*.lender.json"))
    anchors = {json.load(open(k))["anchor"]["tx"] for k in kits}
    books = glob.glob(os.path.join(os.path.dirname(book), "statements", "book-*.json"))
    row = {"deals": len(deals), **c, "bought g": grams["bought"], "sold g": grams["sold"], "statements": len(kits),
           "whole-stock files": len(books), "anchors": len(anchors)}
    print(desk, json.dumps({k: str(v) for k, v in row.items()}))
    tot.update({k: v for k, v in row.items()})
print("all desks", json.dumps({k: str(v) for k, v in tot.items()}))
