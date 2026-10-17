"""scripts/key_roles.py: roles joined per key; a key in two roles is flagged."""

import json
import os
import sys
import tempfile
import unittest
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, os.path.dirname(__file__))
import key_roles as k  # noqa: E402

OP, AGENT, PAYEE = "0x" + "0a" * 20, "0x" + "0b" * 20, "0x" + "0c" * 20
POLICY = "0x" + "0d" * 20


def app(tmp, invoices):
    t = Path(tmp) / "tenants"
    (t / POLICY / "log").mkdir(parents=True)
    (t / f"{POLICY}.json").write_text(json.dumps({"operator": OP, "agent": AGENT, "keyless": False}))
    (t / f"{POLICY}.preview.json").write_text("{}")
    anchors = [{"agent": AGENT}, {"agent": OP}]
    (t / POLICY / "log" / "anchors.jsonl").write_text("\n".join(json.dumps(a) for a in anchors))
    (t / f"{POLICY}.invoices.jsonl").write_text("\n".join(json.dumps(i) for i in invoices))
    return Path(tmp)


class KeyRoles(unittest.TestCase):
    def roles(self, invoices):
        with tempfile.TemporaryDirectory() as tmp:
            roles, where = defaultdict(set), defaultdict(set)
            n = k.from_records(app(tmp, invoices), roles, where)
            self.assertEqual(n, 1, "the preview file is not an account")
            return roles

    def test_the_agent_anchoring_its_own_root_is_one_role(self):
        r = self.roles([
            {"status": "received", "payee": PAYEE},
            {"status": "received", "payee": PAYEE, "alg": "ml-dsa-65", "publicKey": "0x" + "ab" * 1952},
        ])
        self.assertEqual(r[AGENT], {"agent"})
        self.assertEqual(r[OP], {"relayer"})
        self.assertEqual(r[PAYEE], {"invoice signer"})
        ml = [x for x in r if x.startswith("ml-dsa-65:")]
        self.assertEqual(len(ml), 1, "the post-quantum key is its own key")
        self.assertFalse([x for x, v in r.items() if len(v) > 1])

    def test_an_agent_key_that_also_signs_invoices_is_flagged(self):
        r = self.roles([{"status": "received", "payee": AGENT}])
        self.assertEqual(r[AGENT], {"agent", "invoice signer"})

    def test_the_code_defaults_put_five_roles_on_one_key_file(self):
        with tempfile.TemporaryDirectory() as home:
            Path(home, "treasury.address").write_text(OP + "\n")
            roles, where = defaultdict(set), defaultdict(set)
            k.from_defaults(k.ROOT, Path(home), roles, where)
            self.assertEqual(
                roles[OP], {"relayer", "Hyperliquid master", "HALT clearer", "bill wallet", "guardian"}
            )
            self.assertEqual(roles["file:~/.tameion/statement.key"], {"statement signer"})


if __name__ == "__main__":
    unittest.main()
