"use client";
// One disclosure per row (DESIGN §Page grammar): a sheet from the bottom on mobile, opened at its content's size;
// a right drawer at >= 1024 px. A native modal <dialog>: the page behind is inert (focus stays inside), Esc closes.
import { useEffect, useId, useRef } from "react";

export function Sheet({ open, onClose, title, children }: { open: boolean; onClose: () => void; title: string; children: React.ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    if (!open && d.open) d.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      className="sheet"
      aria-labelledby={titleId}
      onClose={onClose}
      onClick={(e) => {
        // a tap on the backdrop (the dialog element itself, outside its content) closes it
        if (e.target === e.currentTarget) e.currentTarget.close();
      }}
    >
      <div className="flex flex-col gap-(--space-4) p-(--space-5)">
        <div className="flex items-start gap-(--space-3)">
          <h2 id={titleId} className="row-title min-w-0 flex-1">
            {title}
          </h2>
          <button type="button" className="sheet-close row" onClick={() => ref.current?.close()}>
            Close
          </button>
        </div>
        {children}
      </div>
    </dialog>
  );
}
