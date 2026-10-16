"""Offline checks of scripts/verify_desk.py on real files (docs/96): no network, no cast."""
import json
import os
import subprocess
import sys
import unittest
from decimal import Decimal

sys.path.insert(0, os.path.dirname(__file__))
import verify_desk as v  # noqa: E402

FIX = os.path.join(os.path.dirname(__file__), "..", "apps", "console", "test", "fixtures")


class VerifyDesk(unittest.TestCase):
    def book(self, name):
        return os.path.join(FIX, name)

    def run_book(self, name):
        # whole_book refuses before any network check; cast must exist for main() to start.
        return subprocess.run([sys.executable, v.__file__, self.book(name)], capture_output=True, text=True)

    def test_v2_v6_refused_books(self):
        for name in ("desk-book-mixed.json", "desk-book-dup.json"):
            r = self.run_book(name)
            self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
            self.assertIn("not one statement of the whole stock", r.stdout)

    def test_w1_book_that_leaves_out_a_deal(self):
        r = self.run_book("desk-book-omitted.json")
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn("leaves out stock", r.stdout)

    def test_v6_empty_book(self):
        empty = os.path.join(os.environ.get("TMPDIR", "/tmp"), "verify-desk-empty.json")
        with open(empty, "w") as f:
            json.dump({"kits": []}, f)
        r = subprocess.run([sys.executable, v.__file__, empty], capture_output=True, text=True)
        os.remove(empty)
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn("lists no deals", r.stdout)

    def test_v5_funding_share(self):
        sts = json.load(open(self.book("desk-statements-funding.json")))
        for name, st in sts.items():
            acct = Decimal(name.split("acct ")[1])
            held_mg = Decimal(v.grams_minor(st["held"]))
            self.assertTrue(v.funding_ok(st, held_mg, acct), name)
            if abs(acct) >= 1:
                forged = dict(st, fundingUsdc=str(Decimal(st["fundingUsdc"]) * 2))
                self.assertFalse(v.funding_ok(forged, held_mg, acct), name + " doubled")


if __name__ == "__main__":
    unittest.main()
