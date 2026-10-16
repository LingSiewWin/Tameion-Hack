# 2026-10-16 23:55 +0800 The check README.md tells a judge to run, from a fresh clone, with no service of ours running.
# Events: the committed files in proof/arc-log-2026-10-13/. Chain: Arc testnet (5042002), read through a public node.
# The default node https://rpc.testnet.arc.network answered 'Max retries exceeded error sending request' at 23:47 +0800 (public nodes rate-limit); the second public node below answered.

$ AUDIT_EVENTS=proof/arc-log-2026-10-13/0xec4c519904baf7e870953d3dbd3c4553671748957fca65745b02fcdd123a0b94.events.jsonl TAMEION_LOG_DIR=proof/arc-log-2026-10-13 AGENT=0x80c195BE650FBfD55a9ffe059f397Db39f2fe881 RPC_URL=https://explorer.testnet.arc.io/api/eth-rpc cargo run --locked -q -p operator --example audit
# audit proof/arc-log-2026-10-13/0xec4c519904baf7e870953d3dbd3c4553671748957fca65745b02fcdd123a0b94.events.jsonl
[0000] OBSERVATION Holding 0.034450 USDC. Read 21 transfers (2 of them my own payments, which are not evidence) and recovered 3 recurring obligations, of which 3 fall due within 30.0 days totalling 0.037000 USDC; 3 within 3.0 days (or a tenth of their interval), which is when I pay.
[0001] DECISION    REVIEW for 0x00000000000000000000000000000000000000a1. Smallest obligation with zero amount variation and the tightest timing regularity, so it is the cheapest fixed subscription to keep current and clearing it leaves the most balance for the others. Basis: 0x00000000000000000000000000000000000000a1 has been paid 6 times every 3.0m, amounts varying by 0.0% and timing by 0.6%, so
[0002] ESCALATION  UNANSWERED for 11.7 days, past the 7.0-day deadline: cycles-behind: 5614 more cycles of this bill fell due after this one and nobody paid                  them: it may have ended. Settle one by hand to confirm, and I will                  continue from there. I have escalated this and nobody has acted. The bill is still unpaid and I cannot pay it myself.
[0003] DECISION    REVIEW for 0x00000000000000000000000000000000000000a3. Largest exposure and an exact, unvarying subscription amount, so it is the most likely of the remainder to trigger service withdrawal if left unpaid, though I have no counterparty signal beyond the payment history. Basis: 0x00000000000000000000000000000000000000a3 has been paid 6 times every 3.0m, amounts varying by 0.0% and
[0004] ESCALATION  UNANSWERED for 11.7 days, past the 7.0-day deadline: per-action-cap: 0.030000 USDC over cap 0.010000 USDC; cycles-behind: 5584 more cycles of this bill fell due after this one and nobody paid                  them: it may have ended. Settle one by hand to confirm, and I will                  continue from there. I have escalated this and nobody has acted. The bill is still unpai
[0005] DECISION    REVIEW for 0x00000000000000000000000000000000000000a2. Amounts vary by 13.9%, so the billed figure is less certain than the fixed subscriptions and a short delay risks less than underpaying or overpaying a drifting arrangement. Basis: 0x00000000000000000000000000000000000000a2 has been paid 6 times every 3.0m, amounts varying by 13.9% and timing by 1.3%, so this reads as a recur
[0006] ESCALATION  UNANSWERED for 11.7 days, past the 7.0-day deadline: cycles-behind: 5614 more cycles of this bill fell due after this one and nobody paid                  them: it may have ended. Settle one by hand to confirm, and I will                  continue from there. I have escalated this and nobody has acted. The bill is still unpaid and I cannot pay it myself.
events          : 7
recomputed root : 0xec4c519904baf7e870953d3dbd3c4553671748957fca65745b02fcdd123a0b94
file name       : 0xec4c519904baf7e870953d3dbd3c4553671748957fca65745b02fcdd123a0b94   MATCH
chain rootOf    : 0xb30efdbd0171ecbbd9daeac83d5ddd090a096440c2a6df47dadf402b0d986b70   (anchor 0xDAb65A5380BCC2Eebc3A3A91d0AD11Eec95994dB, agent 0x80c195BE650FBfD55a9ffe059f397Db39f2fe881)   LATER ROOT ON CHAIN — this root was anchored in 0xbaf18684efd7c5d9e32a726e0850a64c7f23bd96b35eff7c59ad4bef3ce18b73: checked on chain (sent by the agent to the anchor, succeeded, the root in its calldata)
RESULT          : PASS

$ AUDIT_EVENTS=proof/arc-log-2026-10-13/0xb30efdbd0171ecbbd9daeac83d5ddd090a096440c2a6df47dadf402b0d986b70.events.jsonl TAMEION_LOG_DIR=proof/arc-log-2026-10-13 AGENT=0x80c195BE650FBfD55a9ffe059f397Db39f2fe881 RPC_URL=https://explorer.testnet.arc.io/api/eth-rpc cargo run --locked -q -p operator --example audit
# audit proof/arc-log-2026-10-13/0xb30efdbd0171ecbbd9daeac83d5ddd090a096440c2a6df47dadf402b0d986b70.events.jsonl
[0000] HEDGE       ETH: spot 0.01 @ 2484.24 = 24.8424 USDC; hedge 0 -> target -0.01; drift 10000 bps, outside band; refused: NoCollateral; no order; equity 0
[0001] PROPOSAL    claude-opus-5 proposes pause for ETH: The hedge was refused for NoCollateral with equity of 0 USDC and funding at -0.000626 hourly running against the short, so hedging ETH cannot be sustained until collateral is restored.unused. Recorded for a human; nothing changed.
[0002] OBSERVATION Holding 0.006200 USDC. Read 17 transfers (2 of them my own payments, which are not evidence) and recovered 3 recurring obligations, of which 3 fall due within 30.0 days totalling 0.037350 USDC; 1 within 3.0 days (or a tenth of their interval), which is when I pay.
[0003] DECISION    BLOCK for 0x00000000000000000000000000000000000000a3. only obligation due; no ordering decision to make Basis: 0x00000000000000000000000000000000000000a3 has been paid 5 times every 5.0m, amounts varying by 0.0% and timing by 0.7%, so this reads as a subscription (confidence 0.80).
events          : 4
recomputed root : 0xb30efdbd0171ecbbd9daeac83d5ddd090a096440c2a6df47dadf402b0d986b70
file name       : 0xb30efdbd0171ecbbd9daeac83d5ddd090a096440c2a6df47dadf402b0d986b70   MATCH
chain rootOf    : 0xb30efdbd0171ecbbd9daeac83d5ddd090a096440c2a6df47dadf402b0d986b70   (anchor 0xDAb65A5380BCC2Eebc3A3A91d0AD11Eec95994dB, agent 0x80c195BE650FBfD55a9ffe059f397Db39f2fe881)   MATCH
RESULT          : PASS
