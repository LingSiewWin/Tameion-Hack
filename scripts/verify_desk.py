#!/usr/bin/env python3
"""Check a desk's lender kit without taking the desk's word for anything (docs/83 W7, docs/89 H4 H5).

    python3 scripts/verify_desk.py <kit.json> --signer 0x<statement key> --desk 0x<desk account> \
        --venue 0x<venue account> --party Supplier=0x… --party Vault=0x… --party Buyer=0x…

The kit (from `POST /api/desk/statement`, the /desk page's download, or `prove_desk.rs`) holds a signed statement for
one deal, the disclosure of that deal's steps, each counterparty's own signature, and where the desk's root was anchored.

The trader runs the desk and holds the statement key, so a kit proves nothing about the world by itself. What a lender
must pin, from outside the kit:
  --signer   the desk's statement key, as the trader gave it to you
  --desk     the desk's account, from the trader, confirmed by each counterparty as the desk it signs for (docs/96 V3)
  --venue    the Hyperliquid account that holds the offset (ideally one whose master key you control, docs/87)
  --party    each counterparty's address, confirmed with that counterparty (repeat the flag); add what it signs to
             pin its role too: --party Vault=0x…:arrived,returned (a step of another kind then fails f)
Without all four the verdict is "CONSISTENT, NOT PINNED" (exit 4), never PASS.

Checks, one line each:
  a. statementHash = keccak256(canonical JSON of the statement)            (json.dumps sorted, compact | cast keccak)
  b. the EIP-712 statement signature recovers to the pinned statement key   (cast wallet verify)
  c. every disclosed step hashes to a leaf that reaches the statement's root from its own position: indexes strictly
     increasing, each path walked by its index and the tree size (RFC 9162 §2.1.3.2), never by the kit's left/right flags
  f. every step re-encodes to its disclosed line and is signed by the pinned address of the counterparty it names
     (EIP-712 ShipmentEvent, domain version 2, verifyingContract = the desk; an unsigned step is the trader's own word)
  h. the grams by stage in the statement are what the disclosed steps add up to (one purchase, first)
  m. every money figure follows from those grams, the statement's mark and its offset: value, each class, the
     offset share, the maximum loss and the eligible value after it (docs/91 R1); the goods are priced on their own
     market (gold on PAXG, pinned here); the maximum loss is sized at no less than your --stress and --basis; the cost is
     the signed purchase's; the offset is this deal's share of the short the statement read, by the desk's whole
     holding (docs/92 T2-T4)
  p. the mark is the venue's price at the statement's time (its 1-minute candles, ±0.5 %)
  d. the root is in the anchoring transaction, sent to the record contract pinned here, on two public nodes
  e. the offset claimed is no more than the short the venue shows at the statement's time: its position now, less every
     fill since (refused when those reach the venue's 10,000-fill horizon)
  g. the funding claimed is no more than the venue's own funding record for the account, same sign
  n. with --previous <an earlier kit from this desk>: the book now extends the book then (RFC 9162 consistency), so no step
     booked before that statement was deleted or changed since; for the same deal, every step it showed is shown again
  t. the statement is no older than --max-age hours (24) and was issued after its own last step
  w. with --with <kits of the same desk at the same root> (a whole-stock file passes them itself): one desk total, the
     offsets add up to no more than the short then, and the account still holds that much short now (docs/96 V1)

What no kit can prove: that the trader booked every step (n shows only that nothing was removed after an earlier statement). A sale never booked never appears; the per-deal disclosure
proves inclusion, not completeness. Ask the buyer side, and watch the venue's short against your own count.
Stdlib and cast only. Exit 0 PASS (pinned), 4 consistent but not pinned, 5 out of date (sold or offset reduced since: ask
for the newest file), 1 FAIL, 2 a tool is missing.
"""

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from decimal import ROUND_HALF_UP, Decimal

RPCS = {421614: ["https://sepolia-rollup.arbitrum.io/rpc", "https://arbitrum-sepolia-rpc.publicnode.com"],
        5042002: ["https://rpc.testnet.arc.io", "https://explorer.testnet.arc.io/api/eth-rpc"]}
# The record contract (MemoryAnchor) per chain, pinned here, never taken from the kit (same as the /lender page).
ANCHORS = {421614: "0xe7c381aafa077cc99359e8b297d2125efd76b8f5", 5042002: "0xdab65a5380bcc2eebc3a3a91d0ad11eec95994db"}
HL = "https://api.hyperliquid-testnet.xyz/info"
DOMAIN_TYPES = [{"name": "name", "type": "string"}, {"name": "version", "type": "string"},
                {"name": "chainId", "type": "uint256"}]
STATEMENT_TYPES = {"EIP712Domain": DOMAIN_TYPES,
                   "DeskStatement": [{"name": "deal", "type": "string"}, {"name": "root", "type": "bytes32"},
                                     {"name": "statementHash", "type": "bytes32"}, {"name": "at", "type": "uint64"}]}
EVENT_TYPES = {"EIP712Domain": DOMAIN_TYPES + [{"name": "verifyingContract", "type": "address"}],
               "ShipmentEvent": [{"name": n, "type": "string"} for n in
                                 ("deal", "kind", "goods", "quantity", "usdc", "counterparty", "document")]
               + [{"name": "at", "type": "uint64"}]}
SCALE = {"gold": (3, "g gold")}
# Where each step takes goods from and puts them (desk::MilestoneKind::movement / SALE_ORDER).
MOVES = {"shipped": (["bought"], "in_transit"), "arrived": (["in_transit", "bought"], "in_stock"),
         "to_fabrication": (["in_stock"], "in_fabrication"), "consigned": (["in_stock"], "consigned"),
         "returned": (["in_fabrication", "consigned"], "in_stock")}
SALE_ORDER = ["in_stock", "consigned", "in_transit", "bought", "in_fabrication"]
OZ = Decimal("31.1034768")  # grams per troy ounce: one PAXG contract
# The venue market that prices each good, pinned here, never taken from the statement (docs/92 T2).
MARKET = {"gold": "PAXG"}
CLASSES = {"eligible": ("in_stock", "in_transit"), "consigned": ("consigned",), "ineligible": ("bought", "in_fabrication")}
CENT = Decimal("0.01")
# The desk's stage words, in the desk's order (desk::Stage::words; the largest holding names the deal, ties to the later).
STAGE_WORDS = {"bought": "bought, not delivered", "in_transit": "in transit", "in_stock": "in stock",
               "in_fabrication": "in fabrication", "consigned": "consigned"}  # money is recomputed to within a cent: the statement rounds the offset to 6 places


def cast(*args, stdin=None):
    r = subprocess.run(["cast", *args], input=stdin, capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip() or f"cast {args[0]} failed")
    return r.stdout.strip()


def keccak_hex(hex_bytes):
    return cast("keccak", "0x" + hex_bytes).lower()


def leaf(canonical_line):
    return keccak_hex("00" + canonical_line.encode().hex())


def node(left, right):
    return keccak_hex("01" + left[2:] + right[2:])


def extends(first, m, second, n, path):
    """RFC 9162 §2.1.4.2: the tree of `n` leaves (root `second`) extends the tree of its first `m` (root `first`)."""
    if m == n:
        return not path and first == second
    if not 0 < m < n or not path:
        return False
    path = ([first] if m & (m - 1) == 0 else []) + list(path)
    fn, sn = m - 1, n - 1
    while fn & 1:
        fn, sn = fn >> 1, sn >> 1
    fr = sr = path[0]
    for c in path[1:]:
        if sn == 0:
            return False
        if fn & 1 or fn == sn:
            fr, sr = node(c, fr), node(c, sr)
            while fn and not fn & 1:
                fn, sn = fn >> 1, sn >> 1
        else:
            sr = node(sr, c)
        fn, sn = fn >> 1, sn >> 1
    return fr == first and sr == second and sn == 0


def root_from(leaf_hash, index, size, path):
    """RFC 9162 §2.1.3.2: the side of each sibling comes from the leaf's index and the tree size, so a step cannot be
    shown at a position it does not hold. None when the path does not fit the position."""
    if index >= size:
        return None
    fn, sn, r = index, size - 1, leaf_hash
    for step in path:
        if sn == 0:
            return None
        if fn & 1 or fn == sn:
            r = node(step["sibling"], r)
            while fn and not fn & 1:
                fn, sn = fn >> 1, sn >> 1
        else:
            r = node(r, step["sibling"])
        fn, sn = fn >> 1, sn >> 1
    return r if sn == 0 else None


def hl(body):
    req = urllib.request.Request(HL, json.dumps(body).encode(), {"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.load(r)


def fills_between(user, start_ms, end_ms):
    """Every fill of `user` in [start_ms, end_ms], paged 2,000 at a time (the venue's page size; docs/89 L4)."""
    out, seen, start = [], set(), start_ms
    while True:
        page = hl({"type": "userFillsByTime", "user": user, "startTime": start, "endTime": end_ms})
        new = [f for f in page if (f.get("tid"), f.get("time")) not in seen]
        for f in new:
            seen.add((f.get("tid"), f.get("time")))
        out += new
        if len(page) < 2000 or not new:
            return out
        start = max(int(f["time"]) for f in page)


def verify_typed(typed, address, signature):
    with tempfile.NamedTemporaryFile("w", suffix=".json") as f:
        json.dump(typed, f)
        f.flush()
        try:
            cast("wallet", "verify", "--address", address, "--data", "--from-file", f.name, signature)
            return True
        except RuntimeError:
            return False


def canonical(m):
    scale, unit = SCALE[m["goods"]]
    q = Decimal(m["quantity"]) * (10 ** scale)
    usdc = "-" if m.get("usdc") is None else str(int(Decimal(m["usdc"]) * 10 ** 6))
    if q != q.to_integral_value():
        raise ValueError("quantity finer than the unit")
    return (f"tameion-desk/v1\ndeal={m['deal']}\nkind={m['kind']}\ngoods={m['goods']}\nquantity={int(q)} {unit}\n"
            f"usdc={usdc}\ncounterparty={m['counterparty']}\ndocument={m['document']}\nsigner={m.get('signer', '')}\nat={m['at']}\n")


def fields(canonical_line):
    return dict(l.split("=", 1) for l in canonical_line.split("\n")[1:] if "=" in l)


def stages_from(items):
    """Replay the disclosed steps of one deal into grams (minor units) by stage, as the desk does."""
    st = {s: 0 for s in SALE_ORDER}
    seen = set()
    for n, it in enumerate(items):
        f = fields(it["canonical"])
        q = int(f["quantity"].split(" ", 1)[0])
        # The desk's own rules (docs/100 X4, docs/101 Y1, Y6): one step per kind and document number, printable ASCII.
        doc = f.get("document", "")
        if not doc or not all(0x20 <= ord(c) <= 0x7E for c in doc):
            raise ValueError(f"step {it['index']}: a document number the desk would refuse (printable ASCII only)")
        if (f["kind"], doc) in seen:
            raise ValueError(f"step {it['index']}: {f['kind']} under {doc} twice (the desk books one step per document number)")
        seen.add((f["kind"], doc))
        if (f["kind"] == "bought") != (n == 0):
            raise ValueError(f"step {it['index']}: a deal is one purchase, booked first (the desk refuses a second)")
        if f["kind"] == "bought":
            st["bought"] += q
            continue
        src, dst = (SALE_ORDER, None) if f["kind"] == "sold" else MOVES[f["kind"]]
        left = q
        for s in src:
            take = min(left, st[s])
            st[s] -= take
            left -= take
        if left:
            raise ValueError(f"step {it['index']} moves more than the deal holds")
        if dst:
            st[dst] += q
    return st


def usd(x):
    return x.quantize(Decimal("0.000001"), ROUND_HALF_UP)


def money(st, by):
    """Each money figure of the statement against what follows from the replayed grams (check m). Returns the misses."""
    mark, off = Decimal(st["markUsdc"]), Decimal(st["offsetContracts"])
    oz = {c: Decimal(sum(by[s] for s in stages)) / 1000 / OZ for c, stages in CLASSES.items()}
    held = sum(oz.values())
    value = held * mark
    max_loss = (held - off) * mark * st["stressBps"] / 10000 + value * st["basisBps"] / 10000
    want = {"heldContracts": (held, Decimal("0.000001")), "valueUsdc": (value, CENT), "maxLossUsdc": (max_loss, CENT),
            "eligibleUsdc": (max(Decimal(0), usd(oz["eligible"] * mark) - max_loss), CENT),
            # Below ~0.005 oz the 6-place offset moves the share by more than a bps (docs/94 U6).
            "offsetShareBps": ((off / held * 10000) if held else Decimal(0),
                               Decimal(1) + (Decimal(10000) * Decimal("0.0000005") / held if held else 0))}
    for c in CLASSES:
        want[f"{c}.valueUsdc"] = (oz[c] * mark, CENT)
    miss = []
    if off > held + Decimal("0.000001"):
        miss.append(f"offsetContracts {off} is more than the {usd(held)} oz held")
    # The deal's share of one short (docs/92 T4): short × held ÷ the desk's whole holding, never more than held.
    if "shortContracts" not in st or "deskHeldContracts" not in st:
        miss.append("the statement does not say the short it read or the desk's whole holding; ask for a new one")
    else:
        short, desk_held = Decimal(st["shortContracts"]), Decimal(st["deskHeldContracts"])
        want_off = min(short * held / desk_held, held) if desk_held > 0 else Decimal(0)
        if desk_held + Decimal("0.000001") < held:
            miss.append(f"the desk holds {desk_held} oz in all, less than this deal's {usd(held)}")
        if abs(off - want_off) > Decimal("0.000002"):
            miss.append(f"offsetContracts {off}; this deal's share of the {short} short is {usd(want_off)}")
        # The venue's short bounds the desk's stated total from below (one lot of slack: orders are truncated to the size
        # step), so a total lowered to hide stock, or a short beyond the stock, is refused (docs/100 X1).
        if short > desk_held + Decimal("0.001"):
            miss.append(f"the account is short {short} oz, more than the {desk_held} oz the desk says it holds: stock is "
                        "left out of the desk's total, or the account is short beyond the stock (that loses on a rise)")
    for k, (v, tol) in want.items():
        got = Decimal((st.get(k.split(".")[0]) or {}).get("valueUsdc", "0")) if "." in k else Decimal(str(st[k]))
        if abs(got - v) > tol:
            miss.append(f"{k} {got}, the steps give {usd(v)}")
    return miss


def funding_ok(st, held_mg, acct):
    """This deal's share of the account's funding `acct`, as the service computes it (docs/94 U5): its grams over the
    desk's holding. The service divides grams by grams; deskHeldContracts is printed to 6 places, so the tolerance grows
    with the funding (docs/96 V5)."""
    claimed = Decimal(st["fundingUsdc"])
    desk_held = Decimal(st.get("deskHeldContracts", "0"))
    if desk_held <= 0 or held_mg is None:
        return claimed == 0 or (claimed * acct > 0 and abs(claimed) <= abs(acct) + Decimal("0.000001"))
    share = held_mg / 1000 / OZ / desk_held
    tol = Decimal("0.000002") + abs(acct) * Decimal("0.0000005") / desk_held
    return abs(claimed - usd(acct * min(share, Decimal(1)))) <= tol


def grams_minor(text):
    """'8.000 g gold' → 8000."""
    return int((Decimal(text.split(" ", 1)[0]) * 1000).to_integral_value())


def whole_book(path, kits):
    """A file of every open deal's statement at one moment (POST /api/desk/statements/all): each deal is checked with
    the others (--with), so no two can claim the same short; one verdict for the book."""
    import os
    if not isinstance(kits, list) or not kits:
        print("FAIL: not a statement of the whole stock (it lists no deals)")
        return 1
    # One moment, one desk, distinct deals (docs/96 V2): a file that mixes moments or repeats a deal is not a book.
    try:
        st = [k["signed"]["statement"] for k in kits]
        moments = {(s["root"].lower(), s["at"], k["signed"]["signer"].lower(), (k.get("desk") or "").lower(),
                    s["venueAccount"].lower(), s.get("deskHeldContracts"), s.get("shortContracts")) for k, s in zip(kits, st)}
        deals = [s["deal"] for s in st]
        idx = [int(it["index"]) for k in kits for it in k["disclosure"]["items"]]
    except (KeyError, TypeError, AttributeError) as e:
        print(f"FAIL: not a statement of the whole stock ({e})")
        return 1
    if len(moments) != 1 or len(set(deals)) != len(deals) or len(set(idx)) != len(idx):
        print("FAIL: this is not one statement of the whole stock: its deals differ in moment, root, key, desk, exchange "
              "account or totals, or a deal or step appears twice")
        return 1
    # Whole: its deals add up to the holding the desk states (docs/99 W1); each figure is printed to 6 places.
    if "deskHeldContracts" in st[0]:
        held_sum = sum(Decimal(s["heldContracts"]) for s in st)
        if abs(held_sum - Decimal(st[0]["deskHeldContracts"])) > Decimal("0.0000005") * (len(st) + 1):
            print(f"FAIL: this file leaves out stock: its deals hold {held_sum} oz, the desk says it holds "
                  f"{st[0]['deskHeldContracts']} oz; ask for the whole file")
            return 1
    rest = [x for x in sys.argv[1:] if x != path]
    codes = []
    with tempfile.TemporaryDirectory() as tmp:
        files = []
        for i, k in enumerate(kits):
            f = os.path.join(tmp, f"deal-{i}.json")
            with open(f, "w") as out:
                json.dump(k, out)
            files.append(f)
        for i, f in enumerate(files):
            others = sum((["--with", o] for j, o in enumerate(files) if j != i), [])
            print(f"\n===== {deals[i]} ({i + 1} of {len(kits)})", flush=True)
            codes.append(subprocess.run([sys.executable, __file__, f, *rest, *others]).returncode)
    eligible = sum(Decimal(s["eligibleUsdc"]) for s in st)
    held = sum(Decimal(s["heldContracts"]) for s in st)
    print(f"\nthe book: {len(kits)} open deals, {held} oz held; eligible after each deal's maximum loss {eligible} USDC in all")
    worst = 1 if any(c not in (0, 4, 5) for c in codes) else 5 if 5 in codes else (4 if 4 in codes else 0)
    bad = [d for d, c in zip(deals, codes) if c]
    print("PASS" if worst == 0 else "CONSISTENT, NOT PINNED" if worst == 4
          else "OUT OF DATE: ask the trader for the newest file of the whole stock" if worst == 5 else "FAIL: " + ", ".join(bad))
    return worst


def main():
    if not shutil.which("cast"):
        print("cast (foundry) is required")
        return 2
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("kit")
    ap.add_argument("--signer", help="the desk's statement key, as the trader gave it to you")
    ap.add_argument("--venue", help="the Hyperliquid account that holds the offset")
    ap.add_argument("--desk", help="the desk's account, from the trader at set-up and confirmed by each counterparty")
    ap.add_argument("--party", action="append", default=[], help="Name=0xaddress, confirmed with that counterparty")
    ap.add_argument("--previous", help="an earlier kit from the same desk, already checked: the book must extend it")
    ap.add_argument("--stress", type=int, default=3000, help="the price fall you size the maximum loss at, in bps (default 3000)")
    ap.add_argument("--basis", type=int, default=200, help="your allowance for the goods not tracking the market, in bps (default 200)")
    ap.add_argument("--max-age", type=float, default=24, help="hours a statement may be old (default 24)")
    ap.add_argument("--with", dest="with_", action="append", default=[],
                    help="another kit of the same desk: statements at one root must agree on the desk's total and share one short")
    a = ap.parse_args()
    kit = json.load(open(a.kit))
    if "kits" in kit:
        return whole_book(a.kit, kit["kits"])
    if not all(k in kit for k in ("signed", "disclosure", "anchor")):
        print(f"FAIL: not a lender's kit{': ' + str(kit['error']) if 'error' in kit else ''}")
        return 1
    signed, disc, anchor = kit["signed"], kit["disclosure"], kit["anchor"]
    st = signed["statement"]
    chain = int(anchor["chainId"])
    market = MARKET.get(st.get("goods", ""), "")
    pinned_parties, pinned_kinds = {}, {}
    for p in a.party:
        n, rest = p.split("=", 1)
        pinned_parties[n], _, kinds = rest.partition(":")
        if kinds:
            pinned_kinds[n] = set(kinds.split(","))
    unpinned = [n for n, v in (("--signer", a.signer), ("--desk", a.desk), ("--venue", a.venue), ("--party", a.party)) if not v]
    ok = {}

    # a. the hash covers the statement as served
    got = cast("keccak", stdin=json.dumps(st, sort_keys=True, separators=(",", ":"), ensure_ascii=False)).lower()
    ok["a"] = got == signed["statementHash"].lower()
    print(f"a. statementHash recomputed {got[:18]}… {'matches' if ok['a'] else 'DIFFERS from ' + signed['statementHash']}")

    # b. the statement key
    signer = (a.signer or signed["signer"]).lower()
    typed = {"types": STATEMENT_TYPES, "primaryType": "DeskStatement",
             "domain": {"name": "Tameion Desk", "version": "1", "chainId": chain},
             "message": {"deal": st["deal"], "root": st["root"], "statementHash": signed["statementHash"], "at": st["at"]}}
    ok["b"] = verify_typed(typed, signer, signed["signature"])
    print(f"b. statement signature {'recovers to' if ok['b'] else 'does NOT recover to'} {signer}"
          f"{'' if a.signer else '  (NOT PINNED: the key the kit names)'}")

    # c. inclusion in the root, each step from its own position, each position once (docs/91 R2)
    idx = [int(it["index"]) for it in disc["items"]]
    ok["c"] = (disc["root"].lower() == st["root"].lower() and bool(idx) and all(a < b for a, b in zip(idx, idx[1:])))
    for it in disc["items"]:
        acc = root_from(leaf(it["canonical"]), int(it["index"]), int(disc["leaves"]), it["path"])
        ok["c"] = ok["c"] and acc == st["root"].lower() and f"\ndeal={st['deal']}\n" in it["canonical"]
    print(f"c. {len(idx)} of {disc['leaves']} steps disclosed, at positions {idx}; each reaches root {st['root'][:18]}… "
          f"from its own position, none twice: {ok['c']}")

    # f. each counterparty's own signature, against the address the lender confirmed
    parties = {k: v.lower() for k, v in (pinned_parties or {p["name"]: p["address"] for p in kit.get("parties", [])}).items()}
    # The desk the steps are signed for is the one the lender set up, not the one the kit names (docs/96 V3).
    named = (kit.get("desk") or "").lower()
    desk = (a.desk or named).lower()
    signed_items = kit.get("milestones", [])
    ok["f"] = bool(desk) and named == desk and len(signed_items) == len(disc["items"]) and bool(signed_items)
    if not named:
        print("   the kit names no desk: it predates desk-bound steps (domain version 2)")
    elif named != desk:
        print(f"   this file is for desk {named}, not the one you set up ({desk})")
    for item, sm in zip(disc["items"], signed_items):
        m, sig = sm["milestone"], sm.get("signature", "")
        same = canonical(m) == item["canonical"]
        if not sig:
            good = same and not m.get("signer")
            why = "entered by the trader, unsigned (no counterparty vouches for it)" if good else "names a signer but carries no signature"
        else:
            typed = {"types": EVENT_TYPES, "primaryType": "ShipmentEvent",
                     "domain": {"name": "Tameion Desk", "version": "2", "chainId": chain,
                                "verifyingContract": desk or "0x0000000000000000000000000000000000000000"},
                     "message": {"deal": m["deal"], "kind": m["kind"], "goods": m["goods"], "quantity": m["quantity"],
                                 "usdc": m.get("usdc") or "", "counterparty": m["counterparty"], "document": m["document"],
                                 "at": m["at"]}}
            want = parties.get(m["counterparty"])
            role = pinned_kinds.get(m["counterparty"])
            good = (same and want is not None and want == m["signer"].lower() and verify_typed(typed, want, sig)
                    and (role is None or m["kind"] in role))
            why = (f"{m['counterparty']} may sign {', '.join(sorted(role))}, not {m['kind']}" if role and m["kind"] not in role
                   else f"{m['counterparty']}'s signature {'recovers to' if good else 'does NOT recover to'} {want}")
        ok["f"] = ok["f"] and good
        print(f"   step {item['index']}: {m['kind']} {m['quantity']} g · {m['document']} · {why}")
    print(f"f. counterparty signatures for desk {desk}: {ok['f']}{'' if pinned_parties else '  (NOT PINNED: the parties the kit names)'}"
          f"{'' if a.desk else '  (NOT PINNED: the desk the kit names)'}")

    # h. the statement's grams are what the steps add up to
    try:
        by = stages_from(disc["items"])
        claimed = {l["stage"]: grams_minor(l["grams"]) for l in st.get("byStage", [])}
        # The statement's own list of steps is the disclosed one (docs/89 H5 (1)): same steps, same order.
        key = ("kind", "quantity", "document", "counterparty", "signer", "at")
        same_list = ([tuple(str(m.get(k, "")) for k in key) for m in st.get("milestones", [])]
                     == [tuple(str(sm["milestone"].get(k, "")) for k in key) for sm in signed_items])
        # The words a lender reads as facts follow the steps too: the stage and each class's grams (docs/94 U4).
        held_now = [k for k in STAGE_WORDS if by[k] > 0]  # the desk's stage order; a tie goes to the later stage
        stage = STAGE_WORDS[max(reversed(held_now), key=lambda k: by[k])] if held_now else "sold"
        classes_ok = all(grams_minor((st.get(c) or {}).get("grams", "0 g")) == sum(by[x] for x in CLASSES[c]) for c in CLASSES)
        ok["h"] = (all(by[s] == claimed.get(s, 0) for s in by) and sum(by.values()) == grams_minor(st["held"])
                   and same_list and st.get("stage") == stage and classes_ok)
        if not same_list:
            print("   the statement lists other steps than the ones disclosed")
        if st.get("stage") != stage:
            print(f"   the statement says \"{st.get('stage')}\"; the steps say \"{stage}\"")
        if not classes_ok:
            print("   the grams by class (eligible, consigned, not eligible) are not what the steps add up to")
        shown = ", ".join(f"{k} {v / 1000:.3f} g" for k, v in by.items() if v) or "nothing held"
        print(f"h. from the steps: {shown} — {'matches' if ok['h'] else 'does NOT match'} the statement")
    except (KeyError, ValueError) as e:
        ok["h"], by = False, None
        print(f"h. the steps do not add up: {e}")

    # m. the money follows from the grams (docs/91 R1): what a lender lends on is recomputed, not read
    miss = money(st, by) if by else ["no grams to compute from (h)"]
    if st.get("market") != market or not market:
        miss.append(f"the statement prices {st.get('goods')} on {st.get('market')}; {st.get('goods')} is priced on "
                    f"{market or 'no market this check knows'} (docs/92 T2)")
    if any(fields(it["canonical"]).get("goods") != st.get("goods") for it in disc["items"]):
        miss.append("a disclosed step is for other goods than the statement's")
    if int(st["stressBps"]) < a.stress or int(st["basisBps"]) < a.basis:
        miss.append(f"the maximum loss is sized at a {int(st['stressBps']) / 100}% fall and {int(st['basisBps']) / 100}% basis; "
                    f"you asked for at least {a.stress / 100}% and {a.basis / 100}% (--stress, --basis)")
    bought = next((fields(it["canonical"]) for it in disc["items"] if fields(it["canonical"]).get("kind") == "bought"), None)
    if bought and bought.get("usdc", "-") != "-":
        want_cost = f"{Decimal(bought['usdc']) / Decimal(10 ** 6):.6f} USDC"
        if st.get("costUsdc") != want_cost:
            miss.append(f"costUsdc {st.get('costUsdc')}; the signed purchase says {want_cost}")
    ok["m"] = not miss
    for line in miss:
        print(f"   {line}")
    print(f"m. value, classes, offset share, maximum loss and eligible value follow from the steps "
          f"at {st['markUsdc']}/oz: {ok['m']}")

    # p. the mark is the venue's price then
    t = int(st["at"]) * 1000
    # The venue's 1-minute feed can lag a few seconds behind the minute (seen at 03:00:10 on 10-16, docs/60 P-125).
    for wait in (0, 5, 15):
        time.sleep(wait)
        candles = hl({"type": "candleSnapshot", "req": {"coin": market, "interval": "1m",
                                                        "startTime": t - 120_000, "endTime": t + 120_000}})
        if candles:
            break
    if candles:
        lo, hi = min(Decimal(c["l"]) for c in candles), max(Decimal(c["h"]) for c in candles)
        ok["p"] = lo * Decimal("0.995") <= Decimal(st["markUsdc"]) <= hi * Decimal("1.005")
        print(f"p. {market} traded {lo}–{hi} in the two minutes around the statement; mark {st['markUsdc']}: {ok['p']}")
    else:
        ok["p"] = False
        print(f"p. the venue has no {market} candles around the statement's time yet: run the check again in a minute")

    # d. the root on chain, in the pinned record contract
    seen = []
    pinned_anchor = ANCHORS.get(chain)
    if pinned_anchor != anchor["contract"].lower():
        print(f"   the kit names record contract {anchor['contract']}, not the one pinned here ({pinned_anchor})")
    for rpc in RPCS.get(chain, []):
        try:
            tx = json.loads(cast("tx", anchor["tx"], "--json", "--rpc-url", rpc))
            rc = json.loads(cast("receipt", anchor["tx"], "--json", "--rpc-url", rpc))
            # Who may anchor for this desk is the desk's own operator, read on chain, never the kit's word
            # (docs/103 S2-1): a stranger who builds a kit for a desk cannot anchor as that desk.
            try:
                operator = cast("call", desk, "operator()(address)", "--rpc-url", rpc).strip().lower() if desk else ""
            except Exception:
                operator = ""
            if not operator:
                print(f"   {rpc}: the desk {desk or '(not named)'} is not on chain or names no operator, so who may anchor for it cannot be read")
            elif tx["from"].lower() != operator:
                print(f"   {rpc}: the anchor was sent by {tx['from']}, not by the desk's operator {operator}")
            good = (bool(operator) and tx["from"].lower() == operator and (tx.get("to") or "").lower() == pinned_anchor
                    and int(str(rc["status"]), 0) == 1 and tx["input"].lower().endswith(st["root"].lower()[2:]))
            seen.append(good)
            print(f"   {rpc}: block {int(str(rc['blockNumber']), 0)}, sent by the desk's operator, carries the root in the pinned contract: {good}")
        except Exception as e:  # one node down is reported; the other must answer
            print(f"   {rpc}: {e}")
    ok["d"] = bool(seen) and all(seen)
    print(f"d. root anchored in {anchor['tx'][:18]}… on {len(seen)} node(s): {ok['d']}")

    # e. the offset against the venue's own fills, on the account the lender pinned
    venue = (a.venue or st["venueAccount"]).lower()
    if a.venue and venue != st["venueAccount"].lower():
        print(f"   the statement names venue account {st['venueAccount']}, not the one you pinned")
    at_ms = int(st["at"]) * 1000
    # The position now, less every fill since the statement (docs/91 R12): the venue keeps only its 10,000 most
    # recent fills, so counting up from zero misses an old position. ponytail: a fill landing between the two reads
    # shifts the result by that fill; run again.
    now = hl({"type": "clearinghouseState", "user": venue})
    pos_now = sum((Decimal(p["position"]["szi"]) for p in now.get("assetPositions", [])
                   if p["position"]["coin"] == market), Decimal(0))
    since = fills_between(venue, at_ms + 1, int(now.get("time") or 0) or 2**62)
    pos = pos_now - sum((Decimal(f["sz"]) * (-1 if f["side"] == "A" else 1) for f in since if f["coin"] == market),
                        Decimal(0))
    claimed_off = Decimal(st.get("shortContracts", st["offsetContracts"]))
    held_then = max(-pos, Decimal(0))
    # The offset must still be there now (docs/94 U2): a statement is a point in time, a lender lends on the present.
    short_now = max(-pos_now, Decimal(0))
    gone = short_now + Decimal("0.0005") < Decimal(st["offsetContracts"])
    # A short bought back since the statement means a sale or an offset taken off, whatever the level now: a short cut
    # for the statement and restored after it would otherwise absorb a later sale (docs/100 X2).
    bought_back = sum((Decimal(f["sz"]) for f in since if f["coin"] == market and f["side"] == "B"), Decimal(0))
    cut = bought_back > Decimal("0.0005")
    if cut:
        print(f"   the account bought back {bought_back} oz of its {market} short since this statement: a deal was sold or "
              "the offset taken off; ask for a statement issued after it")
    # The short the statement read is the account's short then, neither more nor less (docs/99 W2): a smaller one makes
    # a sale since look like no change. An older statement names only its own offset, which may be less.
    exact = "shortContracts" in st
    e_core = (venue == st["venueAccount"].lower() and len(since) < 10_000
              and claimed_off <= held_then + Decimal("0.0005")
              and (not exact or claimed_off + Decimal("0.0005") >= held_then))
    ok["e"] = e_core and not gone and not cut
    # Only out of date: true when the statement was issued, overtaken by a sale or a smaller offset since (docs/101 Y2).
    outdated = {"e"} if e_core and (gone or cut) else set()
    if exact and claimed_off + Decimal("0.0005") < held_then:
        print(f"   the statement read a short of {claimed_off}; the account held {held_then} then")
    if gone:
        print(f"   since the statement the account's short fell to {short_now}; the {st['offsetContracts']} oz this statement "
              "counts is not there now: ask for a new one")
    print(f"e. {st['market']} position of {venue[:10]}… at the statement's time: {pos_now} now, less {len(since)} fills "
          f"since = {pos}; short the statement read {claimed_off}: {ok['e']}"
          f"{'' if len(since) < 10_000 else '  (past the venue 10,000-fill horizon: cannot tell)'}"
          f"{'' if a.venue else '  (NOT PINNED: the account the kit names)'}")

    # g. funding against the venue's own record
    if "fundingUsdc" in st:
        first = min((int(sm["milestone"]["at"]) for sm in signed_items), default=int(st["at"]))
        rec = hl({"type": "userFunding", "user": venue, "startTime": first * 1000, "endTime": at_ms})
        acct = sum((Decimal(f["delta"]["usdc"]) for f in rec if f.get("delta", {}).get("coin") == market), Decimal(0))
        claimed_f = Decimal(st["fundingUsdc"])
        ok["g"] = funding_ok(st, Decimal(sum(by.values())) if by else None, acct)
        print(f"g. funding on the account's {st['market']} short since the deal's first step: {acct}; claimed {claimed_f}: {ok['g']}")

    # n. nothing removed since an earlier statement the lender already holds (docs/89 H5 (3))
    if a.previous:
        old = json.load(open(a.previous))
        m, first = int(old["disclosure"]["leaves"]), old["signed"]["statement"]["root"].lower()
        entry = next((h for h in kit.get("history", []) if int(h["leaves"]) == m and h["root"].lower() == first), None)
        ok["n"] = (old.get("desk", "").lower() == (desk or "").lower() and entry is not None
                   and extends(first, m, st["root"].lower(), int(disc["leaves"]), [c.lower() for c in entry["consistency"]]))
        # The same deal: every step the earlier statement showed must be shown again (docs/92 T1).
        if old["signed"]["statement"]["deal"] == st["deal"]:
            shown_now = {(int(it["index"]), it["canonical"]) for it in disc["items"]}
            for it in old["disclosure"]["items"]:
                if (int(it["index"]), it["canonical"]) not in shown_now:
                    ok["n"] = False
                    f = fields(it["canonical"])
                    grams = int(f.get("quantity", "0").split(" ", 1)[0]) / 1000
                    print(f"   step {it['index']} ({f.get('kind')} {grams:.3f} g), shown in your earlier statement, is missing")
        print(f"n. the book of {disc['leaves']} steps extends the one of {m} at your earlier statement "
              f"({old['signed']['statement']['deal']}, root {first[:18]}…): {ok['n']}"
              f"{'' if entry else '  (the kit carries no proof from that statement)'}")
    elif kit.get("history"):
        print(f"   {len(kit['history'])} earlier statements of this desk can be checked against this one: pass one with --previous")

    # t. how old the statement is, and that it follows its own steps (docs/92 T8)
    age_h = (time.time() - int(st["at"])) / 3600
    last_step = max((int(fields(it["canonical"]).get("at", 0)) for it in disc["items"]), default=0)
    ok["t"] = age_h <= a.max_age and int(st["at"]) >= last_step
    print(f"t. issued {time.strftime('%Y-%m-%d %H:%M:%S UTC', time.gmtime(int(st['at'])))}, {age_h:.1f} h ago "
          f"(at most {a.max_age:g} h), after its last step: {ok['t']}")

    if "deskHeldContracts" in st and by:
        others = Decimal(st["deskHeldContracts"]) - Decimal(sum(by.values())) / 1000 / OZ
        # The desk's whole holding is the trader's word: one file cannot prove it (docs/94 U1).
        print(f"   the desk says it holds {st['deskHeldContracts']} oz in all"
              f"{f', {others:.6f} oz of it in other deals' if others > Decimal('0.000001') else ''}; this file cannot prove "
              f"that total. The short is the account's and backs every deal of this desk: check its statements together (--with)")
    # w. statements of the same desk at the same root, checked together (docs/94 U1)
    if a.with_:
        group = [kit] + [json.load(open(f)) for f in a.with_]
        same = [k for k in group if k["signed"]["statement"]["root"].lower() == st["root"].lower()]
        totals = {k["signed"]["statement"].get("deskHeldContracts") for k in same}
        sum_held = sum(Decimal(k["signed"]["statement"]["heldContracts"]) for k in same)
        sum_off = sum(Decimal(k["signed"]["statement"]["offsetContracts"]) for k in same)
        desk_total = Decimal(st.get("deskHeldContracts", "0"))
        # Together they count sum_off of the short; the account must still hold it now (docs/96 V1): on a desk that keeps
        # trading, one deal's own check (e) passes on the other deals' short after it was sold.
        fell = short_now + Decimal("0.0005") < sum_off
        tol = max(Decimal("0.000002"), Decimal("0.0000005") * (len(same) + 1))  # 6-place rounding per deal (docs/99 W8)
        w_core = (len(totals) == 1 and sum_held <= desk_total + tol
                  and sum_off <= Decimal(st.get("shortContracts", "0")) + tol)
        ok["w"] = w_core and not fell
        if w_core and fell:
            outdated.add("w")
        if fell:
            print(f"   since these statements the short fell from {st.get('shortContracts')} to {short_now}, below the "
                  f"{sum_off} oz they count: a deal was sold or the offset reduced; ask for today's file")
        print(f"w. {len(same)} statements at this root: they state {len(totals)} desk total(s); {sum_held} oz held against "
              f"{desk_total}; {sum_off} oz offset against the {st.get('shortContracts')} short, {short_now} now: {ok['w']}")
    print(f"\nwhat the statement says: {st['held']} {st['stage']}, worth {st['valueUsdc']} USDC at {st['markUsdc']}/oz; "
          f"{int(st['offsetShareBps']) / 100}% offset; maximum loss {st['maxLossUsdc']} USDC at a {st['stressBps'] / 100}% fall "
          f"+ {st['basisBps'] / 100}% basis; eligible after it {st['eligibleUsdc']} USDC")
    failed = [k for k, v in ok.items() if not v]
    if failed and set(failed) <= outdated:
        print("OUT OF DATE: a deal was sold or the offset reduced after this statement; ask the trader for the newest one "
              "and do not lend on this one")
        return 5
    if failed:
        print("FAIL: " + ", ".join(failed))
        return 1
    if unpinned:
        print("CONSISTENT, NOT PINNED (" + ", ".join(unpinned) + "): the trader could have made this kit; pin them to PASS")
        return 4
    # docs/91 R10: eligible grams that came in on the trader's own word are said, not hidden in a step line.
    unsigned_in = sum(int(fields(it["canonical"])["quantity"].split(" ", 1)[0])
                      for it, sm in zip(disc["items"], signed_items)
                      if not sm.get("signature") and fields(it["canonical"])["kind"] in ("shipped", "arrived", "returned"))
    eligible = by["in_stock"] + by["in_transit"] if by else 0
    on_word = min(unsigned_in, eligible)
    alone = ("deskHeldContracts" in st and not a.with_
             and Decimal(st["deskHeldContracts"]) > Decimal(st["heldContracts"]) + Decimal("0.000002"))
    print("PASS" + (f" — but {on_word / 1000:.3f} g of the {eligible / 1000:.3f} g eligible came in on steps no counterparty"
                    " signed: count them only on the trader's word" if on_word else "")
          + (" — a sale of this deal does not show here while other stock of this desk is offset: ask for the"
             " whole-stock file" if alone else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
