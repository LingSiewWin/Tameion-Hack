"use client";
// The Owner home's container: reads the account through this app's /api proxy, keeps the last good read for the
// offline / stale state, and runs the owner's taps through owner-call.ts. `?state=skeleton|empty|error|offline` shows
// each state in development (H6's screenshots); it is ignored in a production build.
import { useCallback, useEffect, useRef, useState } from "react";
import { OwnerSkeleton } from "../../components/skeletons.tsx";
import { ApiError, fetchInbox, fetchStatus } from "../../lib/api.ts";
import type { Inbox, Status } from "../../lib/types.ts";
import { setView } from "../../lib/view.ts";
import { SKELETON_MIN_MS } from "../../tokens.ts";
import { calls, checkSettle, decideInvoice, deviceHeaders, keyIndexOf, ownerCall, prepare, savedPub, units, type Account } from "./owner-call.ts";
import { OwnerView, type Outcome, type Phase } from "./owner-view.tsx";
import { halvesByPayment, minor, money, payeeName, type Halves, type Line, type Need, type SymbolonRecord } from "./words.ts";

type Read = { inbox: Inbox; status: Status; halves: Map<string, Halves>; at: number };
type DevState = "skeleton" | "empty" | "error" | "offline" | null;

const devState = (): DevState => {
  if (process.env.NODE_ENV === "production") return null;
  const s = new URLSearchParams(location.search).get("state");
  return s === "skeleton" || s === "empty" || s === "error" || s === "offline" ? s : null;
};

/** H3's projection, where this service has it (a 404 means it does not: then nothing is sealed). */
async function symbolon(policy: string): Promise<SymbolonRecord[] | null> {
  const r = await fetch(`/api/level1/symbolon?policy=${encodeURIComponent(policy)}`, { headers: deviceHeaders(policy), cache: "no-store" });
  if (!r.ok) return null;
  const d = (await r.json().catch(() => null)) as { records?: SymbolonRecord[] } | null;
  return d?.records ?? null;
}

async function read(policy: string): Promise<Read> {
  const [inbox, status, records] = await Promise.all([fetchInbox(policy, deviceHeaders(policy)), fetchStatus(policy), symbolon(policy).catch(() => null)]);
  return { inbox, status, halves: halvesByPayment(records), at: Date.now() };
}

const sleep = (ms: number) => new Promise((ok) => setTimeout(ok, ms));
const clock = (unix: number) => new Date(unix * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });

/** The account's address for its name (DESIGN §Page grammar: routes carry the name; the address stays in API calls). */
async function resolve(name: string): Promise<string> {
  const r = await fetch(`/api/level1/name/${encodeURIComponent(name)}`, { cache: "no-store" });
  const d = (await r.json().catch(() => null)) as { account?: string } | null;
  if (!r.ok || !d?.account) throw new ApiError(r.status, null, r.status === 404 ? "No account has that name." : "The service did not answer.");
  return d.account;
}

export function OwnerHome({ name }: { name: string }) {
  // The address lives in memory and API calls only (DESIGN §Page grammar), never in state that renders.
  const policyRef = useRef<string | null>(null);
  const [data, setData] = useState<Read | null>(null);
  const [failed, setFailed] = useState<{ at: number; words: string } | null>(null);
  const [dev, setDev] = useState<DevState>(null);
  const [now, setNow] = useState(0);
  const [openKey, setOpenKey] = useState<string | null>(null);
  const [asking, setAsking] = useState(false);
  const [stopPhase, setStopPhase] = useState<Phase>(null);
  const [needPhase, setNeedPhase] = useState<Record<string, Phase>>({});
  const [outcome, setOutcome] = useState<Outcome>(null);
  const [toast, setToast] = useState<string | null>(null);
  const busy = useRef(false);
  const acctRef = useRef<Account | null>(null);

  const refresh = useCallback(async () => {
    try {
      policyRef.current ??= await resolve(name);
      const r = await read(policyRef.current);
      setData(r);
      setFailed(null);
      setNow(Math.floor(Date.now() / 1000));
    } catch (e) {
      setFailed({ at: Date.now(), words: e instanceof ApiError ? e.message : "The service did not answer." });
    }
  }, [name]);

  useEffect(() => {
    const d = devState();
    setDev(d);
    if (d === "skeleton" || d === "error") return;
    let live = true;
    // The skeleton, once shown, stays at least SKELETON_MIN_MS (DESIGN §States).
    void Promise.all([refresh(), sleep(SKELETON_MIN_MS)]).then(() => live && setNow(Math.floor(Date.now() / 1000)));
    // Read again every 20 s, never over a tap in flight (its row would be redrawn under the finger).
    const t = setInterval(() => !busy.current && void refresh(), 20_000);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [refresh]);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 4000);
    return () => clearTimeout(t);
  }, [toast]);

  /** Who signs and where: the account's owner contract and chain (tenant route) and the chain's id (config). */
  const account = async (): Promise<Account> => {
    if (acctRef.current) return acctRef.current;
    const policy = policyRef.current;
    if (!policy) throw new Error("The account is not read yet.");
    const [t, cfg] = await Promise.all([
      fetch(`/api/level1/tenant/${encodeURIComponent(policy)}`, { cache: "no-store" }).then((r) => r.json()) as Promise<{ tenant?: { owner: string; chain: string } }>,
      fetch("/api/level1/config", { cache: "no-store" }).then((r) => r.json()) as Promise<{ chains?: { key: string; chainId: number }[] }>,
    ]);
    const chainId = cfg.chains?.find((c) => c.key === t.tenant?.chain)?.chainId;
    if (!t.tenant || !chainId) throw new Error("This page could not read where the account lives.");
    acctRef.current = { policy, owner: t.tenant.owner, chain: t.tenant.chain, chainId };
    return acctRef.current;
  };

  /** One tap: phases named on the control, the outcome in words, never re-sent (DESIGN §Money lifecycle). */
  const run = async (key: string, setPhase: (p: Phase) => void, act: (phase: (label: string) => void) => Promise<string>) => {
    if (busy.current) return;
    busy.current = true;
    setOutcome(null);
    let sent = false;
    const phase = (label: string) => {
      if (label === "Sent") sent = true;
      setPhase({ label });
    };
    try {
      const done = await act(phase);
      setPhase({ label: "Confirmed" });
      await refresh();
      if (key === "stop") setToast(done);
      else setOutcome({ key, text: done, failed: false });
    } catch (e) {
      const why = (e as Error).name === "NotAllowedError" ? "The passkey prompt was closed." : (e as Error).message;
      setOutcome({ key, failed: true, text: sent ? `${why} It was sent; the answer was lost, so the page is reading the account again and will not send it twice.` : `${why} Nothing was sent.` });
      void refresh();
    } finally {
      busy.current = false;
      setPhase(null);
      setAsking(false);
    }
  };

  const onStop = () =>
    run("stop", setStopPhase, async (phase) => {
      const inbox = data!.inbox;
      const mode = inbox.mode === "Watch" ? 1 : 0;
      const acct = await account();
      phase("Signing");
      const prepared = await prepare("prepare-mode", { mode }, acct.policy);
      await ownerCall(acct, calls.setMode(mode), { prepared, keyIndex: keyIndexOf(inbox.owners?.keys), phase });
      if (mode === 0) return "Stopped";
      const delay = Number((await fetch(`/api/level1/tenant/${encodeURIComponent(acct.policy)}`).then((r) => r.json()).catch(() => ({})))?.tenant?.delay_secs ?? 0);
      return `It starts asking before each payment at ${clock(Math.floor(Date.now() / 1000) + delay)}.`;
    });

  const onNeed = (n: Need) =>
    run(n.key, (p) => setNeedPhase((s) => ({ ...s, [n.key]: p })), async (phase) => {
      const inbox = data!.inbox;
      const acct = await account();
      const keyIndex = keyIndexOf(inbox.owners?.keys);
      const name = payeeName(inbox, n.counterparty);
      phase("Signing");
      if (n.kind === "seller" || (n.kind === "invoice" && n.invoice?.everyDays && !n.invoice.approvedPayee)) {
        const prepared = await prepare("prepare-payees", { payees: [n.counterparty] }, acct.policy);
        await ownerCall(acct, calls.setPayees([n.counterparty], true), { prepared, keyIndex, phase });
        if (n.kind === "seller") return `Approved. The agent may pay ${name} once the wait has passed.`;
      }
      if (n.kind === "bill" && n.action === "Raise the cap") {
        const want = units(n.amount);
        const daily = minor(inbox.caps.daily) ?? 0n;
        const floor = minor(inbox.caps.floor) ?? 0n;
        const newDaily = daily > want ? daily : want;
        const amt = (u: bigint) => money(`${u / 1_000_000n}.${(u % 1_000_000n).toString().padStart(6, "0")}`).replace(" USDC", "");
        const prepared = await prepare("prepare-limits", { perAction: amt(want), daily: amt(newDaily), floor: amt(floor) }, acct.policy);
        await ownerCall(acct, calls.setLimits(want, newDaily, floor), { prepared, keyIndex, phase });
        return `Asked. One payment may be up to ${money(n.amount)} once the wait has passed; anyone can see it coming.`;
      }
      if (n.kind === "bill" || (n.kind === "invoice" && !n.invoice?.everyDays)) {
        if (!n.call) throw new Error("The account is not ready to pay this yet; try again after the agent's next check.");
        if (n.call.to.toLowerCase() !== acct.policy.toLowerCase()) throw new Error("Not signing: the payment goes to a contract that is not this account's.");
        checkSettle(n.call.data, n.counterparty, units(n.amount));
        await ownerCall(acct, n.call.data, { prepared: n.call, keyIndex, phase });
        if (n.kind === "bill") return `Paid ${money(n.amount)} to ${name}.`;
      }
      // An invoice: the owner's passkey accepts it (after approving its payee, or paying a one-off, above).
      await decideInvoice(acct, n.invoice!.id, true);
      return n.invoice?.everyDays ? `Accepted. The agent pays ${name} ${money(n.amount)} when it falls due.` : `Paid ${money(n.amount)} to ${name}.`;
    });

  /** "Open in Agent view": flip the capsule; the Agent view scrolls to the record named in the location's hash. */
  const onAgentView = (l: Line) => {
    // The anchor names the agent's check by its time, never by a hash (ruled 06:32): the Agent view marks each record `pass-<at>`.
    if (l.at) history.replaceState(null, "", `#pass-${l.at}`);
    setOpenKey(null);
    setView("agent");
  };

  if (dev === "skeleton" || (!data && !failed && dev !== "error")) return <OwnerSkeleton />;
  if (dev === "error" || (!data && failed))
    return (
      <div role="alert" className="flex flex-col gap-(--space-4) rounded-(--radius-plate) border border-(--rule) bg-(--plate) p-(--space-4)">
        <p className="row-title m-0">The service did not answer, so there are no figures to show yet. Nothing was sent or changed.</p>
        <button type="button" onClick={() => void refresh()} className="min-h-(--cta) rounded-(--radius-plate) border border-(--ink) bg-(--plate) row-title text-(--ink)">
          Try again
        </button>
      </div>
    );
  const shown = data!;
  const inbox: Inbox =
    dev === "empty" ? { ...shown.inbox, paid: [], refused: [], needsYou: [], sellersToApprove: [], invoices: [], waitingCount: 0, payees: [] } : shown.inbox;
  const pub = savedPub();
  const owns = !!pub && !!inbox.owners?.keys.some((k) => k.qx.toLowerCase() === pub.qx.toLowerCase());
  return (
    <>
      <OwnerView
        inbox={inbox}
        byRateDays={shown.status.runway?.byRateDays}
        halves={shown.halves}
        nowSecs={now || Math.floor(shown.at / 1000)}
        staleAt={dev === "offline" ? shown.at - 6 * 60_000 : failed ? shown.at : null}
        guest={!owns}
        openKey={openKey}
        onOpen={setOpenKey}
        onAgentView={onAgentView}
        onRetry={() => void refresh()}
        asking={asking}
        onStopAsk={() => setAsking(true)}
        onStopCancel={() => setAsking(false)}
        onStop={onStop}
        stopPhase={stopPhase}
        needPhase={needPhase}
        onNeed={onNeed}
        outcome={outcome}
      />
      <div role="status" aria-live="polite" className="pointer-events-none fixed inset-x-0 bottom-(--space-5) flex justify-center px-(--space-4)">
        {toast && <p className="row-title m-0 rounded-(--radius-plate) bg-(--ink) px-(--space-5) py-(--space-3) text-(--paper)">{toast}</p>}
      </div>
    </>
  );
}
