# Tami

**The account an agent operator gives its agents.** Agents pay for APIs, vendors and hosting in USDC, around the clock, inside a mandate you signed once. The contract refuses the rest. Every payment, every refusal and every reason lands in a book both sides sign.

Built for the Tameion Agents Hackathon (Canteen × Circle × Arc). Testnet only. The name comes from *tamias*, the Greek steward of the treasury. The record both sides keep is a *symbolon*.

## Try it in five minutes

| | |
|---|---|
| **Owner view, no sign in** | https://tami-home.up.railway.app/a/guest-demo-arc |
| **Agent view, same account** | https://tami-home.up.railway.app/a/guest-demo-arc?view=agent |
| **Make your own account** | https://tami-console.up.railway.app (the invite word is in the submission form) |

Open the owner view. An agent on Arc testnet is paying a demo vendor every two minutes on Autopilot. Flip the toggle to the agent view and the same payments appear as records with three signature slots. To make your own account, name it, tap your passkey, and it arrives funded with 0.05 test USDC. Approve one bill, turn on Autopilot, watch it pay, raise the cap and watch it wait, cut the cap and watch it apply at once.

## What it does

1. **A mandate the contract enforces.** A cap per payment, a trailing 24 hour cap, a reserve floor, a payee list with a cooling off period, and Watch, Propose or Autopilot as contract state. Raising anything waits with a visible cancel. Cutting is instant.
2. **An owner who can lose a device.** The passkey owns the account. Two guardians can replace a lost passkey after a delay the passkey can cancel. Guardians cannot spend.
3. **A book of what was owed and why.** Obligations read from payment history, signed invoices from payees, standing orders. Every line sits under a root anchored on chain, and anyone can recompute it without trusting us.
4. **Three signatures per record.** The agent's, the counterparty's when a signed invoice is settled, and a witness read back from the chain and the Circle Gateway balance.
5. **Rates, not just balances.** Burn rate folds into runway, so the owner reads days and the agent reads a cockpit.
6. **The model proposes, the code bounds.** On 12 labelled cases the rules got 10, the model got 72 percent, and the code blocked all six times the model would have overpaid.

## The brief's three checks, as tests

> A ledger checks that debits equal credits. It does not check that the vendor was the right one, that the invoice was real, or that a retry didn't pay it twice.

| Check | Test | What happens | Proof |
|---|---|---|---|
| The vendor was the right one | `wrong_vendor` | an unapproved payee is refused by the contract, nothing sent | proof/organiser-sentences-2026-10-17.txt |
| The invoice was real | `phantom_invoice` | an invoice the payee did not sign gets 422 and never enters the record | same file |
| A retry didn't pay it twice | `double_pay_on_retry` | one idempotency key sent four times, one payment | same file |

## What is proved

Every row ran against a real chain. The file holds the requests, answers and refusals.

| Claim | Proof |
|---|---|
| An outside agent, over MCP, buys inside the mandate in 8.3 s. Above the cap it is refused in words. | proof/uat-u3-2026-10-16.txt |
| The contract says no, not our server. The agent's key bypassed our service and reverted `OverPerActionCap`. | same file |
| A lost passkey comes back through two guardians. The old key is refused afterwards. | proof/recovery-drill-2026-10-16.txt |
| The agent paid a bill by itself on Arc testnet, and early for a discount only because every other bill stayed covered. | proof/early-discount-2026-10-14.txt |
| Anyone can recompute the anchored root from the events. One changed byte fails it. A stranger did it from a clean clone. | proof/clean-clone-submission-2026-10-18.txt |
| Signature algorithm switched live on chain, register, switch, old algorithm refused, new one accepted. | proof/passkey-agility-2026-10-18.txt |
| Money moved in from another chain by CCTP, burn on Arbitrum Sepolia, attestation, mint on Arc. | proof/home-2026-10-18/h5/cctp-move-proof.txt |
| Credit drawn against the book on Arc testnet, lock, draw, repay, slash, all four confirmed. No external lender, not a lending protocol. | proof/credit-line-2026-10-18.txt |
| Signature counts on the service, 237 records: 236 with one signature, 1 with two, none with three yet. | proof/symbolon-count-2026-10-18-4ce997c.txt |

## The honest state

No customers and no outside operator yet. Every account is the team's and the money moved is cents. The model decides little today, the rules decided all 91 finished passes. The agent has only ever bought from our own x402 seller, because none of the 1,401 payment options in the Bazaar accept an ERC-7710 payment. Contracts are unaudited, there is no mainnet, and the passkey can withdraw everything at once by design. The landing page's satellite scene is NOAA-20 imagery and its vessel tracks are illustrative, and a test fails if the page ever calls them real. Full detail in [DETAILS.md](DETAILS.md).

## Where to look

| You want | Go to |
|---|---|
| The mandate | `contracts/src/BudgetPolicy.sol`, `contracts/src/PasskeyPrincipal.sol` |
| The book | `crates/ledger/src/entry.rs`, `symbolon.rs`, `momentum.rs` |
| How the agent decides and explains | `crates/operator/src/decide.rs` |
| The three named tests | `crates/operator/tests/organiser.rs` |
| Signature counts | `crates/operator/examples/symbolon_count.rs` |
| The owner and agent views | `apps/home` (design rules in `apps/home/DESIGN.md`) |
| The MCP server and CLI | `crates/operator/src/bin/tameion-mcp.rs`, `apps/sdk` |
| The credit primitive | `contracts/src/credit/` |
| Every real run | `proof/` |

## Run it

```bash
git clone --recurse-submodules --branch submission-2026-10-18 https://github.com/LingSiewWin/Tami && cd Tami
(cd apps/console && npm ci) && (cd apps/sdk && npm ci)
scripts/test_all.sh            # the gate, 18 steps
scripts/dev.sh                 # the service on :7880
```

Needs Rust, Foundry, Node 20 and Python 3. The full setup, the audit command that recomputes a root against two public nodes, and the home surface's own gate are in [DETAILS.md](DETAILS.md).

## Naming and prior work

The repository, crates and binaries still say `tameion` in places until the rename lands. `MemoryAnchor.sol` comes from [arc-agent-kit](https://github.com/LingSiewWin/arc-agent-kit), extracted from Arcane, my entry to the Agora round. Nothing else is imported.
