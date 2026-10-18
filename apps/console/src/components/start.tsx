"use client";

import { useQuery } from "@tanstack/react-query";
import { Check, ChevronRight, Fingerprint, Loader2, ShieldCheck } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Page } from "@/components/layout/page";
import { Stat, Stats, Why } from "@/components/layout/section";
import { Seal } from "@/components/seal";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ApiError, createKey, everyText, fetchDemo, fetchName, isAddr, isHandle, post, remember, reserveToken, store, usd } from "@/lib/tameion";
import type { Cfg, Demo, Tenant } from "@/lib/types";

/** The busy state while the browser's passkey prompt is open. */
const PROMPT = "prompt";

/** The passkey prompt's end, in words (docs/60 P-100): a closed or timed-out prompt is said, never a spinner. */
function keyWords(e: unknown): string {
  const name = (e as Error)?.name;
  if (name === "AbortError") return "Cancelled. Nothing was created; tap again when you are ready.";
  if (name === "TimeoutError") return "No answer to the passkey prompt within a minute, so nothing was created. Tap again and answer it.";
  if (name === "NotAllowedError")
    return "The passkey prompt was closed or timed out, so no key was made. Tap again and answer it with Touch ID, your phone or your password manager.";
  if (name === "NotSupportedError" || name === "SecurityError")
    return `This browser cannot make a passkey on this page (${name}). Open it in Chrome or Safari on this Mac.`;
  return (e as Error)?.message || String(e);
}

/** A free-looking name, so the first tap needs no typing; the owner may change it (docs/105). */
const suggestName = () => `acme-agents-${1000 + (crypto.getRandomValues(new Uint16Array(1))[0] % 9000)}`;

/** `/` without an account (docs/109 §10): opens the account an agent operator gives its agents, with a passkey made
 *  on this device. */
export function Start({ cfg, onStarted, partner }: { cfg: Cfg; onStarted: (t: Tenant) => void; partner?: string }) {
  const anchored = cfg.chains.filter((c) => c.anchor);
  // Arbitrum Sepolia first: gas is plentiful there and its public node does not rate-limit (CLAUDE.md, P-12, P-14).
  const [chain, setChain] = useState(anchored.find((c) => c.key === "arbitrum-sepolia")?.key ?? anchored[0]?.key ?? cfg.chains[0]?.key ?? "");
  // Start renders only in the browser (the page waits for localStorage), so
  // initial state may read the URL directly. `/watch` links here with the wallet
  // it read: it is connected to the new account once it exists.
  const [fromWatch] = useState(() => new URLSearchParams(location.search).get("address") ?? "");
  const [name, setName] = useState(suggestName);
  const [checked, setChecked] = useState<{ name: string; taken: boolean | null; error?: string }>({ name: "", taken: null });
  const [keyless, setKeyless] = useState(false);
  // A judge's link carries the invite word (`/?invite=…`, docs/115 §3): nothing to type but the name.
  const [linkInvite] = useState(() => new URLSearchParams(location.search).get("invite")?.trim() ?? "");
  const [invite, setInvite] = useState(linkInvite);
  const demo = useQuery({ queryKey: ["demo"], queryFn: fetchDemo, staleTime: 300_000 });
  const trial = demo.data?.enabled ? demo.data : null;
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  // The open passkey prompt, so Cancel can end it (docs/60 P-100).
  const prompt = useRef<AbortController | null>(null);

  // Is the name free? Asked once typing pauses; an answer for an older name is dropped.
  useEffect(() => {
    if (!isHandle(name)) return;
    let live = true;
    const t = setTimeout(async () => {
      try {
        const r = await fetchName(name);
        if (live) setChecked({ name, taken: !!r });
      } catch (e) {
        if (live) setChecked({ name, taken: null, error: (e as Error).message });
      }
    }, 350);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [name]);

  const status = !name ? "idle" : !isHandle(name) ? "invalid" : checked.name !== name ? "checking" : checked.error ? "unknown" : checked.taken ? "taken" : "free";

  async function open(asDemo = false) {
    setError("");
    if (!isHandle(name)) return setError("Pick a name: 3 to 32 lowercase letters, numbers or dashes, not starting or ending with a dash.");
    if (status === "taken") return setError(`${name} is taken. Try another.`);
    try {
      prompt.current = new AbortController();
      setBusy(PROMPT);
      const pub = await createKey("tameion.pub", prompt.current.signal);
      prompt.current = null;
      setBusy("Opening your account…");
      const d = await post<{ tenant: Tenant; reserveToken?: string | null; demo?: { tx?: string; usdc?: string; error?: string } }>("/api/level1/deploy", {
        chain,
        qx: pub.qx,
        qy: pub.qy,
        name,
        keyless,
        partner: store.get("tameion.partner") || undefined,
        days: 90,
        invite,
        lazy: true,
        // Demo mode funds the new account by itself (docs/115 §3); the account page then walks the five steps.
        demo: asDemo || undefined,
      });
      remember(d.tenant);
      // The demo's funding answers with the open (its transaction, or why nothing was sent): the account page shows it.
      if (asDemo)
        store.set(`tameion.judge.${d.tenant.policy}`, JSON.stringify({ openedAt: Date.now(), fundTx: d.demo?.tx, fundError: d.demo?.error }));
      reserveToken.set(d.tenant.policy, d.reserveToken);
      if (isAddr(fromWatch)) {
        try {
          await post("/api/level1/wallets", { policy: d.tenant.policy, add: fromWatch.trim(), reserveToken: d.reserveToken });
        } catch (e) {
          toast.error(`Your account is open, but the wallet you read was not connected: ${(e as Error).message}. Connect it on the account page.`);
        }
      }
      onStarted(d.tenant);
    } catch (e) {
      setError(e instanceof ApiError && e.status === 409 ? `${name} is taken. Try another.` : keyWords(e));
    } finally {
      prompt.current = null;
      setBusy(null);
    }
  }

  return (
    <Page className="lg:pt-16">
      <div className="grid gap-10 lg:grid-cols-[minmax(0,1fr)_20rem] lg:gap-16">
        <section className="min-w-0">
          {partner && <p className="mb-4 text-sm text-muted-foreground">{partner}</p>}
          <p className="mb-3 text-sm font-medium text-protect">The account your agents pay from.</p>
          <h1 className="max-w-[30ch] text-balance text-headline font-semibold sm:text-4xl sm:leading-tight sm:tracking-tight">
            Your agents pay their bills around the clock in USDC — and can never spend past the mandate you signed.
          </h1>
          <p className="mt-4 max-w-[60ch] text-base leading-relaxed text-muted-foreground">
            A raise to the mandate waits where anyone can see it coming and you can cancel it, a cut takes effect at once, any two of your three keys recover the
            account, and every payment lands in a book of what was owed and why.
          </p>
          <p className="mt-4 flex max-w-[60ch] gap-2 text-sm leading-relaxed">
            <ShieldCheck className="mt-0.5 size-4 shrink-0 text-protect" aria-hidden />
            <span>
              Your key stays on this device, behind Touch ID or your passkey; Tami holds no key that can move your money. An agent&apos;s key pays only the payees you
              approved, under its cap, and nothing else.
            </span>
          </p>

          <div className="mt-8 rounded-xl border bg-card p-4 shadow-card sm:p-6">
            {trial && <TryHead d={trial} />}
            <Label htmlFor="name" className="text-title font-medium">
              Your account&apos;s name
            </Label>
            <Input
              id="name"
              value={name}
              onChange={(e) => setName(e.target.value.toLowerCase().replace(/\s+/g, "-"))}
              onKeyDown={(e) => e.key === "Enter" && !busy && void open(!!trial)}
              placeholder="acme"
              autoComplete="off"
              autoCapitalize="none"
              spellCheck={false}
              maxLength={32}
              aria-describedby="name-status"
              className="mt-2 h-11 text-[15px]"
            />
            <p id="name-status" className="mt-2 flex min-h-5 items-center gap-1.5 text-sm text-muted-foreground" aria-live="polite">
              {status === "idle" && "The name your payees see. Lowercase letters, numbers and dashes."}
              {status === "invalid" && "3 to 32 lowercase letters, numbers or dashes, not starting or ending with a dash."}
              {status === "checking" && (
                <>
                  <Loader2 className="size-3.5 animate-spin" aria-hidden /> Checking {name}…
                </>
              )}
              {status === "free" && (
                <>
                  <Check className="size-4 text-protect" aria-hidden /> <span className="text-foreground">{name}</span> is free.
                </>
              )}
              {status === "taken" && <span className="text-destructive">{name} is taken. Try another.</span>}
              {status === "unknown" && "Could not check the name just now; opening still tells you."}
            </p>
            {cfg.inviteNeeded && !linkInvite && (
              <div className="mt-4 max-w-xs">
                <Label htmlFor="invite" className="text-sm font-medium">
                  Invite word
                </Label>
                <Input id="invite" value={invite} onChange={(e) => setInvite(e.target.value)} className="mt-2 h-10" />
              </div>
            )}

            <div className="mt-5 flex flex-wrap items-center gap-x-4 gap-y-2">
              <Button size="lg" className="h-11 px-6" disabled={!!busy || !cfg.operator || status === "taken"} onClick={() => open(!!trial)}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : <Fingerprint className="size-4" />}
                {busy === PROMPT ? "Waiting for your passkey…" : (busy ?? (trial ? "Try it in 5 minutes" : "Open the account with my passkey"))}
              </Button>
              {busy === PROMPT ? (
                <Button size="lg" variant="outline" className="h-11" onClick={() => prompt.current?.abort()}>
                  Cancel
                </Button>
              ) : (
                <p className="text-caption text-muted-foreground">
                  {trial ? "It opens on chain with test USDC from the demo; nothing here moves real money. " : "Nothing goes on chain until money arrives. "}
                  {!trial && (
                    <Why label="What opening does">
                      It creates a key on this device (Face ID, Touch ID or your password manager) and reserves your account&apos;s address under this name. The contract
                      is created when money can reach it; until then nothing is on chain.
                    </Why>
                  )}
                </p>
              )}
            </div>
            {busy === PROMPT && (
              <p role="status" className="mt-3 flex items-center gap-2 text-sm font-medium">
                <Fingerprint className="size-4 shrink-0 text-protect" aria-hidden /> Answer the Touch ID / passkey prompt your browser just opened. It closes by itself
                after a minute.
              </p>
            )}
            {error && (
              <p role="alert" className="mt-3 text-sm text-destructive">
                {error}
              </p>
            )}
            {!cfg.operator && <p className="mt-3 text-sm text-muted-foreground">This host cannot open accounts. Reading an address still works.</p>}
            <p className="mt-4 text-sm">
              <a className="text-muted-foreground underline-offset-4 hover:text-foreground hover:underline" href="/watch">
                Just looking? Read any wallet first →
              </a>
            </p>

            {/* The demo runs on the chain it is funded on, with an agent key: no other way to set it up. */}
            {!trial && (
            <details className="group mt-5 border-t pt-4 text-sm">
              <summary className="flex cursor-pointer list-none select-none items-center gap-1.5 text-muted-foreground hover:text-foreground [&::-webkit-details-marker]:hidden">
                <ChevronRight className="size-4 transition-transform duration-150 group-open:rotate-90" aria-hidden />
                More ways to set it up
              </summary>
              <div className="mt-4 grid gap-4">
                <div className="grid gap-2 sm:max-w-xs">
                  <Label className="text-sm">Where your USDC is</Label>
                  <Select value={chain} onValueChange={setChain}>
                    <SelectTrigger className="h-10 w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {anchored.map((c) => (
                        <SelectItem key={c.key} value={c.key}>
                          {c.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="flex gap-3">
                  <Checkbox id="keyless" checked={keyless} onCheckedChange={(v) => setKeyless(v === true)} className="mt-0.5" />
                  <Label htmlFor="keyless" className="text-sm font-normal leading-relaxed text-muted-foreground">
                    No agent key at all. The agent reads and proposes; every payment waits for my one tap, and standing orders pay themselves.
                  </Label>
                </div>
              </div>
            </details>
            )}
          </div>

          <p className="mt-6 max-w-[62ch] text-caption text-muted-foreground">
            What it costs: nothing while Tami runs on testnet; nothing here moves real money.
          </p>
        </section>

        <aside className="hidden flex-col items-center gap-8 lg:sticky lg:top-24 lg:flex lg:self-start">
          <Seal size={240} fraction={status === "free" ? 1 : 0} tone={status === "free" ? "protect" : "muted"}>
            <span className="max-w-[10ch] truncate text-2xl font-semibold">{name || "your name"}</span>
            <span className="mt-1 max-w-[14ch] text-sm leading-snug text-muted-foreground">your agents&apos; account</span>
          </Seal>
          <ol className="w-full space-y-3 text-sm">
            {[
              ["Open the account.", "Your passkey is its owner; any two of three keys recover it."],
              ["Approve who your agents may pay.", "Each new payee is your decision, one tap."],
              ["Give an agent a key with a cap.", "It pays around the clock in USDC over x402, inside the mandate the contract enforces."],
            ].map(([t, d], i) => (
              <li key={t} className="flex gap-3">
                <span className="figures mt-0.5 grid size-6 shrink-0 place-items-center rounded-full border text-xs">{i + 1}</span>
                <span>
                  <span className="font-medium">{t}</span> <span className="text-muted-foreground">{d}</span>
                </span>
              </li>
            ))}
          </ol>
        </aside>
      </div>
    </Page>
  );
}

/** The judge's five minutes on the home page (docs/115 §3): the benefit as numbers from the host's own demo values, the
 *  becoming line, and the one button below it. */
function TryHead({ d }: { d: Demo }) {
  return (
    <div className="mb-6 space-y-4 border-b pb-6">
      <div>
        <p className="text-sm font-medium text-protect">Try it in 5 minutes</p>
        <p className="mt-1 text-balance text-title">
          Your agent pays a bill of <strong className="figures">{usd(d.bill.usdc)} USDC {everyText(d.bill.everySecs)}</strong> by itself, only to the payee you approved,
          and never past the mandate you sign. Five steps, each one tap or a wait.
        </p>
      </div>
      <Stats>
        <Stat label="To type" value="0" sub="seed phrases or addresses" />
        <Stat label="A raise waits" value={`${d.delaySecs} s`} sub="anyone can see it; a cut is instant" />
        <Stat label="A bill" value={usd(d.bill.usdc)} sub={`USDC ${everyText(d.bill.everySecs)}, test money`} />
      </Stats>
      <p className="text-sm text-muted-foreground">Run your agents the way a finance team runs its payables.</p>
    </div>
  );
}
