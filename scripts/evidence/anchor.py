#!/usr/bin/env python3
"""Put a pack's hash on chain, and check a pack against it.

    python3 scripts/evidence/anchor.py anchor runtime/evidence/you
    python3 scripts/evidence/anchor.py verify runtime/evidence/you

`pack.py` reads public data and signs nothing. This is the one step that
sends: it writes the pack's sha256 to `MemoryAnchor` on Arc testnet with
`anchor(bytes32)`. Only the hash leaves this machine, and the pack carries a
random salt inside that hash, so publishing it reveals nothing about the
pack and lets nobody confirm a guess about whose it is.

What the anchor proves: the record existed, exactly as it is, no later than
the block it landed in. Change one amount and `verify` says so. It does not
prove the record was right; every line still has to be checked on its own
explorer link.

`verify` needs no key: the pack directory and a public RPC.

Uses Foundry's `cast` for the chain calls, as the rest of the repository does.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import desk  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
ARC_RPC = "https://rpc.testnet.arc.network"
ARC_CHAIN_ID = 5042002
ARC_EXPLORER = "https://explorer.testnet.arc.io"
# keccak256("MemoryAnchored(address,uint256,bytes32,uint256)"), from `cast keccak`.
ANCHORED_TOPIC = "0x5826c51063b81b5692a535aef36f336d4508aa9f5b704dfc92a597e7f3f2eb23"


def cast(*args: str) -> str:
    out = subprocess.run(["cast", *args], capture_output=True, text=True)
    if out.returncode != 0:
        raise RuntimeError(f"cast {args[0]} failed: {out.stderr.strip()[:300]}")
    return out.stdout.strip()


def find_anchor(receipt: dict, contract: str, root: str) -> int | None:
    """The block time a successful receipt anchored `root` at `contract`, or None.

    Reads the `MemoryAnchored(agent, identityId, root, timestamp)` event: topics
    are the signature, the agent and the identity; the data holds the root and
    the timestamp, one 32-byte word each.
    """
    if receipt.get("status") not in ("0x1", 1, "1"):
        return None
    want = root.lower().removeprefix("0x").rjust(64, "0")
    for log in receipt.get("logs", []):
        if log.get("address", "").lower() != contract.lower():
            continue
        topics = [t.lower() for t in log.get("topics", [])]
        if not topics or topics[0] != ANCHORED_TOPIC:
            continue
        data = log.get("data", "").lower().removeprefix("0x")
        if data[:64] == want:
            return int(data[64:128], 16)
    return None


def load(pack_dir: Path) -> dict:
    pack = json.loads((pack_dir / "pack.json").read_text())
    if not desk.verify_hash(pack):
        raise SystemExit(f"{pack_dir}/pack.json does not match its own sha256: it was edited after it was written")
    return pack


def when(ts: int) -> str:
    return datetime.fromtimestamp(ts, timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")


def do_anchor(pack_dir: Path, rpc: str, contract: str, key_file: Path) -> int:
    pack = load(pack_dir)
    if str(cast("chain-id", "--rpc-url", rpc)) != str(ARC_CHAIN_ID):
        raise SystemExit(f"{rpc} is not Arc testnet ({ARC_CHAIN_ID})")
    root = "0x" + pack["sha256"]
    receipt = json.loads(cast("send", contract, "anchor(bytes32)", root,
                              "--rpc-url", rpc, "--private-key", key_file.read_text().strip(), "--json"))
    ts = find_anchor(receipt, contract, root)
    if ts is None:
        raise SystemExit(f"transaction {receipt.get('transactionHash')} did not anchor {root}")
    record = {
        "chain": "arc-testnet",
        "chain_id": ARC_CHAIN_ID,
        "contract": contract,
        "tx": receipt["transactionHash"],
        "block": int(receipt["blockNumber"], 16),
        "timestamp": ts,
        "sha256": pack["sha256"],
        "anchored_by": receipt.get("from"),
    }
    (pack_dir / "pack.anchor.json").write_text(json.dumps(record, indent=2))
    # Outside the hashed record, so adding it changes nothing it proves.
    with (pack_dir / "pack.md").open("a") as md:
        md.write(f"\nAnchored on Arc testnet at {when(ts)}, block {record['block']}: "
                 f"[{record['tx'][:10]}…]({ARC_EXPLORER}/tx/{record['tx']}). "
                 f"Check with `python3 scripts/evidence/anchor.py verify <this folder>`.\n")
    print(f"anchored sha256 {pack['sha256'][:16]}… at {when(ts)}, block {record['block']}")
    print(f"tx {record['tx']}")
    return 0


def do_verify(pack_dir: Path, rpc: str) -> int:
    pack = load(pack_dir)
    print(f"record matches its sha256 {pack['sha256'][:16]}…")
    path = pack_dir / "pack.anchor.json"
    if not path.exists():
        print("not anchored: nothing proves when it was written")
        return 1
    rec = json.loads(path.read_text())
    if rec["sha256"] != pack["sha256"]:
        print(f"ANCHOR MISMATCH: pack.anchor.json is for {rec['sha256'][:16]}…, not this record")
        return 1
    receipt = json.loads(cast("receipt", rec["tx"], "--rpc-url", rpc, "--json"))
    ts = find_anchor(receipt, rec["contract"], "0x" + pack["sha256"])
    if ts is None:
        print(f"ANCHOR NOT FOUND: {rec['tx']} does not anchor this record at {rec['contract']}")
        return 1
    print(f"VERIFIED: this record existed, unchanged, by {when(ts)} "
          f"(Arc testnet block {int(receipt['blockNumber'], 16)}, tx {rec['tx']})")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("action", choices=["anchor", "verify"])
    ap.add_argument("pack_dir", type=Path)
    ap.add_argument("--rpc", default=ARC_RPC)
    ap.add_argument("--contract", help="MemoryAnchor address (default: runtime/arc_anchor.addr)")
    ap.add_argument("--key-file", type=Path, default=Path.home() / ".tameion" / "treasury.key")
    a = ap.parse_args()
    if a.action == "verify":
        return do_verify(a.pack_dir, a.rpc)
    contract = a.contract or (ROOT / "runtime" / "arc_anchor.addr").read_text().strip()
    return do_anchor(a.pack_dir, a.rpc, contract, a.key_file)


if __name__ == "__main__":
    sys.exit(main())
