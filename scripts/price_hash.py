#!/usr/bin/env python3
"""Recompute a pinned price table's hash from its saved JSON (docs/121).

    python3 -I scripts/price_hash.py <app_dir>/price/<hash>.json [...] [--events <root>.events.jsonl]

The hash is keccak256 of the table's `rows` as canonical JSON: keys sorted, no
whitespace, prices as the decimal strings they are (crates/operator/src/price.rs,
`hash_rows`). Keccak-256 is computed here in pure Python (the original Keccak
padding, as Ethereum uses it; hashlib's sha3_256 pads differently), so the check
needs neither the service nor foundry. With --events, it also names the
PRICETABLE event of an anchored record that carries the hash.
Exit 0 when every file's recomputed hash equals its `hash` and its file name.
"""
import json
import sys

_RC = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
    0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
    0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
    0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
    0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
    0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
]
_ROT = [  # rotation offsets r[x][y]
    [0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61],
    [28, 55, 25, 21, 56], [27, 20, 39, 8, 14],
]
_M = (1 << 64) - 1


def _rol(v, n):
    return ((v << n) | (v >> (64 - n))) & _M if n else v


def _permute(a):
    for rc in _RC:
        c = [a[x][0] ^ a[x][1] ^ a[x][2] ^ a[x][3] ^ a[x][4] for x in range(5)]
        d = [c[(x - 1) % 5] ^ _rol(c[(x + 1) % 5], 1) for x in range(5)]
        a = [[a[x][y] ^ d[x] for y in range(5)] for x in range(5)]
        b = [[0] * 5 for _ in range(5)]
        for x in range(5):
            for y in range(5):
                b[y][(2 * x + 3 * y) % 5] = _rol(a[x][y], _ROT[x][y])
        a = [[b[x][y] ^ (~b[(x + 1) % 5][y] & b[(x + 2) % 5][y]) for y in range(5)] for x in range(5)]
        a[0][0] ^= rc
    return a


def keccak256(data: bytes) -> bytes:
    rate = 136
    p = bytearray(data) + b"\x01"
    p += b"\x00" * (-len(p) % rate)
    p[-1] |= 0x80
    a = [[0] * 5 for _ in range(5)]
    for off in range(0, len(p), rate):
        for i in range(rate // 8):
            a[i % 5][i // 5] ^= int.from_bytes(p[off + 8 * i:off + 8 * i + 8], "little")
        a = _permute(a)
    return b"".join(a[i % 5][i // 5].to_bytes(8, "little") for i in range(4))


assert keccak256(b"").hex() == "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
assert keccak256(b"abc").hex() == "4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45"


def table_hash(rows) -> str:
    canonical = json.dumps(rows, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return "0x" + keccak256(canonical.encode("utf-8")).hex()


def main(argv):
    events = None
    if "--events" in argv:
        i = argv.index("--events")
        events = [json.loads(l) for l in open(argv[i + 1]) if l.strip()]
        argv = argv[:i] + argv[i + 2:]
    if not argv:
        print(__doc__.strip())
        return 2
    bad = 0
    for path in argv:
        t = json.load(open(path))
        got = table_hash(t["rows"])
        named = path.rsplit("/", 1)[-1].removesuffix(".json")
        ok = got == t.get("hash") and (not named.startswith("0x") or named == got)
        bad += not ok
        print(f"{'PASS' if ok else 'FAIL'}  {path}: recomputed {got}; file says {t.get('hash')}; "
              f"{len(t['rows'])} rows from {t.get('source')} fetched {t.get('fetchedAt')}")
        if events is not None:
            # The event's canonical bytes carry the hash as a string: u64-BE length, then its bytes.
            needle = (len(got).to_bytes(8, "big") + got.encode()).hex()
            hits = [e["seq"] for e in events if e.get("kind") == "PRICETABLE" and needle in e["canonical"]]
            bad += not hits
            print(f"{'PASS' if hits else 'FAIL'}  the record's PRICETABLE event(s) {hits} carry this hash")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
