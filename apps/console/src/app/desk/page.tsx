"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Package, RefreshCw } from "lucide-react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useState, useSyncExternalStore } from "react";
import { toast } from "sonner";
import { KeysNag } from "@/components/account/keys-nag";
import { TopBar, useOpenOnLastSurface } from "@/components/brand";
import { DealCard } from "@/components/desk/deal";
import { asksFor, BaseStock, EnterStep, Parties, usePartyNeeds, VenueConnect } from "@/components/desk/trust";
import { Columns, Page, PageHeader } from "@/components/layout/page";
import { EmptyState, Section, Stat, Stats, Why } from "@/components/layout/section";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import {
  ApiError,
  currentTenant,
  deskPost,
  fetchConfig,
  fetchDesk,
  fetchKitText,
  fetchStatements,
  grams,
  gramsText,
  GRAMS_PER_OZ,
  ozText,
  reserveToken,
  ownerPub,
  passkeyAssert,
  store,
  usdRound,
} from "@/lib/tameion";
import { deskOffsetChallenge } from "@/lib/guard";
import { keysReady, useKeys } from "@/lib/keys";
import type { DeskOffsetReply, DeskStatementRow, Tenant } from "@/lib/types";
import { DeskInbox, type Need } from "./inbox";

/** The service's default stress (stressBps 3000); the statements list says what each was printed at. */
const STRESS = 30;
const BASIS = 2;
const HL = "https://api.hyperliquid-testnet.xyz/info";

/** The exchange's own record for the account that holds the offset, read in the browser from its public API
 *  (docs/95 P2): the money there, the margin in use, and the gold price at which the short would be closed. */
async function exchangeState(user: string, market: string) {
  const r = await fetch(HL, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ type: "clearinghouseState", user }) });
  const d = (await r.json()) as {
    marginSummary?: { accountValue?: string; totalMarginUsed?: string };
    assetPositions?: { position: { coin: string; szi: string; liquidationPx?: string | null } }[];
  };
  const p = d.assetPositions?.find((x) => x.position.coin === market)?.position;
  return {
    accountValue: Number(d.marginSummary?.accountValue ?? 0),
    marginUsed: Number(d.marginSummary?.totalMarginUsed ?? 0),
    liquidationPx: p?.liquidationPx ? Number(p.liquidationPx) : null,
  };
}
/** The market's price now, when no exchange account is connected to read a mark from. */
async function midPrice(market: string) {
  const r = await fetch(HL, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ type: "allMids" }) });
  return Number(((await r.json()) as Record<string, string>)[market] ?? 0);
}
/** Out here, not in the component: the purity lint reads `Date.now()` inside a function the render reaches as impure. */
const nowSecs = () => Math.floor(Date.now() / 1000);
const clock = (at: number) =>
  new Date(at * 1000).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });

export default function DeskPage() {
  // The account this browser has open; nothing during the static prerender.
  const saved = useSyncExternalStore(
    () => () => {},
    () => store.get("tameion.tenant") ?? "",
    () => null,
  );
  const tenant: Tenant | null | undefined =
    saved === null ? undefined : saved ? currentTenant() : null;
  // A fresh tab opens on the surface chosen last (docs/107 S4-2).
  useOpenOnLastSurface();

  return (
    <>
      <TopBar surface="human">
        <Link
          href="/"
          className="hidden px-2 text-sm text-muted-foreground hover:text-foreground sm:inline"
        >
          Payments
        </Link>
      </TopBar>
      {tenant === undefined ? (
        <Page>
          <Skeleton className="h-40 w-full rounded-xl" />
        </Page>
      ) : !tenant ? (
        <Page>
          <EmptyState
            icon={Package}
            title="Open your account first"
            action={
              <Button asChild>
                <Link href="/">Open it</Link>
              </Button>
            }
          >
            The desk belongs to an account. Open yours on this device, then come
            back.
          </EmptyState>
        </Page>
      ) : (
        <Desk key={tenant.policy} tenant={tenant} />
      )}
    </>
  );
}

function Desk({ tenant }: { tenant: Tenant }) {
  const policy = tenant.policy;
  const router = useRouter();
  // The keys as the chain holds them (docs/104): the nag is part of Needs-you, so the calm "nothing needs you" line gives way to it.
  const keys = useKeys(tenant);
  const keysOpen = !!keys.data && !keysReady(keys.data);
  const qc = useQueryClient();
  const cfg = useQuery({
    queryKey: ["config"],
    queryFn: fetchConfig,
    staleTime: 300_000,
  });
  // Steps a counterparty signs arrive from outside this page: read the book every 10 s.
  const q = useQuery({
    queryKey: ["desk", policy],
    queryFn: () => fetchDesk(policy),
    refetchInterval: 10_000,
    retry: (n, e) => !(e instanceof ApiError && e.status === 403) && n < 1,
  });
  const statements = useQuery({
    queryKey: ["desk-statements", policy],
    queryFn: () => fetchStatements(policy),
    refetchInterval: 30_000,
  });
  const refresh = () =>
    void qc.invalidateQueries({ queryKey: ["desk", policy] });
  const venueAccount = q.data?.venueAccount ?? null;
  const market = q.data?.exposure[0]?.market ?? "PAXG";
  const xs = useQuery({
    queryKey: ["exchange-state", venueAccount, market],
    queryFn: () => exchangeState(venueAccount!, market),
    enabled: !!venueAccount,
    refetchInterval: 30_000,
  });
  const mid = useQuery({ queryKey: ["mid", market], queryFn: () => midPrice(market), refetchInterval: 60_000 });
  // The counterparties that answered an invite and wait for the owner's tap (docs/105): rows of the Needs-you inbox below.
  // A hook, so it stays above the early returns.
  const partyNeeds = usePartyNeeds({ policy, chainId: q.data?.chainId, pub: ownerPub(tenant).pub, onDone: refresh });
  // A step a counterparty signed on a public copy of /sign comes back as a link to this desk (docs/95 P3): book it,
  // on load and when the link is opened in a tab already showing the desk.
  useEffect(() => {
    const take = async () => {
      const m = /^#step=(.+)$/.exec(location.hash);
      if (!m) return;
      history.replaceState(null, "", location.pathname + location.search);
      try {
        const step = JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(m[1].replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0)))) as { policy?: string; milestone: unknown; signature: string };
        if (step.policy?.toLowerCase() !== policy.toLowerCase()) throw new Error("This signed step is for another desk.");
        const { data } = await deskPost<{ booked: string }>("/api/desk/milestone", policy, { milestone: step.milestone, signature: step.signature });
        toast.success(`Booked: ${data.booked.replace(/\(0x[0-9a-fA-F]{40}\)/, "").trim()}`);
        await qc.invalidateQueries({ queryKey: ["desk", policy] });
      } catch (e) {
        toast.error((e as Error).message);
      }
    };
    void take();
    const on = () => void take();
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, [policy, qc]);
  const [moving, setMoving] = useState(false);
  const [issuingAll, setIssuingAll] = useState(false);
  // The newest whole-stock file, issued by the button or by the desk itself in the morning, if from the last day
  // (offered below only while it still describes the stock, docs/96 V1).
  const newestBook = (statements.data?.books ?? []).find((b) => statements.dataUpdatedAt / 1000 - b.at < 86_400);
  async function downloadBook(file: string) {
    try {
      const text = await fetchKitText(policy, file);
      const a = document.createElement("a");
      a.href = URL.createObjectURL(new Blob([text], { type: "application/json" }));
      a.download = file.replace(/^book-/, "book-statement-");
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }
  // Every open deal's statement at one moment, in one file: what a lender financing the whole stock checks together
  // (docs/94 U1; POST /api/desk/statements/all).
  async function statementsForAll() {
    setIssuingAll(true);
    try {
      const { data, text } = await deskPost<{ kits: unknown[]; at: number }>("/api/desk/statements/all", policy, {});
      const a = document.createElement("a");
      a.href = URL.createObjectURL(new Blob([text], { type: "application/json" }));
      a.download = `book-statement-${data.at}.json`;
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
      toast.success(`${data.kits.length} statements signed at one moment and anchored once. The file is for your lender.`);
      await qc.invalidateQueries({ queryKey: ["desk-statements", policy] });
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setIssuingAll(false);
    }
  }
  const book = q.data;

  async function bringInLine() {
    setMoving(true);
    try {
      const post = async (extra: Record<string, unknown>) => (await deskPost<DeskOffsetReply>("/api/desk/offset", policy, extra)).data;
      let r: DeskOffsetReply;
      try {
        r = await post({});
      } catch (e) {
        // Past what the desk may move by itself, only the owner's passkey releases
        // the trade (docs/91 R6); it signs the target this page shows.
        if (!(e instanceof ApiError && e.status === 401) || !book) throw e;
        const issued = nowSecs();
        const target = book.exposure[0]?.offsetTarget ?? "0";
        const auth = await passkeyAssert(deskOffsetChallenge(book.chainId, policy, target, issued), ownerPub(tenant).pub, {
          kind: "offset",
          chainId: book.chainId,
          desk: policy,
          issued,
          target,
        });
        r = await post({ issued, auth });
      }
      // The read-back, or the engine's own read inside the action when the second read was refused.
      const back =
        r.readBack ??
        r.coins[0]?.action.match(/read_back: (-?[\d.]+)/)?.[1] ??
        null;
      const before = Math.abs(parseFloat(r.coins[0]?.before ?? "0"));
      const now = back != null ? Math.abs(parseFloat(back)) : null;
      // Only a short offsets (docs/89 M1, docs/91 R13): a long read back is never "in line".
      if (back != null && parseFloat(back) > 0) {
        toast.error(`The exchange reads a long of ${ozText(parseFloat(back))}: the offset must be a short. Nothing is protected until it is.`);
        refresh();
        return;
      }
      // A refusal is said as one (docs/89 M6): a HALT, a safety refusal, or a
      // read-back that is not the target is never "already in line".
      const action = r.coins[0]?.action ?? "";
      const halted = /^Halted \{ reason: "(.*)" \}$/.exec(action)?.[1];
      const held = /^None\("(.*)"\)$/.exec(action)?.[1];
      const target = r.target != null ? Math.abs(parseFloat(r.target)) : null;
      if (halted) {
        toast.error(
          `The offset is stopped by a safety check: ${halted}. Nothing more trades until a person clears it.`,
        );
        refresh();
        return;
      }
      if (held && held !== "inside band") {
        toast.warning(`Not moved: ${held}.`);
        refresh();
        return;
      }
      if (
        now != null &&
        target != null &&
        Math.abs(now - target) > Math.max(0.002, target * 0.05)
      ) {
        toast.warning(
          `Not in line: the exchange reads ${ozText(now)} short, the book wants ${ozText(target)}.`,
        );
        refresh();
        return;
      }
      toast.success(
        now == null
          ? "Offset step sent; the exchange did not answer the read-back. Try again in a minute."
          : now === before
            ? now === 0
              ? "The offset is flat, checked at the exchange."
              : `Already in line: short ${ozText(now)} on ${r.coins[0]?.coin ?? "PAXG"}, checked at the exchange.`
            : `Moved from ${ozText(before)} to ${ozText(now)} short on ${r.coins[0]?.coin ?? "PAXG"}, checked at the exchange.${r.live ? "" : " (Dry run: the service is not trading.)"}`,
      );
      refresh();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setMoving(false);
    }
  }

  if (q.error) {
    const owner = q.error instanceof ApiError && q.error.status === 403;
    return (
      <Page>
        <EmptyState
          icon={Package}
          title={owner ? "The desk is the owner's" : "The desk did not answer"}
        >
          {owner
            ? reserveToken.get(policy)
              ? "This device's key for the account was refused. Open the desk on the device that opened the account."
              : "Positions and prices stay with the owner: open the desk on the device that opened this account."
            : (q.error as Error).message}
        </EmptyState>
      </Page>
    );
  }
  if (!book || !cfg.data)
    return (
      <Page>
        <div className="space-y-4">
          <Skeleton className="h-10 w-2/3" />
          <Skeleton className="h-28 w-full rounded-xl" />
          <Skeleton className="h-64 w-full rounded-xl" />
        </div>
      </Page>
    );

  const ex = book.exposure[0];
  const heldG = grams(ex?.held);
  const baseG = grams(ex?.unhedgedBase);
  const target = parseFloat(ex?.offsetTarget ?? "0") || 0;
  // The venue's own read-back, taken by the service on every read of the book (never a trade).
  const v = book.venue;
  const readOk = !!v && !v.error && v.position != null;
  // Only a short offsets (docs/89 M1): a long position is shown as none.
  // A desk this service does not trade for (docs/89 C2): it books, nothing is offset,
  // so the tile says 0 rather than a target nobody will trade.
  const noVenue = !book.venueAccount;
  const shown = readOk ? Math.max(0, -parseFloat(v.position!)) : noVenue ? 0 : target;
  const share =
    heldG > 0 && readOk
      ? Math.min(100, (100 * shown * GRAMS_PER_OZ) / heldG)
      : null;
  // Each deal's latest statement (the list is newest first).
  const latest = new Map<string, DeskStatementRow>();
  for (const r of statements.data?.statements ?? [])
    if (!latest.has(r.deal)) latest.set(r.deal, r);
  const open = book.deals.filter((d) => grams(d.held) > 0).reverse();
  const closed = book.deals.filter((d) => grams(d.held) === 0).reverse();
  // The printed maximum comes only from a signed statement that still matches each open deal: same grams, same
  // steps (docs/96 V7), and the short read now is what the statements counted (docs/96 V4): an offset closed since
  // makes the signed figure wrong, whatever the grams.
  const current = open
    .map((d) => latest.get(d.id))
    .filter((r): r is DeskStatementRow => {
      const d = open.find((x) => x.id === r?.deal);
      return !!r && !!d && r.held === d.held && (r.steps ?? 0) === d.milestones.length;
    });
  const counted = current.reduce((a, r) => a + (parseFloat(r.offsetContracts ?? "NaN")), 0);
  // The lender's own tolerance (check e, 0.0005 oz) plus each figure's 6-place rounding (docs/99 W7). Only a fall
  // matters: an offset that grew since leaves the signed figure on the safe side (docs/99 W10).
  const tolOz = 0.0005 + 1e-6 * open.length;
  // Not read is not known: no verdict on the offset either way while the exchange has not answered.
  const unread = !readOk && !noVenue;
  const offsetMoved = open.length > 0 && current.length === open.length && !unread && !(shown + tolOz >= counted);
  // The whole-stock file stays on offer only while it lists every open deal at its grams and steps now, and the
  // offset it counted is still there (docs/96 V1): a lender would otherwise count gold sold since.
  const bookRows = newestBook ? (statements.data?.statements ?? []).filter((r) => r.at === newestBook.at) : [];
  const bookCounted = bookRows.reduce((a, r) => a + parseFloat(r.offsetContracts ?? "NaN"), 0);
  const latestBook =
    newestBook &&
    bookRows.length === open.length &&
    open.every((d) => bookRows.some((r) => r.deal === d.id && r.held === d.held && (r.steps ?? 0) === d.milestones.length)) &&
    !unread &&
    shown + tolOz >= bookCounted
      ? newestBook
      : null;
  const printed =
    open.length === 0
      ? 0
      : current.length === open.length && !offsetMoved
        ? current.reduce((a, r) => a + parseFloat(r.maxLossUsdc), 0)
        : null;
  const lender =
    open.length > 0 && printed != null
      ? current.reduce((a, r) => a + parseFloat(r.eligibleUsdc), 0)
      : null;
  // The fall the statements were printed at; the service's default when none says.
  const stress =
    (current.find((r) => r.stressBps != null)?.stressBps ?? STRESS * 100) / 100;
  const chain = cfg.data.chains.find((c) => c.key === tenant.chain);
  // A live estimate of the maximum loss from the book, unsigned (docs/95 P5): seeing your own risk is not a trust decision.
  const mark = (readOk && v.mark ? parseFloat(v.mark) : 0) || mid.data || 0;
  const heldOz = heldG / GRAMS_PER_OZ;
  // A long on the account loses with the gold: it adds to the loss, never offsets it (docs/96 V4).
  const long = readOk ? Math.max(0, parseFloat(v.position!)) : 0;
  // Unread, the live figure assumes nothing is offset (docs/99 W4): the target is what should be there, not what is.
  const liveShort = readOk ? shown : 0;
  const live = mark > 0 ? (heldOz - Math.min(liveShort, heldOz) + long) * mark * (stress / 100) + heldOz * mark * (BASIS / 100) : null;
  // No gold but a position left on the account: that position is the whole risk (docs/99 W5).
  const leftOz = readOk && heldG === 0 ? Math.abs(parseFloat(v.position!)) : 0;
  const leftRisk = mark > 0 ? leftOz * mark * (stress / 100) : 0;
  // How far gold can rise before the exchange closes the short (docs/95 P2).
  const liq = xs.data?.liquidationPx ?? null;
  const cover = liq && mark > 0 ? (liq / mark - 1) * 100 : null;
  // What needs the owner, in one place and first on the page (the surface rule): prices far from the market, nothing connected,
  // an offset waiting, thin cover. Each row says what and why, with the one tap that settles it where this page already has one.
  const bring = { label: "Bring in line", onClick: () => void bringInLine(), busy: moving };
  const needs: Need[] = [
    ...partyNeeds,
    ...open.flatMap((d) =>
      d.milestones
        .filter((m) => m.priceNote)
        .map((m) => ({ key: `price-${d.id}-${m.document}`, title: "Check a price", why: `${m.counterparty}'s price on ${d.id} is ${m.priceNote!.replace(/^priced /, "")}.` })),
    ),
    ...(noVenue && heldG > 0 ? [{ key: "no-venue", title: "Connect an exchange account", why: "Nothing is offset until one is connected.", action: { label: "Connect", href: "#venue" } }] : []),
    ...(readOk && Math.abs(shown - target) > 0.002 && !noVenue ? [{ key: "offset", title: "Bring the offset in line", why: `The offset is ${ozText(Math.abs(target - shown))} away from what the book needs.`, action: bring }] : []),
    ...(unread && printed != null && open.length > 0 ? [{ key: "unread", title: "The exchange has not answered", why: "The maximum loss shown cannot be checked against the offset right now. This page asks again every few seconds." }] : []),
    ...(offsetMoved
      ? [
          {
            key: "moved",
            title: "Issue new statements",
            why: `Your last statements counted ${ozText(counted || 0)} of offset; ${noVenue ? "no exchange account is connected now" : `the exchange shows ${ozText(shown)} now`}. The figures shown are live: issue new statements before your lender relies on the old ones.`,
            action: { label: "Issue statements", onClick: () => void statementsForAll(), busy: issuingAll },
          },
        ]
      : []),
    ...(long > 0 ? [{ key: "long", title: "The exchange account is long", why: `It is long ${ozText(long)}: a fall in gold costs you on it as well as on the stock.`, action: bring }] : []),
    ...(shown > 0 && cover != null && cover < stress ? [{ key: "cover", title: "Add funds at the exchange", why: `A ${stress} % rise in gold would reach the exchange's closing price (${usdRound(String(liq))} USDC/oz).` }] : []),
  ];

  return (
    <Page>
      <PageHeader
        title="Desk"
        meta={
          <Badge variant="outline">
            Gold · offset on {ex?.market ?? "PAXG"}
          </Badge>
        }
      />

      <div className="mb-5 space-y-3">
        <KeysNag tenant={tenant} onAdd={() => router.push("/?keys")} />
        <DeskInbox needs={needs} alsoWaiting={keysOpen} />
      </div>

      <p className="mb-5 max-w-[60ch] text-balance text-headline font-normal sm:text-2xl sm:leading-snug sm:tracking-tight">
        {heldG === 0 ? (
          <>
            {leftOz > 0.0005 ? (
              <>
                You hold no gold, but the exchange account is still {parseFloat(v!.position!) < 0 ? "short" : "long"} {ozText(leftOz)}: a {stress} %{" "}
                {parseFloat(v!.position!) < 0 ? "rise" : "fall"} in gold would cost you up to about{" "}
                <strong className="font-semibold">{usdRound(leftRisk.toFixed(6))} USDC</strong>. Bring the offset in line below.
              </>
            ) : unread ? (
              <>You hold no gold right now; the exchange has not answered, so whether a position is left on it is not known.</>
            ) : (
              <>You hold no gold right now: nothing is at risk{readOk ? " and the exchange account is flat" : ""}.</>
            )}
          </>
        ) : (
          <>
            You hold{" "}
            <strong className="font-semibold">{gramsText(heldG)}</strong> of
            gold
            {share != null ? (
              <>
                ,{" "}
                <strong className="font-semibold">
                  {share.toFixed(share >= 99.995 ? 0 : 2)} %
                </strong>{" "}
                of it offset
              </>
            ) : null}
            {printed != null ? (
              <>
                . A {stress} % fall costs you at most{" "}
                <strong className="font-semibold">
                  {usdRound(printed.toFixed(6))} USDC
                </strong>
                , and your lender would count{" "}
                <strong className="font-semibold">
                  {usdRound(lender!.toFixed(6))} USDC
                </strong>
                .
              </>
            ) : (
              <>
                {share == null && !noVenue ? (
                  // Not read is not none: a read that failed says so (docs/60 P-131).
                  <>; the exchange has not answered, so how much of it is offset is not known right now</>
                ) : share == null || share < 0.005 ? (
                  <> and <strong className="font-semibold">none of it is offset</strong></>
                ) : null}
                {live != null ? (
                  <>
                    . A {stress} % fall would cost you up to about <strong className="font-semibold">{usdRound(live.toFixed(6))} USDC</strong>
                    {unread ? " if nothing is offset" : ""} (not yet signed for your lender).
                  </>
                ) : (
                  <>. Issue a statement on a deal to print its maximum loss and what your lender would count.</>
                )}
              </>
            )}
          </>
        )}
      </p>

      <Stats className="mb-3">
        <Stat
          label="Held"
          value={gramsText(heldG)}
          sub={
            heldG > 0
              ? `${ozText(heldG / GRAMS_PER_OZ)} of gold`
              : "nothing in stock"
          }
        />
        <Stat
          label="Offset"
          value={gramsText(shown * GRAMS_PER_OZ)}
          meter={readOk && target > 0 ? shown / target : undefined}
          sub={
            noVenue
              ? "not connected to an exchange account"
              : readOk
                ? `${shown ? `${ozText(shown)} short on ${v.market}` : "flat"}, checked at the exchange${v.readAt ? ` ${clock(v.readAt)}` : ""}${baseG > 0 ? ` · ${gramsText(baseG)} kept unoffset` : ""}`
                : v?.error
                  ? "target; the exchange did not answer"
                  : target > 0
                    ? `short ${ex?.market} target; not checked yet`
                    : "nothing to offset"
          }
        />
        <Stat
          label={`Max loss at a ${stress} % ${leftOz > 0.0005 && parseFloat(v!.position!) < 0 ? "rise" : "fall"}`}
          value={leftOz > 0.0005 ? (mark > 0 ? `≈ ${usdRound(leftRisk.toFixed(6))}` : "—") : printed != null ? `${usdRound(printed.toFixed(6))}` : live != null ? `≈ ${usdRound(live.toFixed(6))}` : "—"}
          sub={
            leftOz > 0.0005
              ? "USDC: no gold, but a position is left on the exchange" // docs/100 X9
              : printed != null
              ? open.length
                ? "USDC, from your signed statement"
                : "USDC: nothing held"
              : live != null
                ? "USDC, live estimate: not yet signed for your lender"
                : "issue a statement below"
          }
        />
      </Stats>
      {!noVenue && xs.data && (
        <p className="mb-3 text-caption text-muted-foreground">
          The exchange account holds {usdRound(xs.data.accountValue.toFixed(6))} USDC
          {liq && cover != null && shown > 0
            ? `: gold can rise to ${usdRound(String(liq))} USDC/oz (+${cover.toFixed(0)} %) before the exchange closes the offset.`
            : xs.data.marginUsed > 0
              ? `, ${usdRound(xs.data.marginUsed.toFixed(6))} of it in use.`
              : ", none of it in use."}
        </p>
      )}
      {noVenue ? (
        <div id="venue" className="scroll-mt-20">
          <p className="mb-3 text-caption text-muted-foreground">
            This desk books every step but offsets nothing: no exchange account is
            connected to it on this Mac.
          </p>
          <VenueConnect policy={policy} chainId={book.chainId} pub={ownerPub(tenant).pub} onDone={refresh} />
        </div>
      ) : (
        <div className="mb-8 flex flex-wrap items-center gap-x-3 gap-y-1">
          <Button
            variant="ghost"
            size="sm"
            className="h-8 px-2"
            onClick={bringInLine}
            disabled={moving}
          >
            {moving ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <RefreshCw className="size-3.5" />
            )}{" "}
            Bring the offset in line now
          </Button>
          <VenueConnect policy={policy} chainId={book.chainId} pub={ownerPub(tenant).pub} onDone={refresh} />
          {open.length > 0 && (
            <Button variant="outline" size="sm" className="h-8" onClick={statementsForAll} disabled={issuingAll}>
              {issuingAll && <Loader2 className="size-3.5 animate-spin" />} Statement of your whole stock for your lender
            </Button>
          )}
          {latestBook && (
            <Button variant="ghost" size="sm" className="h-8" onClick={() => void downloadBook(latestBook.file)}>
              Download the {clock(latestBook.at)} statement of your whole stock
            </Button>
          )}
          {newestBook && !latestBook && open.length > 0 && !unread && (
            <p className="text-caption text-muted-foreground">
              The {clock(newestBook.at)} statement of your whole stock no longer matches it (a deal or the offset changed since): issue
              today&apos;s above.
            </p>
          )}
          <p className="text-caption text-muted-foreground">
            The desk moves it by itself inside a limit.{" "}
            <Why label="How?">
              It moves by itself while what it traded without you, since you last
              confirmed with your passkey, stays within{" "}
              {ex?.autoLimit?.replace(" g gold", " g") ?? "1,000 g"}
              {grams(ex?.autoMoved ?? "0") > 0 ? ` (${gramsText(grams(ex?.autoMoved ?? "0"))} so far)` : ""}; past
              that it waits for this tap and your passkey. It never borrows more than 2× at the exchange.
            </Why>
          </p>
        </div>
      )}

      <Columns
        primary={
          book.deals.length === 0 ? (
            <Section title="Deals" icon={Package}>
              <EmptyState icon={Package} title="No deal yet">
                A deal starts when your supplier signs the purchase, or when you
                enter it yourself.
              </EmptyState>
            </Section>
          ) : (
            <>
              {open.map((d) => (
                <DealCard
                  key={d.id}
                  deal={d}
                  policy={policy}
                  cfg={cfg.data!}
                  row={latest.get(d.id)}
                  shortNow={readOk || noVenue ? shown : null}
                  asks={asksFor(d, book.parties, cfg.data!.publicUrl, policy, chain?.chainId ?? book.chainId, tenant.name)}
                />
              ))}
              {open.length === 0 && (
                <p className="text-sm text-muted-foreground">
                  No open deal: every purchase on this desk has been sold.
                </p>
              )}
              {closed.length > 0 && (
                <details className="group">
                  <summary className="cursor-pointer text-sm font-medium">
                    {closed.length} closed deal{closed.length === 1 ? "" : "s"},
                    sold
                  </summary>
                  <div className="mt-4 space-y-6">
                    {closed.map((d) => (
                      <DealCard
                        key={d.id}
                        deal={d}
                        policy={policy}
                        cfg={cfg.data!}
                        row={latest.get(d.id)}
                      />
                    ))}
                  </div>
                </details>
              )}
            </>
          )
        }
        side={
          <>
            <Parties
              policy={policy}
              chain={tenant.chain}
              chainId={chain?.chainId ?? book.chainId}
              parties={book.parties}
              onDone={refresh}
              pub={ownerPub(tenant).pub}
              publicUrl={cfg.data!.publicUrl}
              trader={tenant.name}
              inInbox
            />
            <BaseStock
              policy={policy}
              chainId={chain?.chainId ?? book.chainId}
              pub={ownerPub(tenant).pub}
              base={ex?.unhedgedBase ?? "0"}
              onDone={refresh}
            />
            <EnterStep
              policy={policy}
              chainId={chain?.chainId ?? book.chainId}
              pub={ownerPub(tenant).pub}
              deals={book.deals}
              onDone={refresh}
            />
          </>
        }
      />
    </Page>
  );
}
