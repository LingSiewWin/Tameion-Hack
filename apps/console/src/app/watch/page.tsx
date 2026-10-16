"use client";

import { useQuery } from "@tanstack/react-query";
import { Loader2, ReceiptText, Search } from "lucide-react";
import { Page } from "@/components/layout/page";
import { EmptyState, Section } from "@/components/layout/section";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { Suspense, useEffect, useRef, useState } from "react";
import { TopBar } from "@/components/brand";
import { Seal } from "@/components/seal";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Identicon } from "@/components/account/payee";
import { dueText, everyText, fetchConfig, looksEnded, post, tail, usd } from "@/lib/tameion";
import type { WatchJob, WatchReport } from "@/lib/types";

const VERDICT: Record<string, string> = {
  pay: "would pay, once you approve this payee",
  ask: "would ask you first",
  "pay-or-ask": "due now",
  wait: "not due yet",
};

export default function WatchPage() {
  // useSearchParams opts this page out of the static prerender; the fallback
  // is what the export ships until the browser takes over.
  return (
    <Suspense fallback={<TopBar />}>
      <Watch />
    </Suspense>
  );
}

function Watch() {
  const sp = useSearchParams();
  const cfg = useQuery({ queryKey: ["config"], queryFn: fetchConfig, staleTime: 300_000 });
  const [chain, setChain] = useState(() => sp.get("chain") ?? "arc-testnet");
  const [address, setAddress] = useState(() => sp.get("address") ?? "");
  const [state, setState] = useState<{ kind: "idle" } | { kind: "reading"; pct: number } | { kind: "done"; r: WatchReport } | { kind: "error"; text: string }>({ kind: "idle" });


  async function read() {
    if (!/^0x[0-9a-fA-F]{40}$/.test(address.trim())) return setState({ kind: "error", text: "That is not an address." });
    setState({ kind: "reading", pct: 0 });
    try {
      const r = await post<{ id: string }>("/api/watch", { chain, address: address.trim(), days: 90 });
      for (let n = 0; n < 240; n++) {
        const j: WatchJob = await (await fetch(`/api/watch/${r.id}`)).json();
        if (j.status === "done" && j.report) return setState({ kind: "done", r: j.report });
        if (j.status === "failed") throw new Error(j.error || "the read failed");
        const p = j.progress || {};
        setState({ kind: "reading", pct: p.totalBlocks && p.scannedBlocks ? Math.round((100 * p.scannedBlocks) / p.totalBlocks) : 0 });
        await new Promise((res) => setTimeout(res, 1000));
      }
    } catch (e) {
      setState({ kind: "error", text: (e as Error).message });
    }
  }

  // A shared link (?address=0x…&chain=…) reads at once, as the first page's Watch did.
  const linked = useRef(false);
  useEffect(() => {
    if (linked.current || !/^0x[0-9a-fA-F]{40}$/.test(sp.get("address") ?? "")) return;
    linked.current = true;
    void read();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once, for the address the link carried
  }, []);

  const r = state.kind === "done" ? state.r : null;
  return (
    <>
      <TopBar>
        <Link href="/" className="px-2 text-sm text-muted-foreground hover:text-foreground">
          Open an account
        </Link>
      </TopBar>
      <Page className="max-w-4xl">
        <h1 className="text-balance text-3xl font-semibold tracking-tight sm:text-4xl">What does this wallet owe?</h1>
        <p className="mt-3 max-w-[60ch] text-muted-foreground">
          Paste any address. The agent reads its last 90 days of USDC, finds what repeats, and says what it would pay and what it would ask about. Read-only: no key, nothing
          sent.
        </p>
        <div className="mt-8 grid gap-3 rounded-xl border bg-card p-4 shadow-card sm:grid-cols-[12rem_1fr_auto] sm:items-end sm:p-5">
          <div className="grid gap-2">
            <Label>Chain</Label>
            <Select value={chain} onValueChange={setChain}>
              <SelectTrigger className="h-11">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {(cfg.data?.chains ?? []).map((c) => (
                  <SelectItem key={c.key} value={c.key}>
                    {c.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="grid gap-2">
            <Label htmlFor="addr">Address</Label>
            <Input id="addr" className="h-11 font-mono" placeholder="0x…" value={address} onChange={(e) => setAddress(e.target.value)} onKeyDown={(e) => e.key === "Enter" && read()} />
          </div>
          <Button className="h-11" onClick={read} disabled={state.kind === "reading"}>
            {state.kind === "reading" ? <Loader2 className="size-4 animate-spin" /> : <Search className="size-4" />} Read it
          </Button>
        </div>
        {state.kind === "error" && <p className="mt-4 text-sm text-destructive">{state.text}</p>}
        {state.kind === "reading" && (
          <div className="mt-10 flex justify-center">
            <Seal reading fraction={0} size={200}>
              <span className="figures text-3xl font-semibold">{state.pct ? `${state.pct}%` : "…"}</span>
              <span className="mt-1 text-xs text-muted-foreground">reading 90 days</span>
            </Seal>
          </div>
        )}
        {r && (
          <section className="mt-10 space-y-6">
            <dl className="grid grid-cols-2 divide-x divide-y rounded-xl border bg-card shadow-card sm:grid-cols-4 sm:divide-y-0">
              {[
                ["Payments read", String(r.transfers)],
                [
                  "Bills found",
                  String(r.obligations.length),
                  // Same rule as the rows' "last paid …" and Start (lib/tameion looksEnded).
                  ((n) => (n ? `${n} not paid lately` : ""))(r.obligations.filter((o) => looksEnded(o.nextDue, o.intervalSecs)).length),
                ],
                ["Next 30 days", `${usd(r.liquidity30d)} USDC`],
                ["Holds", `${usd(r.balance)} USDC`],
              ].map(([k, v, note]) => (
                <div key={k} className="px-4 py-3.5">
                  <dt className="text-caption text-muted-foreground">{k}</dt>
                  <dd className="figures mt-1 text-xl font-semibold">{v}</dd>
                  {note && <dd className="text-caption text-muted-foreground">{note}</dd>}
                </div>
              ))}
            </dl>
            <Section title="Bills it found" icon={ReceiptText} flush>
              {r.obligations.length === 0 ? (
                <EmptyState icon={ReceiptText} title="No recurring bill in these 90 days" />
              ) : (
                <ul className="divide-y">
                  {r.obligations.map((o) => (
                    <li key={o.counterparty + o.amount} className="flex flex-wrap items-baseline gap-x-4 gap-y-1 px-4 py-3 text-sm sm:px-5">
                      <span className="figures w-24 font-semibold">{usd(o.amount)}</span>
                      <span className="inline-flex items-center gap-1.5 font-medium">
                        <Identicon addr={o.counterparty} />
                        Payee ••{tail(o.counterparty)}
                      </span>
                      <span className="text-muted-foreground">{everyText(o.intervalSecs, o.every)}</span>
                      {o.nextDue ? <span className="text-muted-foreground">{dueText(o.nextDue, o.intervalSecs)}</span> : null}
                      {o.verdict && (
                        <Badge variant={o.verdict === "wait" ? "secondary" : "outline"} className="ml-auto">
                          {VERDICT[o.verdict] ?? o.verdict}
                        </Badge>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </Section>
            <p className="text-sm text-muted-foreground">
              Like what it found?{" "}
              <Link className="font-medium text-foreground underline underline-offset-4" href={`/?address=${r.address}`}>
                Let it pay these inside a limit you set
              </Link>
              .
            </p>
          </section>
        )}
      </Page>
    </>
  );
}
