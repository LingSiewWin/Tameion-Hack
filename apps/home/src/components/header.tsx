"use client";
// The header (DESIGN §Page grammar): the account's name on the left ("Tameion" when none), then the network word in
// words and shape, then the capsule Owner | Agent — one tap, or `v`; remembered per device. The route carries the
// name (`/a/<name>`); it shows only once the service resolves it to an account.
import { useParams } from "next/navigation";
import { useEffect, useState } from "react";
import { resolveAccount } from "../lib/api.ts";
import { isViewKey, otherView, setView, useView } from "../lib/view.ts";
import { VIEWS, type View } from "../tokens.ts";

const LABEL: Record<View, string> = { owner: "Owner", agent: "Agent" };

export function Header() {
  const wanted = useParams<{ name?: string }>()?.name;
  const view = useView();
  // the name as the service resolved it; null when no account has it (or the read failed)
  const [resolved, setResolved] = useState<{ wanted: string; name: string | null } | undefined>(undefined);

  useEffect(() => {
    if (!wanted) return;
    let live = true;
    resolveAccount(wanted).then(
      (r) => live && setResolved({ wanted, name: r?.name ?? null }),
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
        <span className="pill meta ml-auto">Testnet</span>
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
