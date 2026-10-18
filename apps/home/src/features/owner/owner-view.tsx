"use client";
// The Owner home, drawn from what the service said (DESIGN §Page grammar): the authority sentence and Stop, four facts,
// what the agent paid and why, then Needs you. Pure: the container (owner-home.tsx) reads and acts; this only draws,
// so the zero-hash test can render it from a captured inbox. Depth is one step of fill; rules only between table rows.
import { Sheet } from "../../components/sheet.tsx";
import { Mark } from "../../lib/mark.ts";
import type { Demo, Inbox } from "../../lib/types.ts";
import { button, compact, Mandate, PendingLine, plate, Result, Strip, type Actions } from "./actions-view.tsx";
import { asOf, authority, COPY, facts, foldRuns, halvesWords, lines, needs, payeeName, pendingWords, phaseText, stripOf, type Entry, type Halves, type Line, type Need, type Run } from "./words.ts";

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
  /** ms of the read these figures come from, when they are not this moment's: each figure is marked "as of HH:MM" */
  staleAt?: number | null;
  /** the latest read failed and these are kept from an earlier one: the line above the hero says so */
  silent?: boolean;
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
  /** the owner's other actions (payees, limits, mode, Cancel, the five minutes); absent for a reader */
  act?: Actions;
  /** the host's demo (`GET /api/level1/demo`) when this is a demo account the owner holds: the five minutes show */
  demo?: Demo | null;
  /** the clock countdowns read (unix s, ticking while something counts down); nowSecs when absent */
  clockSecs?: number;
  /** when the account opened (tenant.created, unix s): the five minutes' finish says how long they took */
  openedAt?: number;
};

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

function Hero(p: OwnerViewProps) {
  const stopped = p.inbox.mode === "Watch";
  const verb = stopped ? COPY.start : COPY.stop;
  // A start already asked waits out its delay (the line above says when): no second ask. Stop is always there.
  const control = !p.guest && !(stopped && (p.inbox.pending?.kinds ?? 0) & 2);
  const pending = pendingWords(p.inbox);
  const label = (ph: Phase | undefined, rest: string) => (ph ? phaseText(ph, p.clockMs ?? ph.since) : rest);
  // On a demo account at its Autopilot step, the five minutes hold the countdown and its Cancel; everywhere else, here.
  const inStrip = !!p.demo && !p.guest && stripOf(p.inbox, p.demo).step === 2 && p.inbox.pending?.mode === "Autopilot";
  return (
    <section aria-labelledby="authority" className="flex flex-col gap-(--space-5)">
      <h1 id="authority" className="authority m-0 max-w-[68ch]">
        {authority(p.inbox)}
      </h1>
      {!inStrip && <PendingLine inbox={p.inbox} clockSecs={p.clockSecs ?? p.nowSecs} clockMs={p.clockMs} act={p.guest ? undefined : p.act} outcome={p.outcome} />}
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
      {(p.outcome?.key === "stop" || (p.outcome?.key === "cancel" && !pending)) && <Result o={p.outcome} />}
    </section>
  );
}

function Facts(p: OwnerViewProps & { className?: string }) {
  return (
    <dl className={`m-0 grid grid-cols-1 gap-(--space-3) sm:grid-cols-2 lg:grid-cols-1 ${p.className ?? ""}`} aria-label={COPY.factsLabel}>
      {facts(p.inbox, p.byRateDays, p.nowSecs).map((f) => (
        <div key={f.label} className={`${plate} flex min-w-0 flex-col gap-(--space-1) p-(--space-5)`}>
          <dt className="meta">{f.label}</dt>
          {/* the figure hierarchy (founder): the amount at 39/44, its unit at 16 beside it on the baseline, the label at 13 above */}
          <dd className={`m-0 flex min-w-0 items-baseline gap-x-(--space-2) whitespace-nowrap ${f.attention ? "text-(--attention)" : ""}`}>
            {f.figure !== undefined ? <span className="balance money">{f.figure}</span> : <span className="row-title whitespace-normal">{f.words}</span>}
            {f.unit && <span className="row">{f.unit}</span>}
          </dd>
          {p.staleAt && <dd className="meta m-0">{asOf(p.staleAt)}</dd>}
        </div>
      ))}
    </dl>
  );
}

/** One payment as a ledger line: stacked on a phone; from 768 px the book's columns (date, who and why, amount, state). */
const LINE = "grid w-full grid-cols-[minmax(0,1fr)_auto] gap-x-(--space-4) gap-y-(--space-1) md:grid-cols-[6rem_minmax(0,1fr)_auto_7rem] md:items-start";
const CELL = {
  date: "meta order-5 md:col-start-1 md:row-start-1 md:pt-(--space-1)",
  who: "row-title order-1 flex min-w-0 items-center gap-(--space-2) md:col-start-2 md:row-start-1",
  amount: "row-title money order-2 text-right md:col-start-3 md:row-start-1",
  why: "row order-3 col-span-2 md:col-start-2 md:row-start-2 md:col-span-1",
  state: "meta order-6 flex items-center justify-end gap-(--space-1) md:col-start-4 md:row-start-1 md:pt-(--space-1)",
  stale: "meta order-7 col-span-2 md:col-start-2 md:row-start-3 md:col-span-1",
};

/** One line of the book; a folded run is one line too. The five minutes show their first payment with this same row. */
function LedgerRow({ e, onOpen, staleAt }: { e: Entry; onOpen?: (key: string | null) => void; staleAt?: number | null }) {
  const first = e.run ? e.run.lines[0] : e.line;
  const key = e.run ? e.run.key : e.line.key;
  return (
    <li className="border-b border-(--rule) last:border-b-0">
      <button type="button" onClick={() => onOpen?.(key)} className={`${LINE} min-h-(--touch) bg-transparent py-(--space-4) text-left text-(--ink)`}>
        <span className={CELL.date}>{first.when}</span>
        <span className={`${CELL.who} ${e.run ? "col-span-2 md:col-span-2" : ""}`}>
          <Mark id={first.payeeId} kind="payee" size={24} />
          <span className="min-w-0">{e.run ? e.run.title : first.payee}</span>
        </span>
        {!e.run && <span className={CELL.amount}>{first.amount}</span>}
        <span className={CELL.why}>{first.why}</span>
        <span className={`${CELL.state} ${first.state === "Refused" ? "text-(--attention)" : ""}`}>
          {first.sealed && <SealMark />}
          {first.state}
        </span>
        {staleAt && <span className={CELL.stale}>{asOf(staleAt)}</span>}
      </button>
    </li>
  );
}

function Paid(p: OwnerViewProps & { entries: Entry[]; className?: string }) {
  return (
    <section aria-labelledby="paid-title" className={`flex min-w-0 flex-col gap-(--space-3) ${p.className ?? ""}`}>
      <h2 id="paid-title" className="row-title m-0">
        {COPY.paidTitle}
      </h2>
      {p.entries.length ? (
        <div className={`${plate} px-(--space-5)`}>
          <div aria-hidden className={`${LINE} meta hidden border-b border-(--rule) py-(--space-3) md:grid`}>
            <span className="md:col-start-1">{COPY.paidColumns[0]}</span>
            <span className="md:col-start-2">{COPY.paidColumns[1]}</span>
            <span className="text-right md:col-start-3">{COPY.paidColumns[2]}</span>
            <span className="text-right md:col-start-4">{COPY.paidColumns[3]}</span>
          </div>
          <ul className="m-0 list-none p-0">
            {p.entries.map((e) => (
              <LedgerRow key={e.run ? e.run.key : e.line.key} e={e} onOpen={p.onOpen} staleAt={p.staleAt} />
            ))}
          </ul>
        </div>
      ) : (
        <p className={`${plate} row m-0 p-(--space-5)`}>{p.inbox.payees?.length ? COPY.paidEmptyApproved : COPY.paidEmptyNoPayee}</p>
      )}
    </section>
  );
}

/** Who is who: the agent's mark and each approved payee's seal, by name (the marks the rows and the sheet wear). */
function Marks(p: OwnerViewProps & { className?: string }) {
  const payees = p.inbox.payees ?? [];
  if (!p.agentId && !payees.length) return null;
  return (
    <section aria-labelledby="marks-title" className={`flex min-w-0 flex-col gap-(--space-3) ${p.className ?? ""}`}>
      <h2 id="marks-title" className="row-title m-0">
        {COPY.marksTitle}
      </h2>
      <ul className={`${plate} m-0 flex list-none flex-col gap-(--space-3) p-(--space-5)`}>
        {p.agentId && (
          <li className="row flex items-center gap-(--space-3)">
            <Mark id={p.agentId} kind="agent" size={24} />
            {COPY.theAgent}
          </li>
        )}
        {payees.map((y) => (
          <li key={y.address} className="row flex items-center gap-(--space-3)">
            <Mark id={y.address.toLowerCase()} kind="payee" size={24} />
            {payeeName(p.inbox, y.address)}
          </li>
        ))}
      </ul>
    </section>
  );
}

function Needs(p: OwnerViewProps & { items: Need[]; className?: string }) {
  return (
    <section aria-labelledby="needs-title" className={`flex min-w-0 flex-col gap-(--space-3) ${p.className ?? ""}`}>
      <h2 id="needs-title" className={`row-title m-0 ${p.items.length ? "text-(--attention)" : ""}`}>
        {COPY.needsTitle}
      </h2>
      {/* a failure whose item is gone after the read that followed it still says what happened */}
      {p.outcome && /^(seller|invoice|bill)-/.test(p.outcome.key) && !p.items.some((n) => n.key === p.outcome?.key) && <Result o={p.outcome} />}
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

function Disclosure(p: OwnerViewProps & { line?: Line; run?: Run }) {
  const run = p.run;
  // A folded run opens one sheet: its payments, newest first, and the newest one's record in the Agent view.
  const l = run ? run.lines[0] : p.line;
  return (
    <Sheet open={!!l} onClose={() => p.onOpen?.(null)} title={run ? run.title : l ? `${l.amount} to ${l.payee}` : ""}>
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
          {run && (
            <ul className="m-0 list-none p-0">
              {run.lines.map((x) => (
                <li key={x.key} className="row flex items-baseline justify-between gap-(--space-4) border-b border-(--rule) py-(--space-2) last:border-b-0">
                  <span className="meta">{x.when}</span>
                  <span className="money">{x.amount}</span>
                </li>
              ))}
            </ul>
          )}
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
  const entries = foldRuns(rows, p.nowSecs);
  const items = needs(p.inbox);
  const run = entries.find((e) => e.run?.key === p.openKey)?.run;
  const demo = p.demo && p.act && !p.guest ? p.demo : null;
  // The five minutes' last step is the first payment to Demo Vendor: the oldest row of the book that names it.
  const first = demo ? rows.filter((r) => r.payeeId === demo.vendor.toLowerCase() && r.state !== "Refused").at(-1) : undefined;
  return (
    <div className="flex flex-col gap-(--space-6)">
      {p.staleAt && p.silent && (
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
      {demo && p.act && (
        <Strip
          inbox={p.inbox}
          demo={demo}
          clockSecs={p.clockSecs ?? p.nowSecs}
          clockMs={p.clockMs}
          act={p.act}
          outcome={p.outcome}
          pending={<PendingLine inbox={p.inbox} clockSecs={p.clockSecs ?? p.nowSecs} clockMs={p.clockMs} act={p.act} outcome={p.outcome} />}
          openedAt={p.openedAt}
          first={
            first && (
              <ul className="m-0 list-none p-0">
                <LedgerRow e={{ line: first }} onOpen={p.onOpen} />
              </ul>
            )
          }
        />
      )}
      {/* From 1024 px two columns (founder): the statement left (at most 680 px), a rail on the right with the four facts, Needs
          you, what the owner may change and the marks; the authority sentence above spans both. Below that, one column in
          reading order. */}
      <div className="grid grid-cols-1 gap-(--space-6) lg:grid-cols-[minmax(0,680px)_minmax(0,1fr)] lg:grid-rows-[auto_auto_auto_1fr] lg:items-start">
        <Facts {...p} className="lg:col-start-2 lg:row-start-1" />
        <Paid {...p} entries={entries} className="lg:col-start-1 lg:row-span-4 lg:row-start-1" />
        <Needs {...p} items={items} className="lg:col-start-2 lg:row-start-2" />
        {p.act && !p.guest && <Mandate inbox={p.inbox} act={p.act} outcome={p.outcome} clockMs={p.clockMs} className="lg:col-start-2 lg:row-start-3" />}
        <Marks {...p} className="lg:col-start-2 lg:row-start-4" />
      </div>
      <Disclosure {...p} run={run} line={run ? undefined : rows.find((r) => r.key === p.openKey)} />
    </div>
  );
}
