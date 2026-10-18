"use client";

import { Cpu, Fingerprint, ShieldAlert, ShieldCheck, Wallet } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { build } from "@/lib/guard";
import { connectGuardian, keysReady, keysSet, landed, useKeys, ZERO_ADDRESS } from "@/lib/keys";
import { post, sign, spanText, tail } from "@/lib/tameion";
import type { Cfg, Pub, Tenant } from "@/lib/types";
import { Receipt, useAction, type ActionState } from "./ctx";
import { Identicon } from "./payee";

const same = (a?: string | null, b?: string | null) => !!a && !!b && a.toLowerCase() === b.toLowerCase();

function Out({ state }: { state: ActionState }) {
  if (state.kind === "error") return <p className="text-sm text-destructive">{state.text}</p>;
  if (state.kind === "working") return <p className="text-sm text-muted-foreground">{state.label}</p>;
  if (state.kind === "done")
    return (
      <p className="break-all text-sm text-protect">
        {state.text} <Receipt href={state.href} />
      </p>
    );
  return null;
}

function Slot({ icon: Icon, name, children, set }: { icon: typeof Wallet; name: string; children: React.ReactNode; set: boolean }) {
  return (
    <li className="flex items-start gap-3 px-3 py-3 sm:px-4">
      <Icon className={`mt-0.5 size-4 shrink-0 ${set ? "text-protect" : "text-muted-foreground"}`} aria-hidden />
      <div className="min-w-0 flex-1 space-y-1 text-sm">
        <p className="font-medium">
          {name} <span className={`ml-1 text-caption font-normal ${set ? "text-protect" : "text-attention"}`}>{set ? "set" : "not set"}</span>
        </p>
        {children}
      </div>
    </li>
  );
}

/** The owner's three keys (docs/104): the passkey that signs, and two guardians who together can replace it after the
 *  recovery delay. State read from the chain; both guardians named with one passkey tap; nothing typed. */
export function Keys({ tenant, cfg, pub, owns }: { tenant: Tenant; cfg: Cfg; pub: Pub | null; owns: boolean }) {
  const q = useKeys(tenant);
  const k = q.data;
  const [picked, setPicked] = useState<[string | null, string | null]>([null, null]);
  const connect = useAction();
  const save = useAction();
  const chain = cfg.chains.find((c) => c.key === tenant.chain);
  const chip = pub?.credentialId === "secure-enclave";
  const named = k?.guardians ?? [];
  const slots = [picked[0] ?? named[0] ?? null, picked[1] ?? named[1] ?? null];
  const differs = slots.some((a, i) => !!a && !same(a, named[i])) || (named.length === 2 && (k?.threshold ?? 0) < 2);
  const ready = !!k && keysReady(k);
  const tameionRecovery = !!k && !same(k.recovery, ZERO_ADDRESS) && same(k.recovery, cfg.operator);
  const delay = k?.delaySecs ? spanText(k.delaySecs) : spanText(cfg.delaySecs);
  const busy = connect.state.kind === "working" || save.state.kind === "working";
  const { refetch } = q;
  // The watcher's "keys" event (a replacement key proposed): read the chain again at once, not on the next poll.
  useEffect(() => {
    if (typeof EventSource === "undefined") return;
    const es = new EventSource(`/api/level1/events?policy=${tenant.policy}`);
    es.addEventListener("keys", () => void refetch());
    es.onerror = () => es.close();
    return () => es.close();
  }, [tenant.policy, refetch]);

  const pick = (i: 0 | 1) =>
    connect.run("Waiting for your wallet…", async () => {
      if (!chain) throw new Error("This page does not know the account's chain; reload it.");
      const other = slots[i === 0 ? 1 : 0];
      const a = await connectGuardian(tenant, chain.chainId, [
        { address: cfg.operator, why: "That is Tami's own key. A guardian is a wallet you, or someone you trust, holds." },
        { address: tenant.owner, why: "That is this account itself." },
        { address: tenant.policy, why: "That is this account itself." },
        { address: other, why: `That wallet is already guardian ${i === 0 ? 2 : 1}. Pick a different wallet: two accounts of one wallet share one recovery phrase.` },
      ]);
      setPicked((p) => (i === 0 ? [a, p[1]] : [p[0], a]));
    });

  const name = () =>
    save.run("Asking your passkey…", async (step) => {
      const guardians = slots as string[];
      // The page builds the call itself and signs only if the service prepared exactly that (lib/guard.ts).
      const data = build("setGuardians", [guardians, 2]);
      const prep = await post<{ to: string; data: string }>("/api/level1/prepare-guardians", { owner: tenant.owner, guardians, threshold: 2 }, tenant.policy);
      const ex = await sign({ tenant, cfg, inbox: undefined, pub }, prep.to, prep.data, { fn: ["setGuardians"], data });
      step("Waiting for the chain…");
      await landed(tenant.chain, ex.tx);
      await refetch();
      setPicked([null, null]);
      return { text: "Guardians named. Any two of your three keys can now replace the third.", href: ex.explorer };
    });

  // A replacement the owner did not ask for: the daily key cancels it inside the delay (PasskeyPrincipal.sol:117). The page
  // builds the call; there is nothing for the service to prepare.
  const cancelReplacement = () =>
    save.run("Asking your passkey…", async (step) => {
      const data = build("cancelRecovery", []);
      const ex = await sign({ tenant, cfg, inbox: undefined, pub }, tenant.owner, data, { fn: ["cancelRecovery"], data });
      step("Waiting for the chain…");
      await landed(tenant.chain, ex.tx);
      await refetch();
      return { text: "Cancelled. Your key stays; the guardians would have to start again.", href: ex.explorer };
    });

  const dropTameion = () =>
    save.run("Asking your passkey…", async (step) => {
      const data = build("setRecovery", [ZERO_ADDRESS]);
      const prep = await post<{ to: string; data: string }>("/api/level1/prepare-recovery", { owner: tenant.owner, recovery: ZERO_ADDRESS }, tenant.policy);
      const ex = await sign({ tenant, cfg, inbox: undefined, pub }, prep.to, prep.data, { fn: ["setRecovery"], data });
      step("Waiting for the chain…");
      await landed(tenant.chain, ex.tx);
      await refetch();
      return { text: "Removed. Only your guardians can replace your key now.", href: ex.explorer };
    });

  const wallet = (a: string, i: number) => (
    <span className="inline-flex items-center gap-1.5">
      <Identicon addr={a} />
      <a className="font-medium underline-offset-4 hover:underline" href={chain ? `${chain.explorer}/address/${a}` : undefined} target="_blank" rel="noopener" title={a}>
        Wallet ••{tail(a)}
      </a>
      <span className="text-muted-foreground">{same(a, named[i]) ? "· on chain" : "· connected, not named yet"}</span>
    </span>
  );

  return (
    <div className="space-y-3 border-b pb-6 last:border-b-0">
      <div>
        <h3 className="text-title font-semibold">Keys</h3>
        {!k ? (
          <p className="mt-1 text-sm text-muted-foreground">{q.error ? `Could not read your keys from the chain: ${(q.error as Error).message}` : "Reading your keys from the chain…"}</p>
        ) : ready ? (
          <p className="mt-1 flex items-start gap-1.5 text-sm text-protect">
            <ShieldCheck className="mt-0.5 size-4 shrink-0" aria-hidden />
            <span>3 of 3 keys set. Any two can replace the third; a guardian cannot spend.</span>
          </p>
        ) : (
          <p role="status" className="mt-2 flex items-start gap-2 rounded-lg border border-attention/45 bg-attention-soft/40 px-3 py-2 text-sm">
            <ShieldAlert className="mt-0.5 size-4 shrink-0 text-attention" aria-hidden />
            <span>
              <span className="font-medium">{keysSet(k)} of 3 keys set</span> — add two guardians before raising a limit.{" "}
              {chip ? "This account's key lives in this Mac's chip: if the Mac is lost, nobody can replace it." : "If you lose your passkey today, nobody can replace it."}
            </span>
          </p>
        )}
      </div>

      {k?.pending && (
        <p role="alert" className="rounded-lg border border-destructive/40 px-3 py-2 text-sm text-destructive">
          A new key is waiting to replace yours at {new Date(k.pending.eta * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}. If you did not ask for it,
          cancel it before then.{" "}
          {owns && (
            <Button variant="destructive" size="sm" className="ml-1" disabled={busy} onClick={cancelReplacement}>
              Cancel it now
            </Button>
          )}
        </p>
      )}
      <ol className="divide-y rounded-lg border">
        <Slot icon={chip ? Cpu : Fingerprint} name="Daily key" set>
          <p className="text-muted-foreground">
            {chip
              ? "In this Mac's security chip. It cannot be copied, so it is lost with this Mac. Touch ID signs every decision."
              : "Your passkey. When your Apple or Google account syncs it, a lost laptop loses nothing; Face ID or a PIN signs every decision."}
          </p>
        </Slot>
        {([0, 1] as const).map((i) => (
          <Slot key={i} icon={Wallet} name={`Guardian ${i + 1}`} set={same(slots[i], named[i])}>
            {slots[i] ? <p>{wallet(slots[i]!, i)}</p> : <p className="text-muted-foreground">{i === 0 ? "Your email wallet, or a hardware wallet." : "A hardware wallet in a drawer, or someone you trust — not your daily phone."}</p>}
            {owns && (
              <Button variant="outline" size="sm" className="mt-1" disabled={busy} onClick={() => pick(i)}>
                {slots[i] ? "Use another wallet" : "Connect a wallet"}
              </Button>
            )}
          </Slot>
        ))}
      </ol>

      {named.length > 2 && (
        <p className="text-sm text-attention">
          This account has {named.length} guardians on chain ({named.map((g) => `••${tail(g)}`).join(", ")}), {k?.threshold} of whom must agree. Naming two
          here replaces the whole list.
        </p>
      )}
      {owns && slots[0] && slots[1] && differs && (
        <Button disabled={busy} onClick={name}>
          Name both guardians — one passkey tap
        </Button>
      )}
      <Out state={connect.state.kind === "idle" ? save.state : connect.state} />

      {k && !same(k.recovery, ZERO_ADDRESS) && (
        <p className="text-sm text-muted-foreground">
          {tameionRecovery ? (
            <>
              <span className="text-attention">Tami&apos;s relayer can also propose a new key for you alone</span> (a test-account setting).{" "}
              {owns && (
                <Button variant="link" size="sm" className="h-auto p-0" disabled={busy} onClick={dropTameion}>
                  Remove it
                </Button>
              )}
            </>
          ) : (
            <>
              Wallet ••{tail(k.recovery)} can also propose a new key alone; it waits {delay} and you can cancel it.
            </>
          )}
        </p>
      )}
      <p className="text-caption leading-relaxed text-muted-foreground">
        Two guardians together can name a new daily key; it waits {delay}, and your daily key can cancel it. On your iPhone, turn on Stolen Device
        Protection: away from home your passkey then needs your face, not just your passcode.
      </p>
    </div>
  );
}
