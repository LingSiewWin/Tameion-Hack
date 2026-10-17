"use client";

import { Check, CheckCircle2, Inbox, Loader2 } from "lucide-react";
import { EmptyState, Row as Line, Rows, Section, Why } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { invoiceDecideChallenge, minor, PAY, PAYEES } from "@/lib/guard";
import { dateText, everyText, num, passkeyAssert, plainWhy, pretty, raw, TC, usd } from "@/lib/tameion";
import type { Inbox as InboxData, Invoice, NeedRow, Seller } from "@/lib/types";
import { useConfirm } from "./confirm";
import { Receipt, useAccount, useAction, type ActionState } from "./ctx";
import { partyOf, Payee } from "./payee";

function Row({
  title,
  why,
  state,
  children,
  emphasis,
}: {
  title: React.ReactNode;
  why?: React.ReactNode;
  state: ActionState;
  children?: React.ReactNode;
  emphasis?: boolean;
}) {
  return (
    <Line
      emphasis={emphasis}
      title={title}
      detail={
        state.kind === "done" ? (
          <p className="flex flex-wrap items-center gap-1.5 text-protect">
            <Check className="size-4" /> {state.text} <Receipt href={state.href} />
          </p>
        ) : state.kind === "error" ? (
          <p role="alert" className="text-destructive">
            {state.text}
          </p>
        ) : (
          why
        )
      }
      actions={state.kind !== "done" ? children : undefined}
    />
  );
}

function Working({ state, label }: { state: ActionState; label: string }) {
  return state.kind === "working" ? (
    <>
      <Loader2 className="size-4 animate-spin" /> {state.label}
    </>
  ) : (
    <>{label}</>
  );
}

export function NeedsYou() {
  const { inbox } = useAccount();
  const { confirm, dialog } = useConfirm();
  if (!inbox) return null;
  const sellers = inbox.sellersToApprove ?? [];
  const prices = inbox.pricesToApprove ?? [];
  const invoices = inbox.invoices ?? [];
  const empty = !inbox.needsYou.length && !sellers.length && !invoices.length && !prices.length;
  // P-59: a bill whose reference was used on chain without paying its payee is
  // refused, and the owner has something to do; say it in the service's words.
  const misused = (inbox.refused ?? []).filter((x) => /used on chain without paying/.test(x.why || ""));
  const refused = (inbox.refused ?? []).filter((x) => !misused.includes(x));
  return (
    <Section title="Needs you" icon={Inbox} tone={empty ? "default" : "attention"} description={empty ? undefined : "The agent prepared these. Your tap decides."} flush>
      {empty ? (
        <EmptyState icon={CheckCircle2} title="Nothing waiting">
          {inbox.mode === "Propose" ? "While the agent asks before paying, every bill comes here first; it buys nothing by itself until Autopilot." : undefined}
        </EmptyState>
      ) : (
        <Rows>
          {inbox.run && inbox.needsYou.length >= 2 && <PayAll confirm={confirm} />}
          {prices.map((x) => (
            <PriceRow key={x.seller + x.amount} x={x} />
          ))}
          {sellers.map((s) => (
            <SellerRow key={s.seller + s.host} s={s} confirm={confirm} />
          ))}
          {invoices.map((v) => (
            <InvoiceRow key={v.id} v={v} />
          ))}
          {inbox.needsYou.map((n) => (
            <NeedItem key={n.actionHash ?? n.counterparty + n.amount} n={n} />
          ))}
        </Rows>
      )}
      {misused.map((x, i) => (
        <p key={i} role="alert" className="border-t px-4 py-3 text-sm leading-relaxed text-destructive sm:px-5">
          {usd(x.amount)} USDC to <Payee addr={x.counterparty} nameable={false} />: {pretty(x.why)}.
        </p>
      ))}
      {!!refused.length && (
        <p className="border-t px-4 py-3 text-xs leading-relaxed text-destructive sm:px-5">
          Refused by the checks, nothing for you to do:{" "}
          {refused.slice(0, 4).map((x, i) => (
            <span key={i}>
              {i > 0 && "; "}
              {usd(x.amount)} USDC to <Payee addr={x.counterparty} nameable={false} /> ({plainWhy((x.why || "").split(" · ")[0])})
            </span>
          ))}
          {refused.length > 4 && ` and ${refused.length - 4} more`}.
        </p>
      )}
      {dialog}
    </Section>
  );
}

type Confirm = ReturnType<typeof useConfirm>["confirm"];

function PayAll({ confirm }: { confirm: Confirm }) {
  const { inbox, sign, refresh, explorer } = useAccount();
  const { state, run } = useAction();
  const r = inbox!.run!;
  return (
    <Row
      emphasis
      state={state}
      title={
        <span className="font-semibold">
          Pay all {r.count} — <span className="figures">{usd(r.total)} USDC</span>
        </span>
      }
      why={
        <>
          One approval for every bill below. A bill already paid is skipped, never paid twice.{" "}
          <Why>If the limits or keys change before it lands, nothing is paid.</Why>
        </>
      }
    >
      <Button
        disabled={state.kind === "working"}
        onClick={() =>
          run("Paying…", async () => {
            const d = TC().describe(r.data);
            // The dialog's numbers come from the call itself, not the service's summary.
            const ok = await confirm({
              title: `Pay ${d.rows?.length ?? 0} bills, ${usd(d.total)} USDC`,
              action: "Approve with passkey",
              body: (
                <div className="space-y-3">
                  <ul className="divide-y rounded-lg border">
                    {(d.rows ?? []).map((x) => (
                      <li key={x.to} className="flex justify-between gap-3 px-3 py-2 text-foreground">
                        <span className="min-w-0">
                          <Payee addr={x.to} nameable={false} />
                        </span>
                        <span className="figures shrink-0">{usd(x.amount)} USDC</span>
                      </li>
                    ))}
                  </ul>
                  <p>A bill already paid is skipped. Valid for ten minutes, and only while the terms you see hold.</p>
                </div>
              ),
            });
            if (!ok) throw new Error("cancelled");
            const ex = await sign(r.to, r.data, { fn: ["settleRunIf"], within: inbox!.needsYou.map((n) => ({ to: n.counterparty, amount: minor(n.amount) })) });
            setTimeout(refresh, 4000);
            return { text: `Paid ${r.count} bills, ${usd(r.total)} USDC, with one approval.`, href: ex.explorer || (ex.tx ? `${explorer}/tx/${ex.tx}` : undefined) };
          })
        }
      >
        <Working state={state} label={`Pay all ${r.count}`} />
      </Button>
    </Row>
  );
}

function NeedItem({ n }: { n: NeedRow }) {
  const { sign, refresh, post, tenant } = useAccount();
  const pay = useAction();
  const ended = useAction();
  const mayHaveEnded = /cycles-behind|may have ended/i.test(n.why || "");
  const state = ended.state.kind !== "idle" ? ended.state : pay.state;
  return (
    <Row
      state={state}
      title={
        <>
          Pay <span className="figures font-semibold">{usd(n.amount)} USDC</span> to <Payee addr={n.counterparty} />
        </>
      }
      why={[n.reason, plainWhy(n.why)].filter(Boolean).join(" · ")}
    >
      {mayHaveEnded && (
        <Button
          variant="outline"
          disabled={state.kind === "working"}
          onClick={() =>
            ended.run("Stopping…", async () => {
              const prep = await post<{ to: string; data: string }>("/api/level1/prepare-payees", { policy: tenant.policy, payees: [n.counterparty], allow: false });
              const ex = await sign(prep.to, prep.data, { fn: PAYEES, attested: false, rows: [{ to: n.counterparty }] });
              setTimeout(refresh, 4000);
              return { text: "Stopped paying this payee. The agent will not ask again.", href: ex.explorer };
            })
          }
        >
          <Working state={ended.state} label="Bill ended" />
        </Button>
      )}
      <Button
        disabled={state.kind === "working"}
        onClick={() =>
          pay.run(n.attest ? "Approving…" : "Paying…", async (step) => {
            if (n.attest) await sign(n.attest.to, n.attest.data, { fn: PAYEES, attested: true, rows: [{ to: n.counterparty }] });
            step("Paying…");
            const ex = await sign(n.to, n.data, { fn: PAY, rows: [{ to: n.counterparty, amount: minor(n.amount) }] });
            setTimeout(refresh, 4000);
            return { text: `Paid ${usd(n.amount)} USDC.`, href: ex.explorer };
          })
        }
      >
        <Working state={pay.state} label={n.attest ? "Approve & pay" : "Pay"} />
      </Button>
    </Row>
  );
}

/** A purchase held because it costs more than 25 % above what this seller usually charges: one tap lets one such purchase
 *  through within ten minutes (`POST /api/level1/purchase-approve`, the owner's device only); the limits still bound it. */
function PriceRow({ x }: { x: NonNullable<InboxData["pricesToApprove"]>[number] }) {
  const { post, tenant, refresh } = useAccount();
  const { state, run } = useAction();
  const host = x.host || "this seller";
  return (
    <Row
      state={state}
      title={
        <>
          Let the agent buy from <span className="font-semibold">{host}</span> at <span className="figures">{usd(x.amount)} USDC</span>
          {x.usual ? (
            <>
              {" "}
              (usually <span className="figures">{usd(x.usual)}</span>)
            </>
          ) : null}
        </>
      }
      why={`${x.reason ? `It wanted it for: ${x.reason}. ` : ""}It held this purchase because the price is more than a quarter above what ${host} usually charges. One tap lets one purchase through at up to this price within ten minutes; your limits still bound it.`}
    >
      <Button
        disabled={state.kind === "working"}
        onClick={() =>
          run("Letting it through…", async () => {
            const r = await post<{ until: number }>("/api/level1/purchase-approve", { policy: tenant.policy, seller: x.seller, amount: raw(x.amount) });
            setTimeout(refresh, 3000);
            const until = new Date(r.until * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
            return { text: `The agent may buy once from ${host} at up to ${usd(x.amount)} USDC until ${until}.` };
          })
        }
      >
        <Working state={state} label="Let it through" />
      </Button>
    </Row>
  );
}

function SellerRow({ s, confirm }: { s: Seller; confirm: Confirm }) {
  const { sign, refresh, inbox, cfg } = useAccount();
  const { state, run } = useAction();
  // An outside agent's request for a payee reads as a payee, not a seller (docs/110 row 9): its amount is the per-payment limit.
  const payee = !!s.askedBy;
  // x402 buying needs Autopilot (docs/60 P-174): on "Asks before paying" approving is not enough, and the row says so.
  const auto = inbox?.mode === "Autopilot";
  const what = payee ? "pay it" : `buy from ${s.host}`;
  return (
    <Row
      state={state}
      title={
        payee ? (
          <>
            Let the agent pay <Payee addr={s.seller} /> — up to <span className="figures">{usd(s.amount)} USDC</span> a payment
          </>
        ) : (
          <>
            Let the agent buy from <span className="font-semibold">{s.host}</span> — <span className="figures">{usd(s.amount)} USDC</span> a
            call, paid to <Payee addr={s.seller} />
          </>
        )
      }
      why={`${payee ? `${s.askedBy} asked` : "It wanted it for"}: ${s.reason ?? "—"}. ${
        auto
          ? `After this it may ${what} by itself, never more than your limits.`
          : `It may ${what} by itself only on Autopilot; you are on Asks before paying, so after you approve it still waits until you turn Autopilot on.`
      }`}
    >
      <Button
        disabled={state.kind === "working"}
        onClick={() =>
          run("Approving…", async () => {
            const ok = await confirm({
              title: payee ? "Let the agent pay this payee?" : `Let the agent buy from ${s.host}?`,
              action: "Approve with passkey",
              body: (
                <p>
                  Approves <Payee addr={s.seller} nameable={false} /> as a payee, so the agent may {what} inside your limits
                  {auto ? "" : " once Autopilot is on"}. Valid for ten minutes, and only while the terms you see hold.
                </p>
              ),
            });
            if (!ok) throw new Error("cancelled");
            const ex = await sign(s.to, s.data, { fn: PAYEES, attested: true, rows: [{ to: s.seller }] });
            setTimeout(refresh, 4000);
            // The contract's waiting period, said as a time (docs/60 P-172 item 9).
            const from = new Date(Date.now() + cfg.delaySecs * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
            return {
              text: `Approved. The agent may ${what} from about ${from}, after the ${cfg.delaySecs} s waiting period${auto ? "" : ", and only once Autopilot is on"}.`,
              href: ex.explorer,
            };
          })
        }
      >
        <Working state={state} label="Approve" />
      </Button>
    </Row>
  );
}

function InvoiceRow({ v }: { v: Invoice }) {
  const { inbox, sign, refresh, post, tenant, runPass, cfg, pub } = useAccount();
  const yes = useAction();
  const no = useAction();
  // The owner's decision, signed by the passkey over a challenge this page
  // computes (docs/60 P-72/P-73). An account locked to agent keys takes this
  // browser's key instead, as before.
  const decide = async (accept: boolean) => {
    let signed = {};
    if (!inbox?.agentTokens?.length) {
      const chainId = cfg.chains.find((c) => c.key === tenant.chain)?.chainId;
      if (!chainId) throw new Error("This page does not know the account's chain; reload it.");
      const issued = Math.floor(Date.now() / 1000);
      signed = { issued, auth: await passkeyAssert(invoiceDecideChallenge(chainId, tenant.policy, v.id, accept, issued), pub) };
    }
    await post("/api/level1/invoice-decide", { policy: tenant.policy, id: v.id, accept, ...signed });
  };
  const state = no.state.kind !== "idle" ? no.state : yes.state;
  const every = v.everyDays ? everyText(v.everyDays * 86400) : "once";
  const terms = v.discountBps ? ` · ${v.discountBps / 100}% off if paid ${v.earlyDays} days early` : "";
  // A one-off is paid by the owner's own signature, which needs no approved
  // payee: approving one would let the vendor's next repeating invoice in
  // without the passkey (docs/60 P-46).
  const label = v.everyDays ? (v.approvedPayee ? "Accept" : "Approve & accept") : "Pay";
  const notes = (v.notes ?? []).filter((x) => !((v.approvedPayee || !v.everyDays) && /approved payees yet/.test(x))).map(pretty);
  const gap = !v.everyDays ? num(v.amountText) - num(inbox?.balance) : 0;
  // docs/81: an invoice whose sender claims a name another payee holds says so first.
  const party = partyOf(inbox, v.payee);
  if (party?.kind === "clash") notes.unshift(`Careful: ${party.note}.`);
  if (gap > 0) notes.push(`The account can reach ${usd(inbox?.balance)} USDC now; this needs ${usd(gap.toFixed(6))} more before it can be paid.`);
  return (
    <Row
      state={state}
      title={
        <>
          Invoice from <Payee addr={v.payee} />: <span className="figures font-semibold">{usd(v.amountText)} USDC</span>
          <span className="text-muted-foreground">
            {v.dueAt ? `, due ${dateText(v.dueAt)}` : ""}, {every}
            {terms}
          </span>
        </>
      }
      why={notes.join(" ")}
    >
      <Button
        variant="outline"
        disabled={state.kind === "working"}
        onClick={() =>
          no.run("Declining…", async () => {
            await decide(false);
            setTimeout(refresh, 2000);
            return { text: "Declined." };
          })
        }
      >
        <Working state={no.state} label="Decline" />
      </Button>
      <Button
        disabled={state.kind === "working"}
        onClick={() =>
          yes.run(v.everyDays && !v.approvedPayee ? "Approving…" : v.everyDays ? "Accepting…" : "Paying…", async (step) => {
            let href: string | undefined;
            if (v.everyDays && !v.approvedPayee) {
              const prep = await post<{ to: string; data: string }>("/api/level1/prepare-payees", { policy: tenant.policy, payees: [v.payee] });
              href = (await sign(prep.to, prep.data, { fn: PAYEES, attested: true, rows: [{ to: v.payee }] })).explorer;
            }
            if (!v.everyDays) {
              if (!v.pay) throw new Error("The account is not ready to pay yet; try again after its first check.");
              step("Paying…");
              href = (await sign(v.pay.to, v.pay.data, { fn: PAY, rows: [{ to: v.payee, amount: minor(v.amountText) }] })).explorer;
            }
            step("Accepting…");
            await decide(true);
            if (v.everyDays) void runPass(true);
            setTimeout(refresh, 4000);
            return v.everyDays
              ? { text: `Added: ${usd(v.amountText)} USDC ${every}. The agent pays it when due, inside your limits.`, href }
              : { text: `Paid ${usd(v.amountText)} USDC.`, href };
          })
        }
      >
        <Working state={yes.state} label={label} />
      </Button>
    </Row>
  );
}
