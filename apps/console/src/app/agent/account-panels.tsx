"use client";

import { ExternalLink, OctagonAlert, ShieldCheck } from "lucide-react";
import Link from "next/link";
import { safeHref } from "@/lib/guard";
import { plainWhy, tail, usd } from "@/lib/tameion";
import type { Inbox } from "@/lib/types";
import type { AnchoredRecord } from "@/lib/verify";
import { cn } from "@/lib/utils";
import { age, askRows, paidRows, refusedRows, shortTx, spentShare, stamp } from "./feed";
import { Empty, Fact, Panel, Td, Th } from "./panel";

// What an account's agents did, read from the account's own record (the inbox, its anchored records, its last pass), laid out like the
// exchange panels: dense, right-aligned figures, a time on every row. Every string is the service's; nothing here is made up.

/** The account's own name for a payee (docs/81), else the last four of the address: an address is never a name. */
const nameOf = (inbox: Inbox) => (address: string) => inbox.parties?.[address.toLowerCase()]?.label || `••${tail(address)}`;

const MODE: Record<string, string> = {
  Watch: "reads the account, never pays",
  Propose: "prepares each payment and waits for your tap",
  Autopilot: "pays approved payees by itself, inside the caps",
};

/** The account halts itself when its own checks fail (`log/HALT`); only a person clears it. */
export function haltOf(inbox: Inbox): { reason: string; clear: string | null } | null {
  const h = inbox.halted;
  if (!h) return null;
  if (typeof h === "object") {
    const o = h as { reason?: unknown; clear?: unknown };
    return { reason: typeof o.reason === "string" && o.reason ? o.reason : "halted", clear: typeof o.clear === "string" ? o.clear : null };
  }
  return { reason: String(h), clear: null };
}

export function AccountHaltBanner({ halt }: { halt: { reason: string; clear: string | null } }) {
  return (
    <div role="alert" className="flex items-start gap-3 rounded-lg border border-destructive/50 bg-destructive/10 px-3 py-2.5">
      <OctagonAlert aria-hidden className="mt-0.5 size-4 shrink-0 text-destructive" />
      <div className="min-w-0 text-sm">
        <p className="font-semibold text-destructive">The account&apos;s agent is halted</p>
        <p className="mt-0.5 break-words">{halt.reason}</p>
        <p className="mt-0.5 text-foreground/80">Nothing is paid until a person clears it{halt.clear ? `: ${halt.clear}` : ""}.</p>
      </div>
    </div>
  );
}

// ---- the mandate ----------------------------------------------------------------------------------------------------------
export function MandatePanel({ inbox }: { inbox: Inbox }) {
  const { caps } = inbox;
  const share = spentShare(caps.spent24h, caps.daily);
  const daily = parseFloat(caps.daily);
  const onChain = inbox.deployed !== false;
  const words = inbox.pending?.words;
  return (
    <Panel title="Mandate" sub="what the agents may spend" right={<span className="rounded-full border px-2 py-0.5 text-xs font-medium text-foreground">{inbox.mode}</span>}>
      <div className="px-3 py-3">
        {!onChain ? (
          <p className="text-caption text-muted-foreground">The mandate is not on chain yet, so nothing can be spent. It starts when the account is funded.</p>
        ) : (
          <>
            <p className="text-xs text-muted-foreground">Left now</p>
            <p className="mt-0.5 text-3xl font-semibold leading-none tracking-tight">
              {caps.leftNow != null ? usd(caps.leftNow) : <span className="text-muted-foreground">—</span>}
              <span className="ml-1.5 text-caption font-normal text-muted-foreground">USDC</span>
            </p>
            {share != null && caps.spent24h != null ? (
              <div className="mt-3">
                <div
                  role="meter"
                  aria-label="Spent in the last 24 hours, against the daily cap"
                  aria-valuemin={0}
                  aria-valuemax={daily}
                  aria-valuenow={Math.min(parseFloat(caps.spent24h), daily)}
                  aria-valuetext={`${usd(caps.spent24h)} of ${usd(caps.daily)} USDC spent in the last 24 hours`}
                  className="relative h-2.5 rounded-full bg-muted"
                >
                  <div
                    className={cn("h-full rounded-full transition-[width] duration-300 ease-out", share >= 1 ? "bg-destructive" : share >= 0.8 ? "bg-attention" : "bg-protect")}
                    style={{ width: `${Math.min(1, share) * 100}%` }}
                  />
                </div>
                <div className="mt-1.5 flex justify-between gap-3 text-xs text-muted-foreground">
                  <span>
                    Spent in 24 h <span className="figures font-medium text-foreground">{usd(caps.spent24h)}</span>
                    {share >= 1 ? " · at the cap" : share >= 0.8 ? " · near the cap" : ""}
                  </span>
                  <span className="figures">of {usd(caps.daily)}</span>
                </div>
              </div>
            ) : (
              <p className="mt-2 text-caption text-muted-foreground">This service did not report what was spent in the last 24 hours.</p>
            )}
          </>
        )}
        <dl className="mt-3 grid grid-cols-3 gap-3 border-t pt-3">
          <Fact label="Per payment">{usd(caps.perAction)}</Fact>
          <Fact label="Per day">{usd(caps.daily)}</Fact>
          <Fact label="Account holds">{usd(inbox.balance)}</Fact>
        </dl>
        <p className="mt-2 text-xs text-muted-foreground">USDC. {MODE[inbox.mode] ? `${inbox.mode}: ${MODE[inbox.mode]}.` : ""}</p>
        {words && <p className="mt-2 text-caption text-attention">A change waits: {words}</p>}
      </div>
    </Panel>
  );
}

// ---- the last pass and the last anchor -------------------------------------------------------------------------------------
type Q<T> = { data?: T; isPending: boolean; isError: boolean };

export function PassAnchorPanel({ inbox, records, elapsedSecs, job, now, explorer }: { inbox: Inbox; records: Q<AnchoredRecord[]>; elapsedSecs: number | null; job: Q<unknown>; now: number; explorer: string | null }) {
  const halt = haltOf(inbox);
  const lp = inbox.lastPass;
  const last = records.data?.length ? records.data[records.data.length - 1] : null;
  const tx = last?.anchorTx ?? null;
  const href = explorer && tx && /^0x[0-9a-fA-F]{64}$/.test(tx) && /^https:\/\//i.test(explorer) ? `${explorer}/tx/${tx}` : null;
  const row = "flex items-baseline justify-between gap-4 px-3 py-2";
  const none = <span className="text-muted-foreground">none yet</span>;
  return (
    <Panel title="Pass & anchor" sub="the agent's last check and its record">
      <dl className="divide-y text-caption">
        <div className={row}>
          <dt className="text-muted-foreground">Halt</dt>
          <dd className="min-w-0 text-right">
            {halt ? (
              <span className="font-semibold text-destructive">
                Halted<span className="font-normal text-foreground"> · {halt.reason}</span>
              </span>
            ) : (
              <span className="inline-flex items-center gap-1 text-protect">
                <ShieldCheck aria-hidden className="size-3.5" /> None
              </span>
            )}
          </dd>
        </div>
        <div className={row}>
          <dt className="text-muted-foreground">Last pass</dt>
          <dd className="figures min-w-0 text-right">
            {lp ? (
              <>
                {stamp(lp.at, now || lp.at * 1000)}
                {now > 0 && <span className="text-muted-foreground"> · {age(Math.max(0, now - lp.at * 1000))} ago</span>}
                <span className="block text-xs text-muted-foreground">
                  {lp.obligations} bill{lp.obligations === 1 ? "" : "s"} found · {lp.transfers ?? 0} transfer{lp.transfers === 1 ? "" : "s"} read
                </span>
              </>
            ) : (
              none
            )}
          </dd>
        </div>
        <div className={row}>
          <dt className="text-muted-foreground">Pass latency</dt>
          <dd className="figures">{!lp ? <span className="text-muted-foreground">—</span> : elapsedSecs != null ? `${elapsedSecs} s` : job.isPending ? "…" : <span className="text-muted-foreground">not reported</span>}</dd>
        </div>
        {inbox.nextPassSecs != null && (
          <div className={row}>
            <dt className="text-muted-foreground">Next pass</dt>
            <dd className="figures">in {age(inbox.nextPassSecs * 1000)}</dd>
          </div>
        )}
        <div className={row}>
          <dt className="text-muted-foreground">Last anchor</dt>
          <dd className="figures min-w-0 text-right">
            {records.isPending ? (
              "…"
            ) : records.isError ? (
              <span className="text-muted-foreground">not readable</span>
            ) : last ? (
              <>
                {stamp(last.at, now || last.at * 1000)}{" "}
                {href && tx ? (
                  <a href={href} target="_blank" rel="noopener" className="inline-flex items-center gap-1 font-medium underline-offset-4 hover:underline" aria-label={`${shortTx(tx)}, the anchor transaction, opens the explorer`}>
                    {shortTx(tx)} <ExternalLink aria-hidden className="size-3" />
                  </a>
                ) : tx ? (
                  <span className="text-muted-foreground">{shortTx(tx)}</span>
                ) : null}
                <span className="block text-xs text-muted-foreground">
                  the record of {last.events} event{last.events === 1 ? "" : "s"}, on chain
                </span>
              </>
            ) : (
              none
            )}
          </dd>
        </div>
        {inbox.agentAuthority && (
          <div className={row}>
            <dt className="text-muted-foreground">Agent key</dt>
            <dd className="figures">{inbox.agentAuthority.lapsed ? <span className="text-attention">lapsed</span> : `${inbox.agentAuthority.daysLeft} days left`}</dd>
          </div>
        )}
      </dl>
    </Panel>
  );
}

// ---- what was paid ---------------------------------------------------------------------------------------------------------
const Receipt = ({ href, label }: { href: string | null; label: string }) => {
  const safe = safeHref(href);
  return safe ? (
    <a href={safe} target="_blank" rel="noopener" className="inline-flex items-center gap-1 font-medium underline-offset-4 hover:underline" aria-label={`Receipt for ${label}, opens the explorer`}>
      Receipt <ExternalLink aria-hidden className="size-3" />
    </a>
  ) : (
    <span className="text-muted-foreground">—</span>
  );
};

export function PaymentsPanel({ inbox, now }: { inbox: Inbox; now: number }) {
  const rows = paidRows(inbox.paid ?? [], nameOf(inbox)).slice(0, 12);
  return (
    <Panel title="Payments" sub="what the agents paid, and why" right={<span className="figures">{usd(inbox.paidTotal)} USDC in all</span>}>
      {rows.length === 0 ? (
        <Empty>No payment yet. Each one will show here with the reason the agent gave.</Empty>
      ) : (
        <table className="w-full text-caption">
          <caption className="sr-only">Payments the account has made, newest first, with the reason given for each</caption>
          <thead>
            <tr>
              <Th>Time</Th>
              <Th className="sm:hidden">Payment</Th>
              <Th className="hidden sm:table-cell">To</Th>
              <Th className="hidden sm:table-cell">Why</Th>
              <Th right>USDC</Th>
              <Th className="hidden text-right sm:table-cell">Receipt</Th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {rows.map((r, i) => (
              <tr key={`${r.receipt ?? i}-${r.at ?? i}`} className="align-top hover:bg-muted/50">
                <Td className="figures whitespace-nowrap">{r.at != null ? stamp(r.at, now || r.at * 1000) : "—"}</Td>
                <Td className="sm:hidden">
                  <span className="font-medium">{r.to}</span>
                  {r.why && <span className="block text-xs text-muted-foreground">{r.why}</span>}
                  <span className="block text-xs">
                    <Receipt href={r.receipt} label={`${usd(r.amount)} USDC to ${r.to}`} />
                  </span>
                </Td>
                <Td className="hidden font-medium sm:table-cell">{r.to}</Td>
                <Td className="hidden text-muted-foreground sm:table-cell">{r.why || "no reason recorded"}</Td>
                <Td right>{usd(r.amount)}</Td>
                <Td className="hidden text-right sm:table-cell">
                  <Receipt href={r.receipt} label={`${usd(r.amount)} USDC to ${r.to}`} />
                </Td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}

// ---- what the checks turned away ---------------------------------------------------------------------------------------------
export function RefusedPanel({ inbox, now }: { inbox: Inbox; now: number }) {
  const rows = refusedRows(inbox.refused ?? [], inbox.purchases ?? [], plainWhy, nameOf(inbox)).slice(0, 10);
  return (
    <Panel title="Refused" sub="turned away by the checks, in words">
      {rows.length === 0 ? (
        <Empty>Nothing has been turned away.</Empty>
      ) : (
        <table className="w-full text-caption">
          <caption className="sr-only">Payments and purchases the checks refused, newest first, with the reason in words</caption>
          <thead>
            <tr>
              <Th>Time</Th>
              <Th>What and why</Th>
              <Th right>USDC</Th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {rows.map((r, i) => (
              <tr key={`${r.what}-${r.at ?? i}-${i}`} className="align-top hover:bg-muted/50">
                <Td className="figures whitespace-nowrap">{r.at != null ? stamp(r.at, now || r.at * 1000) : "—"}</Td>
                <Td>
                  <span className="font-medium">{r.what}</span>
                  <span className="block text-destructive">{r.why}</span>
                  {r.reason && <span className="block text-xs text-muted-foreground">the agent wanted it for: {r.reason}</span>}
                </Td>
                <Td right>{usd(r.amount)}</Td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}

// ---- what the agents asked of the owner ----------------------------------------------------------------------------------------
export function AskedPanel({ inbox, now }: { inbox: Inbox; now: number }) {
  const rows = askRows((inbox as { agentAsks?: unknown }).agentAsks, inbox.waiting ?? []);
  if (rows.length === 0) return null;
  return (
    <Panel title="Asked of you" sub="by the agents, with their reasons">
      <ul className="divide-y text-caption">
        {rows.map((r, i) => (
          <li key={`${r.text}-${i}`} className="px-3 py-2">
            <p>
              {r.at != null && <span className="figures mr-2 text-muted-foreground">{stamp(r.at, now || r.at * 1000)}</span>}
              {r.text}
            </p>
            {r.why && <p className="text-xs text-muted-foreground">because: {r.why}</p>}
          </li>
        ))}
      </ul>
      <p className="border-t px-3 py-2 text-xs text-muted-foreground">
        You decide on the{" "}
        <Link href="/" className="font-medium text-foreground underline-offset-4 hover:underline">
          Payments page
        </Link>
        .
      </p>
    </Panel>
  );
}
