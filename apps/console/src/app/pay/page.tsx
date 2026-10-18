"use client";

import { useQuery } from "@tanstack/react-query";
import { SearchX, ShieldCheck } from "lucide-react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Suspense } from "react";
import { TopBar } from "@/components/brand";
import { Page } from "@/components/layout/page";
import { CopyRow, EmptyState, Section } from "@/components/layout/section";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { fetchName, isHandle } from "@/lib/tameion";

/** `/pay?to=<name>` (docs/79 §6a Console 3): what someone paying the account
 *  needs — its name, its address, the chains it takes USDC on. The query string
 *  is read in the browser, so the static export ships one page for every name. */
export default function PayPage() {
  return (
    <Suspense fallback={<TopBar />}>
      <Pay />
    </Suspense>
  );
}

function Pay() {
  const to = (useSearchParams().get("to") ?? "").trim().toLowerCase();
  const q = useQuery({ queryKey: ["name", to], queryFn: () => fetchName(to), enabled: isHandle(to), retry: 1 });
  const r = q.data;
  const live = r?.chains.filter((c) => c.deployed) ?? [];
  const chains = live.length ? live : (r?.chains ?? []);

  return (
    <>
      <TopBar />
      <Page className="max-w-xl lg:pt-16">
        {isHandle(to) && q.isPending ? (
          <div className="space-y-4">
            <Skeleton className="h-10 w-1/2" />
            <Skeleton className="h-40 w-full rounded-xl" />
          </div>
        ) : q.error ? (
          <p className="text-sm text-destructive">Could not look up {to} just now: {(q.error as Error).message}. Try again in a moment.</p>
        ) : !r ? (
          <EmptyState icon={SearchX} title={to ? `No account is called ${to}` : "No account named"}>
            Check the link with whoever sent it. A name is lowercase letters, numbers and dashes, like /pay?to=acme.
          </EmptyState>
        ) : (
          <>
            <h1 className="text-headline font-semibold">Pay {r.name}</h1>
            <p className="mt-2 text-muted-foreground">Send USDC to {r.name}&apos;s account. The address is the same on every chain listed here.</p>
            <Section className="mt-6" title="Their account">
              <div className="space-y-3 text-sm">
                {/* One address everywhere, unless a chain's account differs: then each is named. */}
                {new Set(chains.map((c) => (c.account ?? r.account).toLowerCase())).size > 1 ? (
                  chains.map((c) => (
                    <div key={c.key} className="space-y-1">
                      <p className="text-muted-foreground">On {c.name}</p>
                      <CopyRow value={c.account ?? r.account} shown={`${r.name} on ${c.name} · copy the address`} label={`${r.name}'s address on ${c.name}`} />
                    </div>
                  ))
                ) : (
                  <CopyRow value={r.account} shown={`${r.name} · copy the address`} label={`${r.name}'s address`} />
                )}
                {/* The payer's wallet asks for the address; it is here for them to compare, not to read. */}
                <details className="text-xs">
                  <summary className="cursor-pointer text-muted-foreground">Show the address</summary>
                  <p className="mt-1 break-all font-mono">{r.account}</p>
                </details>
                <div className="flex flex-wrap items-center gap-1.5">
                  <span className="text-muted-foreground">{live.length ? "Live on" : "Takes USDC on"}</span>
                  {chains.map((c) => (
                    <Badge key={c.key} variant="outline">
                      {c.name}
                    </Badge>
                  ))}
                </div>
                <p className="text-caption text-muted-foreground">Send USDC only, and only on a chain listed here.</p>
              </div>
            </Section>
            <p className="mt-6 flex gap-2 text-sm leading-relaxed text-muted-foreground">
              <ShieldCheck className="mt-0.5 size-4 shrink-0 text-protect" aria-hidden />
              <span>The money goes to {r.name}&apos;s own account on chain. Only {r.name}&apos;s passkey can take it out; Tami cannot.</span>
            </p>
            <p className="mt-8 text-sm">
              <Link href="/" className="text-muted-foreground underline-offset-4 hover:text-foreground hover:underline">
                Open an account like this for your company →
              </Link>
            </p>
          </>
        )}
      </Page>
    </>
  );
}
