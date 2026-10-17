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
import { calls, checkSettle, decideInvoice, keyIndexOf, ownerCall, prepare, raiseCap, Refused, savedPub, units, type Account } from "./owner-call.ts";
import { OwnerView, type Outcome, type Phase } from "./owner-view.tsx";
import { agentAnchor, COPY, halvesByPayment, money, payeeName, type Halves, type Line, type Need } from "./words.ts";

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
  return { ...inbox, parties: { ...inbox.parties, [k]: { ...p, label: COPY.demoVendor, kind: "tameion", verified: true, note: COPY.demoVendorNote } } };
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
  const [clockMs, setClockMs] = useState(0);
  const busy = useRef(false);
  const acctRef = useRef<{ name: string; acct: Account } | null>(null);

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
          setFailed({ at: Date.now(), words: e instanceof ApiError ? e.message : COPY.serviceSilent });
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

  // While a tap is in flight its control counts the seconds (DESIGN §States, waits).
  const inFlight = !!stopPhase || Object.values(needPhase).some(Boolean);
  useEffect(() => {
    if (!inFlight) return;
    const t = setInterval(() => setClockMs(Date.now()), 1000);
    return () => clearInterval(t);
  }, [inFlight]);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 4000);
    return () => clearTimeout(t);
  }, [toast]);

  /** Where the owner signs: the account and its owner contract (by address, in memory only) and the chain's id. */
  const account = async (): Promise<Account> => {
    // Kept per name: a page that moves to another account must never sign for the one before.
    if (acctRef.current?.name === name) return acctRef.current.acct;
    const [found, tenant] = await Promise.all([resolveAccount(name), fetchTenant(name)]);
    const chainId = found?.chains.find((c) => c.key === tenant.chain)?.chainId;
    if (!found || !chainId) throw new Error(COPY.whereUnknown);
    const acct = { policy: found.policy, owner: tenant.owner, chain: tenant.chain, chainId };
    acctRef.current = { name, acct };
    return acct;
  };

  /** One tap: phases named on the control, the outcome in words, never re-sent (DESIGN §Money lifecycle). */
  const run = async (key: string, setPhase: (p: Phase) => void, act: (phase: (label: string) => void) => Promise<string>) => {
    if (busy.current) return;
    busy.current = true;
    setOutcome(null);
    setClockMs(Date.now());
    let sent = false;
    const phase = (label: string) => {
      if (label === COPY.phase.sent) sent = true;
      setPhase({ label, since: Date.now() });
    };
    try {
      const done = await act(phase);
      setPhase({ label: COPY.phase.confirmed, since: Date.now() });
      await refresh();
      // Success is a toast: the item it answered may be gone after the read that followed it.
      setToast(done);
    } catch (e) {
      const closed = (e as Error).name === "NotAllowedError" || (e as Error).name === "AbortError";
      // Whether money (or a setting) may have moved: a refusal says so in its envelope; no answer at all after the call
      // went out is "still checking", and the page never sends it again (DESIGN §Money lifecycle).
      const moved = e instanceof Refused ? e.executed : sent ? "unknown" : "no";
      setOutcome({
        key,
        failed: true,
        text: closed ? COPY.promptClosed : moved === "yes" ? COPY.sentWriting : moved === "unknown" ? COPY.lostAnswer : COPY.nothingSent,
        detail: closed ? undefined : (e as Error).message,
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
      phase(COPY.phase.signing);
      const acct = await account();
      const prepared = await prepare(acct, "prepare-mode", { mode });
      await ownerCall(acct, calls.setMode(mode), { prepared, keyIndex: keyIndexOf(inbox.owners?.keys), phase });
      if (mode === 0) return COPY.stopped;
      const delay = Number((await fetchTenant(name).catch(() => null))?.delay_secs ?? 0);
      return COPY.started(clock(Math.floor(Date.now() / 1000) + delay));
    });

  const onNeed = (n: Need) =>
    run(n.key, (p) => setNeedPhase((s) => ({ ...s, [n.key]: p })), async (phase) => {
      const inbox = data!.inbox;
      const keyIndex = keyIndexOf(inbox.owners?.keys);
      const who = payeeName(inbox, n.counterparty);
      phase(COPY.phase.signing);
      const acct = await account();
      if (n.kind === "seller" || (n.kind === "invoice" && n.invoice?.everyDays && !n.invoice.approvedPayee)) {
        const prepared = await prepare(acct, "prepare-payees", { payees: [n.counterparty] });
        await ownerCall(acct, calls.setPayees([n.counterparty], true), { prepared, keyIndex, phase });
        if (n.kind === "seller") return COPY.approved(who);
      }
      if (n.kind === "bill" && n.action === "Raise the cap") {
        const raise = raiseCap(inbox.caps, n.amount);
        const prepared = await prepare(acct, "prepare-limits", raise.body);
        await ownerCall(acct, raise.data, { prepared, keyIndex, phase });
        return COPY.capAsked(money(n.amount));
      }
      if (n.kind === "bill" || (n.kind === "invoice" && !n.invoice?.everyDays)) {
        if (!n.call) throw new Error(COPY.notReady);
        if (n.call.to.toLowerCase() !== acct.policy.toLowerCase()) throw new Error(COPY.notThisAccount);
        checkSettle(n.call.data, n.counterparty, units(n.amount));
        await ownerCall(acct, n.call.data, { prepared: n.call, keyIndex, phase });
        if (n.kind === "bill") return COPY.paidTo(money(n.amount), who);
      }
      // An invoice: the owner's passkey accepts it (after approving its payee, or paying a one-off, above).
      await decideInvoice(acct, n.invoice!.id, true);
      return n.invoice?.everyDays ? COPY.accepted(who, money(n.amount)) : COPY.paidTo(money(n.amount), who);
    });

  /** "Open in Agent view": the anchor names the agent's check by its time, never a hash (ruled 06:32); the Agent view
   *  marks each record `pass-<at>` and scrolls to the location's anchor once its records are read. */
  const onAgentView = (l: Line) => {
    const anchor = agentAnchor(l);
    if (anchor) history.replaceState(null, "", `#${anchor}`);
    setOpenKey(null);
    setView("agent");
  };

  if (dev === "skeleton" || (loading && dev !== "error")) return <OwnerSkeleton />;
  if (dev === "error" || (!data && failed))
    return (
      <div role="alert" className="flex flex-col gap-(--space-4) rounded-(--radius-plate) bg-(--plate) p-(--space-5)">
        <p className="row-title m-0">{missing ? COPY.noAccount : COPY.noFigures}</p>
        {!missing && (
          <button type="button" onClick={() => void refresh()} className="inline-flex min-h-(--button) items-center justify-center self-start rounded-(--radius-button) bg-(--ink) px-(--space-5) row-title text-(--paper)">
            {COPY.tryAgain}
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
        clockMs={clockMs}
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
        {toast && <p className="row-title m-0 rounded-(--radius-button) bg-(--ink) px-(--space-5) py-(--space-3) text-(--paper)">{toast}</p>}
      </div>
    </>
  );
}
