"use client";
// The Agent view (DESIGN: the Agent is at sea): `tameion top` for the account, then its record stream with every
// record's three slots. The one place a hash, a leaf, an explorer link or a slot id appears. Reads status, inbox and
// symbolon through the /api proxy every 30 s; each figure is labelled by the field it comes from and the page computes no
// money. A read that fails keeps the last answer on screen, marked with its time; what the service said sits behind a
// disclosure.
import { Fragment, useCallback, useEffect, useState } from "react";
import { useSkeleton } from "../../components/skeleton.tsx";
import { AgentSkeleton } from "../../components/skeletons.tsx";
import { fetchInbox, fetchStatus, fetchSymbolon } from "../../lib/api.ts";
import type { Inbox, Status, Symbolon } from "../../lib/types.ts";
import { copy, SOURCES } from "./copy.ts";
import { clock, money, perMinute } from "./format.ts";
import { Stream } from "./stream.tsx";

const EVERY_MS = 30_000;

/** One source as last read: its answer (kept when a later read fails), when it came, and why the latest read failed. */
type Src<T> = { value: T | null; at: number | null; error: string | null };
export type Reads = { status: Src<Status>; inbox: Src<Inbox>; symbolon: Src<Symbolon> };
const none = <T,>(): Src<T> => ({ value: null, at: null, error: null });
const settle = <T,>(prev: Src<T>, r: PromiseSettledResult<T>, at: number): Src<T> =>
  r.status === "fulfilled" ? { value: r.value, at, error: null } : { ...prev, error: (r.reason as Error)?.message || "" };

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
  const failed = (Object.keys(reads) as (keyof Reads)[]).filter((k) => reads[k].error !== null);
  const latest = Math.max(...Object.values(reads).map((s: Src<unknown>) => s.at ?? 0));
  return (
    <div className="mono-dense flex flex-col gap-(--space-5)">
      <div className="flex flex-wrap gap-x-(--space-5) gap-y-(--space-1)">
        <h1 className="row-title m-0">{copy.title}</h1>
        {policy && <span className="break-all">{policy}</span>}
        {symbolon.value?.chain && <span>{symbolon.value.chain}</span>}
        {inbox.value?.mode && <span>{copy.mode(inbox.value.mode)}</span>}
        {latest > 0 && <span className="meta">{copy.asOf(clock(latest))}</span>}
      </div>
      {failed.length > 0 && (
        <div role="alert" className="flex flex-col gap-(--space-2)">
          <div className="flex flex-wrap items-center gap-x-(--space-4) gap-y-(--space-2)">
            <p className="mono-read m-0">
              {failed.map((k) => (
                <span key={k} className="mr-(--space-4) inline-block">
                  {copy.didNotAnswer(SOURCES[k], reads[k].at ? clock(reads[k].at) : null)}
                </span>
              ))}
            </p>
            {onRetry && (
              <button
                type="button"
                onClick={onRetry}
                className="min-h-(--button-compact) cursor-pointer rounded-(--radius-button) border-0 bg-(--plate-2) px-(--space-4) text-(--ink) [font:inherit]"
              >
                {copy.tryAgain}
              </button>
            )}
          </div>
          {failed.some((k) => reads[k].error) && (
            <details className="meta">
              <summary className="cursor-pointer">{copy.details}</summary>
              {failed.map((k) => (
                <p key={k} className="m-0 break-all">
                  {k}: {reads[k].error}
                </p>
              ))}
            </details>
          )}
        </div>
      )}
      <Top status={status.value} inbox={inbox.value} />
      {symbolon.value ? <Stream data={symbolon.value} now={now} /> : <p className="mono-read m-0">{copy.recordsMissing}</p>}
    </div>
  );
}

/** `tameion top` for one unit (USDC today): held, reserved, consumed, rate per minute, runway; each with its source. */
export function Top({ status, inbox }: { status: Status | null; inbox: Inbox | null }) {
  const T = copy.top;
  const qty = (s: string | null | undefined) => money(s)?.qty ?? null;
  const days = (n: number | null | undefined) => (typeof n === "number" ? T.days(n) : null);
  const cols: { head: string; source: string; value: string | null }[] = [
    { head: T.held.head, source: T.held.source, value: qty(inbox?.balance) },
    { head: T.reserved.head, source: T.reserved.source(inbox?.runway?.horizonDays), value: qty(inbox?.runway?.horizonTotal) },
    { head: T.consumed.head, source: T.consumed.source, value: qty(inbox?.paidTotal) },
    { head: T.rate.head, source: T.rate.source(status?.windowDays), value: perMinute(status?.rate) },
    { head: T.runway.head, source: T.runway.source, value: days(status?.runway?.byRateDays) },
    { head: T.shortfall.head, source: T.shortfall.source, value: days(status?.runway?.firstShortfallDays) },
  ];
  const missing = cols.filter((c) => c.value === null).map((c) => c.source.split(" ")[0]);
  return (
    <section aria-label={T.region} className="flex flex-col gap-(--space-1)">
      <div role="region" aria-label={T.table} tabIndex={0} className="relative overflow-x-auto">
        <table className="mono-dense border-collapse text-left">
          <thead>
            <tr>
              <th scope="col" className="pr-(--space-5) font-normal">
                {T.unit}
              </th>
              {cols.map((c) => (
                <th key={c.head} scope="col" className="whitespace-nowrap pr-(--space-5) text-right font-normal">
                  {c.head === T.reserved.head && (
                    <span aria-hidden className="mr-(--space-2) inline-block size-(--space-2) rounded-(--radius-mark) bg-(--transit) align-middle" />
                  )}
                  {c.head}
                </th>
              ))}
            </tr>
            <tr className="meta border-b border-(--rule)">
              <th scope="col" className="pb-(--space-1) pr-(--space-5) font-normal">
                <span className="sr-only">{T.source}</span>
              </th>
              {cols.map((c) => (
                <th key={c.head} scope="col" className="pb-(--space-1) pr-(--space-5) text-right align-top font-normal">
                  {/* a long field name breaks after its dots, so the table fits the 1120 px surface */}
                  {c.source.split(".").map((part, i, all) => (
                    <Fragment key={i}>
                      {part}
                      {i < all.length - 1 && (
                        <>
                          .<wbr />
                        </>
                      )}
                    </Fragment>
                  ))}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            <tr>
              <th scope="row" className="pt-(--space-1) pr-(--space-5) font-normal">
                {T.usdc}
              </th>
              {cols.map((c) => (
                <td key={c.head} className="whitespace-nowrap pt-(--space-1) pr-(--space-5) text-right">
                  {c.value ?? copy.none}
                </td>
              ))}
            </tr>
          </tbody>
        </table>
      </div>
      {missing.length > 0 && <p className="mono-read m-0 text-(--ink-2)">{T.noFigure(missing)}</p>}
    </section>
  );
}
