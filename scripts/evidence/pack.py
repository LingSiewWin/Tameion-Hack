#!/usr/bin/env python3
"""Write a source-of-funds pack for a set of addresses.

    python3 scripts/evidence/pack.py --earner "Name" \\
        --address 0xabc... --address 0xdef... \\
        --chains arbitrum,base,ethereum --from 2026-01-01 --to 2026-10-01 \\
        --payers payers.json --out runtime/evidence/name

`payers.json` is what the earner says about who paid them:

    {"0x1234...": {"label": "ETHGlobal Bangkok prize", "category": "prize"}}

Output: `pack.md` for a person to read, `pack.json` for a machine, both with
the same sha256 over the canonical JSON. Personal financial data: write it
under runtime/, which is git-ignored, and never commit it.

    python3 scripts/evidence/pack.py --verify-tokens

re-checks every stablecoin contract the desk recognises against its explorer.
"""

from __future__ import annotations

import argparse
import json
import sys
from datetime import date, datetime, timezone
from decimal import Decimal
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import desk  # noqa: E402


def money(x: str | Decimal | None, places: str = "0.01") -> str:
    if x is None or x == "":
        return "—"
    return f"{Decimal(x).quantize(Decimal(places)):,}"


def render(pack: dict) -> str:
    inflows = [desk.Inflow(**i) for i in pack["inflows"]]
    inc = [i for i in inflows if i.verdict == "INCLUDED"]
    ask = [i for i in inflows if i.verdict == "ASK"]
    exc = [i for i in inflows if i.verdict == "EXCLUDED"]

    def total(rows, field):
        return sum((Decimal(getattr(r, field)) for r in rows if getattr(r, field)), Decimal(0))

    lines = []
    w = lines.append
    w(f"# Source of funds: {pack['earner']}")
    w("")
    w(f"Period {pack['period']['from']} to {pack['period']['to']} · chains {', '.join(pack['chains'])}")
    w("")
    w("Addresses covered:")
    for a in pack["addresses"]:
        w(f"- `{a}`")
    w("")
    w("## How to check this")
    w("")
    w("Every line below links to its transaction on a public block explorer. A reviewer can open")
    w("any of them and see the sender, the recipient, the token contract and the amount without")
    w("relying on this document. Values in ringgit use " + pack["rate_source"] + ".")
    w("")
    w("## Summary")
    w("")
    w("| | Payments | USD | MYR |")
    w("|---|---:|---:|---:|")
    for verdict in ("INCLUDED", "ASK"):
        for cat, s in sorted(pack["summary"].get(verdict, {}).items()):
            name = {"INCLUDED": "Explained", "ASK": "Awaiting the earner's confirmation"}[verdict]
            w(f"| {name}: {cat} | {s['count']} | {money(s['usd'])} | {money(s['myr'])} |")
    exc_count = sum(s["count"] for s in pack["summary"].get("EXCLUDED", {}).values())
    w(f"| Excluded (own transfers, dust, look-alikes, unrecognised tokens) | {exc_count} | | |")
    w("")
    w(f"Explained inflows total **{money(total(inc, 'amount'))} USD**, about **RM {money(total(inc, 'myr'))}**.")
    w("")
    if inc:
        w("## Explained inflows")
        w("")
        w("| Date (UTC) | Chain | Amount | MYR | Rate (date) | Reason | Transaction |")
        w("|---|---|---:|---:|---|---|---|")
        for i in inc:
            w(f"| {i.timestamp[:10]} | {i.chain} | {money(i.amount)} {i.token} | {money(i.myr)} | "
              f"{i.usd_myr or '—'} ({i.rate_date or '—'}) | {i.basis} | [{i.tx_hash[:10]}…]({i.link()}) |")
        w("")
    if ask:
        w("## Awaiting the earner's confirmation")
        w("")
        w("The desk does not guess what a payment was for. These payers are not yet labelled.")
        w("")
        w("| Date (UTC) | Chain | Amount | Payer | Why it is asked | Transaction |")
        w("|---|---|---:|---|---|---|")
        for i in ask:
            w(f"| {i.timestamp[:10]} | {i.chain} | {money(i.amount)} {i.token} | `{i.payer}` | {i.basis} | "
              f"[{i.tx_hash[:10]}…]({i.link()}) |")
        w("")
    if exc:
        w("## Excluded, and why")
        w("")
        cats: dict[str, int] = {}
        for i in exc:
            cats[i.category] = cats.get(i.category, 0) + 1
        for c, n in sorted(cats.items()):
            w(f"- {c}: {n}")
        w("")
        bait = [i for i in exc if i.category == "poisoning-bait"]
        if bait:
            w("Poisoning bait found in this history (dust from look-alikes of real payers):")
            w("")
            for i in bait[:20]:
                w(f"- {i.timestamp[:10]} {i.chain} `{i.payer}`: {i.basis}")
            w("")
    w("## What this is, and is not")
    w("")
    w("- A record of stablecoin payments received at the addresses above, read from public")
    w("  blockchains. It covers the stablecoin contracts listed in `pack.json` only.")
    w("- Reasons marked \"earner states\" are the earner's own statement, attached to a payment")
    w("  whose sender, amount and date anyone can verify. Exchange withdrawals are labelled from")
    w("  the explorer's public tags.")
    w("- It is not a tax computation or advice. A licensed tax agent should review any figure")
    w("  used for a return.")
    w("")
    w(f"sha256 of the canonical record: `{pack['sha256']}`")
    w("")
    return "\n".join(lines)


def verify_tokens() -> int:
    bad = 0
    for chain, tokens in desk.STABLECOINS.items():
        for addr, (sym, dec) in tokens.items():
            d = desk._get(f"{desk.EXPLORERS[chain]}/api/v2/tokens/{addr}")
            ok = d.get("symbol", "").upper().startswith(sym.split(".")[0].upper()) and int(d.get("decimals", -1)) == dec
            bad += not ok
            print(f"{'OK ' if ok else 'BAD'} {chain:9} {addr} {d.get('symbol')} dec={d.get('decimals')} holders={d.get('holders_count')}")
    return 1 if bad else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--earner", default="(unnamed)")
    ap.add_argument("--address", action="append", default=[])
    ap.add_argument("--chains", default="arbitrum,base,ethereum,optimism,polygon")
    ap.add_argument("--from", dest="since", default="2026-01-01")
    ap.add_argument("--to", dest="until", default=datetime.now(timezone.utc).date().isoformat())
    ap.add_argument("--payers", help="JSON file: payer address -> {label, category}")
    ap.add_argument("--out", default="runtime/evidence/pack")
    ap.add_argument("--verify-tokens", action="store_true")
    a = ap.parse_args()

    if a.verify_tokens:
        return verify_tokens()
    if not a.address:
        ap.error("at least one --address")

    chains = [c.strip() for c in a.chains.split(",") if c.strip()]
    since, until = date.fromisoformat(a.since), date.fromisoformat(a.until)
    payers = json.loads(Path(a.payers).read_text()) if a.payers else {}

    inflows: list[desk.Inflow] = []
    for chain in chains:
        for addr in a.address:
            got = desk.fetch_inflows(chain, addr, since, until)
            print(f"  {chain:9} {addr}  {len(got)} incoming token transfers", file=sys.stderr)
            inflows += got

    desk.label(inflows, own=set(a.address), payers=payers)
    desk.value(inflows, desk.Rates())
    pack = desk.build_pack(a.earner, a.address, chains, since, until, inflows)

    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "pack.json").write_text(json.dumps(pack, indent=2, sort_keys=True))
    (out / "pack.md").write_text(render(pack))
    # The questions, as a file the earner can answer and pass back with
    # --payers. One entry per payer, not per payment.
    todo: dict[str, dict] = {}
    for i in inflows:
        if i.verdict == "ASK" and i.payer not in todo:
            todo[i.payer] = {"label": "", "category": "one of: " + ", ".join(desk.CATEGORIES),
                             "seen": f"{i.timestamp[:10]} {i.amount} {i.token} on {i.chain}",
                             "explorer_name": i.payer_name or ""}
    if todo:
        (out / "payers.todo.json").write_text(json.dumps(todo, indent=2))
        print(f"  {len(todo)} payer(s) to label: fill {out}/payers.todo.json and pass it back with --payers",
              file=sys.stderr)
    counts = {v: sum(s["count"] for s in cats.values()) for v, cats in pack["summary"].items()}
    print(f"  wrote {out}/pack.md and pack.json · {counts} · sha256 {pack['sha256'][:16]}…", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
