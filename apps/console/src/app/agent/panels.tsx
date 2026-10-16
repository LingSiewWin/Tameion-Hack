"use client";

import { Check, CircleAlert, Loader2, OctagonAlert, RefreshCw, ShieldCheck, TriangleAlert, X } from "lucide-react";
import { Why } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { isAddr, tail } from "@/lib/tameion";
import { cn } from "@/lib/utils";
import {
  age,
  bandOf,
  clock,
  fillOf,
  fixed,
  money,
  places,
  ratioText,
  STALE_AFTER_MS,
  stamp,
  VENUE_STALE_AFTER_MS,
  type Band,
  type EventRow,
  type Frame,
  type Order,
} from "./feed";
import type { Link } from "./live";
import { Delta, Empty, Fact, Panel, Td, Th } from "./panel";

// ---- is the feed alive? -----------------------------------------------------------------------------------
export type FeedState = {
  kind: "connecting" | "live" | "stale" | "down";
  /** since the last frame arrived */
  quietMs: number | null;
  /** since the venue's own reading last changed (the service never sends one older than 2 s) */
  venueQuietMs: number | null;
};

/** What the page may claim about its numbers: they are live only while frames keep arriving, and the venue's are only as
 *  fresh as the venue's own clock last moved. */
export function feedState(link: Link, last: { seenAt: number; venueSeenAt: number | null } | null, now: number): FeedState {
  const quietMs = last && now > 0 ? Math.max(0, now - last.seenAt) : null;
  const venueQuietMs = last?.venueSeenAt != null && now > 0 ? Math.max(0, now - last.venueSeenAt) : null;
  if (!last) return { kind: link.kind === "down" ? "down" : "connecting", quietMs, venueQuietMs };
  return { kind: quietMs != null && quietMs > STALE_AFTER_MS ? "stale" : "live", quietMs, venueQuietMs };
}

/** The venue's reading has stopped changing (the connection itself may still be alive). */
export const venueStale = (state: FeedState) => state.venueQuietMs != null && state.venueQuietMs > VENUE_STALE_AFTER_MS;

/** The time a venue panel's figures are the venue's own: its clock, not the service's. */
const venueAt = (f: Frame) => (f.venueTime != null ? f.venueTime / 1000 : f.at);

/** "as of 14:03:22" (the time of what the panel shows), or, once it is old, how long: the connection has gone quiet, or, on a
 *  venue panel, the venue's own reading has stopped changing. */
export function AsOf({ at, state, venue = false }: { at: number; state: FeedState; venue?: boolean }) {
  const old = state.kind === "stale" ? state.quietMs : venue && venueStale(state) ? state.venueQuietMs : null;
  return old != null ? (
    <span className="font-medium text-attention">Stale for {age(old)}</span>
  ) : (
    <span>
      as of <time dateTime={new Date(at * 1000).toISOString()}>{clock(at)}</time>
    </span>
  );
}

export const venueText = (v: string | null) => (v ? (isAddr(v) ? `••${tail(v)}` : v) : "—");

// ---- the strip over everything: alive, how old, which account, halted ------------------------------------
export type Chip = { tone: "live" | "stale" | "down" | "connecting"; label: string };
export type StripFact = { label: string; value: React.ReactNode; sub?: React.ReactNode };

/** The bar over everything: is it alive, then the few facts that decide whether to look closer. The page chooses the facts:
 *  an account's mode, what waits for the owner, the last pass; an exchange's account and last tick when one is connected. */
export function Strip({ chip, facts, action, note }: { chip: Chip; facts: StripFact[]; action?: { label: string; onClick: () => void }; note?: string | null }) {
  return (
    <div className="rounded-lg border bg-card px-3 py-2.5">
      <div className="flex flex-wrap items-center gap-x-8 gap-y-2.5">
        <div role="status" className="flex w-full items-center gap-2 text-caption font-medium sm:w-auto sm:min-w-[8rem]">
          {chip.tone === "live" ? (
            <>
              <span aria-hidden className="size-2 animate-pulse rounded-full bg-protect" />
              {chip.label}
            </>
          ) : chip.tone === "stale" ? (
            <span className="flex items-center gap-1.5 text-attention">
              <TriangleAlert aria-hidden className="size-4" /> {chip.label}
            </span>
          ) : chip.tone === "down" ? (
            <span className="flex items-center gap-1.5 text-destructive">
              <CircleAlert aria-hidden className="size-4" /> {chip.label}
            </span>
          ) : (
            <span className="flex items-center gap-1.5 text-muted-foreground">
              <Loader2 aria-hidden className="size-4 animate-spin" /> {chip.label}
            </span>
          )}
        </div>
        <dl className="grid w-full grid-cols-2 gap-x-3 gap-y-2.5 sm:flex sm:w-auto sm:flex-wrap sm:gap-x-8">
          {facts.map((f) => (
            <Fact key={f.label} label={f.label} sub={f.sub}>
              {f.value}
            </Fact>
          ))}
        </dl>
        {action && (
          <Button size="sm" variant="outline" onClick={action.onClick} className="sm:ml-auto">
            <RefreshCw aria-hidden className="size-3.5" /> {action.label}
          </Button>
        )}
      </div>
      {note && (
        <p role="status" className="mt-2 text-xs text-attention">
          {note}
        </p>
      )}
    </div>
  );
}

export function HaltBanner({ halt, now }: { halt: NonNullable<Frame["halt"]>; now: number }) {
  return (
    <div role="alert" className="flex items-start gap-3 rounded-lg border border-destructive/50 bg-destructive/10 px-3 py-2.5">
      <OctagonAlert aria-hidden className="mt-0.5 size-4 shrink-0 text-destructive" />
      <div className="min-w-0 text-sm">
        <p className="font-semibold text-destructive">The offset is halted</p>
        <p className="mt-0.5 break-words">
          {halt.reason}
          {halt.at != null && <span className="text-foreground/80"> · since {stamp(halt.at, now || halt.at * 1000)}</span>}
        </p>
        <p className="mt-0.5 text-foreground/80">Nothing trades until a person clears it.</p>
      </div>
    </div>
  );
}

// ---- positions -----------------------------------------------------------------------------------------------
export function PositionsPanel({ frame, state }: { frame: Frame; state: FeedState }) {
  const ps = frame.positions;
  const dp = { size: places((ps ?? []).map((p) => p.size)), entry: places((ps ?? []).map((p) => p.entry)), mark: places((ps ?? []).map((p) => p.mark)) };
  return (
    <Panel title="Positions" sub="USDC" right={<AsOf at={venueAt(frame)} state={state} venue />}>
      {ps === null ? (
        <Empty>The venue&apos;s positions are not known right now{frame.venueError ? " (see the note above)" : "; this frame carried none"}.</Empty>
      ) : ps.length === 0 ? (
        <Empty>Flat. No open position on the venue account.</Empty>
      ) : (
        <table className="w-full text-caption">
          <caption className="sr-only">Open positions on the venue account</caption>
          <thead>
            <tr>
              <Th>Coin</Th>
              <Th className="hidden sm:table-cell">Side</Th>
              <Th right>Size</Th>
              <Th right>Entry</Th>
              <Th right>Mark</Th>
              <Th right>PnL</Th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {ps.map((p) => {
              const n = p.size == null ? NaN : Number(p.size);
              const side = Number.isNaN(n) ? "—" : n < 0 ? "Short" : "Long";
              return (
                <tr key={p.coin} className="hover:bg-muted/50">
                  <Td className="font-medium">
                    {p.coin}
                    <span className="block text-xs font-normal text-muted-foreground sm:hidden">{side}</span>
                  </Td>
                  <Td className="hidden sm:table-cell">{side}</Td>
                  <Td right>{fixed(p.size, dp.size)}</Td>
                  <Td right>{fixed(p.entry, dp.entry)}</Td>
                  <Td right>{fixed(p.mark, dp.mark)}</Td>
                  <Td right>
                    <Delta value={p.pnl} />
                  </Td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </Panel>
  );
}

// ---- margin ---------------------------------------------------------------------------------------------------
const FILL: Record<Band, string> = { critical: "bg-destructive", "below-rule": "bg-attention", ok: "bg-protect" };

function BandBadge({ band, rule, stale }: { band: Band; rule: number; stale: boolean }) {
  const [Icon, text, tone] =
    stale
      ? [TriangleAlert, `Last seen: ${band === "ok" ? "inside the rule" : band === "below-rule" ? `under the ${rule}× rule` : "below 1.0×"}`, "border-border bg-muted text-muted-foreground"]
      : band === "ok"
      ? [ShieldCheck, "Inside the rule", "border-protect/40 bg-protect/10 text-protect"]
      : band === "below-rule"
        ? [TriangleAlert, `Under the ${rule}× rule`, "border-attention/45 bg-attention/10 text-attention"]
        : [OctagonAlert, "Below 1.0×", "border-destructive/45 bg-destructive/10 text-destructive"];
  return (
    <span className={cn("inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-medium", tone)}>
      <Icon aria-hidden className="size-3" /> {text}
    </span>
  );
}

/** The ratio against the desk's rule, 0 to the rule: a tick at 1.0× (where the venue's own line is), the fill red below
 *  it, amber up to the rule, green at it. The value and the badge say the same in words. */
function KMeter({ k, rule, band, stale }: { k: number; rule: number; band: Band; stale: boolean }) {
  const one = (1 / rule) * 100;
  return (
    <div className="mt-3">
      <div
        role="meter"
        aria-label="Account value divided by maintenance margin, against the desk's rule"
        aria-valuemin={0}
        aria-valuemax={rule}
        aria-valuenow={Math.min(k, rule)}
        aria-valuetext={`${ratioText(k)} the maintenance margin; the desk's rule is ${rule}×${stale ? "; this reading is old" : ""}`}
        className="relative h-2.5 rounded-full bg-muted"
      >
        <div className={cn("h-full rounded-full transition-[width] duration-300 ease-out", stale ? "bg-muted-foreground" : FILL[band])} style={{ width: `${fillOf(k, rule) * 100}%` }} />
        <span aria-hidden className="absolute -inset-y-[3px] w-px bg-foreground/60" style={{ left: `${one}%` }} />
      </div>
      <div aria-hidden className="relative mt-1.5 h-4 text-xs text-muted-foreground">
        <span className="absolute left-0">0</span>
        <span className="absolute -translate-x-1/2" style={{ left: `${one}%` }}>
          1.0×
        </span>
        <span className="absolute right-0">{rule}× rule</span>
      </div>
    </div>
  );
}

export function MarginPanel({ frame, state }: { frame: Frame; state: FeedState }) {
  const { kRatio: k, kPolicy: rule } = frame;
  const band = k != null && rule != null ? bandOf(k, rule) : null;
  const stale = state.kind === "stale" || venueStale(state);
  const why =
    k != null
      ? "This frame carried no rule to read the ratio against."
      : frame.positions && frame.positions.length === 0
        ? "No open position, so there is nothing to maintain."
        : frame.venueError
          ? "Not known right now (see the note above)."
          : "This frame carried no ratio.";
  return (
    <Panel title="Margin" sub="account value ÷ maintenance" right={<AsOf at={venueAt(frame)} state={state} venue />}>
      <div className="px-3 py-3">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
          <p className={cn("text-3xl font-semibold leading-none tracking-tight", k == null && "text-muted-foreground")}>{k != null ? ratioText(k) : "—"}</p>
          {band && rule != null && <BandBadge band={band} rule={rule} stale={stale} />}
        </div>
        {k != null && rule != null && band ? <KMeter k={k} rule={rule} band={band} stale={stale} /> : <p className="mt-2 text-caption text-muted-foreground">{why}</p>}
        <dl className="mt-3 grid grid-cols-3 gap-3 border-t pt-3">
          <Fact label="Account value">{money(frame.accountValue)}</Fact>
          <Fact label="Margin used">{money(frame.marginUsed)}</Fact>
          <Fact label="Maintenance">{money(frame.maintenanceMarginUsed)}</Fact>
        </dl>
        <p className="mt-2 text-xs text-muted-foreground">
          {frame.venue ? "USDC, from the venue's own account state. " : "USDC. "}
          <Why label="What is the ratio?">
            The venue needs a minimum amount in the account to keep a position open: its maintenance margin. This ratio is the account&apos;s value divided by that minimum.
            {rule != null ? ` The desk's own rule is to keep it at ${rule}× or more;` : ""} at 1.0× or below, the venue closes the position.
          </Why>
        </p>
      </div>
    </Panel>
  );
}

// ---- orders and their read-back ----------------------------------------------------------------------------------
function ReadBack({ v }: { v: Order["readBack"] }) {
  return v === "match" ? (
    <span className="inline-flex items-center gap-1 text-protect">
      <Check aria-hidden className="size-3.5" /> match
    </span>
  ) : v === "mismatch" ? (
    <span className="inline-flex items-center gap-1 font-semibold text-destructive">
      <X aria-hidden className="size-3.5" /> mismatch
    </span>
  ) : (
    <span className="text-muted-foreground">—</span>
  );
}

export function OrdersPanel({ frame, state, now }: { frame: Frame; state: FeedState; now: number }) {
  const dp = places(frame.orders.map((o) => o.size));
  return (
    <Panel title="Orders" sub="position read back after each" right={<AsOf at={frame.at} state={state} />}>
      {frame.orders.length === 0 ? (
        <Empty>No order recorded by this service yet.</Empty>
      ) : (
        <table className="w-full text-caption">
          <caption className="sr-only">The desk&apos;s last orders and whether the venue&apos;s position matched each one</caption>
          <thead>
            <tr>
              <Th>Time</Th>
              <Th className="hidden sm:table-cell">Coin</Th>
              <Th className="hidden sm:table-cell">Side</Th>
              <Th right className="hidden sm:table-cell">
                Size
              </Th>
              <Th right className="hidden sm:table-cell">
                Order
              </Th>
              <Th className="sm:hidden">Order</Th>
              <Th right>Read-back</Th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {frame.orders.map((o, i) => (
              <tr key={`${o.oid ?? i}-${o.at ?? i}`} className={cn("hover:bg-muted/50", o.readBack === "mismatch" && "bg-destructive/10 hover:bg-destructive/10")}>
                <Td className="figures whitespace-nowrap">{o.at != null ? stamp(o.at, now || o.at * 1000) : "—"}</Td>
                <Td className="hidden font-medium sm:table-cell">{o.coin}</Td>
                <Td className="hidden capitalize sm:table-cell">{o.side ?? "—"}</Td>
                <Td right className="hidden sm:table-cell">
                  {fixed(o.size, dp)}
                </Td>
                <Td right className="hidden text-muted-foreground sm:table-cell">
                  {o.oid ?? "—"}
                </Td>
                <Td className="sm:hidden">
                  <span className="font-medium">{o.coin}</span> <span className="capitalize">{o.side ?? "—"}</span> <span className="figures">{fixed(o.size, dp)}</span>
                  {o.oid && <span className="figures block text-xs text-muted-foreground">#{o.oid}</span>}
                </Td>
                <Td right>
                  <ReadBack v={o.readBack} />
                </Td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}

// ---- funding -------------------------------------------------------------------------------------------------------
export function FundingPanel({ frame, state }: { frame: Frame; state: FeedState }) {
  // Two small figures read against each other: the same places, up to four, never rounded to a cent.
  const dp = Math.min(4, Math.max(2, places([frame.fundingLastHour, frame.funding24h])));
  return (
    <Panel title="Funding" sub="USDC" right={<AsOf at={frame.fundingReadAt ?? frame.at} state={state} />}>
      <dl className="divide-y text-caption">
        <div className="flex items-baseline justify-between gap-4 px-3 py-2">
          <dt className="text-muted-foreground">Last hour</dt>
          <dd>
            <Delta value={frame.fundingLastHour} dp={dp} />
          </dd>
        </div>
        <div className="flex items-baseline justify-between gap-4 px-3 py-2">
          <dt className="text-muted-foreground">24 hours</dt>
          <dd>
            <Delta value={frame.funding24h} dp={dp} />
          </dd>
        </div>
      </dl>
      <p className="border-t px-3 py-2 text-xs text-muted-foreground">+ the desk received · − the desk paid</p>
    </Panel>
  );
}

// ---- safety: halt, pass, anchor -----------------------------------------------------------------------------------------
export function HedgePanel({ frame, state, now }: { frame: Frame; state: FeedState; now: number }) {
  const when = (at: number | null) => (at == null ? "not recorded" : stamp(at, now || at * 1000));
  const ago = (at: number | null) => (at == null || now === 0 ? null : age(Math.max(0, now - at * 1000)));
  const row = "flex items-baseline justify-between gap-4 px-3 py-2";
  return (
    <Panel title="Hedge" sub="the exchange offset's halt and passes">
      <dl className="divide-y text-caption">
        <div className={row}>
          <dt className="text-muted-foreground">Halt</dt>
          <dd className="min-w-0 text-right">
            {frame.halt ? (
              <span className="font-semibold text-destructive">
                Halted<span className="font-normal text-foreground"> · {frame.halt.reason}</span>
              </span>
            ) : state.kind === "stale" ? (
              <span className="text-muted-foreground">None at the last tick</span>
            ) : (
              <span className="inline-flex items-center gap-1 text-protect">
                <ShieldCheck aria-hidden className="size-3.5" /> None
              </span>
            )}
          </dd>
        </div>
        <div className={row}>
          <dt className="text-muted-foreground">Pass latency</dt>
          <dd className="figures">{frame.passLatencyMs != null ? `${Math.round(frame.passLatencyMs).toLocaleString("en-US")} ms` : "not recorded"}</dd>
        </div>
        <div className={row}>
          <dt className="text-muted-foreground">Last pass</dt>
          <dd className="figures">
            {when(frame.lastPassAt)}
            {ago(frame.lastPassAt) && <span className="text-muted-foreground"> · {ago(frame.lastPassAt)} ago</span>}
          </dd>
        </div>
      </dl>
    </Panel>
  );
}

// ---- events ---------------------------------------------------------------------------------------------------------------
const TONE: Record<EventRow["tone"], string> = { info: "", ok: "", warn: "text-attention", bad: "text-destructive" };

function EventTable({ rows, empty }: { rows: EventRow[]; empty: string }) {
  if (rows.length === 0) return <Empty>{empty}</Empty>;
  return (
    <div role="region" aria-label="Events, newest first" tabIndex={0} className="max-h-72 overflow-y-auto outline-ring focus-visible:outline-2 focus-visible:-outline-offset-2">
      <table className="w-full text-caption">
        <thead className="sticky top-0 bg-card">
          <tr>
            <Th className="w-0">Seen</Th>
            <Th className="w-0">Source</Th>
            <Th>What happened</Th>
          </tr>
        </thead>
        <tbody className="divide-y">
          {rows.map((r) => (
            <tr key={r.id} className="hover:bg-muted/50">
              <Td className="figures whitespace-nowrap text-muted-foreground">{clock(r.at / 1000)}</Td>
              <Td className="whitespace-nowrap">{r.source}</Td>
              <Td className={TONE[r.tone]}>{r.text}</Td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function EventsPanel({ rows, link }: { rows: EventRow[]; link: Link }) {
  // The stream opens to this device's token, the one the desk's own routes take (docs/103 S4-1), so a refusal means this is
  // not the owner's device: nothing to paste, nothing to type.
  const nothing =
    link.kind !== "down"
      ? "Nothing yet. Checks and changes on the account show here as they happen."
      : link.status === 401 || link.status === 403
        ? "This device cannot read the account's events."
        : `The event stream is not reachable (${link.message}). It tries again by itself.`;
  return (
    <Tabs defaultValue="all">
      <Panel
        title="Events"
        sub="the account, as this page saw it"
        right={
          <TabsList className="h-7! pointer-coarse:h-10!" aria-label="Filter events">
            <TabsTrigger value="all" className="px-2.5 text-xs">
              All
            </TabsTrigger>
            <TabsTrigger value="Check" className="px-2.5 text-xs">
              Checks
            </TabsTrigger>
            <TabsTrigger value="Account" className="px-2.5 text-xs">
              Account
            </TabsTrigger>
          </TabsList>
        }
      >
        <TabsContent value="all">
          <EventTable rows={rows} empty={nothing} />
        </TabsContent>
        <TabsContent value="Check">
          <EventTable rows={rows.filter((r) => r.source === "Check")} empty={rows.length ? "No check yet in this visit." : nothing} />
        </TabsContent>
        <TabsContent value="Account">
          <EventTable rows={rows.filter((r) => r.source === "Account")} empty={rows.length ? "No account change yet in this visit." : nothing} />
        </TabsContent>
      </Panel>
    </Tabs>
  );
}
