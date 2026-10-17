"use client";

import { useState } from "react";
import { Mandate } from "@/components/layout/page";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { build, minor } from "@/lib/guard";
import { num, raw, usd } from "@/lib/tameion";
import { Receipt, useAccount, useAction } from "./ctx";
import { useKeysGate } from "./keys-nag";

/** The outcome the owner signs, in the owner's voice, filled from real values
 *  (docs/73 T1). `mode` undefined is the sentence signed at Start; on the
 *  account it follows the contract's mode, so it never says more than is true.
 *  The words are not in the signed terms (`termsFingerprint` covers the caps,
 *  floor, mode, keys, delay and pending change — numbers only); the price clause
 *  shows only for an account with an exchange connected (docs/109 §10). */
export function MandateSentence({ n, daily, mode, hedged }: { n: number; daily?: string | null; mode?: string; hedged?: boolean }) {
  if (mode === "Watch") return <>Pay nothing for now; keep reading and reporting; ask me for anything else.</>;
  const bills = n > 1 ? `these ${n} bills` : n === 1 ? "this bill" : "the bills you find";
  return (
    <>
      {mode === "Propose" ? "Prepare " : "Pay "}
      <strong>{bills}</strong> when due{mode === "Propose" ? " and wait for my tap" : ""}, up to <strong className="figures">{usd(daily) || "—"} USDC a day</strong>, only to
      payees I approved; {hedged ? "keep the money they need protected from a price fall; " : ""}ask me for anything else.
    </>
  );
}

/** The account's standing instruction, with "Change" for the limits (a trust
 *  decision: lowering applies at once, raising waits the delay). */
export function AccountMandate() {
  const { inbox, owns } = useAccount();
  const [open, setOpen] = useState(false);
  if (!inbox) return null;
  const reserved = inbox.deployed === false && !inbox.lastPass;
  const n = inbox.lastPass?.obligations ?? inbox.preview?.bills?.length ?? 0;
  const daily = reserved ? inbox.chosenDaily : inbox.caps?.daily;
  return (
    <>
      <Mandate
        signed={!reserved}
        action={
          !reserved && inbox.caps && owns ? (
            <Button variant="outline" size="sm" onClick={() => setOpen(true)}>
              Change
            </Button>
          ) : undefined
        }
        note={
          reserved
            ? "A starting limit, sized from the bills it knows: it moves as you add bills, and you confirm it with your passkey when money arrives."
            : "Signed with your passkey; the contract enforces the limits."
        }
      >
        <MandateSentence n={n} daily={daily} mode={reserved ? undefined : inbox.mode} hedged={!!inbox.exposure?.hedge?.on} />
      </Mandate>
      {open && <LimitsDialog onClose={() => setOpen(false)} />}
    </>
  );
}

function LimitsDialog({ onClose }: { onClose: () => void }) {
  const { inbox, tenant, cfg, sign, post, refresh } = useAccount();
  const { state, run } = useAction();
  const caps = inbox!.caps!;
  const [daily, setDaily] = useState(raw(caps.daily));
  const [per, setPer] = useState(raw(caps.perAction));
  const raising = num(daily) > num(caps.daily) || num(per) > num(caps.perAction);
  const keys = useKeysGate(tenant);
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Change the limits</DialogTitle>
          <DialogDescription>
            Lowering applies at once. Raising waits {cfg.delaySecs} s, and you can cancel it until then.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-4 sm:grid-cols-2">
          <div className="grid gap-2">
            <Label htmlFor="lim-daily">A day, at most</Label>
            <Input id="lim-daily" inputMode="decimal" className="figures" value={daily} onChange={(e) => setDaily(e.target.value)} />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="lim-per">One payment, at most</Label>
            <Input id="lim-per" inputMode="decimal" className="figures" value={per} onChange={(e) => setPer(e.target.value)} />
          </div>
        </div>
        {state.kind === "error" && <p className="text-sm text-destructive">{state.text}</p>}
        {state.kind === "done" && (
          <p className="text-sm text-protect">
            {state.text} <Receipt href={state.href} />
          </p>
        )}
        <DialogFooter>
          <Button
            disabled={state.kind === "working" || !(num(daily) > 0) || !(num(per) > 0) || num(per) > num(daily)}
            onClick={() =>
              run("Signing…", async () => {
                if (raising && !(await keys.gate(`raise the limits to ${usd(per)} USDC a payment and ${usd(daily)} USDC a day`))) throw new Error("cancelled");
                let data: ReturnType<typeof build>;
                try {
                  data = build("setLimits", [minor(per), minor(daily), minor(caps.floor)]);
                } catch {
                  throw new Error("Write each limit as a plain number of USDC, e.g. 1200.50.");
                }
                const prep = await post<{ to: string; data: string }>("/api/level1/prepare-limits", {
                  policy: tenant.policy,
                  perAction: per.trim(),
                  daily: daily.trim(),
                  floor: raw(caps.floor),
                });
                const ex = await sign(prep.to, prep.data, { fn: ["setLimits"], data });
                refresh();
                return {
                  text: raising ? "Asked. The higher limit applies after the delay unless you cancel." : "Lowered, at once.",
                  href: ex.explorer,
                };
              })
            }
          >
            {state.kind === "working" ? state.label : "Sign with my passkey"}
          </Button>
        </DialogFooter>
        {keys.dialog}
      </DialogContent>
    </Dialog>
  );
}
