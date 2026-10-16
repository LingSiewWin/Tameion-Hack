#!/usr/bin/env python3
"""Check the ERC-7730 clear-signing descriptors in contracts/erc7730/ (PLAN 5.19).

Every descriptor must parse, name at least one deployment, and give every
format an `intent` and a `fields` list. With python-jsonschema installed it
is also validated against the vendored official schema
(contracts/erc7730/erc7730-v2.schema.json); without it, the structural
check alone runs and says so. Exit code 1 on any failure.
"""
import glob
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DIR = os.path.join(HERE, "..", "contracts", "erc7730")
SCHEMA = os.path.join(DIR, "erc7730-v2.schema.json")

try:
    import jsonschema  # type: ignore
except ImportError:  # pragma: no cover
    jsonschema = None

failed = 0
files = sorted(glob.glob(os.path.join(DIR, "calldata-*.json")))
if not files:
    print("no descriptors found")
    sys.exit(1)
validator = None
# The gate must not pass by skipping its strongest check (docs/60 P-36): a
# Python without jsonschema fails here unless ERC7730_NO_SCHEMA=1 says the
# skip is deliberate.
if jsonschema is None and os.environ.get("ERC7730_NO_SCHEMA") != "1":
    print(f"python-jsonschema is not installed for {sys.executable}; install it (pip install jsonschema) "
          "or run with a Python that has it — or set ERC7730_NO_SCHEMA=1 to skip the schema on purpose")
    sys.exit(1)
if jsonschema is not None:
    schema = json.load(open(SCHEMA))
    cls = jsonschema.Draft202012Validator if "2020-12" in schema.get("$schema", "") else jsonschema.Draft7Validator
    validator = cls(schema)
for f in files:
    name = os.path.basename(f)
    try:
        d = json.load(open(f))
    except json.JSONDecodeError as e:
        print(f"{name}: not JSON: {e}")
        failed += 1
        continue
    problems = []
    if not d.get("context", {}).get("contract", {}).get("deployments"):
        problems.append("no context.contract.deployments")
    formats = d.get("display", {}).get("formats", {})
    if not formats:
        problems.append("no display.formats")
    for sig, fmt in formats.items():
        if not fmt.get("intent"):
            problems.append(f"{sig}: no intent")
        if not isinstance(fmt.get("fields"), list):
            problems.append(f"{sig}: no fields list")
    # Coverage (added 2026-10-14, docs/60 P-20): every state-changing
    # function the compiled contract exposes needs a format, or a wallet
    # shows raw bytes for it. Read from forge's artifact when it exists.
    contract = name[len("calldata-"):-len(".json")]
    art = os.path.join(HERE, "..", "contracts", "out", f"{contract}.sol", f"{contract}.json")
    if os.path.exists(art):
        abi = json.load(open(art)).get("abi", [])
        have = {sig.split("(")[0] for sig in formats}
        need = sorted({e["name"] for e in abi if e.get("type") == "function"
                       and e.get("stateMutability") not in ("view", "pure")})
        for fn in need:
            if fn not in have:
                problems.append(f"{fn}: state-changing function with no clear-signing format")
    if validator is not None:
        for e in validator.iter_errors(d):
            problems.append("schema: " + "/".join(str(x) for x in e.path) + ": " + e.message[:160])
    if problems:
        failed += 1
        print(f"{name}: FAIL")
        for p in problems[:10]:
            print("   ", p)
    else:
        print(f"{name}: ok ({len(formats)} formats, {len(d['context']['contract']['deployments'])} deployments"
              + (", schema valid" if validator is not None else ", schema check skipped: no python-jsonschema") + ")")
sys.exit(1 if failed else 0)
