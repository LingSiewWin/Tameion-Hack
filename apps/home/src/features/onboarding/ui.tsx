"use client";
// The few shapes onboarding repeats, from tokens only (DESIGN §Shape): the 56 px CTA plate (ink border and text), a
// quiet second plate, the input, and the progress line (filled segments are taller, so the state is shape, not colour).
import { useEffect, useState } from "react";

export const CTA =
  "row-title flex h-(--cta) w-full items-center justify-center rounded-(--radius-plate) border border-(--ink) bg-(--plate) px-(--space-4) text-(--ink) text-center";
export const CTA_QUIET =
  "row-title flex h-(--cta) w-full items-center justify-center rounded-(--radius-plate) border border-(--rule) bg-(--plate) px-(--space-4) text-(--ink) text-center";
export const FIELD = "row h-12 w-full rounded-(--radius-plate) border border-(--rule) bg-(--plate) px-(--space-3) text-(--ink)";
export const PLATE = "rounded-(--radius-plate) border border-(--rule) bg-(--plate) p-(--space-4)";

/** A line of `labels.length` segments; the ones up to `at` are filled. `label` says it in words for screen readers. */
export function Segments({ labels, at, label, show = false }: { labels: readonly string[]; at: number; label: string; show?: boolean }) {
  return (
    <ol aria-label={label} className="grid gap-(--space-2)" style={{ gridTemplateColumns: `repeat(${labels.length}, minmax(0, 1fr))` }}>
      {labels.map((l, i) => (
        <li key={l} aria-current={i === at ? "step" : undefined} className="flex flex-col gap-(--space-1)">
          <span aria-hidden className={i <= at ? "block h-1 rounded-(--radius-capsule) bg-(--ink)" : "mt-[1.5px] block h-px bg-(--rule)"} />
          <span className={show ? (i === at ? "meta text-(--ink)" : "meta") : "sr-only"}>
            {l}
            {i < at && <span className="sr-only">, done</span>}
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
