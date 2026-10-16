"use client";

import { FileCheck2, RefreshCw, ShieldCheck } from "lucide-react";
import { Seal } from "@/components/seal";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Stat, Stats, Why } from "@/components/layout/section";
import { ago, dateText, num, usd } from "@/lib/tameion";
import { useAccount } from "./ctx";
import { usePayeeName } from "./payee";

export function Status() {
  const { inbox, runPass } = useAccount();
  if (!inbox) return null;
  const r = inbox.running;
  if (r) {
    const p = r.progress || {};
    const pct = p.totalBlocks && p.scannedBlocks ? Math.round((100 * p.scannedBlocks) / p.totalBlocks) : 0;
    return (
      <div className="flex flex-col gap-2" aria-live="polite">
        <p className="text-sm">
          <span className="font-medium">Checking your bills…</span>
          {pct ? <span className="text-muted-foreground"> {pct}%</span> : null}
          {p.etaSecs ? <span className="text-muted-foreground"> · about {p.etaSecs < 90 ? `${p.etaSecs} s` : `${Math.round(p.etaSecs / 60)} min`} left</span> : null}
        </p>
        <Progress value={pct || 8} className="h-1.5" />
      </div>
    );
  }
  let text: string;
  if (inbox.deployed === false && !inbox.lastPass)
    text = "Nothing is on chain yet. The moment money can reach the account, the agent creates it and runs its first check.";
  else if (inbox.lastPass)
    text = `Last checked ${ago(inbox.lastPass.at)} · ${inbox.lastPass.obligations} recurring bill${inbox.lastPass.obligations === 1 ? "" : "s"} found${
      inbox.nextPassSecs ? ` · checks every ${Math.round(inbox.nextPassSecs / 60)} min` : ""
    }${inbox.lastPass.historyNote ? ` · ${inbox.lastPass.historyNote}` : ""}`;
  else text = "Not checked yet.";
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
      {!!inbox.halted && (
        <p className="w-full text-sm font-medium text-destructive">
          Paused by a safety check: a payment&apos;s receipt did not match what the agent sent. Nothing more is paid until a human clears it.
        </p>
      )}
      <p className="text-sm text-muted-foreground">
        {text}
        {inbox.lastPass?.modelUnavailable ? " · ordered by rule that time: the model was unavailable, the limits were not" : ""}
      </p>
      {(inbox.deployed !== false || inbox.lastPass) && (
        <Button variant="ghost" size="sm" className="h-7 px-2 text-muted-foreground" onClick={() => void runPass(true)} aria-label="Check now">
          <RefreshCw className="size-3.5" /> Check now
        </Button>
      )}
    </div>
  );
}

export function Hero() {
  const { inbox, explorer } = useAccount();
  const atRiskName = usePayeeName(inbox?.runway?.firstAtRisk?.counterparty);
  if (!inbox) return null;
  const rw = inbox.runway;
  const reserved = inbox.deployed === false;
  // The limit is the contract's cap; what it can reach today is less when the
  // account holds less (the chain's "protected per day" = min(cap, balance)).
  const cap = reserved ? inbox.chosenDaily : inbox.caps?.daily;
  const reach = num(inbox.protectedPerDay) < num(cap) && !reserved ? ` (${usd(inbox.protectedPerDay)} today: that is all the account holds)` : "";
  const most = reserved
    ? `The agent's key will never move more than ${usd(cap)} USDC a day.`
    : `The agent's key can never move more than ${usd(cap)} USDC a day${reach}.`;

  let fraction = 0;
  let tone: "protect" | "attention" | "muted" = "muted";
  let big = "—";
  let label = "not checked yet";
  let sentence = "Nothing is paid without a limit.";
  if (reserved && !inbox.lastPass) {
    label = "waiting for money";
    big = "0";
    sentence = "Your account's address is reserved. Fund it and the agent starts.";
  } else if (rw?.firstAtRisk) {
    const d = rw.days;
    fraction = Math.max(0.03, Math.min(1, d / (rw.horizonDays || 90)));
    tone = d < 7 ? "attention" : "protect";
    big = d > 0 ? String(d) : "<1";
    label = d === 1 ? "day of bills covered" : "days of bills covered";
    const who = atRiskName;
    sentence = `The first bill at risk is ${usd(rw.firstAtRisk.amount)} USDC to ${who} on ${dateText(rw.firstAtRisk.onUnix)}.`;
  } else if (rw) {
    fraction = 1;
    tone = "protect";
    big = String(rw.horizonDays);
    label = "days of bills covered";
    sentence = `Every bill for the next ${rw.horizonDays} days is covered.`;
  }

  const waiting = inbox.needsYou.length + (inbox.sellersToApprove?.length ?? 0) + (inbox.invoices?.length ?? 0);
  const found = inbox.lastPass?.obligations ?? inbox.preview?.bills?.length ?? 0;
  const anchor = inbox.lastPass?.anchorTx;

  return (
    <section className="flex flex-col items-center gap-6 sm:flex-row sm:items-center sm:gap-8">
      <Seal size={168} fraction={fraction} tone={tone} className="shrink-0">
        <span className="figures text-5xl font-semibold leading-none">{big}</span>
        <span className="mt-1.5 max-w-[11ch] text-caption leading-snug text-muted-foreground">{label}</span>
      </Seal>
      <div className="w-full min-w-0 space-y-4">
        <p className="text-balance text-headline font-medium">
          {found > 0 && !reserved ? `The agent found ${found} recurring bill${found === 1 ? "" : "s"}. ` : ""}
          {sentence}
        </p>
        {/* Where the key is not, before what the agent does (CLAUDE.md); the
            Mandate above already says what it does. */}
        <ul className="space-y-1.5 text-sm text-muted-foreground">
          <li className="flex gap-2">
            <ShieldCheck className="mt-0.5 size-4 shrink-0 text-protect" aria-hidden />
            <span>
              The money stays in your own contract; adding a payee or withdrawing needs your passkey. <Why label="The key's limit">{most}</Why>
            </span>
          </li>
          <li className="flex gap-2">
            <FileCheck2 className="mt-0.5 size-4 shrink-0 text-protect" aria-hidden />
            <span>
              Every decision is recorded on chain where anyone can check it.{" "}
              {anchor && explorer ? (
                <a className="font-medium text-foreground underline-offset-4 hover:underline" href={`${explorer}/tx/${anchor}`} target="_blank" rel="noopener">
                  The last check&apos;s record
                </a>
              ) : null}
            </span>
          </li>
        </ul>
        <Stats>
          <Stat label="Paid" value={String(inbox.paid.length)} sub={inbox.paid.length ? `${usd(inbox.paidTotal)} USDC` : "nothing yet"} />
          <Stat label="Needs you" value={String(waiting)} sub={waiting ? "tap to decide" : "all clear"} tone={waiting ? "attention" : undefined} />
          {inbox.exposure?.hedge?.on ? <Protected /> : <AgentsCanSpend />}
        </Stats>
      </div>
    </section>
  );
}

/** Price protection in one line: what a fall would cost, and whether the agent
 *  covers it (Hyperliquid testnet; the venue key can trade, never withdraw). */
function ProtectionLine() {
  const { inbox } = useAccount();
  const e = inbox?.exposure;
  const eth = Number(e?.ethTotal ?? 0);
  if (!e || !eth || !Number(e.notionalUsd)) return <>The bill money is held in USDC, so a price fall cannot touch it.</>;
  const days = e.fall30Days != null ? `about ${e.fall30Days} day${e.fall30Days === 1 ? "" : "s"} of bills` : `$${e.fall30Usd}`;
  return e.hedge?.on ? (
    <>
      Your wallet&apos;s {eth.toLocaleString("en-US", { maximumFractionDigits: 4 })} ETH is protected: a 30% fall would have cost {days}; the agent holds an offsetting position.
    </>
  ) : (
    <>
      Your wallet&apos;s {eth.toLocaleString("en-US", { maximumFractionDigits: 4 })} ETH is exposed: a 30% fall would cost {days}. Protection turns on with a trade-only venue key.
    </>
  );
}

/** What an agent operator reads instead of a hedge tile (docs/109 §10): the most the agents can still spend now — the
 *  smaller of the cap on one payment, what is left of the 24-hour cap, and what the account holds above the amount kept.
 *  The contract's own 24-hour total is not served to the page, so "left" subtracts every payment this page lists from the
 *  last 24 hours (as of when it read them): it can come out lower than the contract's figure, never higher. */
function AgentsCanSpend() {
  const { inbox } = useAccount();
  if (!inbox) return null;
  const label = "Agents can spend now";
  if (inbox.keyless) return <Stat label={label} value="0.00" sub="USDC: no agent key, so every payment waits for your tap" />;
  if (inbox.mode === "Watch") return <Stat label={label} value="0.00" sub="USDC: the agent pays nothing for now" />;
  const since = inbox.readAt ? inbox.readAt / 1000 - 86_400 : 0;
  const spent = inbox.paid.reduce((a, p) => ((p.at ?? 0) >= since ? a + num(p.amount) : a), 0);
  const per = num(inbox.caps.perAction);
  const left = Math.max(0, num(inbox.caps.daily) - spent);
  const above = Math.max(0, num(inbox.balance) - num(inbox.caps.floor));
  const can = Math.min(per, left, above);
  const why =
    above < Math.min(per, left)
      ? `all the account holds above the ${usd(inbox.caps.floor)} USDC you keep`
      : left < per
        ? `${usd(left.toFixed(6))} of the ${usd(inbox.caps.daily)} a day is left`
        : `the cap on one payment; ${usd(left.toFixed(6))} of the ${usd(inbox.caps.daily)} a day is left`;
  return <Stat label={label} value={usd(can.toFixed(6))} sub={`USDC: ${why}${inbox.mode === "Propose" ? "; each payment waits for your tap" : ""}`} />;
}

/** docs/73 T2/T3. The number is shown only when the served data proves it: the bill
 *  money is USDC (a price fall cannot touch it), so when the runway's first bill
 *  at risk is more than 30 days away, 100% of the next 30 days is covered. Hedge
 *  coverage and the cost of protection are not served yet (docs/66, route change);
 *  no number is made up for them. */
function Protected() {
  const { inbox } = useAccount();
  const pr = inbox?.protection;
  const hedgeOn = !!inbox?.exposure?.hedge?.on;
  // Hedge coverage in buyer words (docs/73 T2). Measured 2026-10-15: no account holds
  // an offsetting position, so 0% is the true figure until the service serves one.
  const value = pr?.coveragePct != null ? `${pr.coveragePct}%` : hedgeOn ? "—" : "0%";
  const sub =
    pr?.coveragePct != null
      ? "of the next 30 days of bills"
      : hedgeOn
        ? "coverage not reported yet"
        : "no protection held in the last 30 days";
  const cost = pr?.funding30d ? num(pr.funding30d.usdc) : null;
  const pct = pr?.coveragePct;
  return (
    <Stat
      label="Protected"
      value={value}
      // 0–100: under 100 the owner has bills a fall could reach, so it reads in amber.
      tone={pct != null && pct < 100 ? "attention" : undefined}
      meter={pct != null ? pct / 100 : undefined}
      sub={
        <>
          {sub} ·{" "}
          <Why label="Cost">
            {pr?.protectedUsd && pr.bills30d && (
              <span className="block font-medium">
                {usd(pr.protectedUsd)} of {usd(pr.bills30d)} USDC due in the next 30 days is protected.
              </span>
            )}
            <span className="mt-2 block first:mt-0">
              <ProtectionLine />
            </span>
            <span className="mt-2 block">
              {cost != null
                ? `Protection last 30 days: ${cost <= 0 ? `${usd(String(cost))} USDC, the market paid you` : `cost you ${usd(String(cost))} USDC`}.`
                : hedgeOn
                  ? "Protection last 30 days: the service does not report its cost yet."
                  : "Protection last 30 days: none held, so no cost."}
            </span>
            <span className="mt-2 block">
              How it works: when the company holds ETH, the agent keeps an offsetting position on Hyperliquid (testnet
              today), so a fall in price is matched by a gain, and it reads every position back after each order.{" "}
              <a className="font-medium text-foreground underline underline-offset-4" href="/proof/hedge-testnet-live-open-2026-10-13.txt" target="_blank" rel="noopener">
                A live run
              </a>
              {" · "}
              <a className="font-medium text-foreground underline underline-offset-4" href="/proof/hedge-testnet-nowithdraw-2026-10-13.txt" target="_blank" rel="noopener">
                its key refused a withdrawal
              </a>
              .
            </span>
          </Why>
        </>
      }
    />
  );
}
