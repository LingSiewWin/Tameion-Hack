"""Offline checks of the chip bridge's owner-contract calls (docs/104 §E2): the challenge it recomputes equals the
contract's own `challengeFor` (values read from Arbitrum Sepolia on 2026-10-16), the words come from the calldata, and
anything else is refused. Needs Foundry's `cast` (keccak), no network."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import desk_se_bridge as b  # noqa: E402

OWNER = "0xb2B6E1471755b4C77f1D01b80d3aAD8d1A6baBd9"
A, B = "0x5f48d8f887ada7da30228a2a236e49001e7e002a", "0x67b13190c7e25d98950fc3153623560e66c56e0f"
WORD = lambda a: "0" * 24 + a[2:]  # noqa: E731
GUARDIANS = "0xe94e9e99" + "0" * 62 + "40" + "0" * 63 + "2" + "0" * 63 + "2" + WORD(A) + WORD(B)


def call(data, nonce=5, target=OWNER):
    return {"kind": "call", "chainId": 421614, "owner": OWNER, "nonce": str(nonce), "target": target, "data": data}


class OwnerCalls(unittest.TestCase):
    def test_guardians_challenge_is_the_contracts_own_and_the_words_name_both(self):
        ch, say = b.decide(call(GUARDIANS))
        # cast call 0xb2B6…aBd9 'challengeFor(address,uint256,bytes,uint256)(bytes32)' <owner> 0 <data> 5
        self.assertEqual(ch, "0x365eaab05f6fbd87215b8310b4d871273ce88efc46a1ec3af3b73cb2d31d8d81")
        self.assertIn(f"Name 2 guardians of account {OWNER}: {A}, {B}; 2 of them together", say)

    def test_cancel_and_recovery_words(self):
        ch, say = b.decide(call("0x0ba234d6", nonce=7))
        self.assertEqual(ch, "0x926412d0855edee8f5f30079d0e85001641bc7dd96a0b7250276832bee841f2a")
        self.assertEqual(say, f"Cancel the waiting replacement of the key of account {OWNER}")
        self.assertIn("Remove the address", b.decide(call("0xf0d85c89" + "0" * 64))[1])
        self.assertIn(f"Let {A} alone propose", b.decide(call("0xf0d85c89" + WORD(A)))[1])

    def test_anything_else_is_refused(self):
        withdraw = "0xf3fef3a3" + WORD(A) + "0" * 63 + "1"
        for d, why in [
            (call(GUARDIANS, target=A), "own owner contract"),  # the same bytes aimed elsewhere
            (call(withdraw), "words only"),  # a call the bridge cannot put in words
            (call(GUARDIANS[:-2]), "whole 32-byte words"),  # a cut call
            (call("0xe94e9e99" + "0" * 62 + "40" + "0" * 63 + "2" + "0" * 63 + "3" + WORD(A) + WORD(B)), "not a setGuardians call"),  # says 3 names, holds 2
        ]:
            with self.assertRaises(ValueError) as e:
                b.decide(d)
            self.assertIn(why, str(e.exception))

    def test_an_unattended_bridge_never_decides_who_can_replace_the_key(self):
        # As scripts/dev.sh runs the demo bridge: --unattended, no --allow-unattended-calls.
        b.UNATTENDED, b.ALLOW_UNATTENDED_CALLS = True, False
        try:
            for data in (GUARDIANS, "0x0ba234d6", "0xf0d85c89" + WORD(A)):
                with self.assertRaises(b.Refused) as e:
                    b.decide(call(data))
                self.assertEqual(str(e.exception), "guardian and recovery changes need a key that asks for Touch ID")
            # Proof runs only: the explicit flag lets the same call be worded and signed.
            b.ALLOW_UNATTENDED_CALLS = True
            self.assertEqual(b.decide(call(GUARDIANS))[0], "0x365eaab05f6fbd87215b8310b4d871273ce88efc46a1ec3af3b73cb2d31d8d81")
        finally:
            b.UNATTENDED, b.ALLOW_UNATTENDED_CALLS = False, False


if __name__ == "__main__":
    unittest.main()
