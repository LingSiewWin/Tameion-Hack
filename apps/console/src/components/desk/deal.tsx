"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { BadgeCheck, Download, FileText, Loader2, MessageSquareText, Package, PenLine, Send, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { NameAt } from "@/components/desk/trust";
import { CopyRow, KeyValue, Section } from "@/components/layout/section";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { lenderLink, shareLink } from "@/lib/links";
import { codeText, interopName, nameAt } from "@/lib/names";
import { currentTenant, deskPost, fetchKitText, grams, gramsText, noAddr, ozText, tail, usdRound } from "@/lib/tameion";
import { cn } from "@/lib/utils";
import type { Cfg, DeskClass, DeskDeal, DeskKit, DeskMilestone, DeskStatementRow } from "@/lib/types";

/** A lender's class in a lender's words (docs/84 §6: stock and transit count,
 *  work in process does not, consigned goods have their own rate). */
export const CLASS: Record<DeskClass, string> = { eligible: "eligible", consigned: "consigned", ineligible: "not eligible" };

/** The gold path (docs/84 §11.1). Shipping is optional: gold often flies straight into a vault. */
const STEPS = ["Bought", "Shipped", "In stock", "Fabrication or consigned", "Sold"] as const;
/** The same steps where five columns are narrow (a phone). */
const SHORT = ["Bought", "Shipped", "In stock", "Fab. / consigned", "Sold"];
const STEP_OF: Record<DeskMilestone["kind"], number> = { bought: 0, shipped: 1, arrived: 2, returned: 2, to_fabrication: 3, consigned: 3, sold: 4 };
const NOW_OF: Record<string, number> = { "bought, not delivered": 0, "in transit": 1, "in stock": 2, "in fabrication": 3, consigned: 3, sold: 4 };

const what = (m: DeskMilestone) => {
  const g = gramsText(grams(m.quantity));
  const usdc = m.usdc ? ` for ${usdRound(m.usdc)} USDC` : "";
  return {
    bought: `Bought ${g}${usdc}`,
    shipped: `${g} shipped`,
    arrived: `${g} into stock`,
    to_fabrication: `${g} sent to fabrication`,
    consigned: `${g} consigned to a retailer`,
    returned: `${g} back into stock`,
    sold: `Sold ${g}${usdc}`,
  }[m.kind];
};
const timeText = (at: number) => new Date(at * 1000).toLocaleString(undefined, { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });

/** `row` is the deal's latest statement from the service's list, if any. */
export function DealCard({
  deal,
  policy,
  cfg,
  row,
  asks = [],
  shortNow = null,
}: {
  deal: DeskDeal;
  policy: string;
  cfg: Cfg;
  row?: DeskStatementRow;
  /** The account's short read now, oz; null when not read (docs/96 V4). */
  shortNow?: number | null;
  /** Prefilled signing links for the next step each counterparty may sign (docs/95 P3). */
  asks?: { label: string; url: string }[];
}) {
  // The desk's chain, for `name@chain` (docs/105): the card shows the account this browser has open.
  const chainId = cfg.chains.find((c) => c.key === currentTenant()?.chain)?.chainId ?? 0;
  const reached = new Set(deal.milestones.map((m) => STEP_OF[m.kind]));
  const now = NOW_OF[deal.stage] ?? 0;
  const last = Math.max(-1, ...reached);
  const open = grams(deal.held) > 0;
  return (
    <Section
      title={deal.id}
      icon={Package}
      description={`${gramsText(grams(deal.bought))} of gold from ${deal.supplier} for ${usdRound(deal.cost)} USDC`}
      action={<Badge variant={open ? "outline" : "secondary"}>{open ? `${gramsText(grams(deal.held))} ${deal.stage}` : "sold, closed"}</Badge>}
      flush
    >
      <ol className="grid grid-cols-5 gap-1 px-4 pt-4 sm:px-5" aria-label="Where the gold is">
        {STEPS.map((s, i) => {
          const done = reached.has(i);
          const skipped = !done && i < last;
          const current = open && i === now;
          return (
            <li key={s} className="flex flex-col items-center gap-1.5 text-center" aria-current={current ? "step" : undefined}>
              <div className="flex w-full items-center">
                <span className={cn("h-px flex-1", i === 0 ? "bg-transparent" : done || skipped || i <= last ? "bg-protect/50" : "bg-border")} />
                <span
                  className={cn(
                    "grid size-3.5 shrink-0 place-items-center rounded-full border-2",
                    done ? "border-protect bg-protect" : skipped ? "border-dashed border-muted-foreground/50" : "border-border bg-card",
                    current && "ring-4 ring-protect/20",
                  )}
                />
                <span className={cn("h-px flex-1", i === STEPS.length - 1 ? "bg-transparent" : i < last ? "bg-protect/50" : "bg-border")} />
              </div>
              <span className={cn("break-words text-xs leading-tight sm:text-caption", done ? "font-medium text-foreground" : "text-muted-foreground")}>
                <span className="sm:hidden">{SHORT[i]}</span>
                <span className="hidden sm:inline">{s}</span>
                {skipped && <span className="block">skipped</span>}
              </span>
            </li>
          );
        })}
      </ol>

      {deal.byStage.length > 0 && (
        <ul className="flex flex-wrap gap-2 px-4 pt-4 sm:px-5" aria-label="Where it is, as a lender counts it">
          {deal.byStage.map((b) => (
            <li key={b.stage} className="flex items-center gap-2 rounded-lg border bg-background px-3 py-1.5 text-sm">
              <span className="figures font-medium">{gramsText(grams(b.grams))}</span>
              <span className="text-muted-foreground">{b.words}</span>
              <Badge variant={b.class === "eligible" ? "default" : b.class === "consigned" ? "secondary" : "outline"}>{CLASS[b.class]}</Badge>
            </li>
          ))}
        </ul>
      )}

      <ul className="mt-3 divide-y border-t" aria-label="Every step, and who vouched for it">
        {deal.milestones.map((m, i) => (
          <li key={i} className="flex items-start gap-3 px-4 py-3 sm:px-5">
            {m.signed ? <BadgeCheck aria-hidden className="mt-0.5 size-4 shrink-0 text-protect" /> : <PenLine aria-hidden className="mt-0.5 size-4 shrink-0 text-muted-foreground" />}
            <div className="min-w-0">
              <p className="figures text-sm font-medium">{what(m)}</p>
              <p className="text-caption text-muted-foreground">
                {m.signed ? (
                  <span className="font-medium text-protect">
                    Signed by <NameAt name={m.counterparty} address={m.signer} chainId={chainId} />
                  </span>
                ) : (
                  <span>Entered by you, unsigned{m.counterparty ? ` (${m.counterparty} has not signed it)` : ""}</span>
                )}
                {" · "}
                {m.document} · {timeText(m.at)}
              </p>
              {m.priceNote && (
                <p className="mt-1 flex items-start gap-1.5 text-caption font-medium text-attention">
                  <TriangleAlert aria-hidden className="mt-0.5 size-3.5 shrink-0" />
                  <span>This contract is {m.priceNote}.</span>
                </p>
              )}
            </div>
          </li>
        ))}
      </ul>

      {open && asks.length > 0 && (
        <div className="border-t px-4 py-3 sm:px-5">
          <p className="text-caption text-muted-foreground">Ask for the next step — each link opens with this deal filled in:</p>
          <div className="mt-2 grid gap-2 sm:grid-cols-2">
            {asks.map((a) => (
              <CopyRow key={a.url} value={a.url} label={a.label} shown={a.label} />
            ))}
          </div>
        </div>
      )}
      <Statement deal={deal} policy={policy} cfg={cfg} row={row} shortNow={shortNow} />

      <details className="border-t px-4 py-3 text-caption text-muted-foreground sm:px-5">
        <summary className="cursor-pointer font-medium text-foreground">Who signed, by address</summary>
        <ul className="mt-2 space-y-1 font-mono">
          {deal.milestones.map((m, i) => (
            <li key={i} className="break-all">
              {m.document}: {m.signed ? `${nameAt(m.counterparty, chainId)} ${interopName(m.signer, chainId)}` : "unsigned (your own entry)"}
            </li>
          ))}
        </ul>
      </details>
    </Section>
  );
}

function Statement({ deal, policy, cfg, row, shortNow }: { deal: DeskDeal; policy: string; cfg: Cfg; row?: DeskStatementRow; shortNow?: number | null }) {
  const qc = useQueryClient();
  const [busy, setBusy] = useState(false);
  // The kit exactly as the service stored it: what is shown is what is downloaded.
  const kitQ = useQuery({ queryKey: ["desk-kit", policy, row?.file], queryFn: () => fetchKitText(policy, row!.file), enabled: !!row, staleTime: Infinity });
  const text = kitQ.data;
  const kept = text ? { kit: JSON.parse(text) as DeskKit, text } : undefined;
  const s = kept?.kit.signed.statement;
  const open = grams(deal.held) > 0;

  async function issue() {
    setBusy(true);
    try {
      const { data, text } = await deskPost<DeskKit>("/api/desk/statement", policy, { deal: deal.id });
      // The service stores the same bytes as `<deal>-<at>.lender.json`; seed it so the list's new row needs no second read.
      qc.setQueryData(["desk-kit", policy, `${deal.id}-${data.signed.statement.at}.lender.json`], text);
      await qc.invalidateQueries({ queryKey: ["desk-statements", policy] });
      toast.success("Statement signed and anchored. Download it for your lender.");
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  // The statement as a link (D14 of docs/102, docs/105): the share sheet where there is one, the clipboard elsewhere.
  async function send() {
    if (!kept || !s) return;
    try {
      const url = await lenderLink(kept.text, cfg.publicUrl || location.origin, currentTenant()?.name);
      const how = await shareLink(url, `Statement ${s.deal}`, `My statement for ${s.deal}: open it to check it yourself.`);
      if (how === "copied") toast.success("Statement link copied: paste it to your lender. They open it and check it in their browser.");
      if (how === "shared") toast.success("Statement sent.");
    } catch (e) {
      toast.error((e as Error).message);
    }
  }

  function download() {
    if (!kept || !s) return;
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([kept.text], { type: "application/json" }));
    a.download = `${s.deal}-statement-${s.at}.json`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
  }

  const issueButton = (
    <Button variant={s ? "outline" : "default"} size="sm" onClick={issue} disabled={busy || !open}>
      {busy ? <Loader2 className="size-4 animate-spin" /> : <FileText className="size-4" />}
      {s ? "Issue a new statement" : "Statement for your lender"}
    </Button>
  );

  if (row && !s)
    return (
      <p className="border-t px-4 py-4 text-caption text-muted-foreground sm:px-5">
        {kitQ.error ? `The last statement did not load: ${(kitQ.error as Error).message}` : "Reading the last statement…"}
      </p>
    );
  if (!s)
    return (
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-t px-4 py-4 sm:px-5">
        {issueButton}
        <p className="text-caption text-muted-foreground">
          {open ? "What a lender would count against this deal, signed and anchored on chain so they can check it without asking you." : "Nothing left to finance: the deal is sold."}
        </p>
      </div>
    );

  // A step since the statement (a move to the workshop, a return) changes what a lender may count, even at the same grams (docs/96 V7).
  const stale = s.held !== deal.held || (s.milestones?.length ?? 0) !== deal.milestones.length;
  // The offset it counts is no longer on the account (the lender's check e would fail it; docs/96 V4).
  const offsetGone = shortNow != null && shortNow + 0.0005 < Math.abs(parseFloat(s.offsetContracts));
  // Gold that is sold is not financed: no figure, no download (docs/95 P1).
  if (!open) {
    const soldAt = Math.max(...deal.milestones.map((m) => m.at));
    return (
      <p className="border-t px-4 py-4 text-sm sm:px-5">
        <strong>Sold {timeText(soldAt)} — nothing left to finance.</strong>{" "}
        <span className="text-muted-foreground">If your lender has the statement of {timeText(s.at)}, tell them this deal is closed.</span>
      </p>
    );
  }
  const anchorChain = cfg.chains.find((c) => c.chainId === kept.kit.anchor.chainId);
  const signer = cfg.statementSigner ?? kept.kit.signed.signer;
  return (
    <div className="border-t bg-protect-soft/30 px-4 py-4 sm:px-5">
      <p className="text-title">
        Your lender would count <strong className="figures">{usdRound(s.eligibleUsdc)} USDC</strong> against this deal.
      </p>
      <p className="mt-0.5 text-caption text-muted-foreground">
        Issued {timeText(s.at)}
        {stale && <span className="font-medium text-attention"> · from before the last step: issue a new one before you send it</span>}
        {!stale && offsetGone && <span className="font-medium text-attention"> · the offset it counts is not on the exchange now: issue a new one before you send it</span>}
      </p>
      <dl className="mt-3 divide-y">
        <KeyValue label="Held" value={`${gramsText(grams(s.held))} ${s.stage}, worth ${usdRound(s.valueUsdc)} USDC`} />
        <KeyValue label="Eligible" value={`${gramsText(grams(s.eligible.grams))} · ${usdRound(s.eligible.valueUsdc)} USDC`} />
        <KeyValue label="Not eligible (work in process, not delivered)" value={`${gramsText(grams(s.ineligible.grams))} · ${usdRound(s.ineligible.valueUsdc)} USDC`} />
        <KeyValue label="Consigned (the lender sets its own rate)" value={`${gramsText(grams(s.consigned.grams))} · ${usdRound(s.consigned.valueUsdc)} USDC`} />
        <KeyValue label="Offset" value={`${(Number(s.offsetShareBps) / 100).toFixed(2)} % · short ${ozText(Math.abs(parseFloat(s.offsetContracts)))} ${s.market}`} />
        <KeyValue label={`Maximum loss at a ${s.stressBps / 100} % fall (+${s.basisBps / 100} % basis)`} value={`${usdRound(s.maxLossUsdc)} USDC`} />
        <KeyValue label="Eligible after that loss" value={`${usdRound(s.eligibleUsdc)} USDC`} />
        {s.fundingUsdc != null && (
          <KeyValue
            label="Cost of the offset so far (funding)"
            value={parseFloat(s.fundingUsdc) > 0 ? `${usdRound(String(-parseFloat(s.fundingUsdc)))} USDC: the market paid you` : `${usdRound(String(-parseFloat(s.fundingUsdc)))} USDC`}
          />
        )}
        <KeyValue label={`Gold price used (${s.market} mark)`} value={`${usdRound(s.markUsdc)} USDC/oz`} />
      </dl>
      <Explain policy={policy} deal={deal.id} at={s.at} />
      <p className="mt-3 text-sm text-muted-foreground">Who controls the offset: {noAddr(s.hedgeControl)}.</p>
      <div className="mt-4 flex flex-wrap gap-2">
        {!stale && !offsetGone && (
          <>
            <Button size="sm" onClick={() => void send()}>
              <Send className="size-4" /> Send to lender
            </Button>
            <Button size="sm" variant="ghost" onClick={download}>
              <Download className="size-4" /> Download the file
            </Button>
          </>
        )}
        {issueButton}
      </div>
      <p className="mt-3 text-caption text-muted-foreground">
        Your lender opens the link and checks it in their browser, against the chain and the exchange — not against you. The first time, they call you to
        confirm two codes: desk <span className="figures font-mono font-semibold text-foreground">{codeText(policy, kept.kit.anchor.chainId)}</span> and statement
        key <span className="figures font-mono font-semibold text-foreground">{codeText(signer, kept.kit.anchor.chainId)}</span>.
      </p>
      <details className="mt-4 text-caption text-muted-foreground">
        <summary className="cursor-pointer font-medium text-foreground">How your lender checks it</summary>
        <div className="mt-2 max-w-sm">
          <CopyRow value={signer} label="the statement key" shown={`Statement key ••${tail(signer)}`} />
        </div>
        <p className="mt-2">
          One command, trusting only the chain and the exchange&apos;s public record (needs Foundry&apos;s cast). Your lender should confirm the
          exchange account and each counterparty&apos;s address themselves; without those pins the check says &quot;consistent, not pinned&quot;:
        </p>
        <pre className="mt-1 overflow-x-auto rounded-md border bg-background p-2 font-mono text-xs">
          python3 scripts/verify_desk.py {`${s.deal}-statement-${s.at}.json`} --signer {signer} --desk {policy} --venue {s.venueAccount}{" "}
          {(kept.kit.parties ?? []).map((p) => `--party '${p.name.replace(/'/g, "'\\''")}=${p.address}'`).join(" ")}
        </pre>
        <p className="mt-2">What it assumes: {s.assumptions}</p>
        <p className="mt-2 break-all font-mono">
          Statement key {signer} · anchored by {kept.kit.anchor.from} in {kept.kit.anchor.tx}
        </p>
        {anchorChain && (
          <a className="mt-1 inline-block font-medium text-foreground underline underline-offset-4" href={`${anchorChain.explorer}/tx/${kept.kit.anchor.tx}`} target="_blank" rel="noreferrer">
            See the anchor on {anchorChain.name}
          </a>
        )}
      </details>
    </div>
  );
}

/** The statement in words, written by the model on this Mac (docs/83 D6). The model
 *  writes the words and names the numbers; the service puts the numbers in from the
 *  signed statement, and sets the text aside if it wrote a number of its own. */
function Explain({ policy, deal, at }: { policy: string; deal: string; at: number }) {
  const [r, setR] = useState<{ text: string; by: string; ms: number; rejected?: string; statementAt?: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const shown = r && r.statementAt === at ? r : null;
  async function explain() {
    setBusy(true);
    try {
      const { data } = await deskPost<{ text: string; by: string; ms: number; rejected?: string; statementAt?: number }>("/api/desk/explain", policy, { deal });
      setR(data);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="mt-3 rounded-md border bg-background/60 px-3 py-2.5">
      {shown ? (
        <>
          <p className="text-sm">{shown.text}</p>
          <p className="mt-1 text-caption text-muted-foreground">
            {shown.by.startsWith("ollama")
              ? `Written by ${shown.by.replace("ollama:", "")} on this Mac in ${(shown.ms / 1000).toFixed(1)} s; nothing left the machine. The figures are the signed statement's, put in by code; the sentences are the model's and are not checked.`
              : `A plain summary: the model on this Mac was set aside (${shown.rejected ?? "no answer"}).`}
          </p>
        </>
      ) : (
        <Button variant="ghost" size="sm" className="-mx-2 h-8" onClick={explain} disabled={busy}>
          {busy ? <Loader2 className="size-4 animate-spin" /> : <MessageSquareText className="size-4" />}
          {busy ? "Writing it on this Mac…" : "Explain it in words"}
        </Button>
      )}
    </div>
  );
}
