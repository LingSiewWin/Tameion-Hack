"use client";
// The few shapes onboarding repeats, in one place so the token pass touches one file. Depth is one step of fill, never a
// border or a shadow (DESIGN §Shape): a plate on paper, inputs and inner groups on plate-2, the one primary action in ink.
import { useEffect, useState } from "react";
import * as copy from "./copy.ts";

/** Type roles (DESIGN §Typography), as the classes tokens.css defines. */
export const T = {
  title: "sheet-title",
  rowTitle: "row-title",
  row: "row",
  meta: "meta",
  money: "money",
  mono: "mono-dense",
  attention: "row text-(--attention)",
};

// Press: 100 ms in to 0.98 (DESIGN §Motion), on the wrapper; reduced motion keeps it still.
const BUTTON = "flex h-(--button) w-full items-center justify-center rounded-(--radius-button) px-(--space-4) text-center transition-transform duration-(--press-out) active:scale-(--press-scale) active:duration-(--press-in) motion-reduce:transition-none motion-reduce:active:scale-100";
/** The one primary action: ink. */
export const CTA = `${T.rowTitle} ${BUTTON} bg-(--ink) text-(--paper)`;
/** A second action: one step of fill. */
export const CTA_QUIET = `${T.rowTitle} ${BUTTON} bg-(--plate-2) text-(--ink)`;
export const FIELD = `${T.row} h-(--button) w-full rounded-(--radius-inner) bg-(--plate-2) px-(--space-3) text-(--ink)`;
export const PLATE = "rounded-(--radius-plate) bg-(--plate) p-(--space-5)";

/** A line of `labels.length` segments: the first `done` in ink; `current` is the one the screen is on, drawn as a transit
 *  mark while it is in flight (`transit`), else a step of fill. Labels show under the line with `show`, else for readers only. */
export function Segments({ labels, done, current, transit = false, label, show = false }: { labels: readonly string[]; done: number; current: number; transit?: boolean; label: string; show?: boolean }) {
  return (
    <ol aria-label={label} className="grid gap-(--space-2)" style={{ gridTemplateColumns: `repeat(${labels.length}, minmax(0, 1fr))` }}>
      {labels.map((l, i) => (
        <li key={l} aria-current={i === current ? "step" : undefined} className="flex flex-col gap-(--space-1)">
          <span aria-hidden className={`block h-1 rounded-(--radius-mark) ${i < done ? "bg-(--ink)" : i === current && transit ? "bg-(--transit)" : "bg-(--plate-2)"}`} />
          <span className={show ? (i < done || i === current ? `${T.meta} text-(--ink)` : T.meta) : "sr-only"}>
            {l}
            {i < done && <span className="sr-only">, done</span>}
          </span>
        </li>
      ))}
    </ol>
  );
}

/** Seconds since the epoch, ticking once a second while `on`. */
export function useNow(on = true) {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    if (!on) return;
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(t);
  }, [on]);
  return now;
}

/** A wait in words (DESIGN §States): the seconds after 3 s, "Still checking. It will not be sent twice." after 15 s. */
export function Waited({ since, now }: { since: number; now: number }) {
  const s = Math.max(0, now - since);
  if (!since || s < 3) return null;
  return (
    <p className={`${T.meta} ${T.money}`} aria-live="polite">
      {copy.wait.seconds(s)}
      {s >= 15 && <> {copy.wait.still}</>}
    </p>
  );
}

/** A failure: what happened, whether money moved, one action; the service's own words behind a disclosure. */
export function Failure({ title, words, action }: { title: string; words: string; action?: React.ReactNode }) {
  return (
    <div role="alert" className={`${PLATE} flex flex-col gap-(--space-3)`}>
      <p className={T.rowTitle}>{title}</p>
      {action}
      <details className={T.meta}>
        <summary className="cursor-pointer">{copy.wait.details}</summary>
        <p className={`${T.row} mt-(--space-2)`}>{words}</p>
      </details>
    </div>
  );
}
