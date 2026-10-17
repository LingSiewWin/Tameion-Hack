"use client";
// The Owner home's container: reads the account by its name through this app's /api proxy (the proxy turns the name
// into the address), keeps the last good read for the offline / stale state, and runs the owner's taps through
// owner-call.ts. `?state=skeleton|empty|error|offline` shows each state in development (H6's screenshots); ignored in a
// production build.
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { useSkeleton } from "../../components/skeleton.tsx";
import { OwnerSkeleton } from "../../components/skeletons.tsx";
import { ApiError, fetchDemo, fetchInbox, fetchStatus, fetchSymbolon, fetchTenant, resolveAccount } from "../../lib/api.ts";
import type { Inbox, Status } from "../../lib/types.ts";
import { setView } from "../../lib/view.ts";
import { calls, checkSettle, decideInvoice, keyIndexOf, ownerCall, prepare, savedPub, units, type Account } from "./owner-call.ts";
import { OwnerView, type Outcome, type Phase } from "./owner-view.tsx";
import { amount, halvesByPayment, minor, money, payeeName, type Halves, type Line, type Need } from "./words.ts";

type Read = { inbox: Inbox; status: Status; halves: Map<string, Halves>; at: number };
type DevState = "skeleton" | "empty" | "error" | "offline" | null;

const devState = (): DevState => {
  if (process.env.NODE_ENV === "production") return null;
  const s = new URLSearchParams(location.search).get("state");
  return s === "skeleton" || s === "empty" || s === "error" || s === "offline" ? s : null;
};
const noSubscribe = () => () => {};

/** The demo's vendor has a name on every host (`GET /api/level1/demo` names its address; apps/console calls it
 *  "Demo Vendor"): a reader without the owner's device still reads it by name. Other payees keep the owner's names. */
const withDemoVendor = (inbox: Inbox, vendor?: string): Inbox => {
  const k = vendor?.toLowerCase();
  const p = k ? inbox.parties?.[k] : undefined;
  if (!k || !p || p.kind !== "new") return inbox;
  return { ...inbox, parties: { ...inbox.parties, [k]: { ...p, label: "Demo Vendor", kind: "tameion", verified: true, note: "the demo's vendor" } } };
};

async function read(name: string): Promise<Read> {
  const [inbox, status, records, demo] = await Promise.all([
    fetchInbox(name),
    fetchStatus(name),
    // H3's projection, where this service has it; without it (a 404, an older service) nothing is sealed.
    fetchSymbolon(name).then((r) => r.records, () => null),
    fetchDemo().catch(() => null),
  ]);
  return { inbox: withDemoVendor(inbox, demo?.vendor), status, halves: halvesByPayment(records), at: Date.now() };
}

const clock = (unix: number) => new Date(unix * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });

export function OwnerHome({ name }: { name: string }) {
  const [data, setData] = useState<Read | null>(null);
  const [failed, setFailed] = useState<{ at: number; words: string } | null>(null);
  const [missing, setMissing] = useState(false);
  const dev = useSyncExternalStore(noSubscribe, devState, () => null);
  const [now, setNow] = useState(0);
  const [openKey, setOpenKey] = useState<string | null>(null);
  const [asking, setAsking] = useState(false);
  const [stopPhase, setStopPhase] = useState<Phase>(null);
  const [needPhase, setNeedPhase] = useState<Record<string, Phase>>({});
  const [outcome, setOutcome] = useState<Outcome>(null);
  const [toast, setToast] = useState<string | null>(null);
  const busy = useRef(false);
  const acctRef = useRef<Account | null>(null);

  const refresh = useCallback(
    () =>
      read(name).then(
        (r) => {
          setData(r);
          setFailed(null);
          setNow(Math.floor(Date.now() / 1000));
        },
        (e: unknown) => {
          if (e instanceof ApiError && e.status === 404) setMissing(true);
          setFailed({ at: Date.now(), words: e instanceof ApiError ? e.message : "The service did not answer." });
        },
      ),
    [name],
  );

  useEffect(() => {
    if (dev === "skeleton" || dev === "error") return;
    void refresh();
    // Read again every 20 s, never over a tap in flight (its row would be redrawn under the finger).
    const t = setInterval(() => !busy.current && void refresh(), 20_000);
    return () => clearInterval(t);
  }, [refresh, dev]);
  // The skeleton, once shown, stays at least 300 ms (DESIGN §States).
  const loading = useSkeleton(!data && !failed);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 4000);
    return () => clearTimeout(t);
  }, [toast]);

  /** Where the owner signs: the account and its owner contract (by address, in memory only) and the chain's id. */
  const account = async (): Promise<Account> => {
    if (acctRef.current) return acctRef.current;
    const [found, tenant] = await Promise.all([resolveAccount(name), fetchTenant(name)]);
    const chainId = found?.chains.find((c) => c.key === tenant.chain)?.chainId;
    if (!found || !chainId) throw new Error("This page could not read where the account lives.");
    acctRef.current = { policy: found.policy, owner: tenant.owner, chain: tenant.chain, chainId };
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
      setOutcome({
        key,
        failed: true,
        text: sent ? `${why} It was sent and the answer was lost: still checking. The page reads the account again and never sends it twice.` : `${why} Nothing was sent.`,
      });
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
      phase("Signing");
      const acct = await account();
      const prepared = await prepare(acct, "prepare-mode", { mode });
      await ownerCall(acct, calls.setMode(mode), { prepared, keyIndex: keyIndexOf(inbox.owners?.keys), phase });
      if (mode === 0) return "Stopped";
      const delay = Number((await fetchTenant(name).catch(() => null))?.delay_secs ?? 0);
      return `Started. It asks before each payment from ${clock(Math.floor(Date.now() / 1000) + delay)}.`;
    });

  const onNeed = (n: Need) =>
    run(n.key, (p) => setNeedPhase((s) => ({ ...s, [n.key]: p })), async (phase) => {
      const inbox = data!.inbox;
      const keyIndex = keyIndexOf(inbox.owners?.keys);
      const who = payeeName(inbox, n.counterparty);
      phase("Signing");
      const acct = await account();
      if (n.kind === "seller" || (n.kind === "invoice" && n.invoice?.everyDays && !n.invoice.approvedPayee)) {
        const prepared = await prepare(acct, "prepare-payees", { payees: [n.counterparty] });
        await ownerCall(acct, calls.setPayees([n.counterparty], true), { prepared, keyIndex, phase });
        if (n.kind === "seller") return `Approved. The agent may pay ${who} once the wait has passed.`;
      }
      if (n.kind === "bill" && n.action === "Raise the cap") {
        const want = units(n.amount);
        const daily = minor(inbox.caps.daily) ?? 0n;
        const floor = minor(inbox.caps.floor) ?? 0n;
        const newDaily = daily > want ? daily : want;
        const plainAmount = (u: bigint) => amount(u).replace(/,/g, "");
        const prepared = await prepare(acct, "prepare-limits", { perAction: plainAmount(want), daily: plainAmount(newDaily), floor: plainAmount(floor) });
        await ownerCall(acct, calls.setLimits(want, newDaily, floor), { prepared, keyIndex, phase });
        return `Asked. One payment may be up to ${money(n.amount)} once the wait has passed; anyone can see it coming.`;
      }
      if (n.kind === "bill" || (n.kind === "invoice" && !n.invoice?.everyDays)) {
        if (!n.call) throw new Error("The account is not ready to pay this yet; try again after the agent's next check.");
        if (n.call.to.toLowerCase() !== acct.policy.toLowerCase()) throw new Error("Not signing: the payment goes to a contract that is not this account's.");
        checkSettle(n.call.data, n.counterparty, units(n.amount));
        await ownerCall(acct, n.call.data, { prepared: n.call, keyIndex, phase });
        if (n.kind === "bill") return `Paid ${money(n.amount)} to ${who}.`;
      }
      // An invoice: the owner's passkey accepts it (after approving its payee, or paying a one-off, above).
      await decideInvoice(acct, n.invoice!.id, true);
      return n.invoice?.everyDays ? `Accepted. The agent pays ${who} ${money(n.amount)} when it falls due.` : `Paid ${money(n.amount)} to ${who}.`;
    });

  /** "Open in Agent view": the anchor names the agent's check by its time, never a hash (ruled 06:32); the Agent view
   *  marks each record `pass-<at>` and scrolls to the location's anchor once its records are read. */
  const onAgentView = (l: Line) => {
    if (l.at) history.replaceState(null, "", `#pass-${l.at}`);
    setOpenKey(null);
    setView("agent");
  };

  if (dev === "skeleton" || (loading && dev !== "error")) return <OwnerSkeleton />;
  if (dev === "error" || (!data && failed))
    return (
      <div role="alert" className="flex flex-col gap-(--space-4) rounded-(--radius-plate) border border-(--rule) bg-(--plate) p-(--space-4)">
        <p className="row-title m-0">
          {missing ? "No account has this name on this service." : "The service did not answer, so there are no figures to show yet. Nothing was sent or changed."}
        </p>
        {!missing && (
          <button type="button" onClick={() => void refresh()} className="min-h-(--cta) rounded-(--radius-plate) border border-(--ink) bg-(--plate) row-title text-(--ink)">
            Try again
          </button>
        )}
      </div>
    );
  const shown = data!;
  const inbox: Inbox =
    dev === "empty" ? { ...shown.inbox, paid: [], refused: [], needsYou: [], sellersToApprove: [], invoices: [], waitingCount: 0, payees: [] } : shown.inbox;
  // Only the device that holds one of the account's owner keys sees Stop and the actions; everyone else reads.
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
