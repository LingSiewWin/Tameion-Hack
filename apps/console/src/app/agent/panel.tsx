import { useId } from "react";
import { cn } from "@/lib/utils";
import { signed, signedAt } from "./feed";

/** One panel of the ops grid, laid out like a terminal's: a title bar (marker, UPPERCASE title, a `|` sub-label,
 *  what the panel is "as of" at the far end), then a table or a few rows that touch its edges. Denser than the
 *  `Section` card used on the human pages (13 px figures, 12 px heads, nothing under 12 px) because a person
 *  watching an agent reads many numbers at once; the colours are the same tokens. */
export function Panel({
  title,
  sub,
  right,
  className,
  children,
}: {
  title: string;
  sub?: React.ReactNode;
  right?: React.ReactNode;
  className?: string;
  children: React.ReactNode;
}) {
  const id = useId();
  return (
    <section aria-labelledby={id} className={cn("flex min-w-0 flex-col overflow-hidden rounded-lg border bg-card text-card-foreground", className)}>
      <header className="flex min-h-9 flex-wrap items-center gap-x-2 gap-y-0.5 border-b bg-muted/50 px-3 py-1.5">
        <span aria-hidden className="size-2 shrink-0 rounded-[2px] bg-protect" />
        <h2 id={id} className="text-xs font-semibold uppercase tracking-wider">
          {title}
        </h2>
        {sub && <span className="text-xs text-muted-foreground before:mr-2 before:content-['|']">{sub}</span>}
        {right && <div className="ml-auto flex items-center gap-2 text-xs text-muted-foreground">{right}</div>}
      </header>
      <div className="min-h-0 flex-1">{children}</div>
    </section>
  );
}

/** A column head: quiet, small, aligned with its figures. */
export function Th({ right, className, ...props }: React.ComponentProps<"th"> & { right?: boolean }) {
  return <th scope="col" className={cn("whitespace-nowrap px-2 py-1.5 text-xs font-medium text-muted-foreground sm:px-3", right ? "text-right" : "text-left", className)} {...props} />;
}

/** A cell; `right` is a figure: right-aligned, tabular, so the points line up down the column. */
export function Td({ right, className, ...props }: React.ComponentProps<"td"> & { right?: boolean }) {
  return <td className={cn("px-2 py-1.5 align-baseline sm:px-3", right && "figures text-right", className)} {...props} />;
}

/** A change, signed in the text (a plus, a real minus) and coloured as well, so colour is never the only thing that says it. */
export function Delta({ value, dp, className }: { value: string | null; dp?: number; className?: string }) {
  const { text, sign } = dp == null ? signed(value) : signedAt(value, dp);
  return <span className={cn("figures", sign > 0 && "text-protect", sign < 0 && "text-destructive", sign === 0 && "text-muted-foreground", className)}>{text}</span>;
}

/** A label above its value, as the venue's own header lays out a market's facts. */
export function Fact({ label, sub, children }: { label: string; sub?: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="figures mt-0.5 truncate text-caption font-medium">{children}</dd>
      {sub && <dd className="figures text-xs text-muted-foreground">{sub}</dd>}
    </div>
  );
}

/** A panel with nothing in it, said plainly. */
export function Empty({ children }: { children: React.ReactNode }) {
  return <p className="px-3 py-5 text-caption text-muted-foreground">{children}</p>;
}
