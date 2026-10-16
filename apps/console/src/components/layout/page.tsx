import { BadgeCheck, PenLine } from "lucide-react";
import { cn } from "@/lib/utils";

/** The frame every screen sits in: one width, one gutter, one rhythm. */
export function Page({ className, ...props }: React.ComponentProps<"main">) {
  return <main className={cn("mx-auto w-full max-w-6xl px-4 pb-24 pt-6 sm:px-6 sm:pt-10", className)} {...props} />;
}

/** The title row: the name, small facts beside it, actions at the end, and at
 *  most one status line under it. */
export function PageHeader({
  title,
  meta,
  actions,
  children,
}: {
  title: React.ReactNode;
  meta?: React.ReactNode;
  actions?: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <header className="mb-6 flex flex-col gap-1.5 sm:mb-8">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <h1 className="text-headline font-semibold">{title}</h1>
        {meta && <div className="flex flex-wrap items-center gap-1.5">{meta}</div>}
        {actions && <div className="ml-auto flex items-center gap-2">{actions}</div>}
      </div>
      {children}
    </header>
  );
}

/** Two columns on a desktop: the wide one holds what the owner acts on, the
 *  narrow one what they glance at. One column on a phone, primary first. */
export function Columns({ primary, side }: { primary: React.ReactNode; side: React.ReactNode }) {
  return (
    <div className="grid grid-cols-1 items-start gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,22.5rem)] xl:gap-8">
      <div className="min-w-0 space-y-6">{primary}</div>
      <div className="min-w-0 space-y-6">{side}</div>
    </div>
  );
}

/** The owner's instruction to the agent in one sentence they sign (docs/73):
 *  the hero of Start and of the account page, so it replaces the paragraphs
 *  that explained the same thing. The sentence is the largest text on the
 *  screen; wrap its numbers and payees in <strong>. The passkey button goes in
 *  `action`; once signed, `note` says when, and the frame turns protect-green. */
export function Mandate({
  eyebrow = "Your instruction to the agent",
  signed = false,
  action,
  note,
  children,
}: {
  eyebrow?: string;
  signed?: boolean;
  action?: React.ReactNode;
  note?: React.ReactNode;
  children: React.ReactNode;
}) {
  const Mark = signed ? BadgeCheck : PenLine;
  return (
    <figure className={cn("rounded-xl border bg-card p-5 shadow-card sm:p-7", signed && "border-protect/40")}>
      <figcaption className="flex items-center gap-2 text-caption font-medium text-muted-foreground">
        <Mark aria-hidden className={cn("size-4", signed && "text-protect")} />
        {eyebrow}
      </figcaption>
      <blockquote className="mt-3 max-w-[48ch] text-balance text-headline font-normal sm:text-2xl sm:leading-snug sm:tracking-tight [&_strong]:font-semibold">
        {children}
      </blockquote>
      {(action || note) && (
        <div className="mt-5 flex flex-wrap items-center gap-x-4 gap-y-2">
          {action}
          {note && <p className="text-caption text-muted-foreground">{note}</p>}
        </div>
      )}
    </figure>
  );
}
