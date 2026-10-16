"use client";

import { useQuery } from "@tanstack/react-query";
import { Package } from "lucide-react";
import Link from "next/link";
import { useSyncExternalStore } from "react";
import { TopBar } from "@/components/brand";
import { EmptyState } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { currentTenant, fetchConfig, store } from "@/lib/tameion";
import type { Tenant } from "@/lib/types";
import { useEvents, useLiveFeed, useNow } from "./live";
import { EventsPanel, FeedDown, feedState, FundingPanel, HaltBanner, MarginPanel, OrdersPanel, PositionsPanel, SafetyPanel, Strip } from "./panels";

/** The Agent surface (docs/102 D13): the desk's agent as a person watches it, in the layout of a terminal screen —
 *  a grid of dense panels, right-aligned figures, red and green changes with their sign, a time on every row, an events
 *  column. Only what the live feed carries is drawn. The agent itself reads `tameion-mcp` and `/api/desk`, not this page. */
export default function AgentPage() {
  // The account this browser has open; nothing during the static prerender.
  const saved = useSyncExternalStore(
    () => () => {},
    () => store.get("tameion.tenant") ?? "",
    () => null,
  );
  const tenant: Tenant | null | undefined = saved === null ? undefined : saved ? currentTenant() : null;
  return (
    <>
      <TopBar surface="agent">
        <Link href="/" className="hidden px-2 text-sm text-muted-foreground hover:text-foreground sm:inline">
          Payments
        </Link>
      </TopBar>
      <main className="mx-auto w-full max-w-6xl px-4 pb-16 pt-5 sm:px-6">
        {tenant === undefined ? (
          <Skeleton className="h-40 w-full rounded-xl" />
        ) : !tenant ? (
          <EmptyState
            icon={Package}
            title="Open your account first"
            action={
              <Button asChild>
                <Link href="/">Open it</Link>
              </Button>
            }
          >
            The agent belongs to an account. Open yours on this device, then come back.
          </EmptyState>
        ) : (
          <Agent key={tenant.policy} tenant={tenant} />
        )}
      </main>
    </>
  );
}

function Agent({ tenant }: { tenant: Tenant }) {
  const policy = tenant.policy;
  const now = useNow();
  const feed = useLiveFeed(policy);
  const events = useEvents(policy);
  const cfg = useQuery({ queryKey: ["config"], queryFn: fetchConfig, staleTime: 300_000 });
  const explorer = cfg.data?.chains.find((c) => c.key === tenant.chain)?.explorer ?? null;
  const state = feedState(feed.link, feed.last, now);
  const f = feed.last?.frame;
  return (
    <>
      <header className="mb-4 flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h1 className="text-headline font-semibold">Agent</h1>
        <p className="text-caption text-muted-foreground">What the desk&apos;s agent sees and does on the exchange, live.</p>
      </header>
      <div className="space-y-3">
        <Strip link={feed.link} last={feed.last} state={state} rejected={feed.rejected} retry={feed.retry} now={now} />
        {f?.halt && <HaltBanner halt={f.halt} now={now} />}
        {f?.venueError && (
          <p role="status" className="rounded-lg border border-attention/45 bg-card px-3 py-2 text-caption text-attention">
            The venue&apos;s numbers are not shown: {f.venueError}.
          </p>
        )}
        {!f ? (
          <>
            <FeedDown link={feed.link} rejected={feed.rejected} retry={feed.retry} />
            <EventsPanel rows={events.rows} link={events.link} />
          </>
        ) : (
          // Two columns of panels at their own heights (a short table is not stretched into an empty card): what the
          // venue says on the right, what the desk holds and did on the left; one column on a phone.
          <div className="grid grid-cols-1 gap-3 lg:grid-cols-12 lg:items-start">
            {/* First in the page, so first on a phone: is the desk safe, is anything halted. Right of the tables on a desktop. */}
            <div className="grid min-w-0 gap-3 lg:col-span-5 lg:col-start-8 lg:row-start-1">
              <MarginPanel frame={f} state={state} />
              <SafetyPanel frame={f} state={state} now={now} explorer={explorer} />
              <FundingPanel frame={f} state={state} />
            </div>
            <div className="grid min-w-0 gap-3 lg:col-span-7 lg:col-start-1 lg:row-start-1">
              <PositionsPanel frame={f} state={state} />
              <OrdersPanel frame={f} state={state} now={now} />
              <EventsPanel rows={events.rows} link={events.link} />
            </div>
          </div>
        )}
      </div>
    </>
  );
}
