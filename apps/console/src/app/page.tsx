"use client";

import { useQuery } from "@tanstack/react-query";
import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Account } from "@/components/account/account";
import { TopBar } from "@/components/brand";
import { Start } from "@/components/start";
import { Skeleton } from "@/components/ui/skeleton";
import { currentTenant, fetchConfig, ownerPub, remember, store, tail, tenants } from "@/lib/tameion";
import type { Tenant } from "@/lib/types";

export default function Home() {
  const cfg = useQuery({ queryKey: ["config"], queryFn: fetchConfig, staleTime: 300_000 });
  const router = useRouter();
  const [tenant, setTenant] = useState<Tenant | null | undefined>(undefined);
  const [partner, setPartner] = useState<string>();

  useEffect(() => {
    const q = new URLSearchParams(location.search);
    (async () => {
      // A partner's link (?partner=<id>) names the partner on Start.
      const pid = (q.get("partner") || store.get("tameion.partner") || "").trim();
      if (/^[a-z0-9-]{2,32}$/.test(pid)) {
        try {
          const r = await fetch(`/api/partner/public?id=${pid}`);
          if (r.ok) {
            const p = await r.json();
            store.set("tameion.partner", p.id);
            setPartner(`Set up with ${p.name}. They see that your account exists and how it is doing — never a key, never your money.`);
          }
        } catch {}
      }
      // ?policy=0x… opens an existing account here (another device, a fresh browser).
      const wanted = (q.get("policy") || "").trim();
      if (/^0x[0-9a-fA-F]{40}$/.test(wanted)) {
        try {
          const r = await fetch(`/api/level1/tenant/${wanted}`);
          if (r.ok) {
            const t = (await r.json()).tenant as Tenant | undefined;
            // Only the account asked for (docs/60 P-72 H3); one this browser holds no
            // key for is saved only after the person says they want to look at it.
            if (!t?.policy || t.policy.toLowerCase() !== wanted.toLowerCase()) toast.error("The service answered with a different account than the link names; not opened.");
            else if (
              tenants().some((x) => x.policy.toLowerCase() === wanted.toLowerCase()) ||
              ownerPub(t).owns ||
              window.confirm(
                `Open someone else's account read-only?\n\n${t.name || "An account"} (ends ••${tail(wanted)})\n\nNo passkey on this device owns it: you can look, but nothing can be paid, funded or approved from here. A link that asks you to fund or approve it is not from its owner.`,
              )
            )
              remember(t);
          } else toast.error("No such account on this host.");
        } catch (e) {
          toast.error((e as Error).message);
        }
        history.replaceState(null, "", "/");
      }
      setTenant(currentTenant());
    })();
  }, []);

  if (!cfg.data || tenant === undefined) {
    return (
      <>
        <TopBar />
        <main className="mx-auto max-w-6xl px-4 pt-12 sm:px-6">
          {cfg.error ? (
            <p className="text-sm text-destructive">The service did not answer: {(cfg.error as Error).message}</p>
          ) : (
            <div className="space-y-4">
              <Skeleton className="h-14 w-2/3" />
              <Skeleton className="h-6 w-1/2" />
              <Skeleton className="h-48 w-full rounded-xl" />
            </div>
          )}
        </main>
      </>
    );
  }

  if (tenant) {
    return (
      <Account
        key={tenant.policy}
        tenant={tenant}
        cfg={cfg.data}
        onSwitch={(t) => {
          store.set("tameion.tenant", JSON.stringify(t));
          setTenant(t);
        }}
        onNew={() => {
          store.set("tameion.tenant", "");
          setTenant(null);
        }}
      />
    );
  }

  return (
    <>
      <TopBar>
        <a href="/watch" className="hidden px-2 text-sm text-muted-foreground hover:text-foreground sm:inline">
          Read any address
        </a>
      </TopBar>
      {/* `/` is the desk's entry (D11): a desk just opened goes to the desk, not the payments page. */}
      <Start
        cfg={cfg.data}
        partner={partner}
        onStarted={() => router.push("/desk")}
      />
    </>
  );
}
