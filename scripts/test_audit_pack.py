"""Offline checks of scripts/audit_pack.py on a real account's files (crates/operator/tests/fixtures/audit-pack, a trimmed
copy of the Arbitrum Sepolia account uat2-u1-rc): the two nodes' and Circle's answers are replayed from answers.json,
recorded from a live run (AUDIT_PACK_RECORD). Needs cast: invoice signatures, and Keccak when hashlib lacks it."""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
FIX = os.path.join(HERE, "..", "crates", "operator", "tests", "fixtures", "audit-pack")
POLICY = "0xdaf055fff0ad86f112a5f7f9055eee4e5a9ad4c1"
BLOCK = "320121361"  # the block the answers were recorded at
SETTLED = "0xbef05bf7a7ec4a22611d4c37e7fcb9d3cc095f09297b311b9c73422ed291b7a2"  # the record holding the x402 payment


def build(app_dir, out):
    answers = "file://" + os.path.abspath(os.path.join(FIX, "answers.json"))
    return subprocess.run([sys.executable, os.path.join(HERE, "audit_pack.py"), app_dir, POLICY, "2026-10-16",
                           "2026-10-17", "--rpc", answers, "--rpc2", answers, "--gateway", answers, "--block", BLOCK,
                           "--out", out], capture_output=True, text=True)


@unittest.skipUnless(shutil.which("cast"), "cast (foundry) is not installed")
class AuditPack(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.out = os.path.join(self.tmp, "out")

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def read(self, name):
        with open(os.path.join(self.out, POLICY, "2026-10-16_2026-10-17", name), encoding="utf-8") as f:
            return f.read()

    def test_the_ledger_reconciles_to_the_chain(self):
        r = build(FIX, self.out)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        ledger = self.read("ledger.beancount")
        self.assertIn("2026-10-18 balance Assets:USDC:Arbitrum-Sepolia:0xdaf055  0.092000 ~ 0 USDC", ledger)
        # Two agent payments the chain shows and this app dir's anchored record does not hold, flagged.
        self.assertEqual(ledger.count("\n2026-10-17 ! "), 2)
        self.assertIn('invoice: "0xe5725877d212151cfe651dd8c674993ef5745038269da699755625126e5b3d70"', ledger)
        self.assertEqual(json.loads(self.read("countersign.json"))["ratio"], "1/5")
        roots = json.loads(self.read("root.json"))
        self.assertEqual([x["verdict"] for x in roots["records"]], ["MATCH", "MATCH", "MATCH"])
        self.assertEqual(roots["now"]["verdict"], "MATCH")
        invoices = json.loads(self.read("signatures.json"))["invoices"]
        self.assertEqual([(i["signature"], i["storedIdMatches"]) for i in invoices], [("valid", True)] * 2)
        gateway = json.loads(self.read("sources.json"))["witnesses"][1]
        self.assertEqual((gateway["alg"], gateway["keyId"]),
                         ("balance-read", "gateway:0x3c559e1152b109323eb70ad0fcf6d4e7aa9210f5"))
        self.assertIn("Absence of bias", self.read("README.md"))

    def test_an_edited_event_fails_against_its_anchored_root(self):
        # docs/117 §5 threat model: a pack built from edited events fails the recompute against the root on chain.
        app = os.path.join(self.tmp, "app")
        shutil.copytree(FIX, app)
        path = os.path.join(app, "tenants", POLICY, "log", f"{SETTLED}.events.jsonl")
        with open(path, encoding="utf-8") as f:
            lines = f.read().splitlines()
        event = json.loads(lines[0])
        # The settlement's amount: i128 1000 then the USDC tag, made 1001.
        edited = event["canonical"].replace("000000000000000000000000000003e801", "000000000000000000000000000003e901", 1)
        self.assertNotEqual(edited, event["canonical"])
        lines[0] = json.dumps(dict(event, canonical=edited))
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
        r = build(app, self.out)
        self.assertEqual(r.returncode, 1, r.stdout + r.stderr)
        self.assertIn(f"record {SETTLED}: its events recompute to another root", r.stdout)
        self.assertIn("the chain shows another payee, amount or payer in that transaction", r.stdout)
        if shutil.which("bean-check"):
            self.assertIn("Balance failed", r.stdout)


if __name__ == "__main__":
    unittest.main()
