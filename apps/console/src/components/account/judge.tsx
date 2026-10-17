"use client";

import { useQuery } from "@tanstack/react-query";
import { Check, CheckCircle2, Fingerprint, Loader2, Scissors, ShieldCheck, Sparkles, TrendingUp, Undo2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useNow } from "@/app/agent/live";
import { Stat, Stats } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { billsChallenge, build, minor, PAYEES } from "@/lib/guard";
import { clock, cutOf, nextDue, raiseOf, STEPS, stepOf, type Caps } from "@/lib/judge";
import { ago, everyText, fetchDemo, getJson, noAddr, num, passkeyAssert, raw, reserveToken, saveName, short, store, usd } from "@/lib/tameion";
import type { Demo, Paid, Tenant } from "@/lib/types";
import type { AnchoredRecord } from "@/lib/verify";
import { cn } from "@/lib/utils";
import { Receipt, useAccount, useAction, type AccountCtx } from "./ctx";
import { keysReady, keysSet, useKeys } from "@/lib/keys";
import { MandateSentence } from "./mandate";
import { Payee } from "./payee";

/** What this browser keeps of the judge's run: when it opened, what step 5 signed. The rest is read from the account. */
type Saved = { openedAt?: number; fundTx?: string; fundError?: string; billTx?: string; autoTx?: string; raiseTx?: string; raisedTo?: string; cancelled?: boolean; cutTx?: string; cutAt?: number };
const KEY = (policy: string) => `tameion.judge.${policy}`;
const load = (policy: string): Saved => {
  try {
    return JSON.parse(store.get(KEY(policy)) || "{}") as Saved;
  } catch {
    return {};
  }
};

/** An account opened with "Try it in 5 minutes" on this device (Start keeps the mark), or one the service calls a demo. */
export const isDemo = (t: Tenant) => t.demo === true || store.get(KEY(t.policy)) !== null;

/** The statement's line for one payment (`GET /api/level1/statement`, statement.rs `Line::to_json`). */
type BookLine = { at: number; seq: number; root: string; kind: string; payee: string; amount: string; tx?: string | null; detail: string; paidBy?: string };
type Book = { lines: BookLine[]; record: { root: string; anchorTx?: string | null } };

const span = (ms: number) => {
  const s = Math.max(0, Math.round(ms / 1000));
  return s < 60 ? `${s} s` : `${Math.floor(s / 60)} min ${s % 60} s`;
};

const limitsCall = async (a: AccountCtx, to: Caps) => {
  const floor = raw(a.inbox!.caps.floor);
  const data = build("setLimits", [minor(to.perAction), minor(to.daily), minor(floor)]);
  const prep = await a.post<{ to: string; data: string }>("/api/level1/prepare-limits", { policy: a.tenant.policy, perAction: to.perAction, daily: to.daily, floor });
  return a.sign(prep.to, prep.data, { fn: ["setLimits"], data });
};
const cancelCall = async (a: AccountCtx) => {
  const eta = a.inbox?.pending?.eta;
  try {
    const prep = await a.post<{ to: string; data: string }>("/api/level1/prepare-cancel", { policy: a.tenant.policy });
    return await a.sign(prep.to, prep.data, { fn: ["cancelPending"], data: build("cancelPending", []) });
  } catch (e) {
    // Said in words, with what still holds (docs/60 P-171): a failed cancel leaves the change waiting.
    const at = eta ? ` before ${new Date(eta * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}` : "";
    throw new Error(`The cancel did not go through (${(e as Error).message}). The change still waits: tap again${at}, or cut once it applies — a cut is instant.`);
  }
};

/** The judge's five minutes on the account page (docs/115 §3): "1 Open · 2 Bill · 3 Autopilot · 4 Watch it pay · 5 Raise
 *  and cut", each step one tap or a wait with a countdown, then the three numbers and the mandate. `said` is the account's
 *  live stream in words (the page's own `GET /api/level1/events`). */
export function JudgePath({ said }: { said?: string }) {
  const a = useAccount();
  const { inbox, tenant, refresh } = a;
  const demo = useQuery({ queryKey: ["demo"], queryFn: fetchDemo, staleTime: 300_000 });
  const d = demo.data?.enabled ? demo.data : null;
  const now = useNow() / 1000;
  const [saved, setSaved] = useState(() => load(tenant.policy));
  const keep = (p: Saved) =>
    setSaved((s) => {
      const next = { ...s, ...p };
      store.set(KEY(tenant.policy), JSON.stringify(next));
      return next;
    });

  const vendor = d?.vendor.toLowerCase() ?? "";
  const payee = inbox?.payees?.find((p) => p.address.toLowerCase() === vendor);
  const bill = inbox?.typedBills?.find((b) => b.payee.toLowerCase() === vendor);
  // Every payment to the demo vendor; an owner's tap (the agent asked instead) shows as "by you" and also ends step 4, so a
  // judge is never stuck, and the page says who paid.
  const toVendor = (inbox?.paid ?? []).filter((p) => p.counterparty.toLowerCase() === vendor).sort((x, y) => (y.at ?? 0) - (x.at ?? 0));
  const paid = toVendor.filter((p) => p.by !== "you");
  const asked = inbox?.needsYou.find((n) => n.counterparty.toLowerCase() === vendor);
  const funded = num(inbox?.balance) > 0 || !!inbox?.paid.length;
  const step = stepOf({ funded, billSaved: !!payee && !!bill, autopilot: inbox?.mode === "Autopilot", paid: toVendor.length, cut: !!saved.cutTx });

  const every = bill?.everySecs || d?.bill.everySecs || 0;
  const upcoming = bill ? nextDue(bill.nextDue, every, now) : 0;
  const lastDue = bill && now > bill.nextDue ? upcoming - every : 0;
  // Unpaid since its last due time; 30 s of slack for a chain clock behind this page's.
  const dueUnpaid = lastDue > 0 && (toVendor[0]?.at ?? 0) < lastDue - 30;
  const eta = inbox?.pending?.eta ?? 0;

  // While the judge waits on the chain (the funding, a change past its delay, a bill past due), read the account every
  // 5 s: the stream says when a check ends, not when money lands.
  const poll = !!inbox && !!d && step < 5 && (!funded || (eta > 0 && now > eta) || (dueUnpaid && !asked));
  useEffect(() => {
    if (!poll) return;
    const t = setInterval(refresh, 5_000);
    return () => clearInterval(t);
  }, [poll, refresh]);

  if (!inbox || !d || !now) return null;
  const elapsed = saved.openedAt ? now * 1000 - saved.openedAt : 0;
  return (
    <section aria-labelledby="judge-title" className="rounded-xl border border-protect/35 bg-card p-4 shadow-card sm:p-6">
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h2 id="judge-title" className="text-title font-semibold">
          Try it in 5 minutes
        </h2>
        {elapsed > 0 && step < 5 && <span className="figures text-caption text-muted-foreground">{clock(elapsed / 1000)} since you opened it</span>}
      </div>
      <Strip step={step} />
      <Done step={step} d={d} saved={saved} byAgent={paid.length > 0} />
      <div className="mt-4">
        {step === 0 && <Funding saved={saved} now={now} />}
        {step === 1 && <BillStep d={d} approved={!!payee} keep={keep} />}
        {step === 2 && <AutopilotStep d={d} now={now} keep={keep} />}
        {step === 3 && (
          <WatchStep d={d} now={now} upcoming={upcoming} dueUnpaid={dueUnpaid} asked={asked?.why} payableFrom={payee?.payableFrom} said={said} />
        )}
        {step === 4 && <RaiseCutStep d={d} now={now} saved={saved} keep={keep} />}
        {step === 5 && <Finish d={d} paid={paid} all={toVendor.length} saved={saved} />}
      </div>
      {toVendor.length > 0 && <Payments paid={toVendor} next={step >= 3 && step < 5 && !dueUnpaid ? upcoming - now : null} />}
    </section>
  );
}

/** The five steps in a row; on a phone, only the current one. */
function Strip({ step }: { step: number }) {
  return (
    <>
      <ol className="mt-3 hidden flex-wrap items-center gap-x-2 gap-y-1 text-sm sm:flex">
        {STEPS.map((label, i) => (
          <li
            key={label}
            aria-current={i === step ? "step" : undefined}
            className={cn("flex items-center gap-1.5", i === step ? "font-medium text-foreground" : i < step ? "text-protect" : "text-muted-foreground")}
          >
            {i > 0 && (
              <span aria-hidden className="mr-0.5 text-muted-foreground">
                ·
              </span>
            )}
            <span
              className={cn(
                "figures grid size-5 place-items-center rounded-full border text-xs",
                i === step && "border-foreground",
                i < step && "border-protect/40 bg-protect-soft",
              )}
            >
              {i < step ? <Check className="size-3" aria-label="done" /> : i + 1}
            </span>
            {label}
          </li>
        ))}
      </ol>
      <p className="mt-2 text-sm sm:hidden">
        {step < 5 ? (
          <>
            Step {step + 1} of 5 · <span className="font-medium">{STEPS[step]}</span>
          </>
        ) : (
          <span className="text-protect">All five steps done</span>
        )}
      </p>
    </>
  );
}

/** One line for each step behind the judge, so what happened stays on the screen. */
function Done({ step, d, saved, byAgent }: { step: number; d: Demo; saved: Saved; byAgent: boolean }) {
  const { inbox, explorer } = useAccount();
  // What the demo funded: what the account holds plus what it paid (a demo account has no other money in or out).
  const funded = num(inbox?.balance) + (inbox?.paid ?? []).reduce((s, p) => s + num(p.amount), 0);
  const lines = [
    `Funded: ${usd(funded.toFixed(6))} USDC, by the demo, no wallet needed.`,
    `Demo Vendor approved; the bill saved: ${usd(d.bill.usdc)} USDC ${everyText(d.bill.everySecs)}.`,
    "Autopilot on: the agent pays approved payees by itself, inside your limits.",
    byAgent ? "The agent paid the bill by itself on its due time." : "The bill was paid by your tap: the agent asked you first (its words are below).",
  ].slice(0, Math.min(step, 4));
  if (!lines.length) return null;
  return (
    <ul className="mt-3 space-y-1 text-sm text-muted-foreground">
      {lines.map((t, i) => (
        <li key={t} className="flex gap-2">
          <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-protect" aria-hidden />
          <span>
            {t}
            {i === 0 && saved.fundTx && (
              <>
                {" "}
                <Receipt href={`${explorer}/tx/${saved.fundTx}`} />
              </>
            )}
            {i === 1 && saved.billTx && (
              <>
                {" "}
                <Receipt href={saved.billTx} />
              </>
            )}
            {i === 2 && saved.autoTx && (
              <>
                {" "}
                <Receipt href={saved.autoTx} />
              </>
            )}
          </span>
        </li>
      ))}
    </ul>
  );
}

function Funding({ saved, now }: { saved: Saved; now: number }) {
  const waited = saved.openedAt ? now - saved.openedAt / 1000 : 0;
  // The open answered that nothing was sent (the relayer short, or its transfer failed): say so; opening it again from
  // the home page with the same passkey and name funds it then (demo.rs `fund_once`).
  if (saved.fundError)
    return (
      <div className="space-y-1">
        <p className="text-title font-medium text-attention">The demo could not fund this account.</p>
        <p className="text-sm">{saved.fundError}</p>
      </div>
    );
  return (
    <div className="space-y-1">
      <p className="flex items-center gap-2 text-title font-medium">
        <Loader2 className="size-4 animate-spin" aria-hidden /> Funding your account with test USDC…
        {waited > 0 && <span className="figures font-normal text-muted-foreground">{clock(waited)}</span>}
      </p>
      <p className="text-caption text-muted-foreground">
        {waited > 120
          ? "Still waiting for the money. The demo funds every new account by itself; if nothing lands soon, it may be paused on this host."
          : "The demo funds every new account by itself: no wallet, no faucet. The agent's first check then puts the account on chain."}
      </p>
    </div>
  );
}

function BillStep({ d, approved, keep }: { d: Demo; approved: boolean; keep: (p: Saved) => void }) {
  const a = useAccount();
  const { state, run } = useAction();
  const vendor = d.vendor.toLowerCase();
  const tap = () =>
    run("Waiting for your passkey…", async (step) => {
      const policy = a.tenant.policy;
      const reserve = reserveToken.get(policy) ?? undefined;
      // Ask first whether this device may save bills here, so a refusal costs no passkey tap (docs/60 P-225).
      await a.post("/api/level1/bills", { policy, check: true, reserveToken: reserve });
      // The contract pays only payees it approved: the one passkey tap approves Demo Vendor (skipped on a retry).
      let href: string | undefined;
      if (!approved) {
        const prep = await a.post<{ to: string; data: string }>("/api/level1/prepare-payees", { policy, payees: [vendor] });
        href = (await a.sign(prep.to, prep.data, { fn: PAYEES, attested: true, rows: [{ to: vendor }] })).explorer;
      }
      step("Saving the bill…");
      const every = d.bill.everySecs;
      // First due one cycle from now, so the judge sees the countdown (docs/115 §3 step 4). `everyDays: 0` is sent so the
      // signed bytes never rest on the service's default; the cadence is `everySecs`.
      const bill = { payee: vendor, amount: d.bill.usdc, everyDays: 0, everySecs: every, nextDue: Math.floor(Date.now() / 1000) + every };
      // An account with agent keys takes a bill from this device only with the owner's passkey over exactly this bill
      // (docs/60 P-226): a second tap there; a fresh demo account has none.
      let signed = {};
      if (a.inbox?.agentTokens?.length) {
        const chainId = a.cfg.chains.find((c) => c.key === a.tenant.chain)?.chainId;
        if (!chainId) throw new Error("This page does not know the account's chain; reload it.");
        const issued = Math.floor(Date.now() / 1000);
        signed = { issued, auth: await passkeyAssert(billsChallenge(chainId, policy, [bill], issued), a.pub) };
      }
      const r = await a.post<{ refused?: { why: string }[] }>("/api/level1/bills", { policy, reserveToken: reserve, bills: [bill], ...signed });
      if (r.refused?.length) throw new Error(r.refused[0].why);
      await saveName(policy, vendor, "Demo Vendor").catch(() => undefined);
      if (href) keep({ billTx: href });
      a.refresh();
      return { text: `Demo Vendor approved; the agent may pay it ${usd(d.bill.usdc)} USDC ${everyText(every)}.`, href };
    });
  return (
    <div className="space-y-2">
      <Button size="lg" className="h-auto min-h-11 w-full whitespace-normal py-2 text-left sm:w-auto" disabled={state.kind === "working"} onClick={tap}>
        {state.kind === "working" ? <Loader2 className="size-4 shrink-0 animate-spin" /> : <Fingerprint className="size-4 shrink-0" />}
        {state.kind === "working" ? state.label : `Let the agent pay Demo Vendor ${usd(d.bill.usdc)} USDC ${everyText(d.bill.everySecs)}`}
      </Button>
      <p className="text-caption text-muted-foreground">One passkey tap: it approves Demo Vendor as a payee and saves the bill. Nothing is paid yet.</p>
      <Outcome state={state} />
    </div>
  );
}

function AutopilotStep({ d, now, keep }: { d: Demo; now: number; keep: (p: Saved) => void }) {
  const a = useAccount();
  // A demo account waits the demo's delay, not the host's.
  const { inbox, tenant } = a;
  const { state, run } = useAction();
  const p = inbox?.pending;
  const waiting = !!p && ((p.kinds ?? 0) & 2) !== 0 && p.mode === "Autopilot";
  const left = waiting ? (p.eta ?? 0) - now : 0;
  return (
    <div className="space-y-2">
      {waiting ? (
        <>
          <p className="text-title font-medium">
            {left > 0 ? (
              <>
                Autopilot applies in <span className="figures">{clock(left)}</span> unless you cancel.
              </>
            ) : (
              <span className="flex items-center gap-2">
                <Loader2 className="size-4 animate-spin" aria-hidden /> The wait is over; applying Autopilot…
              </span>
            )}
          </p>
          <p className="text-caption text-muted-foreground">Anyone can see the change waiting on chain; nothing changes until the wait ends.</p>
          <Button
            variant="outline"
            disabled={state.kind === "working" || left <= 0}
            onClick={() =>
              run("Cancelling…", async () => {
                const ex = await cancelCall(a);
                a.refresh();
                return { text: "Cancelled: Autopilot will not apply. Tap it again when you are ready.", href: ex.explorer };
              })
            }
          >
            <Undo2 className="size-4" /> Cancel
          </Button>
        </>
      ) : (
        <>
          <Button
            size="lg"
            className="h-11"
            disabled={state.kind === "working"}
            onClick={() =>
              run("Waiting for your passkey…", async () => {
                const prep = await a.post<{ to: string; data: string }>("/api/level1/prepare-mode", { policy: tenant.policy, mode: 2 });
                const ex = await a.sign(prep.to, prep.data, { fn: ["setMode"], data: build("setMode", [2]) });
                keep({ autoTx: ex.explorer });
                a.refresh();
                return { text: `Asked. Autopilot turns on in ${d.delaySecs} s unless you cancel.`, href: ex.explorer };
              })
            }
          >
            {state.kind === "working" ? <Loader2 className="size-4 animate-spin" /> : <Sparkles className="size-4" />}
            {state.kind === "working" ? state.label : "Turn Autopilot on"}
          </Button>
          <p className="text-caption text-muted-foreground">
            The agent then pays approved payees by itself, inside your limits. More autonomy waits {d.delaySecs} s, where anyone can see it coming.
          </p>
        </>
      )}
      <Outcome state={state} />
      <KeysLine />
    </div>
  );
}

function WatchStep({
  d,
  now,
  upcoming,
  dueUnpaid,
  asked,
  payableFrom,
  said,
}: {
  d: Demo;
  now: number;
  upcoming: number;
  dueUnpaid: boolean;
  asked?: string;
  payableFrom?: number;
  said?: string;
}) {
  return (
    <div className="space-y-2">
      <p className="text-caption font-medium text-muted-foreground">{dueUnpaid ? "The bill is due" : "The bill is due in"}</p>
      <p className="figures text-4xl font-semibold leading-none">{dueUnpaid ? "Now" : clock(upcoming - now)}</p>
      <p className="text-sm">
        {asked ? (
          <span className="text-attention">The agent asked you instead of paying by itself. One tap under Needs you below pays it.</span>
        ) : dueUnpaid ? (
          <span className="flex items-center gap-2">
            <Loader2 className="size-4 shrink-0 animate-spin" aria-hidden /> The agent is paying Demo Vendor {usd(d.bill.usdc)} USDC by itself…
          </span>
        ) : (
          `Then the agent pays Demo Vendor ${usd(d.bill.usdc)} USDC by itself, inside your limits, and says why.`
        )}
      </p>
      {payableFrom && payableFrom > now && (
        <p className="text-caption text-muted-foreground">
          A new payee cools off first: the agent may pay Demo Vendor from {new Date(payableFrom * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}.
        </p>
      )}
      {asked && (
        <p className="text-sm leading-relaxed">
          <span className="text-muted-foreground">Why, in the agent&apos;s words: </span>
          <q>{noAddr(asked)}</q>
        </p>
      )}
      {said && (
        <p className="flex items-center gap-2 text-caption text-muted-foreground">
          <span className="size-1.5 shrink-0 rounded-full bg-protect" aria-hidden /> Live: {said}
        </p>
      )}
    </div>
  );
}

function RaiseCutStep({ d, now, saved, keep }: { d: Demo; now: number; saved: Saved; keep: (p: Saved) => void }) {
  const a = useAccount();
  // A demo account waits the demo's delay, not the host's.
  const { inbox } = a;
  const raise = useAction();
  const cancel = useAction();
  const cut = useAction();
  const caps = inbox!.caps;
  const p = inbox?.pending;
  // A cancel this page signed is final: an inbox that still lists the raise for a while after it (P-286) must not show
  // "applying the higher cap" over a raise that was cancelled.
  const waiting = !!p && ((p.kinds ?? 0) & 1) !== 0 && !saved.cancelled;
  const left = waiting ? (p.eta ?? 0) - now : 0;
  const up = raiseOf(caps);
  const down = cutOf(caps, d.bill.usdc);
  const busy = [raise, cancel, cut].some((x) => x.state.kind === "working");
  return (
    <div className="space-y-3">
      {!saved.raiseTx && !waiting && (
        <>
          <Button
            size="lg"
            className="h-11"
            disabled={busy}
            onClick={() =>
              raise.run("Waiting for your passkey…", async () => {
                const ex = await limitsCall(a, up);
                keep({ raiseTx: ex.explorer || ex.tx || "signed", raisedTo: up.perAction });
                a.refresh();
                return { text: `Asked. One payment may be up to ${usd(up.perAction)} USDC in ${d.delaySecs} s, unless you cancel.`, href: ex.explorer };
              })
            }
          >
            {raise.state.kind === "working" ? <Loader2 className="size-4 animate-spin" /> : <TrendingUp className="size-4" />}
            {raise.state.kind === "working" ? raise.state.label : `Raise one payment to ${usd(up.perAction)} USDC`}
          </Button>
          <p className="text-caption text-muted-foreground">
            Today one payment may be up to {usd(caps.perAction)} USDC. A raise waits {d.delaySecs} s where anyone can see it; you can cancel it until then.
          </p>
        </>
      )}
      {waiting && (
        <>
          <p className="text-title font-medium">
            {left > 0 ? (
              <>
                The higher cap, {usd(p?.perActionCap)} USDC a payment, applies in <span className="figures">{clock(left)}</span>.
              </>
            ) : (
              <span className="flex items-center gap-2">
                <Loader2 className="size-4 animate-spin" aria-hidden /> The wait is over; applying the higher cap…
              </span>
            )}
          </p>
          <p className="text-caption text-muted-foreground">
            It waits on chain where anyone can see it coming. <Receipt href={saved.raiseTx?.startsWith("https://") ? saved.raiseTx : undefined}>The raise</Receipt>
          </p>
          <Button
            variant="outline"
            disabled={busy || left <= 0}
            onClick={() =>
              cancel.run("Cancelling…", async () => {
                const ex = await cancelCall(a);
                keep({ cancelled: true });
                a.refresh();
                return { text: "Cancelled at once: the cap stays where it was.", href: ex.explorer };
              })
            }
          >
            {cancel.state.kind === "working" ? <Loader2 className="size-4 animate-spin" /> : <Undo2 className="size-4" />}
            Cancel the raise
          </Button>
        </>
      )}
      {saved.raiseTx && !waiting && (
        <>
          <p className="text-sm">
            {saved.cancelled || num(caps.perAction) < num(saved.raisedTo)
              ? "You cancelled the raise: it never applied."
              : `The raise applied after the wait: one payment may now be up to ${usd(caps.perAction)} USDC.`}{" "}
            Now cut: a cut never waits.
          </p>
          {down ? (
            <Button
              size="lg"
              className="h-11"
              disabled={busy}
              onClick={() =>
                cut.run("Waiting for your passkey…", async () => {
                  const ex = await limitsCall(a, down);
                  keep({ cutTx: ex.explorer || ex.tx || "signed", cutAt: Date.now() });
                  a.refresh();
                  return { text: `Cut at once: one payment may be up to ${usd(down.perAction)} USDC, ${usd(down.daily)} a day.`, href: ex.explorer };
                })
              }
            >
              {cut.state.kind === "working" ? <Loader2 className="size-4 animate-spin" /> : <Scissors className="size-4" />}
              {cut.state.kind === "working"
                ? cut.state.label
                : num(down.perAction) < num(caps.perAction)
                  ? `Cut one payment to ${usd(down.perAction)} USDC`
                  : `Cut the day to ${usd(down.daily)} USDC`}
            </Button>
          ) : (
            <p className="text-sm text-muted-foreground">The caps are already as low as the bill allows.</p>
          )}
        </>
      )}
      {[raise, cancel, cut].map((x, i) => (
        <Outcome key={i} state={x.state} />
      ))}
      <KeysLine />
    </div>
  );
}

function Finish({ d, paid, all, saved }: { d: Demo; paid: Paid[]; all: number; saved: Saved }) {
  const { inbox } = useAccount();
  const https = (h?: string) => (h?.startsWith("https://") ? h : undefined);
  const caps = inbox!.caps;
  const per = num(caps.perAction);
  const left = caps.leftNow != null ? num(caps.leftNow) : num(caps.daily);
  const above = Math.max(0, num(inbox!.balance) - num(caps.floor));
  const can = inbox!.mode === "Autopilot" ? Math.min(per, left, above) : 0;
  const total = paid.reduce((s, p) => s + num(p.amount), 0);
  return (
    <div className="space-y-4">
      <p className="text-title font-semibold">
        Done{saved.openedAt && saved.cutAt ? ` in ${span(saved.cutAt - saved.openedAt)}` : ""}.{" "}
        {paid.length
          ? "Your agent paid its bills by itself; you made the trust decisions."
          : "The agent asked you before paying, and your one tap paid; the limits held the whole time."}
      </p>
      <Stats>
        <Stat label="Agents can spend now" value={usd(can.toFixed(6))} sub={`USDC in one payment; ${usd(left.toFixed(6))} of ${usd(caps.daily)} left today`} />
        <Stat label="Paid by the agent" value={usd(total.toFixed(6))} sub={`USDC to Demo Vendor, ${paid.length} payment${paid.length === 1 ? "" : "s"}`} />
        <Stat label="Paid with no human" value={String(paid.length)} sub={`of ${all} bill${all === 1 ? "" : "s"} paid, ${usd(d.bill.usdc)} USDC each; ${all - paid.length} by your tap`} />
      </Stats>
      <p className="text-sm">
        The raise waited {d.delaySecs} s where anyone could see it{saved.cancelled ? ", and you cancelled it" : ""} <Receipt href={https(saved.raiseTx)} />; the cut applied at
        once <Receipt href={https(saved.cutTx)} />.
      </p>
      <blockquote className="max-w-[48ch] text-balance text-headline font-normal [&_strong]:font-semibold">
        <MandateSentence n={1} daily={caps.daily} mode={inbox!.mode} />
      </blockquote>
      <p className="flex gap-2 text-sm leading-relaxed">
        <ShieldCheck className="mt-0.5 size-4 shrink-0 text-protect" aria-hidden />
        <span>
          Keys: your passkey, on this device. The agent holds a bounded key: it pays only payees you approved, up to the mandate you signed. Two guardians, added in
          Settings, bring the account back if this device is lost.
        </span>
      </p>
    </div>
  );
}

/** Each payment the agent made to the demo vendor: amount, payee, the agent's reason word for word, the receipt, and the
 *  line the signed statement holds for it. */
function Payments({ paid, next }: { paid: Paid[]; next: number | null }) {
  const { tenant, explorer } = useAccount();
  const book = useQuery({
    queryKey: ["statement", tenant.policy, paid.length],
    queryFn: () => {
      const tok = reserveToken.get(tenant.policy);
      return getJson<Book>(`/api/level1/statement?policy=${tenant.policy}`, tenant.policy, tok ? { "x-tameion-device": tok } : undefined);
    },
    staleTime: 30_000,
  });
  // Each pass anchors its own record: a line's root is on chain when the account's anchored records list it.
  const records = useQuery({
    queryKey: ["records", tenant.policy, paid.length],
    queryFn: () => getJson<{ records: AnchoredRecord[] }>(`/api/level1/records?policy=${tenant.policy}`, tenant.policy),
    staleTime: 30_000,
  });
  return (
    <div className="mt-5 border-t pt-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <p className="text-title font-medium">Paid to Demo Vendor</p>
        {next != null && next > 0 && <p className="figures text-caption text-muted-foreground">next bill due in {clock(next)}</p>}
      </div>
      <ol className="mt-2 divide-y">
        {paid.slice(0, 6).map((p, i) => {
          const line = book.data?.lines.find((l) => l.kind === "paid" && !!p.tx && l.tx?.toLowerCase() === p.tx.toLowerCase());
          const anchored = line?.root
            ? (records.data?.records.find((r) => r.root.toLowerCase() === line.root.toLowerCase())?.anchorTx ??
              (book.data?.record.root === line.root ? book.data.record.anchorTx : null))
            : null;
          return (
            <li key={(p.tx ?? "") + i} className="space-y-1 py-3 text-sm [overflow-wrap:anywhere]">
              <p className="flex flex-wrap items-baseline gap-x-2">
                <span className="figures font-semibold">{usd(p.amount)} USDC</span>
                <span>
                  to <Payee addr={p.counterparty} />
                </span>
                <span className="text-muted-foreground">
                  {p.by === "you" ? "by you · " : "by the agent · "}
                  {p.at ? ago(p.at) : ""}
                </span>
                <span className="ml-auto">
                  <Receipt href={p.explorer} />
                </span>
              </p>
              <p className="leading-relaxed">
                <span className="text-muted-foreground">Why, as recorded: </span>
                {p.reason ? <q>{p.reason}</q> : <span className="text-muted-foreground">the service sent no reason for this payment.</span>}
              </p>
              <p className="text-caption text-muted-foreground">
                In the book:{" "}
                {line ? (
                  <>
                    {line.kind} {usd(line.amount)} USDC{line.detail ? ` — ${line.detail}` : ""}
                    {/* The owner's own payments are read from the chain's events (statement.rs `OwnerPaid`), not from the agent's record. */}
                    {!line.root ? (
                      line.paidBy === "owner" ? "; read from the chain's own record of the payment." : ""
                    ) : anchored ? (
                      <>
                        ; line {line.seq} of record {short(line.root)} · <Receipt href={`${explorer}/tx/${anchored}`}>on chain</Receipt>
                      </>
                    ) : (
                      `; line ${line.seq} of record ${short(line.root)}, put on chain with the agent's next check.`
                    )}
                  </>
                ) : book.isPending ? (
                  "reading the statement…"
                ) : book.isError ? (
                  `the statement could not be read (${(book.error as Error).message}).`
                ) : (
                  "not in this week's signed statement yet; it is written with the agent's check."
                )}
              </p>
            </li>
          );
        })}
      </ol>
    </div>
  );
}

function Outcome({ state }: { state: ReturnType<typeof useAction>["state"] }) {
  if (state.kind === "error") return <p className="text-sm text-destructive">{state.text}</p>;
  if (state.kind === "done")
    return (
      <p className="text-sm text-protect">
        {state.text} <Receipt href={state.href} />
      </p>
    );
  return null;
}

/** On a demo account the guardians warning (P-202) stays as a line, without the confirm tap before Autopilot or a raise
 *  (PM ruling 2026-10-17, for the founder to veto): a judge has five minutes and the account holds test USDC. Real
 *  accounts keep the gate (`useKeysGate`). */
function KeysLine() {
  const { tenant } = useAccount();
  const k = useKeys(tenant).data;
  if (!k || keysReady(k)) return null;
  return (
    <p className="text-caption text-muted-foreground">
      {keysSet(k)} of 3 keys set: on a real account, add two guardians before turning Autopilot on or raising a limit, so a lost device does not lose
      the account.
    </p>
  );
}
