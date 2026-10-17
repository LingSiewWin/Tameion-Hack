#!/usr/bin/env python3
"""One role per key (docs/117 §2): which key signs in which role, from the code's
defaults and the service's records, and exit 1 when one key holds two roles.

    python3 -I scripts/key_roles.py [app_dir]          # default runtime/app

Roles read:
- the key files the code reads when nothing is set: the relayer, the statement
  signer, the Hyperliquid master, the HALT clearer, the bill wallet, the guardian
  (DEFAULTS below: each is the first `.tameion/<file>` after its variable in that
  source file). A key file is joined to its address through `<name>.address`
  beside it in ~/.tameion when there is one, else it is named by its path;
- the records under <app_dir>/tenants: each account's operator (the relayer that
  deploys and relays for it), its agent key, who anchored each root
  (`<policy>/log/anchors.jsonl`), and who signed each invoice (the payee's
  address, or `ml-dsa-65:<sha256 of the key>` for a post-quantum one).

Never opens a private key: a key file is known by its path and its address file.
"""

import hashlib
import json
import os
import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# (role, source file, the variable that overrides the default key file)
DEFAULTS = [
    ("relayer", "crates/operator/src/level1.rs", "TAMEION_RELAYER_KEY_FILE"),
    ("statement signer", "crates/operator/src/statement.rs", "TAMEION_STATEMENT_KEY_FILE"),
    ("Hyperliquid master", "crates/hedge/examples/approve_agent.rs", "TAMEION_TREASURY_KEY_FILE"),
    ("HALT clearer", "crates/hedge/examples/unhalt.rs", "TAMEION_OPERATOR_KEY_FILE"),
    ("bill wallet", "scripts/gateway_fund.sh", "TAMEION_TREASURY_KEY_FILE"),
    ("guardian", "scripts/guardian_approve.sh", "TAMEION_KEY_FILE"),
]
NOBODY = {"0x" + "0" * 40, "0x000000000000000000000000000000000000dead"}


def default_file(root, source, var):
    """The `.tameion/<file>` the source falls back to when `var` is unset."""
    try:
        text = (root / source).read_text()
    except OSError:
        return None
    at = text.find(var)
    m = re.search(r"\.tameion/([\w.\-]+)", text[at:]) if at >= 0 else None
    return m.group(1) if m else None


def key_id(home, name):
    """A key file's address from `<stem>.address` beside it, else its path."""
    addr = home / (Path(name).stem + ".address")
    try:
        a = addr.read_text().strip().lower()
        if re.fullmatch(r"0x[0-9a-f]{40}", a):
            return a, f"~/.tameion/{name}"
    except OSError:
        pass
    return f"file:~/.tameion/{name}", f"~/.tameion/{name}"


def from_defaults(root, home, roles, where):
    for role, source, var in DEFAULTS:
        name = default_file(root, source, var)
        if not name:
            continue
        kid, label = key_id(home, name)
        roles[kid].add(role)
        where[kid].add(f"{label} is the default {role} key ({source})")


def jsonl(path):
    try:
        for line in path.read_text().splitlines():
            try:
                yield json.loads(line)
            except ValueError:
                continue
    except OSError:
        return


def from_records(app_dir, roles, where):
    tenants = app_dir / "tenants"
    accounts = {}
    for f in sorted(tenants.glob("0x*.json")):
        if f.name.count(".") != 1:
            continue  # <policy>.preview.json, .identity.json, ...
        try:
            t = json.loads(f.read_text())
        except (OSError, ValueError):
            continue
        accounts[f.stem.lower()] = t
    ops = defaultdict(int)
    for policy, t in accounts.items():
        op = str(t.get("operator", "")).lower()
        agent = str(t.get("agent", "")).lower()
        if op and op not in NOBODY:
            roles[op].add("relayer")
            ops[op] += 1
        if agent and agent not in NOBODY and not t.get("keyless"):
            roles[agent].add("agent")
            where[agent].add(f"agent of {policy}")
        for a in jsonl(tenants / policy / "log" / "anchors.jsonl"):
            signer = str(a.get("agent", "")).lower()
            if not signer or signer in NOBODY:
                continue
            role = "agent" if signer == agent else "relayer" if signer == op else "anchor signer"
            roles[signer].add(role)
            if role == "anchor signer":
                where[signer].add(f"anchored a root of {policy}")
        for line in jsonl(tenants / f"{policy}.invoices.jsonl"):
            if line.get("status") != "received":
                continue
            if line.get("alg") == "ml-dsa-65" and line.get("publicKey"):
                try:
                    pk = bytes.fromhex(str(line["publicKey"]).removeprefix("0x"))
                except ValueError:
                    continue
                signer = "ml-dsa-65:" + hashlib.sha256(pk).hexdigest()[:16]
            else:
                signer = str(line.get("payee", "")).lower()
            if signer:
                roles[signer].add("invoice signer")
                where[signer].add(f"signed an invoice to {policy}")
    for op, n in ops.items():
        where[op].add(f"operator of {n} accounts")
    return len(accounts)


def main(argv):
    app_dir = Path(argv[1]) if len(argv) > 1 else ROOT / "runtime" / "app"
    home = Path(os.environ.get("TAMEION_HOME", Path.home() / ".tameion"))
    roles, where = defaultdict(set), defaultdict(set)
    from_defaults(ROOT, home, roles, where)
    accounts = from_records(app_dir, roles, where)
    crossed = sorted(k for k, r in roles.items() if len(r) > 1)
    per_role = defaultdict(int)
    for r in roles.values():
        for x in r:
            per_role[x] += 1
    print(f"key-roles: {len(roles)} keys, {accounts} accounts read from {app_dir}")
    print("  keys per role: " + ", ".join(f"{r} {n}" for r, n in sorted(per_role.items())))
    for k in crossed:
        print(f"CROSS-ROLE {k}: {', '.join(sorted(roles[k]))}")
        for w in sorted(where[k]):
            print(f"    {w}")
    if crossed:
        print(f"{len(crossed)} key(s) sign in more than one role: give each role its own key")
        return 1
    print("one role per key")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
