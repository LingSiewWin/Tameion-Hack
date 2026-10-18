"use client";
// Loading (DESIGN §States): blocks shaped like the layout, pulsing 1.2 s; never a spinner page, never a fake 0.
// Size it with layout classes (h-*, w-*); it carries no text, so the region that holds it says it is loading.
import { useEffect, useState } from "react";
import { SKELETON_MIN_MS } from "../tokens.ts";

export function Skeleton({ className = "" }: { className?: string }) {
  return <div aria-hidden className={`skeleton ${className}`} />;
}

/** The region a skeleton stands in for: announced once as loading, then replaced by the real content. */
export function Loading({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div role="status" aria-busy="true" aria-label={label}>
      {children}
    </div>
  );
}

/** True while `loading`, and once a skeleton has shown, for at least `ms` (300) so a fast answer never flickers. */
export function useSkeleton(loading: boolean, ms = SKELETON_MIN_MS): boolean {
  const [held, setHeld] = useState(false);
  const [wasLoading, setWasLoading] = useState(false);
  if (loading !== wasLoading) {
    // React's "adjust state while rendering": a new load starts a new hold, in the same render
    setWasLoading(loading);
    if (loading) setHeld(true);
  }
  useEffect(() => {
    if (!held) return;
    const t = setTimeout(() => setHeld(false), ms);
    return () => clearTimeout(t);
  }, [held, ms]);
  return loading || held;
}
