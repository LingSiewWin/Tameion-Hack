"use client";
// The Owner home, drawn from what the service said (DESIGN §Page grammar): the authority sentence and Stop, four facts,
// what the agent paid and why, then Needs you. Pure: the container (owner-home.tsx) reads and acts; this only draws,
// so the zero-hash test can render it from a captured inbox (DESIGN Rule 1).
import { Sheet } from "../../components/sheet.tsx";
import type { Inbox } from "../../lib/types.ts";
import { asOf, authority, facts, halvesWords, lines, needs, pendingWords, type Halves, type Line, type Need } from "./words.ts";

/** A control in flight names its phase and never looks disabled (DESIGN §Money lifecycle). */
export type Phase = { label: string } | null;
export type Outcome = { key: string; text: string; failed: boolean } | null;

export type OwnerViewProps = {
  inbox: Inbox;
  /** status.runway.byRateDays */
  byRateDays: number | null | undefined;
  /** which halves of each payment's record are present (H3's projection); empty when the service has none */
  halves: ReadonlyMap<string, Halves>;
  nowSecs: number;
  /** ms of the read these figures come from, when the latest read failed (offline / stale) */
  staleAt?: number | null;
  /** the public guest view: reads only, no Stop, no actions */
  guest?: boolean;
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

const plate = "rounded-(--radius-plate) border border-(--rule) bg-(--plate)";
const cta = "flex min-h-(--cta) w-full items-center justify-center rounded-(--radius-plate) px-(--space-4) row-title";

/** The symbolon mark (DESIGN §Direction): two halves that fit, the witness as the dot between. Drawn only when sealed. */
function SealMark() {
  return (
    <svg width="16" height="12" viewBox="0 0 16 12" aria-hidden className="shrink-0 text-(--ink)">
      <path d="M7 1a5 5 0 0 0 0 10z" fill="currentColor" />
      <path d="M9 1a5 5 0 0 1 0 10z" fill="currentColor" />
      <circle cx="8" cy="6" r="2" fill="currentColor" />
    </svg>
  );
}

function Hero(p: OwnerViewProps) {
  const stopped = p.inbox.mode === "Watch";
  const verb = stopped ? "Start the agent" : "Stop the agent";
  // A start already asked waits out its delay (the line above says when): no second ask. Stop is always there.
  const control = !p.guest && !(stopped && (p.inbox.pending?.kinds ?? 0) & 2);
  return (
    <section aria-labelledby="authority" className="flex flex-col gap-(--space-5)">
      <h1 id="authority" className="statement-line">
        {authority(p.inbox)}
      </h1>
      {pendingWords(p.inbox) && <p className="row m-0">{pendingWords(p.inbox)}</p>}
      {control &&
        (p.asking ? (
          <div className={`${plate} flex flex-col gap-(--space-3) p-(--space-4)`} role="group" aria-labelledby="stop-q">
            <p id="stop-q" className="row-title">
              {stopped
                ? "Start the agent again? It asks before each payment, once the wait has passed."
                : "Stop the agent now? Payments wait for your tap until you start it again."}
            </p>
            <div className="flex flex-col gap-(--space-2) sm:flex-row">
              <button type="button" onClick={p.onStop} className={`${cta} ${stopped ? "bg-(--ink) text-(--paper)" : "bg-(--red) text-(--plate)"}`}>
                {p.stopPhase?.label ?? verb}
              </button>
              {!p.stopPhase && (
                <button type="button" onClick={p.onStopCancel} className={`${cta} border border-(--rule) bg-(--plate) text-(--ink)`}>
                  {stopped ? "Keep it stopped" : "Keep it running"}
                </button>
              )}
            </div>
          </div>
        ) : (
          <button
            type="button"
            onClick={p.onStopAsk}
            className={`${cta} ${stopped ? "border border-(--ink) bg-(--plate) text-(--ink)" : "border border-(--red) bg-(--plate) text-(--red)"}`}
          >
            {verb}
          </button>
        ))}
      {p.outcome?.key === "stop" && <Result o={p.outcome} />}
    </section>
  );
}

function Result({ o }: { o: NonNullable<Outcome> }) {
  return (
    <p role={o.failed ? "alert" : "status"} className={`row ${o.failed ? "text-(--red)" : ""}`}>
      {o.text}
    </p>
  );
}

function Facts(p: OwnerViewProps) {
  return (
    <dl className="grid grid-cols-2 gap-(--space-3)" aria-label="The account at rest">
      {facts(p.inbox, p.byRateDays, p.nowSecs).map((f) => (
        <div key={f.label} className={`${plate} flex min-w-0 flex-col gap-(--space-2) p-(--space-4)`}>
          <dt className="meta">{f.label}</dt>
          <dd className={`m-0 flex min-w-0 flex-wrap items-baseline gap-x-(--space-2) [overflow-wrap:anywhere] ${f.attention ? "text-(--red)" : ""}`}>
            {f.figure !== undefined ? <span className="balance">{f.figure}</span> : <span className="row-title">{f.words}</span>}
            {f.unit && <span className="row">{f.unit}</span>}
          </dd>
          {p.staleAt && <dd className="meta m-0">{asOf(p.staleAt)}</dd>}
        </div>
      ))}
    </dl>
  );
}

function Paid(p: OwnerViewProps & { rows: Line[] }) {
  return (
    <section aria-labelledby="paid-title" className="flex flex-col gap-(--space-3)">
      <h2 id="paid-title" className="row-title">
        What the agent paid, and why
      </h2>
      {p.rows.length ? (
        <ul className={`${plate} m-0 list-none p-0`}>
          {p.rows.map((r) => (
            <li key={r.key} className="border-b border-(--rule) last:border-b-0">
              <button
                type="button"
                onClick={() => p.onOpen?.(r.key)}
                className="grid min-h-(--touch) w-full grid-cols-[minmax(0,1fr)_auto] gap-x-(--space-3) gap-y-(--space-1) bg-transparent px-(--space-4) py-(--space-3) text-left text-(--ink)"
              >
                <span className="row-title min-w-0">{r.payee}</span>
                <span className="row-title money text-right">{r.amount}</span>
                <span className="row col-span-2">{r.why}</span>
                <span className="meta">{r.when}</span>
                <span className={`meta flex items-center justify-end gap-(--space-1) ${r.state === "Refused" ? "text-(--red)" : ""}`}>
                  {r.sealed && <SealMark />}
                  {r.state}
                </span>
                {p.staleAt && <span className="meta col-span-2">{asOf(p.staleAt)}</span>}
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className={`${plate} row p-(--space-4)`}>
          {p.inbox.payees?.length
            ? "Nothing paid yet. The agent pays your approved payees when their bills fall due."
            : "Nothing paid yet. Approve a payee under Needs you and the agent can pay it."}
        </p>
      )}
    </section>
  );
}

function Needs(p: OwnerViewProps & { items: Need[] }) {
  return (
    <section aria-labelledby="needs-title" className="flex flex-col gap-(--space-3)">
      <h2 id="needs-title" className={`row-title ${p.items.length ? "text-(--red)" : ""}`}>
        Needs you
      </h2>
      {p.items.length ? (
        <ul className={`${plate} m-0 list-none p-0`}>
          {p.items.map((n) => (
            <li key={n.key} className="flex flex-col gap-(--space-3) border-b border-(--rule) px-(--space-4) py-(--space-3) last:border-b-0 sm:flex-row sm:items-center">
              <p className="row-title m-0 min-w-0 flex-1">{n.sentence}</p>
              {!p.guest && (
                <button
                  type="button"
                  onClick={() => p.onNeed?.(n)}
                  className="min-h-(--touch) shrink-0 rounded-(--radius-plate) border border-(--ink) bg-(--plate) px-(--space-4) row-title text-(--ink)"
                >
                  {p.needPhase?.[n.key]?.label ?? n.action}
                </button>
              )}
              {p.outcome?.key === n.key && <Result o={p.outcome} />}
            </li>
          ))}
        </ul>
      ) : (
        <p className={`${plate} row p-(--space-4)`}>Nothing needs you.</p>
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
          <p className="row m-0">
            {l.state === "Refused" ? `Refused ${l.when.toLowerCase()}. Nothing was sent.` : `${l.state} by ${l.by}. ${l.when}.`}
          </p>
          <p className="row m-0">{l.why}</p>
          {l.state !== "Refused" && <p className="row m-0">{halvesWords(l.halves, l.payee)}</p>}
          {l.state !== "Refused" && (
            <button type="button" onClick={() => p.onAgentView?.(l)} className="row self-start bg-transparent p-0 text-(--accent) underline underline-offset-4">
              Open in Agent view
            </button>
          )}
        </div>
      )}
    </Sheet>
  );
}

export function OwnerView(p: OwnerViewProps) {
  const rows = lines(p.inbox, p.nowSecs, p.halves);
  const items = needs(p.inbox);
  return (
    <div className="flex flex-col gap-(--space-6)">
      {p.staleAt && (
        <p role="status" className={`${plate} row flex flex-wrap items-center gap-x-(--space-3) gap-y-(--space-2) p-(--space-4)`}>
          <span className="min-w-0 flex-1">The service did not answer. The figures below are {asOf(p.staleAt)}.</span>
          {p.onRetry && (
            <button type="button" onClick={p.onRetry} className="min-h-(--touch) rounded-(--radius-plate) border border-(--ink) bg-(--plate) px-(--space-4) row-title text-(--ink)">
              Try again
            </button>
          )}
        </p>
      )}
      <Hero {...p} />
      <Facts {...p} />
      <Paid {...p} rows={rows} />
      <Needs {...p} items={items} />
      <Disclosure {...p} line={rows.find((r) => r.key === p.openKey)} />
    </div>
  );
}
