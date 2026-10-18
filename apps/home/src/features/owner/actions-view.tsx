"use client";
// The owner's actions on the Owner home (docs/122: every owner action the console had moves here): the judge's five
// minutes on a demo account, a change waiting out the delay with its one Cancel, and the mandate's three parts, one sheet
// each (payees, limits, mode). Pure like owner-view.tsx: the container (owner-home.tsx) signs; this draws, and keeps only
// what is being typed. No address is drawn: payees by name, marks from the address; the one field that holds an address
// is the payee's, typed by the owner (the one marked allowed; test/zero-hash-exceptions.test.mts).
import { useState } from "react";
import { Sheet } from "../../components/sheet.tsx";
import { Mark } from "../../lib/mark.ts";
import type { Demo, Inbox } from "../../lib/types.ts";
import { FIELD } from "../onboarding/styles.ts";
import { Segments } from "../onboarding/ui.tsx";
import type { Outcome, Phase } from "./owner-view.tsx";
import { amount, changeOf, clockText, COPY, cutOf, everyWords, minor, modeIndex, MODES, money, payeeLabel, pendingWords, phaseText, raiseOf, stripOf, timeWords } from "./words.ts";

export const plate = "rounded-(--radius-plate) bg-(--plate)";
export const button = "inline-flex min-h-(--button) items-center justify-center rounded-(--radius-button) px-(--space-5) row-title";
const compactShape = "inline-flex min-h-(--button-compact) shrink-0 items-center justify-center rounded-(--radius-button) bg-(--plate-2) px-(--space-4) row-title";
export const compact = `${compactShape} text-(--ink)`;
const primary = `${button} bg-(--ink) text-(--paper)`;

export type SheetName = "payees" | "limits" | "mode";
/** What the container does for each control; `phases` names each control's phase in flight, by its key. */
export type Actions = {
  phases: Record<string, Phase>;
  sheet: SheetName | null;
  onSheet: (s: SheetName | null) => void;
  onBill: () => void;
  /** `key` names the control in flight (the five minutes' Autopilot is its own) */
  onMode: (mode: 0 | 1 | 2, key?: string) => void;
  onCancel: () => void;
  onApprove: (address: string, name: string) => void;
  onStopPayee: (address: string) => void;
  /** `key` names the control in flight (the five minutes' raise and cut are their own) */
  onLimits: (perAction: string, daily: string, key?: string) => void;
};

export function Result({ o }: { o: NonNullable<Outcome> }) {
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

const label = (ph: Phase | undefined, clockMs: number | undefined, rest: string) => (ph ? phaseText(ph, clockMs ?? ph.since) : rest);

/** A change waiting out the delay (inbox.pending): what applies and when, counting down, and the one Cancel. */
export function PendingLine(p: { inbox: Inbox; clockSecs: number; clockMs?: number; act?: Actions; outcome?: Outcome }) {
  const words = pendingWords(p.inbox);
  if (!words) return null;
  const left = (p.inbox.pending?.eta ?? 0) - p.clockSecs;
  const ph = p.act?.phases.cancel;
  return (
    <div className="flex flex-col gap-(--space-3)">
      <p className="row m-0">
        {words} {left > 0 ? COPY.pendingIn(clockText(left)) : COPY.applying}
      </p>
      {p.act && (left > 0 || ph) && (
        <button type="button" onClick={p.act.onCancel} className={`${compact} self-start`}>
          {label(ph, p.clockMs, COPY.cancel)}
        </button>
      )}
      {p.outcome?.key === "cancel" && <Result o={p.outcome} />}
    </div>
  );
}

/** The judge's five minutes (docs/115 §3) on a demo account the owner holds: each step one tap or a wait it counts down;
 *  the first payment is the ledger's own row (`first`), with its why. */
/** The outcomes the five minutes show themselves (a failure; the cut's success too, owner-home.tsx). */
const STRIP_KEYS = ["demo-bill", "autopilot", "raise", "cut"];

/** USDC as typed back into a field: the shortest exact form, no grouping. */
const plainAmount = (u: bigint) => amount(u).replace(/,/g, "");

export function Strip(p: { inbox: Inbox; demo: Demo; clockSecs: number; clockMs?: number; act: Actions; outcome?: Outcome; pending: React.ReactNode; first?: React.ReactNode; firstByYou?: boolean }) {
  const s = stripOf(p.inbox, p.demo);
  const cutDone = p.outcome?.key === "cut" && !p.outcome.failed;
  const up = raiseOf(p.inbox.caps);
  const down = cutOf(p.inbox.caps, p.demo.bill.usdc);
  const v = p.demo.vendor.toLowerCase();
  const payee = p.inbox.payees?.find((y) => y.address.toLowerCase() === v);
  const asked = p.inbox.needsYou?.some((n) => n.counterparty.toLowerCase() === v);
  const left = s.bill ? s.bill.nextDue - p.clockSecs : 0;
  const waiting = !!p.inbox.pending?.eta && ((p.inbox.pending.kinds ?? 0) & 2) !== 0 && p.inbox.pending.mode === "Autopilot";
  const outcome = p.outcome && STRIP_KEYS.includes(p.outcome.key) ? p.outcome : null;
  return (
    <section aria-labelledby="strip-title" className={`${plate} flex flex-col gap-(--space-4) p-(--space-5)`}>
      <h2 id="strip-title" className="row-title m-0">
        {COPY.strip.title}
      </h2>
      <Segments labels={COPY.strip.steps} done={cutDone ? 5 : s.step} current={cutDone ? 5 : s.step} transit={!cutDone} label={COPY.strip.label} show />
      {s.step === 0 && <p className="row m-0">{COPY.strip.funding}</p>}
      {s.step === 1 && (
        <button type="button" onClick={p.act.onBill} className={`${primary} w-full text-left md:w-auto md:self-start`}>
          {label(p.act.phases["demo-bill"], p.clockMs, COPY.strip.bill(money(p.demo.bill.usdc), everyWords(p.demo.bill.everySecs)))}
        </button>
      )}
      {(s.step === 2 || s.step === 3) && <p className="row m-0">{COPY.strip.billSaved(money(s.bill?.amount ?? p.demo.bill.usdc), everyWords(s.bill?.everySecs || p.demo.bill.everySecs))}</p>}
      {s.step === 2 &&
        (waiting ? (
          p.pending
        ) : (
          <button type="button" onClick={() => p.act.onMode(2, "autopilot")} className={`${primary} w-full md:w-auto md:self-start`}>
            {label(p.act.phases.autopilot, p.clockMs, COPY.strip.autopilot)}
          </button>
        ))}
      {s.step === 3 && (
        <p className="row m-0">{asked ? COPY.strip.asked : left > 0 ? COPY.strip.due(clockText(left)) : COPY.strip.dueNow}</p>
      )}
      {s.step === 3 && payee && payee.payableFrom > p.clockSecs && <p className="row m-0">{COPY.strip.cooling(timeWords(payee.payableFrom))}</p>}
      {s.step === 4 && <p className="row m-0">{p.firstByYou ? COPY.strip.doneByYou : COPY.strip.done}</p>}
      {s.step === 4 && p.first}
      {s.step === 4 && <p className="row m-0">{COPY.strip.raiseCut(`${p.demo.delaySecs} s`)}</p>}
      {s.step === 4 && (
        <div className="flex flex-col gap-(--space-3) md:flex-row">
          <button type="button" onClick={() => p.act.onLimits(plainAmount(up.perAction), plainAmount(up.daily), "raise")} className={compact}>
            {label(p.act.phases.raise, p.clockMs, COPY.strip.raise(amount(up.perAction)))}
          </button>
          {down ? (
            <button type="button" onClick={() => p.act.onLimits(plainAmount(down.perAction), plainAmount(down.daily), "cut")} className={compact}>
              {label(p.act.phases.cut, p.clockMs, down.one ? COPY.strip.cutOne(amount(down.perAction)) : COPY.strip.cutDay(amount(down.daily)))}
            </button>
          ) : (
            <p className="row m-0">{COPY.strip.lowest}</p>
          )}
        </div>
      )}
      {outcome && <Result o={outcome} />}
    </section>
  );
}

/** The mandate's three parts, each behind one sheet (DESIGN §Page grammar: detail behind one disclosure). */
export function Mandate(p: { inbox: Inbox; act: Actions; outcome?: Outcome; clockMs?: number; className?: string }) {
  const close = () => p.act.onSheet(null);
  const parts = ["payees", "limits", "mode"] as const;
  return (
    <>
      <section aria-labelledby="mandate-title" className={`flex min-w-0 flex-col gap-(--space-3) ${p.className ?? ""}`}>
        <h2 id="mandate-title" className="row-title m-0">
          {COPY.mandate.title}
        </h2>
        <div className={`${plate} flex flex-wrap gap-(--space-3) p-(--space-5)`}>
          {parts.map((s) => (
            <button key={s} type="button" onClick={() => p.act.onSheet(s)} className={compact}>
              {COPY.mandate.open[s]}
            </button>
          ))}
        </div>
      </section>
      <Sheet open={p.act.sheet === "payees"} onClose={close} title={COPY.mandate.sheet.payees}>
        <Payees {...p} />
      </Sheet>
      <Sheet open={p.act.sheet === "limits"} onClose={close} title={COPY.mandate.sheet.limits}>
        {/* keyed by the caps and the sheet's state: it opens on what the contract holds now, never on an old draft */}
        <Limits key={`${p.inbox.caps.perAction}|${p.inbox.caps.daily}|${p.act.sheet}`} {...p} />
      </Sheet>
      <Sheet open={p.act.sheet === "mode"} onClose={close} title={COPY.mandate.sheet.mode}>
        <Modes {...p} />
      </Sheet>
    </>
  );
}

function Payees(p: { inbox: Inbox; act: Actions; outcome?: Outcome; clockMs?: number }) {
  const [address, setAddress] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const list = p.inbox.payees ?? [];
  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const a = address.trim();
    if (!/^0x[0-9a-fA-F]{40}$/.test(a)) return setError(COPY.payees.badAddress);
    if (!name.trim()) return setError(COPY.payees.needName);
    if (list.some((y) => y.address.toLowerCase() === a.toLowerCase())) return setError(COPY.payees.already(payeeLabel(p.inbox, a)));
    setError(null);
    p.act.onApprove(a, name.trim());
  };
  return (
    <div className="flex flex-col gap-(--space-5)">
      {list.length ? (
        <ul className="m-0 flex list-none flex-col gap-(--space-4) p-0">
          {list.map((y) => {
            const who = payeeLabel(p.inbox, y.address);
            const k = `stop-${y.address.toLowerCase()}`;
            return (
              <li key={y.address} className="flex flex-col gap-(--space-2)">
                <p className="row-title m-0 flex items-center gap-(--space-3)">
                  <Mark id={y.address.toLowerCase()} kind="payee" size={24} />
                  <span className="min-w-0">{who}</span>
                </p>
                {!y.payableNow && <p className="meta m-0">{COPY.payees.from(timeWords(y.payableFrom))}</p>}
                <button type="button" onClick={() => p.act.onStopPayee(y.address)} className={`${compactShape} self-start text-(--attention)`}>
                  {label(p.act.phases[k], p.clockMs, COPY.payees.stop(who))}
                </button>
                {p.outcome?.key === k && <Result o={p.outcome} />}
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="row m-0">{COPY.payees.none}</p>
      )}
      <form onSubmit={submit} noValidate className="flex flex-col gap-(--space-2)">
        <label htmlFor="payee-address" className="meta">
          {COPY.payees.address}
        </label>
        <input
          id="payee-address"
          data-hash-allowed="payee-address"
          className={FIELD}
          value={address}
          onChange={(e) => setAddress(e.target.value)}
          autoComplete="off"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
        />
        <label htmlFor="payee-name" className="meta pt-(--space-2)">
          {COPY.payees.name}
        </label>
        <input id="payee-name" className={FIELD} value={name} maxLength={40} onChange={(e) => setName(e.target.value)} autoComplete="off" />
        {error && (
          <p role="alert" className="row m-0 text-(--attention)">
            {error}
          </p>
        )}
        <button type="submit" className={`${primary} mt-(--space-2)`}>
          {label(p.act.phases.approve, p.clockMs, COPY.payees.approve)}
        </button>
        {p.outcome?.key === "approve" && <Result o={p.outcome} />}
      </form>
    </div>
  );
}

/** USDC as typed back into a field: the shortest exact form, no grouping. */
const typed = (s: string) => amount(minor(s) ?? 0n).replace(/,/g, "");

function Limits(p: { inbox: Inbox; act: Actions; outcome?: Outcome; clockMs?: number }) {
  const [per, setPer] = useState(() => typed(p.inbox.caps.perAction));
  const [day, setDay] = useState(() => typed(p.inbox.caps.daily));
  const c = changeOf(p.inbox.caps, per, day);
  const go = "error" in c || c.kind === "same" ? null : c.kind === "raise" ? COPY.limits.raise : c.kind === "cut" ? COPY.limits.cut : COPY.limits.change;
  return (
    <form
      noValidate
      className="flex flex-col gap-(--space-2)"
      onSubmit={(e) => {
        e.preventDefault();
        if (go) p.act.onLimits(per, day);
      }}
    >
      <label htmlFor="limit-per" className="meta">
        {COPY.limits.perAction}
      </label>
      <input id="limit-per" inputMode="decimal" className={`${FIELD} money`} value={per} onChange={(e) => setPer(e.target.value)} autoComplete="off" />
      <label htmlFor="limit-day" className="meta pt-(--space-2)">
        {COPY.limits.daily}
      </label>
      <input id="limit-day" inputMode="decimal" className={`${FIELD} money`} value={day} onChange={(e) => setDay(e.target.value)} autoComplete="off" />
      {"error" in c && <p className="row m-0 text-(--attention)">{c.error}</p>}
      {go && (
        <button type="submit" className={`${primary} mt-(--space-2)`}>
          {label(p.act.phases.limits, p.clockMs, go)}
        </button>
      )}
      {p.outcome?.key === "limits" && <Result o={p.outcome} />}
    </form>
  );
}

function Modes(p: { inbox: Inbox; act: Actions; outcome?: Outcome; clockMs?: number }) {
  const now = modeIndex(p.inbox.mode);
  return (
    <ul className="m-0 flex list-none flex-col gap-(--space-3) p-0">
      {MODES.map((m, i) => {
        const k = `mode-${i}`;
        return (
          <li key={m} className="flex flex-col gap-(--space-2)">
            <button
              type="button"
              aria-pressed={i === now}
              aria-describedby={`mode-what-${i}`}
              onClick={() => i !== now && p.act.onMode(i as 0 | 1 | 2)}
              className={`flex min-h-(--button) w-full flex-col items-start justify-center gap-(--space-1) rounded-(--radius-button) px-(--space-5) py-(--space-3) text-left ${i === now ? "bg-(--ink) text-(--paper)" : "bg-(--plate-2) text-(--ink)"}`}
            >
              <span className="row-title">{label(p.act.phases[k], p.clockMs, m)}</span>
              <span id={`mode-what-${i}`} aria-hidden className="row">
                {COPY.modes.what[i]}
              </span>
            </button>
            {p.outcome?.key === k && <Result o={p.outcome} />}
          </li>
        );
      })}
    </ul>
  );
}
