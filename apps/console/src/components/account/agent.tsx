"use client";

import { useQuery } from "@tanstack/react-query";
import { BadgeCheck, Bot, Loader2, Pause, Play, Sparkles } from "lucide-react";
import { Section, Why } from "@/components/layout/section";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { build, minor } from "@/lib/guard";
import { ApiError, dateText, everyText, fetchInbox, inText, raw, sentence, usd } from "@/lib/tameion";
import type { Inbox } from "@/lib/types";
import { Receipt, useAccount, useAction } from "./ctx";
import { Payee } from "./payee";

export const modeWords = (m?: string) => (m === "Watch" ? "Paused" : m === "Autopilot" ? "Autopilot" : "Asks before paying");

export function AgentControls() {
  const { inbox, tenant, cfg, sign, post, refresh, owns } = useAccount();
  const auto = useAction();
  const pause = useAction();
  if (!inbox) return null;
  // One raise waits at a time (autopilot, resume, a higher limit): while it
  // does, the owner's one tap is to cancel it. The account page applies it
  // once the delay has passed.
  const pending = inbox.pending;
  const state = auto.state.kind !== "idle" ? auto.state : pause.state;
  const when = inText(pending?.eta);
  const waiting = !pending
    ? ""
    : (pending.kinds ?? 0) & 2
      ? ` ${pending.mode === "Autopilot" ? "Autopilot turns on" : "It resumes, asking before paying,"} ${when} unless you cancel.`
      : ` A higher limit applies ${when} unless you cancel.`;
  return (
    <Section
      title="The agent"
      icon={Bot}
      action={<Badge variant={inbox.mode === "Autopilot" ? "default" : "secondary"}>{modeWords(inbox.mode)}</Badge>}
    >
      <p className="text-sm leading-relaxed text-muted-foreground">
        {inbox.mode === "Autopilot"
          ? "It pays approved payees inside your limits by itself and tells you after."
          : inbox.mode === "Watch"
            ? "Paused: it reads and reports, and pays nothing."
            : "It prepares every payment and waits for your tap."}
        {waiting} The account holds <span className="figures text-foreground">{usd(inbox.balance)} USDC</span>.
      </p>
      {owns && (
      <div className="mt-4 flex flex-wrap gap-2">
        {pending ? (
          <Button
            disabled={state.kind === "working"}
            onClick={() =>
              auto.run("Cancelling…", async () => {
                // What the owner is cancelling, kept before the call: if the change applied first, say so in words (docs/60 P-171).
                const due = pending.eta;
                const what = (pending.kinds ?? 0) & 2 ? (pending.mode === "Autopilot" ? "Autopilot" : "the resume") : "the higher limit";
                const undo = (pending.kinds ?? 0) & 2 ? "Pause" : "lower the limit";
                try {
                  const prep = await post<{ to: string; data: string }>("/api/level1/prepare-cancel", { policy: tenant.policy });
                  const ex = await sign(prep.to, prep.data, { fn: ["cancelPending"], data: build("cancelPending", []) });
                  refresh();
                  return { text: `Cancelled: ${what} will not apply.`, href: ex.explorer };
                } catch (e) {
                  const now = await fetchInbox(tenant.policy).catch(() => null);
                  refresh();
                  if (now && !now.pending) {
                    const at = due ? new Date(due * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" }) : "its time";
                    throw new Error(`Too late: ${what} applied at ${at}, before the cancel reached the chain. A cut is instant: ${undo} now and it takes effect at once.`);
                  }
                  throw new Error(`The cancel did not go through (${(e as Error).message}). The change still waits: try again${due ? ` before ${new Date(due * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}` : ""}.`);
                }
              })
            }
          >
            {auto.state.kind === "working" ? <Loader2 className="size-4 animate-spin" /> : <Sparkles className="size-4" />}
            Cancel the change
          </Button>
        ) : (
          !inbox.keyless &&
          inbox.mode !== "Autopilot" && (
            <Button
              disabled={state.kind === "working"}
              onClick={() =>
                auto.run("Asking…", async () => {
                  const prep = await post<{ to: string; data: string }>("/api/level1/prepare-mode", { policy: tenant.policy, mode: 2 });
                  const ex = await sign(prep.to, prep.data, { fn: ["setMode"], data: build("setMode", [2]) });
                  refresh();
                  return { text: "Asked. Autopilot turns on after the delay unless you cancel.", href: ex.explorer };
                })
              }
            >
              {auto.state.kind === "working" ? <Loader2 className="size-4 animate-spin" /> : <Sparkles className="size-4" />}
              Turn autopilot on
            </Button>
          )
        )}
        {(inbox.mode !== "Watch" || !pending) && (
          <Button
            variant="outline"
            disabled={state.kind === "working"}
            onClick={() =>
              pause.run("Signing…", async () => {
                const mode = inbox.mode === "Watch" ? 1 : 0;
                const prep = await post<{ to: string; data: string }>("/api/level1/prepare-mode", { policy: tenant.policy, mode });
                const ex = await sign(prep.to, prep.data, { fn: ["setMode"], data: build("setMode", [mode]) });
                refresh();
                // Pausing lowers trust and applies at once; resuming raises it and waits.
                return { text: mode === 0 ? "Paused, at once." : "Asked. It resumes after the delay unless you cancel.", href: ex.explorer };
              })
            }
          >
            {inbox.mode === "Watch" ? <Play className="size-4" /> : <Pause className="size-4" />}
            {inbox.mode === "Watch" ? "Resume" : "Pause"}
          </Button>
        )}
      </div>
      )}
      {state.kind === "error" && <p className="mt-2 text-sm text-destructive">{state.text}</p>}
      {state.kind === "done" && (
        <p className="mt-2 text-sm text-protect">
          {state.text} <Receipt href={state.href} />
        </p>
      )}
      <p className="mt-3 text-caption text-muted-foreground">
        Pausing is instant; raising waits {cfg.delaySecs} s.{" "}
        <Why>Raising autonomy waits the delay, and you can cancel it until then. Lowering or pausing never waits.</Why>
      </p>
      <Identity />
    </Section>
  );
}

/** The agent's public identity card (ERC-8004 registration), when it has one. */
function Identity() {
  const { tenant, chainName } = useAccount();
  const card = useQuery({
    queryKey: ["agent-card", tenant.policy],
    queryFn: async () => {
      const r = await fetch(`/agents/${tenant.policy}.json`);
      if (!r.ok) return null;
      return (await r.json()) as { registrations?: { agentId: string; agentRegistry: string }[] };
    },
    staleTime: 300_000,
  });
  const reg = card.data?.registrations?.[0];
  if (!reg) return null;
  return (
    <p className="mt-3 flex items-center gap-1.5 border-t pt-3 text-xs text-muted-foreground">
      <BadgeCheck className="size-3.5 text-protect" aria-hidden />
      Registered agent #{reg.agentId} on {chainName}, with a public card anyone can check.{" "}
      <a className="font-medium text-foreground underline-offset-4 hover:underline" href={`/agents/${tenant.policy}.json`} target="_blank" rel="noopener">
        Card
      </a>
    </p>
  );
}

function Suggestion({ title, children, action }: { title: string; children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <li className="px-4 py-4 sm:px-5">
      <p className="text-title font-medium">{title}</p>
      <div className="mt-1 text-sm leading-relaxed text-muted-foreground">{children}</div>
      {action && <div className="mt-3">{action}</div>}
    </li>
  );
}

export function Suggestions() {
  const { inbox, tenant, sign, post, refresh } = useAccount();
  const limit = useAction();
  const renew = useAction();
  const autoRec = useAction();
  const so = useAction();
  if (!inbox) return null;
  const sg = inbox.suggestedLimit;
  const aa = inbox.agentAuthority;
  const orders = (inbox.standingOrders ?? []).filter((o) => o.active);
  const cands = inbox.standingOrderCandidates ?? [];
  const any = sg || inbox.recommendAutopilot || aa?.renewSuggested || orders.length || cands.length;
  if (!any) return null;
  // The step a tap is on shows too: "Make it standing" says there when it moves the first date (P-211).
  const out = (s: ReturnType<typeof useAction>["state"]) =>
    s.kind === "working" ? (
      <p className="mt-2 text-sm text-muted-foreground">{s.label}</p>
    ) : s.kind === "error" ? (
      <p className="mt-2 text-sm text-destructive">{s.text}</p>
    ) : s.kind === "done" ? (
      <p className="mt-2 text-sm text-protect">
        {s.text} <Receipt href={s.href} />
      </p>
    ) : null;
  return (
    <Section title="The agent suggests" icon={Sparkles} flush>
      <ul className="divide-y">
        {aa?.renewSuggested && (
          <Suggestion
            title={aa.lapsed ? "Its authority has lapsed" : "Its authority lapses soon"}
            action={
              <>
                <Button
                  disabled={renew.state.kind === "working"}
                  onClick={() =>
                    renew.run("Renewing…", async () => {
                      const until = Math.floor(Date.now() / 1000) + 90 * 86_400;
                      const prep = await post<{ to: string; data: string }>("/api/level1/prepare-renew", { policy: tenant.policy, until });
                      const ex = await sign(prep.to, prep.data, { fn: ["renewAgent"], data: build("renewAgent", [BigInt(until)]) });
                      setTimeout(refresh, 3000);
                      return { text: "Renewed for 90 days.", href: ex.explorer };
                    })
                  }
                >
                  Renew for 90 days
                </Button>
                {out(renew.state)}
              </>
            }
          >
            {aa.lapsed
              ? `Since ${new Date(aa.lapsesAt * 1000).toLocaleDateString()} it pays nothing by itself. Standing orders and your own payments were never affected.`
              : `On ${new Date(aa.lapsesAt * 1000).toLocaleDateString()} (${aa.daysLeft} days) it stops paying by itself unless you renew. A key nobody re-confirms is one nobody is watching.`}
          </Suggestion>
        )}
        {sg && (
          <Suggestion
            title={`A daily limit of ${usd(sg.daily)} USDC`}
            action={
              <>
                <Button
                  variant="outline"
                  disabled={limit.state.kind === "working"}
                  onClick={() =>
                    limit.run("Setting…", async () => {
                      const prep = await post<{ to: string; data: string }>("/api/level1/prepare-limits", {
                        policy: tenant.policy,
                        perAction: raw(sg.perAction),
                        daily: raw(sg.daily),
                        // The floor the agent asked for, when it asked for one (docs/110 row 9); the current one otherwise.
                        floor: raw(sg.floor ?? inbox.caps.floor),
                      });
                      const ex = await sign(prep.to, prep.data, {
                        fn: ["setLimits"],
                        data: build("setLimits", [minor(sg.perAction), minor(sg.daily), minor(sg.floor ?? inbox.caps.floor)]),
                      });
                      setTimeout(refresh, 3000);
                      return { text: "Set.", href: ex.explorer };
                    })
                  }
                >
                  Set it
                </Button>
                {out(limit.state)}
              </>
            }
          >
            {sentence(sg.basis)}. Tightening applies at once; loosening waits the delay.
          </Suggestion>
        )}
        {inbox.recommendAutopilot && inbox.mode !== "Autopilot" && (
          <Suggestion
            title="Autopilot"
            action={
              <>
                <Button
                  variant="outline"
                  disabled={autoRec.state.kind === "working"}
                  onClick={() =>
                    autoRec.run("Asking…", async () => {
                      const prep = await post<{ to: string; data: string }>("/api/level1/prepare-mode", { policy: tenant.policy, mode: 2 });
                      const ex = await sign(prep.to, prep.data, { fn: ["setMode"], data: build("setMode", [2]) });
                      refresh();
                      return { text: "Asked. It turns on after the delay unless you cancel.", href: ex.explorer };
                    })
                  }
                >
                  Turn autopilot on
                </Button>
                {out(autoRec.state)}
              </>
            }
          >
            You settled {inbox.approvedByYou} of its proposals yourself; none were refused by the contract.
          </Suggestion>
        )}
        {orders.map((o) => (
          <OrderRow key={o.id} o={o} />
        ))}
        {cands.slice(0, 4).map((c) => (
          <Suggestion
            key={c.payee + c.amount}
            title={`Make it standing: ${usd(c.amount)} USDC ${everyText(c.everySecs)}`}
            action={
              <Button
                variant="outline"
                disabled={so.state.kind === "working"}
                onClick={() =>
                  so.run("Setting…", async (step) => {
                    const ask = (firstDue: number) =>
                      post<{ to: string; data: string }>("/api/level1/prepare-schedule", {
                        policy: tenant.policy,
                        payee: c.payee,
                        amount: raw(c.amount),
                        everySecs: c.everySecs,
                        firstDue,
                      });
                    // A first due date already paid would pay that cycle twice: the service answers 409 with the next
                    // unpaid one, and the same tap starts the order there, saying so (docs/60 P-211).
                    let first = c.nextDue;
                    let moved = "";
                    let prep: { to: string; data: string };
                    try {
                      prep = await ask(first);
                    } catch (e) {
                      const next = e instanceof ApiError && e.status === 409 ? Number(e.data?.nextUnpaid) : NaN;
                      if (!(next > first)) throw e;
                      moved = `The ${dateText(first)} bill was already paid, so the order starts at the next unpaid one, ${dateText(next)}.`;
                      step(`${moved} Setting…`);
                      first = next;
                      prep = await ask(first).catch((e2: Error) => {
                        throw new Error(`${e2.message} (${moved})`);
                      });
                    }
                    const ex = await sign(prep.to, prep.data, {
                      fn: ["addSchedule"],
                      data: build("addSchedule", [c.payee, minor(c.amount), BigInt(c.everySecs), BigInt(first)]),
                    });
                    setTimeout(refresh, 3000);
                    return { text: moved ? `Standing order set. ${moved}` : "Standing order set.", href: ex.explorer };
                  })
                }
              >
                Make it standing
              </Button>
            }
          >
            To <Payee addr={c.payee} />. {sentence(c.basis)}
          </Suggestion>
        ))}
        {so.state.kind !== "idle" && <li className="px-4 pb-4 sm:px-5">{out(so.state)}</li>}
      </ul>
    </Section>
  );
}

type Order = NonNullable<Inbox["standingOrders"]>[number];

function OrderRow({ o }: { o: Order }) {
  const { tenant, sign, post, refresh } = useAccount();
  const pay = useAction();
  const end = useAction();
  const state = end.state.kind !== "idle" ? end.state : pay.state;
  return (
    <Suggestion
      title={`Standing order: ${usd(o.amount)} USDC ${everyText(o.everySecs)}`}
      action={
        <div className="flex flex-wrap gap-2">
          {o.due && (
            <Button
              variant="outline"
              disabled={state.kind === "working"}
              onClick={() =>
                pay.run("Paying…", async () => {
                  const r = await post<{ explorer: string }>("/api/level1/pay-due", { policy: tenant.policy, id: o.id });
                  setTimeout(refresh, 3000);
                  return { text: "Paid.", href: r.explorer };
                })
              }
            >
              Pay now
            </Button>
          )}
          <Button
            variant="ghost"
            disabled={state.kind === "working"}
            onClick={() =>
              end.run("Ending…", async () => {
                const prep = await post<{ to: string; data: string }>("/api/level1/prepare-unschedule", { policy: tenant.policy, id: o.id });
                const ex = await sign(prep.to, prep.data, { fn: ["removeSchedule"], data: build("removeSchedule", [BigInt(o.id)]) });
                setTimeout(refresh, 3000);
                return { text: "Ended.", href: ex.explorer };
              })
            }
          >
            End it
          </Button>
          {state.kind === "error" && <p className="w-full text-sm text-destructive">{state.text}</p>}
          {state.kind === "done" && (
            <p className="w-full text-sm text-protect">
              {state.text} <Receipt href={state.href} />
            </p>
          )}
        </div>
      }
    >
      To <Payee addr={o.payee} /> · {o.due ? "due now: the agent pays it on its next check" : `next on ${new Date(o.nextDue * 1000).toLocaleDateString()}`}. Paid on its day by
      whoever is awake — the agent, you, anyone.
    </Suggestion>
  );
}
