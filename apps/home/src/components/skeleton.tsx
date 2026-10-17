// Loading (DESIGN §States): blocks shaped like the layout, pulsing 1.2 s; never a spinner page, never a fake 0.
// Size it with layout classes (h-*, w-*); it carries no text, so the region that holds it says it is loading.
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
