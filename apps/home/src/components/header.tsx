"use client";
// The header (DESIGN §Page grammar): the account's name on the left ("Tameion" when none), then the network word in
// words and shape, then the capsule Owner | Agent — one tap, or `v`; remembered per device.
import { useParams } from "next/navigation";
import { useEffect, useState } from "react";
import { fetchTenant } from "../lib/api.ts";
import { isViewKey, otherView, setView, useView } from "../lib/view.ts";
import { VIEWS, type View } from "../tokens.ts";

const LABEL: Record<View, string> = { owner: "Owner", agent: "Agent" };

export function Header() {
  const policy = useParams<{ policy?: string }>()?.policy;
  const view = useView();
  // undefined while the name is being read; null when the account has none (or the read failed)
  const [name, setName] = useState<{ policy: string; name: string | null } | undefined>(undefined);

  useEffect(() => {
    if (!policy) return;
    let live = true;
    fetchTenant(policy).then(
      (t) => live && setName({ policy, name: t.name ?? null }),
      () => live && setName({ policy, name: null }),
    );
    return () => {
      live = false;
    };
  }, [policy]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!isViewKey(e)) return;
      e.preventDefault();
      setView(otherView(view));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view]);

  const reading = policy !== undefined && name?.policy !== policy;
  const title = policy && name?.policy === policy && name.name ? name.name : "Tameion";

  return (
    <header className="border-b border-(--rule)">
      <div className="surface flex min-h-16 items-center gap-(--space-3) py-(--space-2)">
        {reading ? (
          <span className="skeleton block h-[22px] w-32" aria-hidden />
        ) : (
          <span className="row-title min-w-0 truncate">{title}</span>
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
