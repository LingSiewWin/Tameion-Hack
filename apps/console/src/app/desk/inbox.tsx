import { CheckCircle2, Inbox, Loader2 } from "lucide-react";
import { Row, Rows, Section } from "@/components/layout/section";
import { Button } from "@/components/ui/button";

/** One thing that waits for the owner (docs/102 D13): what it is, why, and the one tap that settles it where the page
 *  already has a handler for it. Nothing here is a form: if the owner has to type, it is not an inbox row. */
export type Need = {
  key: string;
  title: string;
  why?: string;
  action?: { label: string; onClick: () => void; busy?: boolean } | { label: string; href: string };
};

/** The first thing on the human home. With nothing waiting it is one calm line, not an empty card, so the numbers
 *  under it are on the first screen; with something waiting it is the amber card the account page uses.
 *  `alsoWaiting`: something else in the same area (the keys nag) is already asking, so "nothing needs you" would be false. */
export function DeskInbox({ needs, alsoWaiting = false }: { needs: Need[]; alsoWaiting?: boolean }) {
  if (needs.length === 0 && alsoWaiting) return null;
  if (needs.length === 0)
    return (
      <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5 rounded-xl border border-protect/35 bg-card px-4 py-3 shadow-card">
        <CheckCircle2 aria-hidden className="size-4 shrink-0 text-protect" />
        <p className="text-title font-medium">Nothing needs you</p>
        <p className="text-caption text-muted-foreground">When something does, it shows up here first.</p>
      </div>
    );
  return (
    <div>
      <Section title="Needs you" icon={Inbox} tone="attention" description="Each one is your decision. The reason is under it." flush>
        <Rows>
          {needs.map((n) => (
            <Row
              key={n.key}
              title={<span className="font-medium">{n.title}</span>}
              detail={n.why}
              actions={
                n.action &&
                ("href" in n.action ? (
                  <Button asChild size="sm">
                    <a href={n.action.href}>{n.action.label}</a>
                  </Button>
                ) : (
                  <Button size="sm" onClick={n.action.onClick} disabled={n.action.busy}>
                    {n.action.busy && <Loader2 aria-hidden className="size-3.5 animate-spin" />} {n.action.label}
                  </Button>
                ))
              }
            />
          ))}
        </Rows>
      </Section>
    </div>
  );
}
