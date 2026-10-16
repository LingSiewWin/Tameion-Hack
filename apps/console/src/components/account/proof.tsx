"use client";

import { useQuery } from "@tanstack/react-query";
import { CheckCircle2, ExternalLink, ShieldCheck, XCircle } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Rows, Row } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { ago, authHeaders, getJson } from "@/lib/tameion";
import { type AnchoredRecord, type CheckResult, checkRecord, NODES } from "@/lib/verify";
import { useAccount } from "./ctx";

type Records = { chain?: string | null; records: AnchoredRecord[] };

/** "Check it yourself": the agent's record rebuilt in this browser and compared with
 *  what two public nodes of the account's chain hold (lib/verify.ts). The service
 *  hands over the events; the browser and the chain give the verdict. */
export function ProofDialog({ children }: { children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>{children}</DialogTrigger>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Check the agent&apos;s record yourself</DialogTitle>
          <DialogDescription>
            After each check the agent writes down every decision and puts a fingerprint of that record on chain. Your
            browser rebuilds the fingerprint from the decisions and asks two public nodes whether your account&apos;s agent
            put exactly that on chain. For a check that trusts nothing on this site, run the command below from your own
            copy of the code.
          </DialogDescription>
        </DialogHeader>
        {open && <RecordList />}
        {open && <StatementLinks />}
      </DialogContent>
    </Dialog>
  );
}

/** This week's signed statement (docs/76 §1d): the week's paid, refused and needs-you lines, the record they
 *  stand on and an EIP-712 signature by the service's statement key. JSON for an agent or
 *  `scripts/verify_statement.sh`, CSV for an accountant. Fetched, not linked, so a locked account's key goes along. */
function StatementLinks() {
  const { tenant } = useAccount();
  const [busy, setBusy] = useState<"json" | "csv" | null>(null);
  async function download(format: "json" | "csv") {
    setBusy(format);
    try {
      const res = await fetch(`/api/level1/statement?policy=${tenant.policy}${format === "csv" ? "&format=csv" : ""}`, {
        headers: authHeaders(tenant.policy),
      });
      if (!res.ok) throw new Error(`the service did not hand over the statement (${res.status})`);
      const url = URL.createObjectURL(await res.blob());
      const a = document.createElement("a");
      a.href = url;
      a.download = `tameion-statement-${tenant.policy.slice(0, 10)}.${format}`;
      a.click();
      URL.revokeObjectURL(url);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  }
  const link = "font-medium text-foreground underline-offset-4 hover:underline disabled:opacity-50";
  return (
    <p className="text-caption text-muted-foreground">
      Download this week&apos;s statement — every line, the record it stands on, and a signature anyone can check with{" "}
      <code className="font-mono">scripts/verify_statement.sh</code>:{" "}
      <button type="button" className={link} onClick={() => void download("json")} disabled={busy !== null}>
        {busy === "json" ? "Fetching…" : "JSON"}
      </button>
      {" · "}
      <button type="button" className={link} onClick={() => void download("csv")} disabled={busy !== null}>
        {busy === "csv" ? "Fetching…" : "CSV"}
      </button>
    </p>
  );
}

function RecordList() {
  const { tenant, cfg } = useAccount();
  const q = useQuery({
    queryKey: ["records", tenant.policy],
    queryFn: () => getJson<Records>(`/api/level1/records?policy=${tenant.policy}`, tenant.policy),
    staleTime: 30_000,
  });
  // The account's own chain, not the service's word for it (review 2026-10-15).
  const chain = tenant.chain;
  const explorer = cfg.chains.find((c) => c.key === chain)?.explorer;
  const records = (q.data?.records ?? []).slice(-6).reverse();

  if (q.isPending) return <p className="text-sm text-muted-foreground">Reading the records…</p>;
  if (q.isError || !records.length)
    return (
      <p className="text-sm text-muted-foreground">
        No record is on chain yet. The agent writes one after its first check of this account.
      </p>
    );
  return (
    <div className="space-y-4">
      <Rows className="-mx-6 border-y">
        {records.map((r) => (
          <RecordRow key={r.root} record={r} chain={chain} explorer={explorer} />
        ))}
      </Rows>
      <p className="text-caption text-muted-foreground">
        Nodes asked: {(NODES[chain] ?? []).map((u) => new URL(u).host).join(" and ") || "none known for this chain"}. From
        a terminal, the same check: <code className="break-all font-mono">scripts/verify_tenant.sh {origin()} {tenant.policy}</code>.{" "}
        <a className="font-medium text-foreground underline-offset-4 hover:underline" href="/level1">
          Every receipt
        </a>
      </p>
    </div>
  );
}

const origin = () => (typeof location === "undefined" ? "" : location.origin);

function RecordRow({ record: r, chain, explorer }: { record: AnchoredRecord; chain: string; explorer?: string }) {
  const { tenant } = useAccount();
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<CheckResult | { error: string } | null>(null);

  async function check() {
    setBusy(true);
    setResult(null);
    try {
      const res = await fetch(`/api/level1/record?policy=${tenant.policy}&root=${r.root}`, { headers: authHeaders(tenant.policy) });
      if (!res.ok) throw new Error(`the service did not hand over this record (${res.status})`);
      setResult(await checkRecord(r, await res.text(), chain, tenant.policy as `0x${string}`));
    } catch (e) {
      setResult({ error: (e as Error).message });
    } finally {
      setBusy(false);
    }
  }

  return (
    <Row
      title={
        <span>
          {ago(r.at)} · {r.events} decisions
        </span>
      }
      actions={
        <>
          {explorer && r.anchorTx && (
            <a
              className="inline-flex items-center gap-1 text-sm text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
              href={`${explorer}/tx/${r.anchorTx}`}
              target="_blank"
              rel="noreferrer"
            >
              On chain <ExternalLink className="size-3" />
            </a>
          )}
          <Button size="sm" variant="outline" onClick={() => void check()} disabled={busy} aria-label={`Check the record from ${ago(r.at)}`}>
            <ShieldCheck className="size-4" /> {busy ? "Checking…" : "Check"}
          </Button>
        </>
      }
      detail={result && <Verdict result={result} />}
    />
  );
}

function Verdict({ result }: { result: CheckResult | { error: string } }) {
  if ("error" in result)
    return (
      <span className="flex items-start gap-1.5 text-destructive">
        <XCircle className="mt-0.5 size-4 shrink-0" /> Could not check: {result.error}.
      </span>
    );
  if (result.ok)
    return (
      <span className="flex items-start gap-1.5 text-protect">
        <CheckCircle2 className="mt-0.5 size-4 shrink-0" />
        Matches. Rebuilt here from {result.count} decisions; both public nodes hold the same fingerprint (
        {result.nodes[0]?.how}).
      </span>
    );
  const bad = !result.matchesClaim
    ? "the decisions handed over do not rebuild the fingerprint the service named"
    : result.nodes.find((n) => !n.ok)?.how ?? "a node did not answer";
  return (
    <span className="flex items-start gap-1.5 text-destructive">
      <XCircle className="mt-0.5 size-4 shrink-0" /> Does not match: {bad}.
    </span>
  );
}
