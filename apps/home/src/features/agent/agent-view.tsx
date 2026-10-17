"use client";
// The Agent view (DESIGN, "a terminal by night"): `tameion top` for the account, then its record stream with every
// record's three slots. The one place a hash, a leaf, an explorer link or a slot id appears (DESIGN Rule 6). Reads
// status, inbox and symbolon through the /api proxy every 30 s; each figure is labelled by the field it comes from and
// the page computes no money (Rule 3). A read that fails keeps the last answer on screen, marked with its time.
import { useCallback, useEffect, useState } from "react";
import { useSkeleton } from "../../components/skeleton.tsx";
import { AgentSkeleton } from "../../components/skeletons.tsx";
import { fetchInbox, fetchStatus, fetchSymbolon } from "../../lib/api.ts";
import type { Inbox, Status, Symbolon } from "../../lib/types.ts";
import { clock, money, perMinute } from "./format.ts";
import { Stream } from "./stream.tsx";

const EVERY_MS = 30_000;

/** One source as last read: its answer (kept when a later read fails), when it came, and why the latest read failed. */
type Src<T> = { value: T | null; at: number | null; error: string | null };
export type Reads = { status: Src<Status>; inbox: Src<Inbox>; symbolon: Src<Symbolon> };
const none = <T,>(): Src<T> => ({ value: null, at: null, error: null });
const settle = <T,>(prev: Src<T>, r: PromiseSettledResult<T>, at: number): Src<T> =>
  r.status === "fulfilled" ? { value: r.value, at, error: null } : { ...prev, error: (r.reason as Error)?.message || "no answer" };

/** `name`: the account's name from the route; the fetchers resolve it (an address works too). */
export function AgentView({ name }: { name: string }) {
  const [reads, setReads] = useState<Reads | null>(null);

  const load = useCallback(async () => {
    const got = await Promise.allSettled([fetchStatus(name), fetchInbox(name), fetchSymbolon(name)]);
    const at = Date.now();
    setReads((p) => {
      const prev = p ?? { status: none<Status>(), inbox: none<Inbox>(), symbolon: none<Symbolon>() };
      return {
        status: settle(prev.status, got[0] as PromiseSettledResult<Status>, at),
        inbox: settle(prev.inbox, got[1] as PromiseSettledResult<Inbox>, at),
        symbolon: settle(prev.symbolon, got[2] as PromiseSettledResult<Symbolon>, at),
      };
    });
  }, [name]);

  useEffect(() => {
    let live = true;
    const tick = () => live && load();
    tick();
    const t = setInterval(tick, EVERY_MS);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [load]);

  const loading = useSkeleton(reads === null);
  if (loading || !reads) return <AgentSkeleton />;
  return <AgentScreen reads={reads} onRetry={load} />;
}

/** What the view shows for a set of reads: pure, so a test renders it from fixtures. */
export function AgentScreen({ reads, onRetry, now }: { reads: Reads; onRetry?: () => void; now?: number }) {
  const { status, inbox, symbolon } = reads;
  // The address is the service's own word for the account; the Agent view is the one place it is shown.
  const policy = symbolon.value?.policy ?? status.value?.policy ?? inbox.value?.policy;
  const failed = (Object.entries(reads) as [keyof Reads, Src<unknown>][]).filter(([, s]) => s.error);
  const latest = Math.max(...Object.values(reads).map((s: Src<unknown>) => s.at ?? 0));
  return (
    <div className="mono flex flex-col gap-(--space-5)">
      <div className="flex flex-wrap gap-x-(--space-5) gap-y-(--space-1)">
        <h1 className="mono m-0">tameion top</h1>
        {policy && <span className="break-all">{policy}</span>}
        {symbolon.value?.chain && <span>{symbolon.value.chain}</span>}
        {inbox.value?.mode && <span>mode {inbox.value.mode}</span>}
        {latest > 0 && <span className="meta">as of {clock(latest)}</span>}
      </div>
      {failed.length > 0 && (
        <div role="alert" className="flex flex-wrap items-center gap-x-(--space-4) gap-y-(--space-2)">
          <p className="m-0">
            {failed.map(([k, s]) => (
              <span key={k} className="mr-(--space-4) inline-block">
                {k} did not answer ({s.error}){s.at ? `; shown as of ${clock(s.at)}` : ""}.
              </span>
            ))}
          </p>
          {onRetry && (
            <button type="button" onClick={onRetry} className="min-h-(--touch) cursor-pointer rounded-(--radius-plate) border border-(--rule) bg-transparent px-(--space-3) text-(--ink) [font:inherit]">
              Try again
            </button>
          )}
        </div>
      )}
      <Top status={status.value} inbox={inbox.value} />
      {symbolon.value ? (
        <Stream data={symbolon.value} now={now} />
      ) : (
        <p className="m-0">The records did not load; the figures above still stand.</p>
      )}
    </div>
  );
}

/** `tameion top` for one unit (USDC today): held, reserved, consumed, rate per minute, runway; each with its source. */
export function Top({ status, inbox }: { status: Status | null; inbox: Inbox | null }) {
  const qty = (s: string | null | undefined) => money(s)?.qty ?? null;
  const days = (n: number | null | undefined) => (typeof n === "number" ? `${n} d` : null);
  const cols: { head: string; source: string; value: string | null }[] = [
    { head: "Held", source: "inbox.balance", value: qty(inbox?.balance) },
    {
      head: "Reserved",
      source: `inbox.runway.horizonTotal${inbox?.runway?.horizonDays ? ` (${inbox.runway.horizonDays} d)` : ""}`,
      value: qty(inbox?.runway?.horizonTotal),
    },
    { head: "Consumed", source: "inbox.paidTotal", value: qty(inbox?.paidTotal) },
    { head: "Rate / min", source: `status.rate${status?.windowDays ? ` (${status.windowDays} d)` : ""} ÷ 1440`, value: perMinute(status?.rate) },
    { head: "Runway", source: "status.runway.byRateDays", value: days(status?.runway?.byRateDays) },
    { head: "First shortfall", source: "status.runway.firstShortfallDays", value: days(status?.runway?.firstShortfallDays) },
  ];
  const missing = cols.filter((c) => c.value === null).map((c) => c.source.split(" ")[0]);
  return (
    <section aria-label="The account now" className="flex flex-col gap-(--space-1)">
      <div role="region" aria-label="The account now, by unit" tabIndex={0} className="relative overflow-x-auto">
        <table className="mono border-collapse whitespace-nowrap text-left">
          <thead>
            <tr>
              <th scope="col" className="pr-(--space-5) font-normal">
                Unit
              </th>
              {cols.map((c) => (
                <th key={c.head} scope="col" className="pr-(--space-5) text-right font-normal">
                  {c.head}
                </th>
              ))}
            </tr>
            <tr className="meta border-b border-(--rule)">
              <th scope="col" className="pb-(--space-1) pr-(--space-5) font-normal">
                <span className="sr-only">source</span>
              </th>
              {cols.map((c) => (
                <th key={c.head} scope="col" className="pb-(--space-1) pr-(--space-5) text-right font-normal">
                  {c.source}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            <tr>
              <th scope="row" className="pt-(--space-1) pr-(--space-5) font-normal">
                USDC
              </th>
              {cols.map((c) => (
                <td key={c.head} className="pt-(--space-1) pr-(--space-5) text-right">
                  {c.value ?? "–"}
                </td>
              ))}
            </tr>
          </tbody>
        </table>
      </div>
      {missing.length > 0 && <p className="meta m-0">– the service gave no figure: {missing.join(", ")}.</p>}
    </section>
  );
}
