// The authority receipt on screen (H4): three sections, each line as receiptOf built it from the mandate, each `<li>`
// carrying the contract rule behind it (`data-rule`). One plate; sections apart by space, never a ruled divider.
import { receipt as words } from "./copy.ts";
import type { Receipt } from "./receipt.ts";
import { PLATE, T } from "./styles.ts";

const SECTIONS = ["can", "never", "worst"] as const;

export function ReceiptView({ receipt, footnote }: { receipt: Receipt; footnote?: React.ReactNode }) {
  return (
    <div className={`${PLATE} flex flex-col gap-(--space-6)`}>
      {SECTIONS.map((key) => (
        <section key={key} aria-labelledby={`receipt-${key}`} className="flex flex-col gap-(--space-2)">
          <h2 id={`receipt-${key}`} className={T.rowTitle}>
            {words.sections[key]}
          </h2>
          <ul className="flex flex-col gap-(--space-2)">
            {receipt[key].map((l) => (
              <li key={l.text} className={`${T.row} ${T.money}`} data-rule={l.rule}>
                {l.text}
              </li>
            ))}
          </ul>
        </section>
      ))}
      {footnote && <p className={T.meta}>{footnote}</p>}
    </div>
  );
}
