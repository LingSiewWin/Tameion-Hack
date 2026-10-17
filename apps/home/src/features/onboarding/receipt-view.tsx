// The authority receipt on screen (H4): three sections, each line as receiptOf built it from the mandate. One plate,
// hairlines between the sections, like a paper receipt. No colour carries meaning here; the headings do.
import type { Receipt } from "./receipt.ts";
import { PLATE } from "./ui.tsx";

const SECTIONS = [
  ["can", "Tameion can"],
  ["never", "Tameion can never"],
  ["worst", "Worst case"],
] as const;

export function ReceiptView({ receipt, footnote }: { receipt: Receipt; footnote?: React.ReactNode }) {
  return (
    <div className={`${PLATE} flex flex-col`}>
      {SECTIONS.map(([key, title], i) => (
        <section key={key} aria-labelledby={`receipt-${key}`} className={i ? "mt-(--space-4) border-t border-(--rule) pt-(--space-4)" : ""}>
          <h2 id={`receipt-${key}`} className="row-title">
            {title}
          </h2>
          <ul className="mt-(--space-2) flex flex-col gap-(--space-2)">
            {receipt[key].map((l) => (
              <li key={l.text} className="row money" data-rule={l.rule}>
                {l.text}
              </li>
            ))}
          </ul>
        </section>
      ))}
      {footnote && <p className="meta mt-(--space-4) border-t border-(--rule) pt-(--space-4)">{footnote}</p>}
    </div>
  );
}
