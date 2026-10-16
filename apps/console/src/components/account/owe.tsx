"use client";

import { Fingerprint, Loader2, ReceiptText, Wallet, X } from "lucide-react";
import { useRef, useState } from "react";
import { Section } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { build, PAYEES, reserveTokenChallenge } from "@/lib/guard";
import { dueText, eth, everyText, isAddr, isZero, looksEnded, passkeyAssert, reserveToken, saveName, spanText, usd, walletAccount } from "@/lib/tameion";
import type { Preview, WatchJob, WatchReport } from "@/lib/types";
import { useAccount, useAction, useRecord } from "./ctx";
import { partyOf, Payee, TypedParty, useTyped } from "./payee";

type Read = { wallet: string } & ({ state: "reading"; pct: number } | { state: "done"; report: WatchReport } | { state: "failed"; error: string });
type Bill = NonNullable<Preview["bills"]>[number];

/** docs/79 §6a Console 2: "Tell it what you owe" — connect the wallets that pay
 *  today so the agent finds the bills, or type one. Nothing here is a trust
 *  decision on a reserved account; on a live one, typing a bill approves its
 *  payee with the passkey (the contract refuses an unapproved payee). */
export function Owe() {
  const { tenant, inbox, cfg, pub, post, refresh, runPass, chainName } = useAccount();
  const rec = useRecord(tenant.policy);
  const add = useAction();
  const drop = useAction();
  const unlock = useAction();
  // The device token this browser was given when it opened the account
  // (docs/60 P-87): a reserved account's wallets and first bills take it, a
  // live account's wallets too (P-93).
  const [hasToken, setHasToken] = useState(() => !!reserveToken.get(tenant.policy));
  const locked = !!inbox && !hasToken;
  const [wallet, setWallet] = useState("");
  const typedWallet = useTyped(wallet, tenant.chain);
  const [read, setRead] = useState<Read | null>(null);
  const readKey = useRef("");
  const wallets = rec.data?.wallets ?? tenant.wallets ?? [];
  // An account opened by name has no pasted wallet: what the agent found is shown
  // here, not in Money's "What the agent found" (which shows it for the others).
  const nameFirst = !isAddr(tenant.watched) || isZero(tenant.watched);
  const gate = () => ({ reserveToken: reserveToken.get(tenant.policy) ?? undefined });

  // The read `/watch` runs (90 days, nothing created): what the wallet just
  // connected pays, shown at once; the agent's own read follows on its check.
  async function readWallet(address: string) {
    const key = `${tenant.chain}:${address.toLowerCase()}`;
    readKey.current = key;
    setRead({ wallet: address, state: "reading", pct: 0 });
    try {
      const r = await post<{ id: string }>("/api/watch", { chain: tenant.chain, address, days: 90 });
      for (let n = 0; n < 160 && readKey.current === key; n++) {
        const j: WatchJob = await (await fetch(`/api/watch/${r.id}`)).json();
        if (readKey.current !== key) return;
        if (j.status === "done" && j.report) return setRead({ wallet: address, state: "done", report: j.report });
        if (j.status === "failed") throw new Error(j.error || "the read failed");
        const p = j.progress || {};
        setRead({ wallet: address, state: "reading", pct: p.totalBlocks && p.scannedBlocks ? Math.round((100 * p.scannedBlocks) / p.totalBlocks) : 0 });
        await new Promise((res) => setTimeout(res, 1000));
      }
      if (readKey.current === key) throw new Error("it is taking long");
    } catch (e) {
      if (readKey.current === key) setRead({ wallet: address, state: "failed", error: (e as Error).message });
    }
  }

  const connect = (address: string) =>
    add.run("Connecting…", async () => {
      const a = address.trim();
      if (!isAddr(a)) throw new Error(typedWallet.found?.error ?? "Type the wallet's ENS name (alice.eth), or paste its address.");
      await post("/api/level1/wallets", { policy: tenant.policy, add: a, ...gate() });
      setWallet("");
      void rec.refetch();
      refresh();
      if (inbox?.deployed) void runPass();
      void readWallet(a);
      return { text: "Wallet connected. Reading what it paid…" };
    });

  // The bills to show: the wallet just read, else (account opened by name) the agent's own read.
  const pv = inbox?.preview;
  const found: Bill[] | null =
    read?.state === "done" ? read.report.obligations : nameFirst && wallets.length && pv?.status === "done" ? (pv.bills ?? []) : null;
  const ended = found?.filter((b) => looksEnded(b.nextDue, b.intervalSecs)).length ?? 0;

  return (
    <Section title="Tell it what you owe" icon={ReceiptText} description="It pays what you owe on time; first it needs to know what that is.">
      <div className="space-y-5 text-sm leading-relaxed">
        {locked && (
          <div className="space-y-2 rounded-lg border p-3">
            <p>
              {inbox?.deployed
                ? "The wallets it reads can be changed only from the device that opened this account."
                : "Until money lands, this account can be changed only from the device that opened it."}
            </p>
            <p className="text-caption text-muted-foreground">On another device with the same passkey? Confirm it once here.</p>
            <Button
              size="sm"
              variant="outline"
              disabled={unlock.state.kind === "working"}
              onClick={() =>
                unlock.run("Confirming…", async () => {
                  const chainId = cfg.chains.find((c) => c.key === tenant.chain)?.chainId;
                  if (!chainId) throw new Error("This page does not know the account's chain; reload it.");
                  // level1::reserve_token_challenge, computed here (docs/60 P-72).
                  const issued = Math.floor(Date.now() / 1000);
                  const auth = await passkeyAssert(reserveTokenChallenge(chainId, tenant.policy, issued), pub);
                  const r = await post<{ reserveToken: string }>("/api/level1/reserve-token", { policy: tenant.policy, issued, auth });
                  reserveToken.set(tenant.policy, r.reserveToken);
                  setHasToken(true);
                })
              }
            >
              {unlock.state.kind === "working" ? <Loader2 className="size-4 animate-spin" /> : <Fingerprint className="size-4" />}
              Use my passkey here
            </Button>
            {unlock.state.kind === "error" && <p className="text-destructive">{unlock.state.text}</p>}
          </div>
        )}
        <div className="space-y-2">
          <Label htmlFor="connect" className="font-medium">
            Connect a wallet that pays your bills today
          </Label>
          <div className="flex flex-col gap-2 sm:flex-row">
            <Input
              id="connect"
              value={wallet}
              onChange={(e) => setWallet(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && add.state.kind !== "working" && connect(typedWallet.address ?? wallet)}
              placeholder="alice.eth, or paste the wallet"
              autoComplete="off"
              spellCheck={false}
              className="h-10 text-[14px]"
            />
            <Button className="h-10" disabled={add.state.kind === "working"} onClick={() => connect(typedWallet.address ?? wallet)}>
              {add.state.kind === "working" && <Loader2 className="size-4 animate-spin" />}
              Connect
            </Button>
            {!!eth() && (
              <Button
                variant="outline"
                className="h-10"
                disabled={add.state.kind === "working"}
                onClick={async () => {
                  const a = await walletAccount().catch(() => undefined);
                  if (a) connect(a);
                }}
              >
                <Wallet className="size-4" /> This browser&apos;s wallet
              </Button>
            )}
          </div>
          <TypedParty t={typedWallet} />
          <p className="text-caption text-muted-foreground">It reads what the wallet paid in the last 90 days and finds what repeats. Nothing moves; up to five wallets.</p>
          {add.state.kind === "error" && <p className="text-destructive">{add.state.text}</p>}
          {add.state.kind === "done" && <p className="text-protect">{add.state.text}</p>}
          {wallets.length > 0 && (
            <ul className="divide-y rounded-lg border">
              {wallets.map((w) => (
                <li key={w} className="flex items-center gap-3 px-3 py-1.5">
                  <span className="min-w-0 flex-1 truncate">
                    <Payee addr={w} />
                  </span>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-7"
                    aria-label={`Disconnect ${partyOf(inbox, w)?.label}`}
                    disabled={drop.state.kind === "working"}
                    onClick={() =>
                      drop.run("Disconnecting…", async () => {
                        await post("/api/level1/wallets", { policy: tenant.policy, remove: w, ...gate() });
                        void rec.refetch();
                        refresh();
                      })
                    }
                  >
                    <X className="size-3.5" />
                  </Button>
                </li>
              ))}
            </ul>
          )}
          {drop.state.kind === "error" && <p className="text-destructive">{drop.state.text}</p>}
          <SpareKeyProposal />
        </div>

        {(read || found) && (
          <div aria-live="polite" className="space-y-2">
            {read?.state === "reading" && (
              <p className="flex items-center gap-2 text-muted-foreground">
                <Loader2 className="size-4 animate-spin" /> Reading the wallet&apos;s last 90 days on {chainName}
                {read.pct ? ` · ${read.pct}%` : "…"}
              </p>
            )}
            {read?.state === "failed" && <p className="text-muted-foreground">Could not read it yet ({read.error}). The agent reads it on its first check.</p>}
            {found &&
              (found.length ? (
                <>
                  <p className="text-title font-semibold">
                    {/* Bills more than two cycles overdue read "last paid …" (dueText); the headline says so too. */}
                    {found.length} {ended ? "bill" : "recurring bill"}
                    {found.length > 1 ? "s" : ""} found{ended ? ` · ${ended} not paid lately` : ""}
                  </p>
                  <ul className="divide-y rounded-lg border">
                    {found.slice(0, 6).map((b) => (
                      <li key={b.counterparty + b.amount} className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5 px-3 py-2.5">
                        <span className="figures w-20 shrink-0 font-semibold">{usd(b.amount)}</span>
                        <span className="min-w-0 truncate">
                          <Payee addr={b.counterparty} />
                        </span>
                        <span className="ml-auto text-muted-foreground">
                          {everyText(b.intervalSecs, b.every)}
                          {b.nextDue ? ` · ${dueText(b.nextDue, b.intervalSecs)}` : ""}
                        </span>
                      </li>
                    ))}
                  </ul>
                  <p className="text-caption text-muted-foreground">Once money lands the agent asks you to approve each payee, then pays them when due.</p>
                </>
              ) : (
                <p className="text-muted-foreground">No recurring bill in what it read. Type the first one below.</p>
              ))}
          </div>
        )}

        <TypeBill />

        <p className="text-muted-foreground">Vendors can send you signed invoices; each one waits in Needs you for your tap.</p>
      </div>
    </Section>
  );
}

/** The first-bill row Start had (docs/60 P-77), now on the account page. */
function TypeBill() {
  const { tenant, inbox, cfg, post, sign, refresh, runPass } = useAccount();
  const save = useAction();
  const [row, setRow] = useState({ payee: "", amount: "", every: "30" });
  const everyDays = Number(row.every);
  const who = useTyped(row.payee, tenant.chain);
  const typed = !inbox?.deployed ? (inbox?.typedBills ?? []) : [];
  return (
    <div className="space-y-2">
      {typed.length > 0 && (
        <div className="space-y-1.5">
          <p className="font-medium">Bills you told it</p>
          <ul className="divide-y rounded-lg border">
            {typed.map((b) => (
              <li key={b.payee + b.amount} className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5 px-3 py-2.5">
                <span className="figures w-20 shrink-0 font-semibold">{usd(b.amount)}</span>
                <span className="min-w-0 truncate">
                  <Payee addr={b.payee} />
                </span>
                <span className="ml-auto text-muted-foreground">{everyText(b.everySecs)}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
      <p className="font-medium">Or type a bill</p>
      <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_8rem_9rem_auto]">
        <div>
          <Label htmlFor="payee" className="sr-only">
            Who you pay: a name, an ENS name, their web domain or their wallet
          </Label>
          <Input
            id="payee"
            value={row.payee}
            onChange={(e) => setRow({ ...row, payee: e.target.value })}
            placeholder="Who: acme, alice.eth, acme.com"
            autoComplete="off"
            spellCheck={false}
            className="h-10 text-[14px]"
          />
        </div>
        <div className="relative">
          <Label htmlFor="amount" className="sr-only">
            Amount in USDC
          </Label>
          <Input
            id="amount"
            inputMode="decimal"
            value={row.amount}
            onChange={(e) => setRow({ ...row, amount: e.target.value })}
            placeholder="Amount"
            className="figures h-10 pr-14 text-[14px]"
          />
          <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground">USDC</span>
        </div>
        <div className="relative">
          <Label htmlFor="every" className="sr-only">
            Every how many days
          </Label>
          <Input
            id="every"
            inputMode="numeric"
            value={row.every}
            onChange={(e) => setRow({ ...row, every: e.target.value })}
            className="figures h-10 pl-14 pr-12 text-[14px]"
          />
          <span className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground">every</span>
          <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground">days</span>
        </div>
        <Button
          variant="outline"
          className="h-10"
          disabled={save.state.kind === "working"}
          onClick={() =>
            save.run("Saving…", async () => {
              const payee = who.address ?? "";
              if (!isAddr(payee) || !(parseFloat(row.amount) > 0) || !Number.isInteger(everyDays) || everyDays < 1 || everyDays > 366)
                throw new Error(
                  who.found?.error ?? "A bill is who you pay (a name, alice.eth, acme.com or their wallet), a USDC amount, and every 1 to 366 days.",
                );
              const bill = { payee, amount: row.amount.trim(), everyDays };
              let href: string | undefined;
              // A live account pays only payees it approved: the passkey approves this one first.
              if (inbox?.deployed) {
                const prep = await post<{ to: string; data: string }>("/api/level1/prepare-payees", { policy: tenant.policy, payees: [payee.toLowerCase()] });
                href = (await sign(prep.to, prep.data, { fn: PAYEES, attested: true, rows: [{ to: payee }] })).explorer;
              }
              const r = await post<{ refused: { why: string }[] }>("/api/level1/bills", {
                policy: tenant.policy,
                bills: [bill],
                reserveToken: reserveToken.get(tenant.policy) ?? undefined,
              });
              if (r.refused.length) throw new Error(r.refused[0].why);
              // A domain it was typed as is kept as its name (docs/81); a Tameion or
              // ENS name is found again by itself, and shown as proved.
              if (who.found?.ok?.kind === "domain") await saveName(tenant.policy, payee, who.label).catch(() => undefined);
              setRow({ payee: "", amount: "", every: "30" });
              if (inbox?.deployed) void runPass();
              else refresh();
              const to = who.label ? ` to ${who.label}` : "";
              return {
                text: inbox?.deployed
                  ? `Added ${usd(bill.amount)} USDC every ${everyDays} days${to}.`
                  : `Saved ${usd(bill.amount)} USDC every ${everyDays} days${to}. Once money lands the agent asks you to approve the payee; it can pay a new payee ${spanText(cfg.delaySecs)} after that.`,
                href,
              };
            })
          }
        >
          {save.state.kind === "working" && <Loader2 className="size-4 animate-spin" />}
          Save bill
        </Button>
      </div>
      <TypedParty t={who} />
      {save.state.kind === "error" && <p className="text-destructive">{save.state.text}</p>}
      {save.state.kind === "done" && <p className="text-protect">{save.state.text}</p>}
    </div>
  );
}

/** Set the spare key with the passkey: `setRecovery` on the owner contract, the
 *  calldata built here and held against the button (lib/guard, docs/60 P-72). */
export function useSetSpare() {
  const { tenant, sign, post } = useAccount();
  const rec = useRecord(tenant.policy);
  const { state, run } = useAction();
  const set = (to: string) =>
    run("Setting…", async () => {
      const a = to.trim().toLowerCase();
      if (!isAddr(a) || isZero(a)) throw new Error("Type the wallet's ENS name (alice.eth), or paste its address.");
      const data = build("setRecovery", [a]);
      const prep = await post<{ to: string; data: string }>("/api/level1/prepare-recovery", { owner: tenant.owner, recovery: a });
      const ex = await sign(prep.to, prep.data, { fn: ["setRecovery"], data });
      void rec.refetch();
      return { text: "That wallet is now your spare key.", href: ex.explorer };
    });
  return { state, set };
}

/** The service proposes the first wallet connected as the spare key (docs/79 §6a
 *  Service 4); it is a trust decision, so it waits for one tap of the passkey. */
export function SpareKeyProposal() {
  const { tenant, owns } = useAccount();
  const rec = useRecord(tenant.policy).data;
  const { state, set } = useSetSpare();
  const proposed = rec && !(isAddr(rec.recovery) && !isZero(rec.recovery)) && isAddr(rec.spare_key) ? rec.spare_key! : "";
  if (!owns || (!proposed && state.kind !== "done")) return null;
  return (
    <div className="space-y-2 rounded-lg border border-attention/45 bg-attention-soft/30 p-3">
      {proposed && (
        <>
          <p>
            Make <Payee addr={proposed} nameable={false} /> your spare key — confirm with your passkey.
          </p>
          <p className="text-caption text-muted-foreground">If this phone is lost, that wallet can name a new passkey; you can cancel it in the meantime. Confirm only if you hold it.</p>
          <Button size="sm" disabled={state.kind === "working"} onClick={() => set(proposed)}>
            {state.kind === "working" && <Loader2 className="size-4 animate-spin" />}
            Confirm with my passkey
          </Button>
        </>
      )}
      {state.kind === "error" && <p className="text-destructive">{state.text}</p>}
      {state.kind === "done" && <p className="text-protect">{state.text}</p>}
    </div>
  );
}
