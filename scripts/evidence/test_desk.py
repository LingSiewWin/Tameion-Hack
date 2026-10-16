"""Offline tests for the evidence desk's verdicts. Run:

    python3 -m unittest scripts/evidence/test_desk.py
"""

import json
import sys
import unittest
from datetime import date
from decimal import Decimal
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import anchor  # noqa: E402
import desk  # noqa: E402

USDC_ARB = "0xaf88d065e77c8cc2239327c5edb3a432268e5831"
ME = "0x1111111111111111111111111111111111111111"
ME2 = "0x2222222222222222222222222222222222222222"
PAYER = "0xa1b2c3d4e5f60718293a4b5c6d7e8f9012345678"
LOOKALIKE = "0xa1b2ffffffffffffffffffffffffffffffff5678"


def inflow(payer, amount, token=USDC_ARB, name=None, tags=(), scam=False, ts="2026-09-30T09:00:00Z"):
    return desk.Inflow(
        chain="arbitrum", tx_hash="0x" + "ab" * 32, log_index=0, timestamp=ts,
        token="USDC", token_address=token, amount=str(amount), payer=payer,
        payer_name=name, payer_tags=list(tags), payer_flagged_scam=scam, recipient=ME,
    )


def verdict(rows, own=(ME,), payers=None):
    return desk.label(rows, set(own), payers or {})


class Verdicts(unittest.TestCase):
    def test_a_stated_payer_is_explained_with_the_earners_words(self):
        [i] = verdict([inflow(PAYER, "1500")], payers={PAYER: {"label": "ETHGlobal prize", "category": "prize"}})
        self.assertEqual((i.verdict, i.category), ("INCLUDED", "prize"))
        self.assertIn("ETHGlobal prize", i.basis)

    def test_an_unknown_payer_is_asked_never_guessed(self):
        [i] = verdict([inflow(PAYER, "1500")])
        self.assertEqual((i.verdict, i.category), ("ASK", "unlabelled"))

    def test_own_transfers_are_excluded(self):
        [i] = verdict([inflow(ME2, "500")], own=(ME, ME2))
        self.assertEqual((i.verdict, i.category), ("EXCLUDED", "own-transfer"))

    def test_a_token_that_only_calls_itself_usdc_is_excluded(self):
        [i] = verdict([inflow(PAYER, "1000", token="0x" + "99" * 20)])
        self.assertEqual((i.verdict, i.category), ("EXCLUDED", "not-a-stablecoin"))

    def test_dust_from_a_look_alike_is_poisoning_bait(self):
        rows = verdict([inflow(PAYER, "1500"), inflow(LOOKALIKE, "0.000001")],
                       payers={PAYER: {"label": "client", "category": "client"}})
        self.assertEqual((rows[1].verdict, rows[1].category), ("EXCLUDED", "poisoning-bait"))
        self.assertIn(PAYER, rows[1].basis)

    def test_real_money_from_a_look_alike_is_asked_about_not_accepted(self):
        rows = verdict([inflow(PAYER, "1500"), inflow(LOOKALIKE, "900")])
        self.assertEqual((rows[1].verdict, rows[1].category), ("ASK", "lookalike-payer"))

    def test_plain_dust_is_excluded(self):
        [i] = verdict([inflow(PAYER, "1")])
        self.assertEqual((i.verdict, i.category), ("EXCLUDED", "dust"))

    def test_an_exchange_withdrawal_is_recognised_from_explorer_tags(self):
        [i] = verdict([inflow(PAYER, "800", tags=["Binance 14"])])
        self.assertEqual((i.verdict, i.category), ("INCLUDED", "exchange-withdrawal"))

    def test_an_explorer_scam_flag_excludes(self):
        [i] = verdict([inflow(PAYER, "800", scam=True)])
        self.assertEqual((i.verdict, i.category), ("EXCLUDED", "flagged-scam"))

    def test_an_unrecognised_category_falls_back_to_other(self):
        [i] = verdict([inflow(PAYER, "50")], payers={PAYER: {"label": "x", "category": "lottery"}})
        self.assertEqual(i.category, "other")


class Rates(unittest.TestCase):
    def test_a_weekend_uses_the_last_business_day(self):
        def fake(url, headers):
            return {"data": {"rate": [{"date": "2026-09-25", "middle_rate": 4.1},
                                      {"date": "2026-09-28", "middle_rate": 4.2}]}}
        r = desk.Rates(fetch=fake)
        self.assertEqual(r.on_or_before(date(2026, 9, 27)), ("2026-09-25", Decimal("4.1")))
        self.assertEqual(r.on_or_before(date(2026, 9, 28)), ("2026-09-28", Decimal("4.2")))

    def test_excluded_lines_are_not_valued(self):
        rows = verdict([inflow(PAYER, "1")])
        desk.value(rows, desk.Rates(fetch=lambda u, h: {"data": {"rate": []}}))
        self.assertIsNone(rows[0].myr)


class Pack(unittest.TestCase):
    def pack(self, rows, salt="00" * 32):
        return desk.build_pack("x", [ME], ["arbitrum"], date(2026, 1, 1), date(2026, 10, 1), rows, salt=salt)

    def test_the_hash_covers_the_record_and_changes_with_it(self):
        # Same salt both times, so only the amount differs.
        rows = verdict([inflow(PAYER, "1500")], payers={PAYER: {"label": "p", "category": "prize"}})
        a = self.pack(rows)
        self.assertEqual(a["sha256"], self.pack(rows)["sha256"], "deterministic for a given salt")
        rows[0].amount = "1501"
        b = self.pack(rows)
        self.assertNotEqual(a["sha256"], b["sha256"])
        self.assertEqual(len(a["sha256"]), 64)

    def test_the_salt_keeps_the_published_hash_from_confirming_a_guess(self):
        rows = verdict([inflow(PAYER, "1500")])
        a = desk.build_pack("x", [ME], ["arbitrum"], date(2026, 1, 1), date(2026, 10, 1), rows)
        b = desk.build_pack("x", [ME], ["arbitrum"], date(2026, 1, 1), date(2026, 10, 1), rows)
        self.assertNotEqual(a["sha256"], b["sha256"], "identical records, different salts")
        self.assertEqual(len(a["salt"]), 64)

    def test_any_edit_to_a_written_pack_is_detected(self):
        rows = verdict([inflow(PAYER, "1500")], payers={PAYER: {"label": "p", "category": "prize"}})
        p = json.loads(json.dumps(self.pack(rows)))  # as read back from pack.json
        self.assertTrue(desk.verify_hash(p))
        p["inflows"][0]["amount"] = "15000"
        self.assertFalse(desk.verify_hash(p))


class Anchor(unittest.TestCase):
    CONTRACT = "0xDAb65A5380BCC2Eebc3A3A91d0AD11Eec95994dB"
    ROOT = "0x" + "ab" * 32

    def receipt(self, status="0x1", address=None, topic=anchor.ANCHORED_TOPIC, root=None, ts=1790000000):
        word = (root or self.ROOT).removeprefix("0x")
        return {"status": status, "logs": [{
            "address": (address or self.CONTRACT).lower(),
            "topics": [topic, "0x" + "00" * 12 + "11" * 20, "0x" + "00" * 32],
            "data": "0x" + word + format(ts, "064x"),
        }]}

    def test_reads_the_block_time_of_the_anchor(self):
        self.assertEqual(anchor.find_anchor(self.receipt(), self.CONTRACT, self.ROOT), 1790000000)

    def test_a_failed_transaction_anchors_nothing(self):
        self.assertIsNone(anchor.find_anchor(self.receipt(status="0x0"), self.CONTRACT, self.ROOT))

    def test_the_same_event_from_another_contract_does_not_count(self):
        r = self.receipt(address="0x" + "99" * 20)
        self.assertIsNone(anchor.find_anchor(r, self.CONTRACT, self.ROOT))

    def test_a_different_root_does_not_count(self):
        r = self.receipt(root="0x" + "cd" * 32)
        self.assertIsNone(anchor.find_anchor(r, self.CONTRACT, self.ROOT))

    def test_another_event_does_not_count(self):
        r = self.receipt(topic="0x" + "00" * 32)
        self.assertIsNone(anchor.find_anchor(r, self.CONTRACT, self.ROOT))


if __name__ == "__main__":
    unittest.main()
