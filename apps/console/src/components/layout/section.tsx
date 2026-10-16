import { Copy } from "lucide-react";
import { useId } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "@/lib/utils";

type Icon = React.ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
type Tone = "default" | "attention" | "protect";

const iconTone: Record<Tone, string> = {
  default: "text-muted-foreground",
  attention: "text-attention",
  protect: "text-protect",
};

/** The one card anatomy (docs/71): a title, at most one line under it, an
 *  action at the end of the title row. A divider sits under the header only
 *  when the body is a list of rows (`flush`), so every card reads the same. */
export function Section({
  title,
  icon: Icon,
  description,
  action,
  tone = "default",
  flush = false,
  className,
  children,
}: {
  title: React.ReactNode;
  icon?: Icon;
  description?: React.ReactNode;
  action?: React.ReactNode;
  tone?: Tone;
  flush?: boolean;
  className?: string;
  children?: React.ReactNode;
}) {
  const id = useId();
  return (
    <section
      aria-labelledby={id}
      className={cn(
        "overflow-hidden rounded-xl border bg-card text-card-foreground shadow-card",
        tone === "attention" && "border-attention/45",
        tone === "protect" && "border-protect/35",
        className,
      )}
    >
      <header className={cn("flex flex-wrap items-start gap-x-3 gap-y-2 px-4 pt-4 sm:px-5", flush ? "border-b pb-3.5" : "pb-3")}>
        {/* The title keeps at least 8rem; an action that does not fit beside it wraps under it. */}
        <div className="min-w-0 flex-1 basis-32">
          <h2 id={id} className="flex items-center gap-2 text-title font-semibold">
            {Icon && <Icon aria-hidden className={cn("size-4 shrink-0", iconTone[tone])} />}
            {title}
          </h2>
          {description && <p className="mt-0.5 text-caption text-muted-foreground">{description}</p>}
        </div>
        {action && <div className="-my-1 ml-auto flex shrink-0 flex-wrap items-center justify-end gap-1.5">{action}</div>}
      </header>
      {flush ? children : <div className="px-4 pb-5 sm:px-5">{children}</div>}
    </section>
  );
}

export function Rows({ className, ...props }: React.ComponentProps<"ul">) {
  return <ul className={cn("divide-y", className)} {...props} />;
}

/** A row the owner acts on: what it is and its buttons share the first line;
 *  the reason runs the full width under both, so it never wraps into a narrow
 *  column beside the buttons. On a phone the buttons come last, under the
 *  reason, where the thumb is. */
export function Row({
  title,
  detail,
  actions,
  emphasis,
  className,
}: {
  title: React.ReactNode;
  detail?: React.ReactNode;
  actions?: React.ReactNode;
  emphasis?: boolean;
  className?: string;
}) {
  return (
    <li
      className={cn(
        "grid gap-x-4 gap-y-1 px-4 py-4 sm:grid-cols-[minmax(0,1fr)_auto] sm:px-5",
        emphasis && "bg-protect-soft/50",
        className,
      )}
    >
      {/* As tall as a button and centred on it, so the reason starts right under the title. */}
      <div className="flex min-w-0 items-center text-title sm:min-h-9">
        <div className="min-w-0">{title}</div>
      </div>
      {detail && <div className="min-w-0 text-sm leading-relaxed text-muted-foreground sm:col-span-2">{detail}</div>}
      {actions && (
        <div className="flex flex-wrap items-center gap-2 pt-2 sm:col-start-2 sm:row-start-1 sm:justify-end sm:pt-0">
          {actions}
        </div>
      )}
    </li>
  );
}

/** The detail behind a number or a sentence, one tap away (a tooltip cannot be
 *  tapped on a phone). Put it in a Stat's `sub` or at the end of a line. */
export function Why({ label = "Why?", children }: { label?: string; children: React.ReactNode }) {
  return (
    <Popover>
      <PopoverTrigger className="rounded-sm font-medium text-foreground underline decoration-dotted underline-offset-4 outline-ring hover:decoration-solid focus-visible:outline-2 pointer-coarse:py-2">
        {label}
      </PopoverTrigger>
      <PopoverContent align="start" className="w-80 max-w-[calc(100vw-2rem)] leading-relaxed">
        {children}
      </PopoverContent>
    </Popover>
  );
}

/** Up to three numbers side by side; the value is the thing read. */
export function Stats({ className, ...props }: React.ComponentProps<"dl">) {
  return <dl className={cn("grid grid-cols-3 divide-x rounded-xl border bg-card shadow-card", className)} {...props} />;
}

/** `meter` (0..1) draws a thin bar under the value for a share, e.g. coverage;
 *  it is decoration (the value already says it), so screen readers skip it. */
export function Stat({ label, value, sub, tone, meter }: { label: string; value: React.ReactNode; sub?: React.ReactNode; tone?: "attention"; meter?: number }) {
  return (
    <div className="min-w-0 px-3 py-3.5 sm:px-5">
      <dt className="text-caption text-muted-foreground">{label}</dt>
      <dd className={cn("figures mt-1 truncate text-xl font-semibold sm:text-2xl", tone === "attention" && "text-attention")}>{value}</dd>
      {meter != null && (
        <dd aria-hidden className="mt-2 h-1 overflow-hidden rounded-full bg-muted">
          <div
            className={cn("h-full rounded-full transition-[width] duration-300 ease-out", tone === "attention" ? "bg-attention" : "bg-protect")}
            style={{ width: `${Math.max(0, Math.min(1, meter)) * 100}%` }}
          />
        </dd>
      )}
      {sub && <dd className="mt-0.5 text-caption text-muted-foreground">{sub}</dd>}
    </div>
  );
}

/** Label on the left, figure on the right; put them in a `<dl className="divide-y">`. */
export function KeyValue({ label, value }: { label: React.ReactNode; value: React.ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 py-2 text-sm">
      <dt className="min-w-0 text-muted-foreground">{label}</dt>
      <dd className="figures shrink-0 text-right font-medium">{value}</dd>
    </div>
  );
}

export function EmptyState({ icon: Icon, title, children, action }: { icon?: Icon; title: string; children?: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="flex flex-col items-center px-5 py-10 text-center">
      {Icon && (
        <span className="grid size-10 place-items-center rounded-full bg-muted text-muted-foreground">
          <Icon aria-hidden className="size-5" />
        </span>
      )}
      <p className="mt-3 text-title font-medium">{title}</p>
      {children && <p className="mt-1 max-w-[42ch] text-sm text-muted-foreground">{children}</p>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

/** A value to hand to someone else (an address, a link), with Copy. */
/** A value to copy. `shown`, when given, is what a person reads instead of the
 *  value itself — an address is copied, never displayed (docs/81). */
export function CopyRow({ value, label, mono, shown }: { value: string; label: string; mono?: boolean; shown?: string }) {
  return (
    <div className="flex items-center gap-2 rounded-lg border bg-background px-3 py-2">
      <code className={cn("min-w-0 flex-1 truncate text-xs", mono && "font-mono", shown && "font-sans text-sm")}>{shown ?? value}</code>
      <Button
        variant="ghost"
        size="icon"
        className="size-7"
        aria-label={`Copy ${label}`}
        onClick={async () => {
          try {
            await navigator.clipboard.writeText(value);
            toast.success("Copied");
          } catch {
            toast(value);
          }
        }}
      >
        <Copy className="size-3.5" />
      </Button>
    </div>
  );
}
