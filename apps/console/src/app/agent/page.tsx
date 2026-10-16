"use client";

import { useQuery } from "@tanstack/react-query";
import { CircleAlert, Package, RefreshCw, ShieldCheck } from "lucide-react";
import Link from "next/link";
import { useSyncExternalStore } from "react";
import { TopBar } from "@/components/brand";
import { EmptyState } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { ApiError, currentTenant, fetchConfig, store } from "@/lib/tameion";
import type { Tenant } from "@/lib/types";
import { AccountHaltBanner, AskedPanel, haltOf, MandatePanel, PassAnchorPanel, PaymentsPanel, RefusedPanel } from "./account-panels";
import { useAccountRecord } from "./account";
import { age, stamp } from "./feed";
import { useEvents, useLiveFeed, useNow } from "./live";
import { EventsPanel, feedState, FundingPanel, HaltBanner, HedgePanel, MarginPanel, OrdersPanel, PositionsPanel, Strip, venueText, type Chip, type StripFact } from "./panels";

/** The Agent surface (docs/102 D13): what an account's agents did, as a person watches it, in the layout of a terminal screen — a grid of
 *  dense panels, right-aligned figures, a time on every row, an events column. The account's own record leads (the mandate with what is
 *  spent and left, what was paid and refused and why, the last pass and its anchor); the exchange panels (positions, margin, funding,
 *  orders) appear only when an exchange account is connected. The agents themselves read `tameion-mcp` and the API, not this page. */
/** The header and the page's frame. `humanHref` is where the toggle's Human segment goes: the account's own page, or the desk's when an exchange is connected. */
function Frame({ humanHref, children }: { humanHref?: string; children: React.ReactNode }) {
  return (
    <>
      <TopBar surface="agent" humanHref={humanHref}>
        <Link href="/" className="hidden px-2 text-sm text-muted-foreground hover:text-foreground sm:inline">
          Payments
        </Link>
      </TopBar>
      <main className="mx-auto w-full max-w-6xl px-4 pb-16 pt-5 sm:px-6">{children}</main>
    </>
  );
}

export default function AgentPage() {
  // The account this browser has open; nothing during the static prerender.
  const saved = useSyncExternalStore(
    () => () => {},
    () => store.get("tameion.tenant") ?? "",
    () => null,
  );
  const tenant: Tenant | null | undefined = saved === null ? undefined : saved ? currentTenant() : null;
  if (tenant) return <Agent key={tenant.policy} tenant={tenant} />;
  return (
    <Frame>
      {tenant === undefined ? (
        <Skeleton className="h-40 w-full rounded-xl" />
      ) : (
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
      )}
    </Frame>
  );
}

function Agent({ tenant }: { tenant: Tenant }) {
  const policy = tenant.policy;
  const now = useNow();
  const feed = useLiveFeed(policy);
  const events = useEvents(policy);
  const acct = useAccountRecord(policy, events.rows[0]?.id);
  const cfg = useQuery({ queryKey: ["config"], queryFn: fetchConfig, staleTime: 300_000 });
  const explorer = cfg.data?.chains.find((c) => c.key === tenant.chain)?.explorer ?? null;
  const state = feedState(feed.link, feed.last, now);
  const f = feed.last?.frame;
  // An exchange account is connected only when the desk's feed names one; without it the page is the account's own record.
  const venue = f && f.venue ? f : null;
  const inbox = acct.inbox.data;
  const halt = inbox ? haltOf(inbox) : null;

  const chip: Chip = venue
    ? state.kind === "live"
      ? { tone: "live", label: "Live" }
      : state.kind === "stale"
        ? { tone: "stale", label: feed.link.kind === "down" ? "Reconnecting" : "No update" }
        : { tone: "connecting", label: "Connecting" }
    : acct.inbox.isError
      ? { tone: "down", label: "Account unavailable" }
      : events.link.kind === "open"
        ? { tone: "live", label: "Live" }
        : events.link.kind === "down"
          ? { tone: "stale", label: "Reconnecting" }
          : { tone: "connecting", label: "Connecting" };
  const facts: StripFact[] = inbox
    ? [
        { label: "Mode", value: inbox.mode },
        { label: "Waiting for you", value: String(inbox.waitingCount ?? 0) },
        inbox.lastPass
          ? { label: "Last pass", value: stamp(inbox.lastPass.at, now || inbox.lastPass.at * 1000), sub: now > 0 ? `${age(Math.max(0, now - inbox.lastPass.at * 1000))} ago` : undefined }
          : { label: "Last pass", value: <span className="font-normal text-muted-foreground">none yet</span> },
        {
          label: "Halt",
          value: halt ? (
            <span className="inline-flex items-center gap-1 text-destructive">
              <CircleAlert aria-hidden className="size-3.5" /> Halted
            </span>
          ) : (
            <span className="inline-flex items-center gap-1 text-protect">
              <ShieldCheck aria-hidden className="size-3.5" /> None
            </span>
          ),
        },
        ...(venue ? [{ label: "Exchange account", value: venueText(venue.venue) }] : []),
      ]
    : [];
  const refused = acct.inbox.error instanceof ApiError && (acct.inbox.error.status === 401 || acct.inbox.error.status === 403);

  return (
    <Frame humanHref={venue ? "/desk" : "/"}>
      <header className="mb-4 flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h1 className="text-headline font-semibold">Agent</h1>
        <p className="text-caption text-muted-foreground">What the account&apos;s agents did, and what they may still spend.</p>
      </header>
      <div className="space-y-3">
        <Strip chip={chip} facts={facts} action={venue && state.kind === "stale" ? { label: "Try now", onClick: feed.retry } : undefined} />
        {halt && <AccountHaltBanner halt={halt} />}
        {venue?.halt && <HaltBanner halt={venue.halt} now={now} />}
        {venue?.venueError && (
          <p role="status" className="rounded-lg border border-attention/45 bg-card px-3 py-2 text-caption text-attention">
            The exchange&apos;s numbers are not shown: {venue.venueError}.
          </p>
        )}
        {acct.inbox.isError ? (
          <div className="flex flex-col items-start gap-3 rounded-lg border border-attention/45 bg-card p-4">
            <p className="flex items-center gap-2 text-title font-medium">
              <CircleAlert aria-hidden className="size-4 text-attention" /> The account cannot be read
            </p>
            <p className="max-w-[60ch] text-sm text-muted-foreground">
              {refused ? "This device cannot read this account. Open it on the device that opened it." : `The service did not answer: ${(acct.inbox.error as Error).message}.`}
            </p>
            <Button size="sm" variant="outline" onClick={() => void acct.inbox.refetch()}>
              <RefreshCw aria-hidden className="size-3.5" /> Try now
            </Button>
          </div>
        ) : !inbox ? (
          <div role="status" aria-label="Reading the account" className="grid gap-3 lg:grid-cols-12">
            {["lg:col-span-7", "lg:col-span-5", "lg:col-span-7", "lg:col-span-5"].map((c, i) => (
              <div key={i} aria-hidden className={`h-36 animate-pulse rounded-lg border bg-card ${c}`} />
            ))}
          </div>
        ) : (
          // Two columns of panels at their own heights; one column on a phone, where the mandate and the last pass come first.
          <div className="grid grid-cols-1 gap-3 lg:grid-cols-12 lg:items-start">
            <div className="grid min-w-0 gap-3 lg:col-span-5 lg:col-start-8 lg:row-start-1">
              <MandatePanel inbox={inbox} />
              <PassAnchorPanel inbox={inbox} records={acct.records} elapsedSecs={acct.job.data?.progress?.elapsedSecs ?? null} job={acct.job} now={now} explorer={explorer} />
              {venue && (
                <>
                  <MarginPanel frame={venue} state={state} />
                  <FundingPanel frame={venue} state={state} />
                  <HedgePanel frame={venue} state={state} now={now} />
                </>
              )}
            </div>
            <div className="grid min-w-0 gap-3 lg:col-span-7 lg:col-start-1 lg:row-start-1">
              {venue && (
                <>
                  <PositionsPanel frame={venue} state={state} />
                  <OrdersPanel frame={venue} state={state} now={now} />
                </>
              )}
              <PaymentsPanel inbox={inbox} now={now} />
              <RefusedPanel inbox={inbox} now={now} />
              <AskedPanel inbox={inbox} now={now} />
              <EventsPanel rows={events.rows} link={events.link} />
            </div>
          </div>
        )}
      </div>
    </Frame>
  );
}
