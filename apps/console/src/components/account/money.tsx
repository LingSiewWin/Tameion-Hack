"use client";

import { ArrowDownToLine, CalendarClock, Globe, KeyRound, Loader2, ScanSearch, Send, Shield, Wallet } from "lucide-react";
import { useState } from "react";
import { CopyRow, Section, Why } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { build, minor } from "@/lib/guard";
import { dateText, erc20, eth, everyText, GATEWAY_WALLET_TESTNET, gatewayDeposit, isAddr, isZero, num, pinnedUsdc, sentence, switchChain, tail, usd, walletAccount, walletSend } from "@/lib/tameion";
import type { Inbox } from "@/lib/types";
import { Receipt, useAccount, useAction, useRecord } from "./ctx";
import { Payee } from "./payee";

function Panel({ title, icon, children, tone }: { title: string; icon: React.ComponentProps<typeof Section>["icon"]; children: React.ReactNode; tone?: "attention" }) {
  return (
    <Section title={title} icon={icon} tone={tone}>
      <div className="space-y-3 text-sm leading-relaxed">{children}</div>
    </Section>
  );
}

function Out({ state }: { state: ReturnType<typeof useAction>["state"] }) {
  if (state.kind === "error") return <p className="text-sm text-destructive">{state.text}</p>;
  if (state.kind === "done")
    return (
      <p className="text-sm text-protect">
        {state.text} <Receipt href={state.href} />
      </p>
    );
  return null;
}

export function Money() {
  const { inbox, owns, tenant } = useAccount();
  if (!inbox) return null;
  // Money in and trust actions only for an owner (docs/60 P-72 H3): a stranger's
  // link must never show "Fund it" or "Allow" for an account they do not own.
  // An account opened by name (docs/79) shows what the agent found in "Tell it what you owe".
  const nameFirst = !isAddr(tenant.watched) || isZero(tenant.watched);
  return (
    <div className="space-y-4">
      {!nameFirst && <Preview />}
      {owns && !inbox.pull && <BringIn />}
      {owns && inbox.pull && <Allow />}
      <RunwayPanel />
      {owns && <Elsewhere />}
      <Protection />
      {owns && <Delegation />}
    </div>
  );
}

/** Approve the account to pull from the bill wallet: built here (never the
 *  service's calldata), sent only from the bill wallet, on the account's chain. */
async function approveFromBillWallet(pl: NonNullable<Inbox["pull"]>, spender: string, amount: bigint, chain: { chainId: number; name: string }) {
  const from = await walletAccount();
  if (!from) throw new Error("Needs your bill wallet in this browser (MetaMask or similar), or use the call below from your wallet.");
  if (from.toLowerCase() !== pl.wallet.toLowerCase()) throw new Error(`This browser's wallet (ends ••${tail(from)}) is not your bill wallet (ends ••${tail(pl.wallet)}).`);
  // The token the approval goes to is pinned here, not taken from the service (audit 2026-10-15).
  return walletSend(pinnedUsdc(chain.chainId, pl.usdc), erc20.approve(spender, amount), chain);
}

/** Before any money: what the agent already read in the bill wallet. */
function Preview() {
  const { inbox } = useAccount();
  const pv = inbox?.preview;
  if (!pv || pv.status === "failed") return null;
  if (pv.status === "reading")
    return (
      <Panel title="What the agent found" icon={ScanSearch}>
        <p className="flex items-center gap-2 text-muted-foreground">
          <Loader2 className="size-4 animate-spin" /> Reading your wallet&apos;s last 90 days…
        </p>
      </Panel>
    );
  const bills = pv.bills ?? [];
  return (
    <Panel title="What the agent found" icon={ScanSearch}>
      <p>
        {bills.length ? (
          <>
            It read your wallet&apos;s last {pv.days} days ({pv.transfers} payments) and found {bills.length} recurring bill
            {bills.length > 1 ? "s" : ""}. The next 30 days of them come to <span className="figures font-medium">{usd(pv.next30)} USDC</span>.
          </>
        ) : (
          <>It read your wallet&apos;s last {pv.days} days ({pv.transfers} payments) and found no recurring bill yet. Fund it and it keeps watching.</>
        )}
      </p>
      {bills.length > 0 && (
        <ul className="divide-y rounded-xl border">
          {bills.slice(0, 6).map((b) => (
            <li key={b.counterparty + b.amount} className="flex items-baseline gap-3 px-3 py-2.5">
              <span className="figures w-20 shrink-0 font-semibold">{usd(b.amount)}</span>
              <span className="min-w-0 flex-1 truncate">
                <Payee addr={b.counterparty} />
              </span>
              <span className="shrink-0 text-muted-foreground">{everyText(b.intervalSecs, b.every)}</span>
            </li>
          ))}
        </ul>
      )}
      {pv.suggestedLimit && num(pv.suggestedLimit) > num(inbox?.chosenDaily) && (
        <p className="text-muted-foreground">
          Your daily limit ({usd(inbox?.chosenDaily)} USDC) is under twice your largest bill; the agent will suggest {usd(pv.suggestedLimit)} after its first
          check.
        </p>
      )}
    </Panel>
  );
}

/** docs/79 §6a Console 2: every way money reaches the account — its one address,
 *  a link for whoever pays you, this browser's wallet, another chain. */
function BringIn() {
  const { inbox, tenant, cfg, chainName, explorer, refresh } = useAccount();
  const { state, run } = useAction();
  const name = useRecord(tenant.policy).data?.name ?? tenant.name;
  const pv = inbox?.preview;
  const fund = pv?.status === "done" ? BigInt(pv.fundMinor || "0") : 0n;
  const amount = usd((Number(fund) / 1e6).toFixed(6));
  const empty = num(inbox?.balance) <= 0;
  const testnet = /testnet|sepolia/.test(tenant.chain);
  const chainId = cfg.chains.find((c) => c.key === tenant.chain)?.chainId ?? 0;
  const payLink = name ? `${location.origin}/pay?to=${name}` : "";
  return (
    <Panel title="Bring money in" icon={ArrowDownToLine} tone={empty ? "attention" : undefined}>
      <p>
        {empty && fund > 0n ? (
          <>
            <span className="figures font-medium">{amount} USDC</span> covers the next 30 days.{" "}
          </>
        ) : null}
        Send USDC on {chainName} to your account. It is the same address on every supported chain, and only your passkey can take money back out.
      </p>
      <CopyRow value={tenant.policy} shown={name ? `${name} · account address` : "Your account address"} label="the account address" />
      {payLink && (
        <div className="space-y-1.5">
          <p className="text-muted-foreground">A link for anyone who pays you:</p>
          <CopyRow value={payLink} label="the payment link" />
        </div>
      )}
      {eth() && empty && fund > 0n && pv?.usdc && (
        <Button
          disabled={state.kind === "working"}
          onClick={() =>
            run("Sending…", async () => {
              const tx = await walletSend(pinnedUsdc(chainId, pv.usdc), erc20.transfer(tenant.policy, fund), { chainId, name: chainName });
              setTimeout(refresh, 4000);
              return { text: `Sent ${amount} USDC. The agent starts as soon as it lands.`, href: explorer ? `${explorer}/tx/${tx}` : undefined };
            })
          }
        >
          {state.kind === "working" ? <Loader2 className="size-4 animate-spin" /> : <Send className="size-4" />}
          Send {amount} USDC from this browser&apos;s wallet
        </Button>
      )}
      <Out state={state} />
      <FromAnotherChain />
      {testnet && (
        <p className="text-xs text-muted-foreground">
          Testnet USDC:{" "}
          <a className="underline underline-offset-4" href="https://faucet.circle.com" target="_blank" rel="noopener">
            Circle&apos;s faucet
          </a>
          , choose {chainName}.
        </p>
      )}
    </Panel>
  );
}

/** Pull mode: the bill wallet approves the limit itself; Tameion holds nothing. */
function Allow() {
  const { inbox, tenant, cfg, chainName, explorer, refresh } = useAccount();
  const allow = useAction();
  const cut = useAction();
  const pl = inbox!.pull!;
  const low = num(pl.allowance) < num(inbox!.caps.daily);
  const chain = { chainId: cfg.chains.find((c) => c.key === tenant.chain)?.chainId ?? 0, name: chainName };
  const send = (amount: bigint, label: string) => async () => {
    const tx = await approveFromBillWallet(pl, tenant.policy, amount, chain);
    setTimeout(refresh, 1000);
    return { text: label, href: explorer ? `${explorer}/tx/${tx}` : undefined };
  };
  return (
    <Panel title="The money stays in your wallet" icon={Wallet} tone={low ? "attention" : undefined}>
      <p>
        {pl.error ? (
          pl.error
        ) : (
          <>
            The agent may pull up to <span className="figures font-medium">{usd(pl.allowance)} USDC</span> from your wallet <Payee addr={pl.wallet} nameable={false} />, which
            holds <span className="figures">{usd(pl.walletBalance)} USDC</span>. Tameion holds nothing.
            {low ? " That is less than one day's limit: allow more for the agent to pay." : ""}
          </>
        )}
      </p>
      <div className="flex flex-wrap gap-2">
        <Button disabled={allow.state.kind === "working"} onClick={() => allow.run("Allowing…", send(minor(pl.suggestedAllowance), `Allowed ${usd(pl.suggestedAllowance)} USDC.`))}>
          {allow.state.kind === "working" && <Loader2 className="size-4 animate-spin" />}
          Allow {usd(pl.suggestedAllowance)} USDC
        </Button>
        <Button variant="outline" disabled={cut.state.kind === "working"} onClick={() => cut.run("Cutting…", send(0n, "Cut to zero."))}>
          Cut it to zero
        </Button>
      </div>
      <Out state={allow.state.kind !== "idle" ? allow.state : cut.state} />
      <p className="text-caption text-muted-foreground">
        {pl.suggestedBasis ? `${sentence(pl.suggestedBasis)}. ` : ""}
        <Why label="Do it from any wallet">
          <span className="break-all font-mono text-xs">approve({tenant.policy}, {usd(pl.suggestedAllowance)} USDC)</span> on USDC{" "}
          <span className="break-all font-mono text-xs">{pl.usdc}</span>; approving 0 cuts the agent off without Tameion.
        </Why>
      </p>
    </Panel>
  );
}

function RunwayPanel() {
  const { inbox, tenant, cfg, chainName, sign, post, refresh, explorer, owns } = useAccount();
  const back = useAction();
  const more = useAction();
  // A withdraw has no idempotency: after a tap (even one that timed out) the
  // button stays off until the account has been read again.
  const [sentAt, setSentAt] = useState(0);
  const rw = inbox?.runway;
  if (!rw) return null;
  const sf = num(rw.shortfall);
  const returnable = num(rw.returnable);
  const pull = inbox!.pull;
  const elsewhere = num(inbox!.holdingsElsewhere?.total);
  const target = pull ? num(pull.allowance) + sf : 0;
  return (
    <Panel title="Runway" icon={CalendarClock} tone={rw.firstAtRisk && rw.days < 7 ? "attention" : undefined}>
      <p>
        {rw.firstAtRisk ? (
          <>
            {rw.days > 0 ? `${rw.days} days` : rw.firstAtRisk.inHours ? `About ${rw.firstAtRisk.inHours} hours` : "Less than an hour"} — the first bill at risk is{" "}
            <span className="figures font-medium">{usd(rw.firstAtRisk.amount)} USDC</span> to <Payee addr={rw.firstAtRisk.counterparty} /> on{" "}
            {dateText(rw.firstAtRisk.onUnix)}.
          </>
        ) : (
          <>
            Every bill in the next {rw.horizonDays} days is covered ({usd(rw.billsInHorizon)} USDC due)
            {num(rw.idleCover) > 0 ? `; ${usd(rw.idleCover)} USDC is not needed for those bills.` : "."}
          </>
        )}
      </p>
      <p className="text-caption text-muted-foreground">
        <Why label="How this is worked out">{sentence(rw.basis)}</Why>
      </p>
      {sf > 0 && (
        <div className="space-y-2">
          <p>
            To cover every bill in the next {rw.horizonDays} days the account needs <span className="figures font-medium">{usd(rw.shortfall)} USDC</span> more.
            {elsewhere >= sf
              ? ` You hold ${usd(inbox!.holdingsElsewhere?.total)} USDC in Circle Gateway: "Bring it into the account" covers it.`
              : pull && !pull.error
                ? num(pull.walletBalance) < num(rw.horizonTotal)
                  ? ` Your wallet holds ${usd(pull.walletBalance)} USDC; the bills need ${usd(rw.horizonTotal)}.`
                  : ""
                : " Send it to your account (Bring money in); only your key can take it back out."}
          </p>
          {owns && pull && !pull.error && elsewhere < sf && num(pull.walletBalance) >= target && (
            <Button
              variant="outline"
              disabled={more.state.kind === "working"}
              onClick={() =>
                more.run("Allowing…", async () => {
                  const chainId = cfg.chains.find((c) => c.key === tenant.chain)?.chainId ?? 0;
                  const tx = await approveFromBillWallet(pull, tenant.policy, BigInt(Math.round(target * 1e6)), { chainId, name: chainName });
                  setTimeout(refresh, 1000);
                  return { text: `Allowed ${usd(target.toFixed(6))} USDC.`, href: explorer ? `${explorer}/tx/${tx}` : undefined };
                })
              }
            >
              Allow {usd(target.toFixed(6))} USDC
            </Button>
          )}
          <Out state={more.state} />
        </div>
      )}
      {owns && returnable > 0 && rw.returnTo && (
        <div className="space-y-2">
          <p>
            <span className="figures font-medium">{usd(rw.returnable)} USDC</span> is not needed for the next {rw.horizonDays * 2} days of bills. The agent suggests sending it
            back to the wallet that pays your bills (<Payee addr={rw.returnTo} nameable={false} />); it can come back with one tap.
          </p>
          <Button
            variant="outline"
            disabled={back.state.kind === "working" || (sentAt > 0 && (inbox?.readAt ?? 0) < sentAt)}
            onClick={() =>
              back.run("Sending…", async () => {
                const to = rw.returnTo!;
                const watched = typeof tenant.watched === "string" ? tenant.watched : "";
                if (watched && watched.toLowerCase() !== to.toLowerCase())
                  throw new Error(`Not signing: the agent suggests sending to an address ending ••${tail(to)}, not your bill wallet (ends ••${tail(watched)}).`);
                try {
                  const prep = await post<{ to: string; data: string }>("/api/level1/prepare-withdraw", {
                    policy: tenant.policy,
                    to,
                    amount: (rw.returnable || "").replace(/ USDC$/, ""),
                  });
                  const ex = await sign(prep.to, prep.data, { fn: ["withdraw"], data: build("withdraw", [to, minor(rw.returnable)]) });
                  return { text: `Sent ${usd(rw.returnable)} USDC back to your wallet.`, href: ex.explorer };
                } finally {
                  setSentAt(Date.now());
                  setTimeout(refresh, 1500);
                }
              })
            }
          >
            Send it back
          </Button>
          <Out state={back.state} />
        </div>
      )}
    </Panel>
  );
}

/** USDC the bill wallet holds in Circle Gateway on other chains. */
function Elsewhere() {
  const { inbox, tenant, post, refresh } = useAccount();
  const bring = useAction();
  const he = inbox?.holdingsElsewhere;
  const have = !!he?.chains?.length;
  const arriving = !!he?.arriving?.length;
  // A pull account has no "Bring money in": its way in from another chain stays here.
  const park = !!inbox?.pull;
  if (!have && !arriving && !park) return null;
  return (
    <Panel title="Elsewhere" icon={Globe}>
      {(have || arriving) && (
        <p>
          {have
            ? `${usd(he!.total)} USDC of your bill wallet's USDC sits in Circle Gateway on ${he!.chains!.length === 1 ? he!.chains![0].chain : `${he!.chains!.length} chains`}. One tap brings it into the account with your wallet's signature; the agent does not count it until then.`
            : `${he!.arriving!.map((a) => `${usd(a.amount)} USDC`).join(" + ")} of your bill wallet's USDC is arriving in Circle Gateway (Circle waits about 15 minutes for deposits from Ethereum rollups).`}
        </p>
      )}
      {have && (
        <Button
          disabled={bring.state.kind === "working"}
          onClick={() =>
            bring.run("Bringing…", async () => {
              const w = eth();
              if (!w) throw new Error("Needs your bill wallet in this browser to sign.");
              const from = await walletAccount();
              const src = he!.chains!.slice().sort((a, b) => num(b.balance) - num(a.balance))[0];
              const prep = await post<{ describe: string; typedData: { message: unknown } }>("/api/level1/prepare-fund", { policy: tenant.policy, from, all: true, sourceDomain: src.domain });
              if (!window.confirm(prep.describe)) throw new Error("cancelled");
              const signature = await w.request({ method: "eth_signTypedData_v4", params: [from, JSON.stringify(prep.typedData)] });
              const r = await post<{ explorer: string }>("/api/level1/fund", { policy: tenant.policy, burnIntent: prep.typedData.message, signature });
              setTimeout(refresh, 3000);
              return { text: "Arrived in the account.", href: r.explorer };
            })
          }
        >
          Bring it into the account
        </Button>
      )}
      <Out state={bring.state} />
      {park && <FromAnotherChain />}
    </Panel>
  );
}

/** Circle Gateway: USDC from another chain, deposited by this browser's wallet
 *  under its own address; "Elsewhere" then brings it into the account. */
function FromAnotherChain() {
  const { tenant, refresh, cfg } = useAccount();
  const park = useAction();
  const [parkChain, setParkChain] = useState("");
  const [parkAmt, setParkAmt] = useState("");
  const myChainId = cfg.chains.find((c) => c.key === tenant.chain)?.chainId;
  const srcs = (cfg.gatewaySources ?? []).filter((c) => c.chainId !== myChainId);
  if (!srcs.length) return null;
  return (
    <details className="text-sm">
      <summary className="cursor-pointer text-muted-foreground hover:text-foreground">From another chain</summary>
      {!eth() ? (
        <p className="mt-2 text-muted-foreground">
          With a wallet in this browser (MetaMask or similar), send USDC from {srcs.map((c) => c.name).join(", ")} into Circle Gateway under your own address; one tap then
          brings it into the account.
        </p>
      ) : (
        <>
          <div className="mt-3 grid gap-2 sm:grid-cols-[1fr_8rem_auto]">
            <Select value={parkChain} onValueChange={setParkChain}>
              <SelectTrigger className="h-9">
                <SelectValue placeholder="From chain" />
              </SelectTrigger>
              <SelectContent>
                {srcs.map((c) => (
                  <SelectItem key={c.chainId} value={String(c.chainId)}>
                    {c.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Input className="h-9" inputMode="decimal" placeholder="USDC" value={parkAmt} onChange={(e) => setParkAmt(e.target.value)} aria-label="Amount in USDC" />
            <Button
              variant="outline"
              className="h-9"
              disabled={park.state.kind === "working"}
              onClick={() =>
                park.run("Depositing…", async () => {
                  const src = srcs.find((c) => String(c.chainId) === parkChain);
                  const amt = parseFloat(parkAmt);
                  if (!src) throw new Error("Pick a chain.");
                  if (!(amt > 0)) throw new Error("Set an amount in USDC.");
                  const units = BigInt(Math.round(amt * 1e6));
                  const on = { chainId: src.chainId, name: src.name };
                  // Token and spender pinned in the page, never the config's word (audit 2026-10-15).
                  const usdc = pinnedUsdc(src.chainId, src.usdc);
                  if (src.wallet.toLowerCase() !== GATEWAY_WALLET_TESTNET.toLowerCase()) throw new Error("That is not Circle Gateway's wallet, so nothing was sent.");
                  await walletSend(usdc, erc20.approve(GATEWAY_WALLET_TESTNET, units), on);
                  await walletSend(GATEWAY_WALLET_TESTNET, gatewayDeposit(usdc, units), on);
                  if (myChainId) await switchChain(myChainId).catch(() => {});
                  setTimeout(refresh, 4000);
                  return { text: `${amt} USDC deposited into Gateway on ${src.name}. It shows as arriving until Circle credits it.` };
                })
              }
            >
              Send to Gateway
            </Button>
          </div>
          <Out state={park.state} />
          <p className="mt-2 text-xs text-muted-foreground">
            Your wallet switches chain and asks you to confirm twice. Tameion never touches the money: it goes from your wallet into Circle&apos;s Gateway under your own
            address.
          </p>
        </>
      )}
    </details>
  );
}

/** What the bill wallet holds outside dollars, in days of bills. */
function Protection() {
  const { inbox } = useAccount();
  const e = inbox?.exposure;
  if (!e?.holdings?.length || !num(e.notionalUsd)) return null;
  const $$ = (x?: string) => Number(x).toLocaleString("en-US", { maximumFractionDigits: 2 });
  return (
    <Panel title="Protection" icon={Shield}>
      <p>
        Your bill wallet also holds {Number(e.ethTotal).toLocaleString("en-US", { maximumFractionDigits: 4 })} ETH (${$$(e.notionalUsd)} at ${$$(e.ethMark)}). A 30% fall
        would cost ${$$(e.fall30Usd)}
        {e.fall30Days != null ? ` — about ${e.fall30Days} day${e.fall30Days === 1 ? "" : "s"} of your bills` : ""}.
      </p>
      <p className="text-caption text-muted-foreground">
        {e.hedge?.on ? "On: the agent keeps an offsetting position." : "Off for this account."}{" "}
        <Why label="How">
        {e.hedge?.on
          ? "The agent keeps an offsetting short on Hyperliquid (testnet), sized to your bills, at most 2x, never closer than four times the venue's liquidation line; its key can trade on that account and cannot withdraw."
          : "Protection is off for this account: it needs a trade-only Hyperliquid key (it can trade on that account and cannot withdraw). Until then the agent reports the exposure and nothing else."}{" "}
        {e.source}
        </Why>
      </p>
    </Panel>
  );
}

/** A wallet account waits for the wallet's one authorization. */
function Delegation() {
  const { inbox } = useAccount();
  const dg = inbox?.delegation;
  if (!dg || dg.active) return null;
  return (
    <Panel title="One signature from your wallet" icon={KeyRound} tone="attention">
      <p>
        Your wallet becomes the account when it signs one authorization pointing its code at the same contract every Tameion account runs. The agent&apos;s next check sees it
        and starts.
      </p>
      <details>
        <summary className="cursor-pointer text-muted-foreground">The command to run from your wallet</summary>
        <pre className="mt-2 overflow-x-auto rounded-lg border bg-background p-3 font-mono text-xs">{`cast send ${dg.wallet} ${dg.initializeData} --auth ${dg.implementation} --private-key <your wallet's key> --rpc-url <chain>`}</pre>
      </details>
    </Panel>
  );
}
