"use client";
// The Owner home's container: reads the account by its name through this app's /api proxy (the proxy turns the name
// into the address), keeps the last good read for the offline / stale state, and runs the owner's taps through
// owner-call.ts. `?state=skeleton|empty|error|offline` shows each state in development (H6's screenshots); ignored in a
// production build.
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { useSkeleton } from "../../components/skeleton.tsx";
import { OwnerSkeleton } from "../../components/skeletons.tsx";
import { ApiError, fetchDemo, fetchInbox, fetchStatus, fetchSymbolon, fetchTenant, resolveAccount } from "../../lib/api.ts";
import type { Demo, Inbox, Status } from "../../lib/types.ts";
import { setView } from "../../lib/view.ts";
import { ownerPost } from "../onboarding/actions.ts";
import { useNow } from "../onboarding/ui.tsx";
import type { Actions, SheetName } from "./actions-view.tsx";
import { calls, checkSettle, decideInvoice, keyIndexOf, limitsCall, ownerCall, prepare, raiseCap, Refused, savedPub, units, type Account } from "./owner-call.ts";
import { OwnerError, OwnerView, type Outcome, type Phase } from "./owner-view.tsx";
import { agentAnchor, changeOf, COPY, everyWords, halvesByPayment, MODES, modeIndex, money, payeeLabel, payeeName, servedAsOf, stripOf, type Halves, type Line, type Need } from "./words.ts";

/** `servedAt`: the service answered with its last good read (stale + asOf), that read's time; else null. */
type Read = { inbox: Inbox; status: Status; halves: Map<string, Halves>; at: number; servedAt: number | null; demo: Demo | null };
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
  return { inbox: withDemoVendor(inbox, demo?.vendor), status, halves: halvesByPayment(records), at: Date.now(), servedAt: servedAsOf(inbox, status), demo };
}

const clock = (unix: number) => new Date(unix * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
/** A step that failed after an earlier one went through: `said` is the summary the owner reads (what holds). */
const partly = (done: string, e: unknown) => Object.assign(new Error((e as Error)?.message ?? String(e)), { said: COPY.partly(done) });

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
  const [phases, setPhases] = useState<Record<string, Phase>>({});
  const [sheet, setSheet] = useState<SheetName | null>(null);
  // The agent's key, for its mark beside "the agent" (read once; in memory only, never rendered as text), and the
  // account's delay: a demo account waits the demo's (`GET /api/level1/demo` delaySecs), which is how the page knows one.
  const [agentId, setAgentId] = useState<string | undefined>(undefined);
  const [delay, setDelay] = useState<number | null>(null);
  useEffect(() => {
    let live = true;
    fetchTenant(name).then(
      (t) => {
        if (!live) return;
        if (typeof t.agent === "string") setAgentId(t.agent.toLowerCase());
        if (typeof t.delay_secs === "number") setDelay(t.delay_secs);
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [name]);
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

  // Countdowns tick each second while something counts down: a change waiting out the delay, the five minutes' bill.
  const pendingEta = data?.inbox.pending?.eta ?? 0;
  const demo = data?.demo?.enabled && delay === data.demo.delaySecs ? data.demo : null;
  const step = demo && data ? stripOf(data.inbox, demo) : null;
  const tick = useNow(pendingEta > 0 || step?.step === 0 || step?.step === 3);
  // Past the eta, or past the bill's due time, the chain moves without this page: read every 5 s until it shows.
  const hurry = (pendingEta > 0 && tick >= pendingEta) || step?.step === 0 || (step?.step === 3 && !!step.bill && tick >= step.bill.nextDue);
  useEffect(() => {
    if (!hurry) return;
    const t = setInterval(() => !busy.current && void refresh(), 5_000);
    return () => clearInterval(t);
  }, [hurry, refresh]);

  // While a tap is in flight its control counts the seconds (DESIGN §States, waits).
  const inFlight = !!stopPhase || Object.values(needPhase).some(Boolean) || Object.values(phases).some(Boolean);
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
      // a sheet closes on success, so the page shows what changed (the toast, a countdown with its Cancel)
      setSheet(null);
      await refresh();
      // Success is a toast: the item it answered may be gone after the read that followed it. The five minutes' cut
      // stays in words (it is their last step, and nothing else on the page says it happened); the rest show as state.
      if (key === "cut") setOutcome({ key, text: done, failed: false });
      else setToast(done);
    } catch (e) {
      const closed = (e as Error).name === "NotAllowedError" || (e as Error).name === "AbortError";
      const said = (e as { said?: string }).said;
      // Whether money (or a setting) may have moved: a refusal says so in its envelope; no answer at all after the call
      // went out is "still checking", and the page never sends it again (DESIGN §Money lifecycle).
      const moved = e instanceof Refused ? e.executed : sent ? "unknown" : "no";
      setOutcome({
        key,
        failed: true,
        text: said ?? (closed ? COPY.promptClosed : moved === "yes" ? COPY.sentWriting : moved === "unknown" ? COPY.lostAnswer : COPY.nothingSent),
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

  const keyed = (key: string) => (p: Phase) => setPhases((s) => ({ ...s, [key]: p }));
  /** One owner call on the account: built here, prepared by the service, compared, signed with the passkey. */
  const sign = async (route: string, body: Record<string, unknown>, data: string, phase: (label: string) => void) => {
    const acct = await account();
    const prepared = await prepare(acct, route, body);
    await ownerCall(acct, data, { prepared, keyIndex: keyIndexOf(data_().inbox.owners?.keys), phase });
    return acct;
  };
  const data_ = () => data!;
  const at = () => clock(Math.floor(Date.now() / 1000) + (delay ?? data_().demo?.delaySecs ?? 0));

  /** The five minutes' bill: approve Demo Vendor (one passkey tap, skipped when it already is) and save its bill, as the
   *  console's judge strip and this app's stepper do; the device token rides on the server (actions.ts ownerPost). */
  const onBill = () =>
    run("demo-bill", keyed("demo-bill"), async (phase) => {
      const { inbox, demo } = data_();
      if (!demo) throw new Error(COPY.notReady);
      const v = demo.vendor.toLowerCase();
      phase(COPY.phase.signing);
      const acct = await account();
      // Ask first whether this device may save bills here, so a refusal costs no passkey tap (docs/60 P-225).
      const check = await ownerPost(acct.policy, "bills", { check: true });
      if ("error" in check) throw new Refused(check.error, 403, "no");
      if (inbox.agentTokens?.length) throw new Error(COPY.strip.agentKeys);
      const approve = !stripOf(inbox, demo).approved;
      if (approve) await sign("prepare-payees", { payees: [v] }, calls.setPayees([v], true), phase);
      try {
        // First due one cycle from now, so the countdown shows; `everyDays: 0` so the bytes never rest on a default.
        const every = demo.bill.everySecs;
        const bill = { payee: v, amount: demo.bill.usdc, everyDays: 0, everySecs: every, nextDue: Math.floor(Date.now() / 1000) + every };
        const saved = await ownerPost(acct.policy, "bills", { bills: [bill] });
        if ("error" in saved) throw new Refused(saved.error, 400, saved.executed);
        const refused = (saved.body as { refused?: { why: string }[] }).refused;
        if (refused?.length) throw new Error(refused[0].why);
        await ownerPost(acct.policy, "contacts", { address: v, name: COPY.demoVendor });
        return COPY.strip.billSaved(money(demo.bill.usdc), everyWords(every));
      } catch (e) {
        throw approve ? partly(COPY.strip.billApproved, e) : e;
      }
    });

  /** Watch, Propose or Autopilot: lowering applies at once, raising waits the delay (BudgetPolicy.setMode). */
  const onMode = (mode: 0 | 1 | 2, key = `mode-${mode}`) =>
    run(key, keyed(key), async (phase) => {
      const raise = mode > modeIndex(data_().inbox.mode);
      phase(COPY.phase.signing);
      await sign("prepare-mode", { mode }, calls.setMode(mode), phase);
      return raise ? COPY.modes.asked(MODES[mode], at()) : COPY.modes.now(MODES[mode]);
    });

  /** Drop the change waiting out the delay (a raised limit or mode): instant. */
  const onCancel = () =>
    run("cancel", keyed("cancel"), async (phase) => {
      phase(COPY.phase.signing);
      await sign("prepare-cancel", {}, calls.cancelPending(), phase);
      return COPY.cancelled;
    });

  /** Approve a payee by address and name: the name first (nothing is signed when it cannot be saved), then the passkey. */
  const onApprove = (address: string, label: string) =>
    run("approve", keyed("approve"), async (phase) => {
      const a = address.toLowerCase();
      phase(COPY.phase.signing);
      const acct = await account();
      const named = await ownerPost(acct.policy, "contacts", { address: a, name: label });
      if ("error" in named) throw new Refused(named.error, 403, "no");
      await sign("prepare-payees", { payees: [a] }, calls.setPayees([a], true), phase);
      return COPY.approved(label);
    });

  /** Stop paying a payee: instant (setPayees(…, false)). */
  const onStopPayee = (address: string) => {
    const a = address.toLowerCase();
    const who = payeeLabel(data_().inbox, a);
    return run(`stop-${a}`, keyed(`stop-${a}`), async (phase) => {
      phase(COPY.phase.signing);
      await sign("prepare-payees", { payees: [a], allow: false }, calls.setPayees([a], false), phase);
      return COPY.payees.stopped(who);
    });
  };

  /** The two caps as typed, the floor kept: a raise waits the delay with its Cancel, a cut applies at once. */
  const onLimits = (perAction: string, daily: string, key = "limits") =>
    run(key, keyed(key), async (phase) => {
      const caps = data_().inbox.caps;
      const c = changeOf(caps, perAction, daily);
      if ("error" in c) throw new Error(c.error);
      const call = limitsCall(caps, c.perAction, c.daily);
      phase(COPY.phase.signing);
      await sign("prepare-limits", call.body, call.data, phase);
      return c.kind === "cut" ? COPY.limits.cutDone(money(call.body.perAction), money(call.body.daily)) : COPY.limits.asked(at());
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
      <OwnerError missing={missing} onRetry={() => void refresh()} />
    );
  const shown = data!;
  const inbox: Inbox =
    dev === "empty" ? { ...shown.inbox, paid: [], refused: [], needsYou: [], sellersToApprove: [], invoices: [], waitingCount: 0, payees: [] } : shown.inbox;
  // Only the device that holds one of the account's owner keys sees Stop and the actions; everyone else reads.
  const pub = savedPub();
  const owns = !!pub && !!inbox.owners?.keys.some((k) => k.qx.toLowerCase() === pub.qx.toLowerCase());
  const act: Actions = { phases, sheet, onSheet: setSheet, onBill, onMode, onCancel, onApprove, onStopPayee, onLimits };
  return (
    <>
      <OwnerView
        inbox={inbox}
        byRateDays={shown.status.runway?.byRateDays}
        halves={shown.halves}
        nowSecs={now || Math.floor(shown.at / 1000)}
        clockMs={clockMs}
        staleAt={dev === "offline" ? shown.at - 6 * 60_000 : failed ? (shown.servedAt ?? shown.at) : shown.servedAt}
        silent={dev === "offline" || !!failed}
        guest={!owns}
        agentId={agentId}
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
        act={act}
        demo={owns ? demo : null}
        clockSecs={Math.max(tick, now || Math.floor(shown.at / 1000))}
      />
      <div role="status" aria-live="polite" className="pointer-events-none fixed inset-x-0 bottom-(--space-5) flex justify-center px-(--space-4)">
        {toast && <p className="row-title m-0 rounded-(--radius-button) bg-(--ink) px-(--space-5) py-(--space-3) text-(--paper)">{toast}</p>}
      </div>
    </>
  );
}
