# @tameion/sdk — a Tameion account from code or a terminal

Your agent reads the account, asks for a check, and buys paid APIs over x402. It can only pay sellers the owner approved, and only up
to the mandate the owner signed. The owner's passkey holds the keys. The agent holds a bounded key that cannot add a payee, raise a
limit or withdraw. The contract enforces the limit, not this SDK.

TypeScript, ESM, zero runtime dependencies, `fetch` only, Node 20+.

```bash
cd apps/sdk && npm install && npm run build      # dist/src/client.js, dist/src/cli.js
npm test                                          # tsc, then node --test with a fake fetch
```

## The CLI

```bash
export TAMEION_URL=http://localhost:7880 TAMEION_POLICY=0x…            # the service and the account
tameion inbox  --token-file ~/.tameion/agent.token                     # or TAMEION_TOKEN in the environment
tameion bills  --token-file ~/.tameion/agent.token
tameion status
tameion buy 'https://seller.example/v1/report' --max 0.01 --why "weekly research report" --token-file ~/.tameion/agent.token
```

| Command | Does | Needs scope |
|---|---|---|
| `status` | contract version, recent updates, the kill switches | none |
| `inbox` | mode, balance, limits (spent in 24 h, left now), payees, everything waiting for the owner, what was paid | `read` |
| `bills` | the bills the agent knows, standing orders, and bill payments waiting for the owner | `read` |
| `buy <url> --max <usdc> --why <text> [--op <id>]` | one call of a paid API, paid from the account inside the mandate | `buy` |

`--json` prints the service's reply as JSON. Exit codes: **0** delivered, **1** the service refused or the outcome is unknown,
**2** usage (nothing was sent).

There is no `pay <payee> <amount>`. No route lets an agent pay a payee directly. The agent pays bills on its own checks (`run`, on
Autopilot, inside the caps) and pays sellers through `buy`.

## The client

```ts
import { TameionClient, newOperationId } from "@tameion/sdk";

const t = new TameionClient({ baseUrl: "https://tameion.example", token: process.env.TAMEION_TOKEN });
const inbox = await t.inbox(policy);
if (inbox.ok) console.log(inbox.data.caps.leftNow, inbox.data.waitingCount);

const operationId = newOperationId();               // keep it until you know the outcome
const r = await t.buy({ policy, url, maxUsdc: "0.01", reason: "weekly research report", operationId });
if (r.ok) console.log(r.data.purchase?.tx, r.replayed);
else if (r.error.executed === "unknown") { /* read the inbox; resend only with the same operationId */ }
else if (r.error.audience === "user") { /* a person must act on the account page; do not retry */ }
```

Methods: `status(policy?)`, `inbox(policy)`, `run(policy)` (a check now; one per account per ten minutes, otherwise the recent one
answers), `bills(policy, bills)` (tell the agent about bills; scope `import`; it pays nothing by itself), `buy(input)`.
Every method returns `{ ok: true, data }` or `{ ok: false, error }`, plus `status`, `replayed` and `raw`. A method throws only when
it is called wrongly: a bad `baseUrl`, or a spend with no valid `operationId`.

## Behaviours, how each one fails, and what to do next

| Behaviour | When it fails | What to do |
|---|---|---|
| `baseUrl` must be `https:`. Plain `http:` works only for `localhost`, `127.0.0.1` and `[::1]`, because the agent key travels in every request. | The constructor throws, or the CLI exits 2. Nothing is sent. | Use the service's https address. |
| Every spend (`buy`) needs an `operationId`: `op_` plus a UUID, 8–128 characters of `[A-Za-z0-9_.:-]`. It is sent as `Idempotency-Key`. | A missing or malformed id throws before anything is sent. | Use `newOperationId()`. Make one id per purchase, not one per attempt. |
| The spend is saved before it is sent. The client keeps `{operationId, path, body, sends}`. With `save` set, it awaits your hook first. The CLI prints the id on stderr first. | If `save` throws, nothing is sent. | Fix the store, then call again with the same id. |
| A lost reply (timeout, network error, or a body that is not JSON) is `executed: "unknown"`. The client resends it once, with the same id and the same body. | If the second reply is lost too, the result is `executed: "unknown"`. The client never sends that id a third time. | Read `inbox` (`paid`, `purchases`, `caps.spent24h`) or the chain. Never pay again under a new id. If nothing went out, the CLI may resend with `--op <id>`. |
| After a lost reply, no refusal can prove the first send did not pay. | The refusal is reported as `executed: "unknown"`. | The same as above. |
| `executed: "yes"` | Not a failure. | Never resend. In this process, the same id returns the saved reply without sending. Another process gets the first reply back with `Idempotent-Replayed: true`. |
| Same id, different body | The client refuses it with `executed: "no"` and does not send it. The service would answer 422. | Use a new `operationId` for a new purchase. |
| `rate_limited` or `idempotency_pending` with `executed: "no"` | The client resends once, after `retryAfterSecs` clamped to 1–60 s. If that is refused too, it returns the refusal. | Wait and call again with the same id. |
| Any other refusal | It is returned as is and never resent. `audience: "user"` means a person must act: approve the payee, raise the limit, switch the mode. | Tell your user what `meaning` and `nextStep` say. Retrying will not help. |
| A refusal without a code (services from before the envelope) | `meaning` is the service's `error` string. On a spend, a 5xx reply is `executed: "unknown"`, any other refusal is `"no"`. The client never resends it automatically. | Read `meaning`. On unknown, follow the lost-reply rule. |
| `contractVersion` differs from `EXPECTED_CONTRACT_VERSION` (`2026-10-17.1`) | `onContractMismatch(got, expected)` is called once per client. By default it prints a warning. The call still goes ahead. | Upgrade the SDK. Codes added since this version arrive in `error.code` as plain strings. |
| The token comes from `TAMEION_TOKEN` or `--token-file`, never from argv | `--token` exits 2 without echoing the value. A token file that group or others can read gets a warning. | `chmod 600` the file. |
| Every line the CLI prints is scrubbed: the token becomes `[token]` | — | — |
| An unknown command gets a did-you-mean | It exits 2. The CLI never echoes the input, which could be a pasted secret. | Run `tameion help`. |

## The reply contract (docs: the service's `/llms.txt`)

Every reply an agent can get carries `ok` and `contractVersion`:

```json
{ "ok": true,  "contractVersion": "2026-10-17.1", "...": "the route's fields, unchanged" }
{ "ok": false, "contractVersion": "2026-10-17.1",
  "error": { "code": "payee_not_approved", "by": "mandate", "executed": "no", "audience": "user",
             "meaning": "0x6a86…c8EE is not on the account's payee list.",
             "nextStep": "Ask the owner to approve the payee; the ask is in Needs you.",
             "retryAfterSecs": 0, "words": "the sentence the route already returned, for a human" } }
```

- `executed`: `yes`, `no` or `unknown`. A refusal before anything was sent is `no`. After a payment may have gone out, the service
  reads the chain, and `unknown` means that read failed too. HTTP status codes keep their meaning, and a replay carries
  `Idempotent-Replayed: true`.
- `by`: `auth` `input` `mandate` `budget` `payee` `idempotency` `chain` `upstream` `service`.
- `audience`: `user` when a human must act, `agent` otherwise.
- The codes the contract starts with: `token_missing` `token_invalid` `token_scope` `token_stopped` `mode_watch` `mode_propose`
  `payee_not_approved` `payee_cooling_off` `over_per_payment` `over_24h` `below_floor` `needs_owner` `idempotency_conflict`
  `idempotency_pending` `invalid_body` `body_too_large` `rate_limited` `chain_unavailable` `upstream_unavailable` `price_out_of_band`
  `x402_unsupported` `service_paused`. The live catalogue, with each code's meaning, is served at `GET /api/level1/codes`.
- The client reads `error.code` when it is present. Otherwise it reads the older `error` string.
