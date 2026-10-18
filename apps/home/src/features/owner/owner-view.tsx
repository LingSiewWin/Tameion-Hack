"use client";
// The Owner home, drawn from what the service said (DESIGN §Page grammar): the authority sentence and Stop, four facts,
// what the agent paid and why, then Needs you. Pure: the container (owner-home.tsx) reads and acts; this only draws,
// so the zero-hash test can render it from a captured inbox. Depth is one step of fill; rules only between table rows.
import { Sheet } from "../../components/sheet.tsx";
import { Mark } from "../../lib/mark.ts";
import type { Inbox } from "../../lib/types.ts";
import { asOf, authority, COPY, facts, halvesWords, lines, needs, pendingWords, phaseText, type Halves, type Line, type Need } from "./words.ts";

/** A control in flight: its phase and since when (DESIGN §Money lifecycle); it never looks disabled. */
export type Phase = { label: string; since: number } | null;
/** What a tap ended in: `detail` is the service's own words, behind a disclosure (DESIGN §States, error). */
export type Outcome = { key: string; text: string; failed: boolean; detail?: string } | null;

export type OwnerViewProps = {
  inbox: Inbox;
  /** status.runway.byRateDays */
  byRateDays: number | null | undefined;
  /** which halves of each payment's record are present (H3's projection), by transaction; empty when there is none */
  halves: ReadonlyMap<string, Halves>;
  nowSecs: number;
  /** the clock for phases in flight (ms) */
  clockMs?: number;
  /** ms of the read these figures come from, when the latest read failed (offline / stale) */
  staleAt?: number | null;
  /** a reader without one of the account's owner keys on this device: reads only, no Stop, no actions */
  guest?: boolean;
  /** the agent's key address, for its mark beside "the agent" (never rendered as text) */
  agentId?: string;
  openKey?: string | null;
  onOpen?: (key: string | null) => void;
  onAgentView?: (line: Line) => void;
  onRetry?: () => void;
  /** Stop (or Start, when stopped): `asking` shows the one question in words */
  asking?: boolean;
  onStopAsk?: () => void;
  onStopCancel?: () => void;
  onStop?: () => void;
  stopPhase?: Phase;
  needPhase?: Record<string, Phase>;
  onNeed?: (n: Need) => void;
  outcome?: Outcome;
};

const plate = "rounded-(--radius-plate) bg-(--plate)";
const button = "inline-flex min-h-(--button) items-center justify-center rounded-(--radius-button) px-(--space-5) row-title";
const compact = "inline-flex min-h-(--button-compact) shrink-0 items-center justify-center rounded-(--radius-button) bg-(--plate-2) px-(--space-4) row-title text-(--ink)";

/** The symbolon mark (DESIGN §Direction): two halves that fit, the witness the dot between; drawn in ink, only when sealed. */
function SealMark() {
  return (
    <svg width="16" height="12" viewBox="0 0 16 12" aria-hidden className="shrink-0 text-(--sealed)">
      <path d="M7 1a5 5 0 0 0 0 10z" fill="currentColor" />
      <path d="M9 1a5 5 0 0 1 0 10z" fill="currentColor" />
      <circle cx="8" cy="6" r="2" fill="currentColor" />
    </svg>
  );
}

function Result({ o }: { o: NonNullable<Outcome> }) {
  return (
    <div className="flex flex-col gap-(--space-1)">
      <p role={o.failed ? "alert" : "status"} className={`row m-0 ${o.failed ? "text-(--attention)" : ""}`}>
        {o.text}
      </p>
      {o.detail && (
        <details className="meta">
          <summary className="cursor-pointer">{COPY.whatHappened}</summary>
          <p className="m-0 pt-(--space-1)">{o.detail}</p>
        </details>
      )}
    </div>
  );
}

function Hero(p: OwnerViewProps) {
  const stopped = p.inbox.mode === "Watch";
  const verb = stopped ? COPY.start : COPY.stop;
  // A start already asked waits out its delay (the line above says when): no second ask. Stop is always there.
  const control = !p.guest && !(stopped && (p.inbox.pending?.kinds ?? 0) & 2);
  const pending = pendingWords(p.inbox);
  const label = (ph: Phase | undefined, rest: string) => (ph ? phaseText(ph, p.clockMs ?? ph.since) : rest);
  return (
    <section aria-labelledby="authority" className="flex flex-col gap-(--space-5)">
      <h1 id="authority" className="authority m-0 max-w-[68ch]">
        {authority(p.inbox)}
      </h1>
      {pending && <p className="row m-0">{pending}</p>}
      {control &&
        (p.asking ? (
          <div className={`${plate} flex flex-col gap-(--space-4) p-(--space-5)`} role="group" aria-labelledby="stop-q">
            <p id="stop-q" className="row-title m-0">
              {stopped ? COPY.startAsk : COPY.stopAsk}
            </p>
            <div className="flex flex-col gap-(--space-3) md:flex-row">
              <button type="button" onClick={p.onStop} className={`${button} ${stopped ? "bg-(--ink) text-(--paper)" : "bg-(--attention) text-(--plate)"}`}>
                {label(p.stopPhase, verb)}
              </button>
              {!p.stopPhase && (
                <button type="button" onClick={p.onStopCancel} className={`${button} bg-(--plate-2) text-(--ink)`}>
                  {stopped ? COPY.keepStopped : COPY.keepRunning}
                </button>
              )}
            </div>
          </div>
        ) : (
          <button
            type="button"
            onClick={p.onStopAsk}
            className={`${button} w-full md:w-auto md:self-start ${stopped ? "bg-(--ink) text-(--paper)" : "bg-(--plate) text-(--attention)"}`}
          >
            {verb}
          </button>
        ))}
      {p.outcome?.key === "stop" && <Result o={p.outcome} />}
    </section>
  );
}

function Facts(p: OwnerViewProps) {
  return (
    <dl className="m-0 grid grid-cols-2 gap-(--space-3) md:grid-cols-4" aria-label={COPY.factsLabel}>
      {facts(p.inbox, p.byRateDays, p.nowSecs).map((f) => (
        <div key={f.label} className={`${plate} flex min-w-0 flex-col gap-(--space-2) p-(--space-5)`}>
          <dt className="meta">{f.label}</dt>
          <dd className={`m-0 flex min-w-0 flex-wrap items-baseline gap-x-(--space-2) [overflow-wrap:anywhere] ${f.attention ? "text-(--attention)" : ""}`}>
            {f.figure !== undefined ? <span className="balance money">{f.figure}</span> : <span className="row-title">{f.words}</span>}
            {f.unit && <span className="row">{f.unit}</span>}
          </dd>
          {p.staleAt && <dd className="meta m-0">{asOf(p.staleAt)}</dd>}
        </div>
      ))}
    </dl>
  );
}

/** One payment as a ledger line: stacked on a phone; the book's columns (date, who, amount, why, state) from 768 px. */
const LINE = "grid w-full grid-cols-[minmax(0,1fr)_auto] gap-x-(--space-4) gap-y-(--space-1) md:grid-cols-[7rem_minmax(8rem,12rem)_7rem_minmax(0,1fr)_9rem] md:items-center";

function Paid(p: OwnerViewProps & { rows: Line[] }) {
  return (
    <section aria-labelledby="paid-title" className="flex flex-col gap-(--space-3)">
      <h2 id="paid-title" className="row-title m-0">
        {COPY.paidTitle}
      </h2>
      {p.rows.length ? (
        <div className={`${plate} px-(--space-5)`}>
          <div aria-hidden className={`${LINE} meta hidden border-b border-(--rule) py-(--space-3) md:grid`}>
            <span>{COPY.paidColumns[0]}</span>
            <span>{COPY.paidColumns[1]}</span>
            <span className="text-right">{COPY.paidColumns[2]}</span>
            <span>{COPY.paidColumns[3]}</span>
            <span className="text-right">{COPY.paidColumns[4]}</span>
          </div>
          <ul className="m-0 list-none p-0">
            {p.rows.map((r) => (
              <li key={r.key} className="border-b border-(--rule) last:border-b-0">
                <button type="button" onClick={() => p.onOpen?.(r.key)} className={`${LINE} min-h-(--touch) bg-transparent py-(--space-4) text-left text-(--ink)`}>
                  <span className="meta order-5 md:order-1">{r.when}</span>
                  <span className="row-title order-1 flex min-w-0 items-center gap-(--space-2) md:order-2">
                    <Mark id={r.payeeId} kind="payee" size={24} />
                    <span className="min-w-0">{r.payee}</span>
                  </span>
                  <span className="row-title money order-2 text-right md:order-3">{r.amount}</span>
                  <span className="row order-3 col-span-2 md:order-4 md:col-span-1">{r.why}</span>
                  <span className={`meta order-6 flex items-center justify-end gap-(--space-1) md:order-5 ${r.state === "Refused" ? "text-(--attention)" : ""}`}>
                    {r.sealed && <SealMark />}
                    {r.state}
                  </span>
                  {p.staleAt && <span className="meta order-7 col-span-2 md:col-span-5">{asOf(p.staleAt)}</span>}
                </button>
              </li>
            ))}
          </ul>
        </div>
      ) : (
        <p className={`${plate} row m-0 p-(--space-5)`}>{p.inbox.payees?.length ? COPY.paidEmptyApproved : COPY.paidEmptyNoPayee}</p>
      )}
    </section>
  );
}

function Needs(p: OwnerViewProps & { items: Need[] }) {
  return (
    <section aria-labelledby="needs-title" className="flex flex-col gap-(--space-3)">
      <h2 id="needs-title" className={`row-title m-0 ${p.items.length ? "text-(--attention)" : ""}`}>
        {COPY.needsTitle}
      </h2>
      {/* a failure whose item is gone after the read that followed it still says what happened */}
      {p.outcome && p.outcome.key !== "stop" && !p.items.some((n) => n.key === p.outcome?.key) && <Result o={p.outcome} />}
      {p.items.length ? (
        <ul className={`${plate} m-0 list-none px-(--space-5) py-0`}>
          {p.items.map((n) => {
            const ph = p.needPhase?.[n.key];
            return (
              <li key={n.key} className="flex flex-col gap-(--space-3) border-b border-(--rule) py-(--space-4) last:border-b-0 md:flex-row md:items-center">
                <p className="row-title m-0 flex min-w-0 flex-1 items-center gap-(--space-3)">
                  <Mark id={n.counterparty.toLowerCase()} kind="payee" size={24} />
                  <span className="min-w-0">{n.sentence}</span>
                </p>
                {!p.guest && n.action && (
                  <button type="button" onClick={() => p.onNeed?.(n)} className={compact}>
                    {ph ? phaseText(ph, p.clockMs ?? ph.since) : n.action}
                  </button>
                )}
                {p.outcome?.key === n.key && <Result o={p.outcome} />}
              </li>
            );
          })}
        </ul>
      ) : (
        <p className={`${plate} row m-0 p-(--space-5)`}>{COPY.needsEmpty}</p>
      )}
    </section>
  );
}

function Disclosure(p: OwnerViewProps & { line?: Line }) {
  const l = p.line;
  return (
    <Sheet open={!!l} onClose={() => p.onOpen?.(null)} title={l ? `${l.amount} to ${l.payee}` : ""}>
      {l && (
        <div className="flex flex-col gap-(--space-4)">
          <p className="row-title m-0 flex items-center gap-(--space-3)">
            <Mark id={l.payeeId} kind="payee" size={40} />
            {l.payee}
          </p>
          <p className="row m-0 flex items-center gap-(--space-3)">
            {l.by === "the agent" && p.agentId && <Mark id={p.agentId} kind="agent" size={24} />}
            <span>{l.state === "Refused" ? COPY.refusedSheet(l.when) : COPY.paidSheet(l.state, l.by, l.when)}</span>
          </p>
          <p className="row m-0">{l.why}</p>
          {l.state !== "Refused" && <p className="row m-0">{halvesWords(l.halves, l.payee)}</p>}
          {l.state !== "Refused" && (
            <button type="button" onClick={() => p.onAgentView?.(l)} className="row self-start bg-transparent px-0 py-(--space-3) text-(--ink) underline underline-offset-4">
              {COPY.agentView}
            </button>
          )}
        </div>
      )}
    </Sheet>
  );
}

/** The cold error: nothing read yet. Says what still holds and the one action; the page's one heading (DESIGN §States). */
export function OwnerError({ missing, onRetry }: { missing: boolean; onRetry?: () => void }) {
  return (
    <div role="alert" className={`${plate} flex flex-col gap-(--space-4) p-(--space-5)`}>
      <h1 className="authority m-0">{missing ? COPY.noAccount : COPY.serviceSilent}</h1>
      {!missing && <p className="row m-0">{COPY.noFigures}</p>}
      {!missing && (
        <button type="button" onClick={onRetry} className={`${button} self-start bg-(--ink) text-(--paper)`}>
          {COPY.tryAgain}
        </button>
      )}
    </div>
  );
}

export function OwnerView(p: OwnerViewProps) {
  const rows = lines(p.inbox, p.nowSecs, p.halves);
  const items = needs(p.inbox);
  return (
    <div className="flex flex-col gap-(--space-6)">
      {p.staleAt && (
        <div role="status" className={`${plate} flex flex-wrap items-center gap-x-(--space-4) gap-y-(--space-2) p-(--space-5)`}>
          <p className="row m-0 min-w-0 flex-1">{COPY.stale(asOf(p.staleAt))}</p>
          {p.onRetry && (
            <button type="button" onClick={p.onRetry} className={compact}>
              {COPY.tryAgain}
            </button>
          )}
        </div>
      )}
      <Hero {...p} />
      <Facts {...p} />
      <Paid {...p} rows={rows} />
      <Needs {...p} items={items} />
      <Disclosure {...p} line={rows.find((r) => r.key === p.openKey)} />
    </div>
  );
}
