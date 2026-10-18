"use client";
// The header (DESIGN §Page grammar): the account's name on the left ("Tameion" when none), then the network word in
// words and shape, then the capsule Owner | Agent — one tap, or `v`; remembered per device. The route carries the
// name (`/a/<name>`); it shows only once the service resolves it to an account.
import { useParams } from "next/navigation";
import { useEffect, useState } from "react";
import { resolveAccount } from "../lib/api.ts";
import { CONSOLE, consoleHref } from "../lib/console.ts";
import { isViewKey, otherView, setView, useView } from "../lib/view.ts";
import { VIEWS, type View } from "../tokens.ts";

const LABEL: Record<View, string> = { owner: "Owner", agent: "Agent" };

export function Header({ consoleUrl = null }: { consoleUrl?: string | null }) {
  const wanted = useParams<{ name?: string }>()?.name;
  const view = useView();
  // the name as the service resolved it; null when no account has it (or the read failed). `policy` (the address) is kept
  // for the Console link's href only.
  const [resolved, setResolved] = useState<{ wanted: string; name: string | null; policy?: string } | undefined>(undefined);

  useEffect(() => {
    if (!wanted) return;
    let live = true;
    resolveAccount(wanted).then(
      (r) => live && setResolved({ wanted, name: r?.name ?? null, policy: r?.policy }),
      () => live && setResolved({ wanted, name: null }),
    );
    return () => {
      live = false;
    };
  }, [wanted]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!isViewKey(e)) return;
      e.preventDefault();
      setView(otherView(view));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view]);

  const reading = wanted !== undefined && resolved?.wanted !== wanted;
  const title = wanted && resolved?.wanted === wanted && resolved.name ? resolved.name : "Tameion";
  const toConsole = resolved?.wanted === wanted ? consoleHref(consoleUrl, resolved?.policy) : null;

  return (
    <header>
      <div className="surface flex min-h-16 items-center gap-(--space-2) py-(--space-2) md:gap-(--space-3)">
        {reading ? (
          <span className="skeleton block h-[22px] w-32" aria-hidden />
        ) : (
          <span className="row-title min-w-0 truncate" title={title}>
            {title}
          </span>
        )}
        {/* every owner action lives in the console (founder: judges get one link); Mono in the Agent view, as .meta is there */}
        {toConsole && (
          <a
            href={toConsole}
            data-hash-allowed="console-link"
            title={CONSOLE.title}
            className="meta ml-auto inline-flex min-h-(--touch) items-center whitespace-nowrap px-(--space-1) text-(--ink) underline underline-offset-4"
          >
            {CONSOLE.label}
          </a>
        )}
        <span className={`pill meta ${toConsole ? "" : "ml-auto"}`}>Testnet</span>
        <div className="capsule" role="group" aria-label="View">
          {VIEWS.map((v) => (
            <button key={v} type="button" aria-pressed={view === v} aria-keyshortcuts="v" onClick={() => view !== v && setView(v)}>
              {LABEL[v]}
            </button>
          ))}
        </div>
      </div>
    </header>
  );
}
