"use client";

import { Bell, KeyRound, ReceiptText, Settings2, Users } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle, SheetTrigger } from "@/components/ui/sheet";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { ANY, build, decodeCall, notifyChallenge, pairCode, PAYEES, pushChallenge, tokenChallenge, tokenRevokeChallenge } from "@/lib/guard";
import { CopyRow } from "@/components/layout/section";
import { createKey, forgetKey, isAddr, isZero, passkeyAssert, spanText, store, TC, unb64u } from "@/lib/tameion";
import type { Cfg, Tenant } from "@/lib/types";
import { useConfirm } from "./confirm";
import { SpareKeyProposal, useSetSpare } from "./owe";
import { Receipt, useAccount, useAction, useRecord, type ActionState } from "./ctx";
import { Keys } from "./keys";
import { Payee, TypedParty, useTyped } from "./payee";

/** The chain id the off-chain challenges carry, and "now" as the page picks it. */
const chainOf = (cfg: Cfg, t: Tenant) => {
  const id = cfg.chains.find((c) => c.key === t.chain)?.chainId;
  if (!id) throw new Error("This page does not know the account's chain; reload it.");
  return id;
};
const nowSecs = () => Math.floor(Date.now() / 1000);

function Out({ state }: { state: ActionState }) {
  if (state.kind === "error") return <p className="text-sm text-destructive">{state.text}</p>;
  if (state.kind === "done")
    return (
      <p className="break-all text-sm text-protect">
        {state.text} <Receipt href={state.href} />
      </p>
    );
  return null;
}

function Block({ title, hint, children }: { title: string; hint?: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="space-y-3 border-b pb-6 last:border-b-0">
      <div>
        <h3 className="text-title font-semibold">{title}</h3>
        {hint && <p className="mt-1 text-sm leading-relaxed text-muted-foreground">{hint}</p>}
      </div>
      {children}
    </div>
  );
}

export function SettingsSheet({ open, onOpenChange, tab = "reports" }: { open: boolean; onOpenChange: (o: boolean) => void; tab?: string }) {
  const { owns, tenant, cfg, pub } = useAccount();
  if (!owns)
    return (
      <Sheet open={open} onOpenChange={onOpenChange}>
        <SheetTrigger asChild>
          <Button variant="ghost" size="icon" aria-label="Settings">
            <Settings2 className="size-4" />
          </Button>
        </SheetTrigger>
        <SheetContent className="w-full overflow-y-auto sm:max-w-lg">
          <SheetHeader>
            <SheetTitle>Settings</SheetTitle>
            <SheetDescription>This browser holds no owner key of this account; it can only ask to join.</SheetDescription>
          </SheetHeader>
          <div className="px-4 pb-8">
            <Owners />
          </div>
        </SheetContent>
      </Sheet>
    );
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetTrigger asChild>
        <Button variant="ghost" size="icon" aria-label="Settings">
          <Settings2 className="size-4" />
        </Button>
      </SheetTrigger>
      <SheetContent className="w-full overflow-y-auto sm:max-w-lg">
        <SheetHeader>
          <SheetTitle>Settings</SheetTitle>
          <SheetDescription>Who may act on this account, and where the agent tells you what it did.</SheetDescription>
        </SheetHeader>
        <Tabs key={tab} defaultValue={tab} className="px-4 pb-8">
          {/* Every tab says its name: icon-only tabs beside a text one read as two kinds of control (docs/71 D-F8). */}
          <TabsList className="grid w-full grid-cols-4">
            <TabsTrigger value="reports">
              <Bell className="size-4" aria-hidden /> Alerts
            </TabsTrigger>
            <TabsTrigger value="owners">
              <Users className="size-4" aria-hidden /> Owners
            </TabsTrigger>
            <TabsTrigger value="agents">
              <KeyRound className="size-4" aria-hidden /> Agents
            </TabsTrigger>
            <TabsTrigger value="bills">
              <ReceiptText className="size-4" aria-hidden /> Bills
            </TabsTrigger>
          </TabsList>
          <TabsContent value="reports" className="mt-6 space-y-6">
            <Push />
            <Webhook />
          </TabsContent>
          <TabsContent value="owners" className="mt-6 space-y-6">
            {/* docs/104: the three keys first — the typed guardian list it replaces made people paste addresses. */}
            <Keys tenant={tenant} cfg={cfg} pub={pub} owns={owns} />
            <Owners />
            <SpareKey />
          </TabsContent>
          <TabsContent value="agents" className="mt-6 space-y-6">
            <AgentKeys />
          </TabsContent>
          <TabsContent value="bills" className="mt-6 space-y-6">
            <AddBills />
          </TabsContent>
        </Tabs>
      </SheetContent>
    </Sheet>
  );
}

function Push() {
  const { tenant, cfg, post, pub } = useAccount();
  const { state, run } = useAction();
  return (
    <Block title="Tell me on this phone" hint="A notification when the agent pays or needs you. Your passkey approves this device, so nobody else can add one.">
      <Button
        disabled={state.kind === "working"}
        onClick={() =>
          run("Turning on…", async () => {
            if (!("serviceWorker" in navigator) || !("PushManager" in window)) throw new Error("This browser cannot show notifications; add the page to your home screen first.");
            const reg = await navigator.serviceWorker.register("/sw.js");
            if ((await Notification.requestPermission()) !== "granted") throw new Error("Notifications are off for this page; allow them in the browser's settings.");
            const { key } = await (await fetch("/api/level1/push-key")).json();
            // The browser's own push service can refuse (no push service in this build,
            // or blocked by policy); its message is browser text, so say it in words.
            const sub =
              (await reg.pushManager.getSubscription()) ||
              (await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: unb64u(key) }).catch(() => {
                throw new Error("This browser would not register for notifications. Try Chrome or Safari on your phone, or use a webhook below.");
              }));
            const subscription = sub.toJSON();
            // The challenge is computed here from this subscription (push.rs challenge), never taken from the service.
            const issued = nowSecs();
            const [endpoint, p256dh, key2] = [subscription.endpoint, subscription.keys?.p256dh, subscription.keys?.auth].map((x) => (x ?? "").trim());
            const auth = await passkeyAssert(pushChallenge(chainOf(cfg, tenant), tenant.policy, endpoint, p256dh, key2, issued), pub);
            const res = await post<{ devices?: number }>("/api/level1/push-subscribe", { policy: tenant.policy, subscription, issued, auth });
            return { text: `On — this device hears when the agent pays or needs you${(res.devices ?? 0) > 1 ? ` (${res.devices} devices)` : ""}.` };
          })
        }
      >
        <Bell className="size-4" /> Tell me on this phone
      </Button>
      <Out state={state} />
    </Block>
  );
}

function Webhook() {
  const { tenant, cfg, post, pub } = useAccount();
  const { state, run } = useAction();
  const [url, setUrl] = useState("");
  const [chat, setChat] = useState("");
  return (
    <Block title="Report to a webhook or Telegram" hint="After every check: what it paid, what needs you. Your passkey signs the address.">
      <div className="grid gap-2">
        <Label htmlFor="hook">Webhook</Label>
        <Input id="hook" placeholder="https://…" value={url} onChange={(e) => setUrl(e.target.value)} />
        <Label htmlFor="tg">Telegram chat id (optional)</Label>
        <Input id="tg" value={chat} onChange={(e) => setChat(e.target.value)} />
      </div>
      <Button
        variant="outline"
        disabled={state.kind === "working"}
        onClick={() =>
          run("Signing…", async () => {
            const [u, c] = [url.trim(), chat.trim()];
            if (u && !/^https?:\/\//.test(u)) throw new Error("The webhook must start with https://.");
            // level1::notify_challenge over the address and chat this page shows.
            const issued = nowSecs();
            const auth = await passkeyAssert(notifyChallenge(chainOf(cfg, tenant), tenant.policy, u, c, issued), pub);
            const res = await post<{ notifySecret?: string; notifyUrl?: string }>("/api/level1/notify-signed", {
              policy: tenant.policy,
              url: u,
              telegramChatId: c,
              issued,
              auth,
            });
            return { text: res.notifySecret ? `Set. Your receiver checks each report with this secret (shown once): ${res.notifySecret}` : "Set." };
          })
        }
      >
        Set with my passkey
      </Button>
      <Out state={state} />
    </Block>
  );
}

function Owners() {
  const { inbox, tenant, post, refresh, pub } = useAccount();
  const ask = useAction();
  const o = inbox?.owners;
  const keys = o?.keys ?? [];
  const mineIx = keys.findIndex((k) => pub && k.qx.toLowerCase() === pub.qx.toLowerCase());
  return (
    <Block
      title="Owners and devices"
      hint={
        o
          ? keys.length === 1
            ? "One passkey; it approves alone."
            : `${keys.length} passkeys; ${o.threshold} must approve every change.${mineIx >= 0 ? ` This device is owner ${mineIx + 1}.` : ""}`
          : "The account's owners appear here once it is on chain."
      }
    >
      {o && (
        <p className="text-sm text-muted-foreground">
          Add a phone, a co-founder or the CFO: open this link on their device and tap &ldquo;Ask to join&rdquo;; it appears here for the owners to approve.
        </p>
      )}
      {o && (
        <CopyRow value={`${location.origin}/?policy=${tenant.policy}`} shown="The link for their device" label="the join link" />
      )}
      {(inbox?.approvals ?? []).map((a, i) => (
        <Approval key={i} a={a} canApprove={mineIx >= 0 && !(a.keys ?? []).includes(mineIx)} />
      ))}
      {mineIx >= 0 && (inbox?.joinRequests?.length ?? 0) > 1 && (
        <p role="alert" className="text-sm text-attention">
          {inbox!.joinRequests!.length} devices ask to join. Add only the one in front of you whose code matches; a request you did not expect means someone else has
          the account&apos;s link.
        </p>
      )}
      {mineIx >= 0 && (inbox?.joinRequests ?? []).map((j) => <Join key={j.qx} j={j} keys={keys} />)}
      {o && mineIx < 0 && (
        <>
          <Button
            variant="outline"
            disabled={ask.state.kind === "working"}
            onClick={() =>
              ask.run("Asking…", async () => {
                // This device's own key for this account; the shared key other accounts use stays.
                const p = await createKey(`tameion.pub.${tenant.policy.toLowerCase()}`);
                const label = window.prompt("Name this device (the owners see it):", "my phone") || "another device";
                await post("/api/level1/join-request", { policy: tenant.policy, qx: p.qx, qy: p.qy, label });
                setTimeout(refresh, 2000);
                return { text: `Asked. Pairing code ${pairCode(p.qx, p.qy)} — show it to the owner; they add this device only if their screen shows the same code.` };
              })
            }
          >
            Ask to join from this device
          </Button>
          <Out state={ask.state} />
        </>
      )}
    </Block>
  );
}

/** The pairing codes of the keys a setCoOwners call would make owners. */
function OwnerCodes({ data }: { data: string }) {
  const { pub } = useAccount();
  const c = decodeCall(data);
  if (c?.functionName !== "setCoOwners") return null;
  const [xs, ys] = c.args as readonly [readonly string[], readonly string[], number];
  return (
    <p className="text-muted-foreground">
      Co-owner keys after this:{" "}
      {xs.length
        ? xs.map((x, i) => (
            <span key={x} className="mr-2 inline-block font-mono font-semibold text-foreground">
              {pairCode(x, ys[i])}
              {pub && x.toLowerCase() === pub.qx.toLowerCase() ? " (this device)" : ""}
            </span>
          ))
        : "none"}
      . Approve only if every code belongs to a device you and your co-owners hold.
    </p>
  );
}

function Approval({ a, canApprove }: { a: { target: string; data: string; have: number; need: number }; canApprove: boolean }) {
  const { sign, refresh } = useAccount();
  const { state, run } = useAction();
  const d = TC().describe(a.data);
  return (
    <div className="rounded-lg border p-3 text-sm">
      <p className="font-medium">
        {d.known ? (
          <>
            {d.action}{" "}
            {(d.payees ?? []).map((p, i) => (
              <span key={p}>
                {i ? ", " : ""}
                <Payee addr={p} nameable={false} />
              </span>
            ))}
          </>
        ) : (
          "A change"
        )}
      </p>
      <OwnerCodes data={a.data} />
      <p className="text-muted-foreground">
        {a.have} of {a.need} owners approved
      </p>
      {canApprove && (
        <Button
          size="sm"
          className="mt-2"
          disabled={state.kind === "working"}
          onClick={() =>
            run("Approving…", async () => {
              const ex = await sign(a.target, a.data, { fn: ANY });
              setTimeout(refresh, 3000);
              return { text: "Approved by enough owners.", href: ex.explorer };
            })
          }
        >
          Approve
        </Button>
      )}
      <Out state={state} />
    </div>
  );
}

function Join({ j, keys }: { j: { qx: string; qy: string; label: string }; keys: { qx: string; qy: string }[] }) {
  const { sign, post, refresh, tenant } = useAccount();
  const { state, run } = useAction();
  const total = keys.length + 1;
  const [m, setM] = useState(String(Math.min(2, total)));
  const b32 = (h: string) => `0x${h.replace(/^0x/, "")}` as `0x${string}`;
  const co = keys
    .slice(1)
    .map((k) => ({ qx: k.qx, qy: k.qy }))
    .concat([{ qx: j.qx, qy: j.qy }]);
  let data: ReturnType<typeof build> | undefined;
  try {
    data = build("setCoOwners", [co.map((k) => b32(k.qx)), co.map((k) => b32(k.qy)), Number(m)]);
  } catch {}
  return (
    <div className="space-y-2 rounded-lg border p-3 text-sm">
      <p className="font-medium">{j.label} asks to become an owner</p>
      <p className="text-muted-foreground">
        Pairing code <span className="font-mono font-semibold text-foreground">{pairCode(j.qx, j.qy)}</span>: add it only if the device in front of you shows the same code.
        Anyone with the link can ask; only you can add.
      </p>
      {data && <OwnerCodes data={data} />}
      <div className="flex flex-wrap gap-2">
        <Select value={m} onValueChange={setM}>
          <SelectTrigger className="h-9 w-56">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {Array.from({ length: total }, (_, i) => i + 1).map((n) => (
              <SelectItem key={n} value={String(n)}>
                {n === 1 ? "Any one of us approves" : `${n} of ${total} must approve`}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button
          size="sm"
          className="h-9"
          disabled={state.kind === "working"}
          onClick={() =>
            run("Adding…", async () => {
              if (!data) throw new Error("Not signing: a key in this request is not a passkey this page can read.");
              const prep = await post<{ to: string; data: string }>("/api/level1/prepare-coowners", { policy: tenant.policy, keys: co, threshold: Number(m) });
              const ex = await sign(prep.to, prep.data, { fn: ["setCoOwners"], data });
              setTimeout(refresh, 3000);
              return { text: "Added.", href: ex.explorer };
            })
          }
        >
          Add as owner
        </Button>
      </div>
      <Out state={state} />
    </div>
  );
}

/** docs/79 §5.4: the spare key — a wallet the owner holds that can name a new
 *  passkey after the delay (the owner cancels in the meantime). The service
 *  proposes the first wallet connected; only the passkey sets it, and the
 *  record takes it once the call went through. */
function SpareKey() {
  const { tenant, cfg } = useAccount();
  const rec = useRecord(tenant.policy).data;
  const { state, set } = useSetSpare();
  const [editing, setEditing] = useState(false);
  const [next, setNext] = useState("");
  const typedSpare = useTyped(next, tenant.chain);
  const spare = rec?.recovery ?? tenant.recovery;
  const has = isAddr(spare) && !isZero(spare);
  const link = (label: string) => (
    <button type="button" className="font-medium underline underline-offset-4" onClick={() => setEditing(true)}>
      {label}
    </button>
  );
  return (
    <Block
      title="If you lose your phone"
      hint={`A spare key is a wallet you hold. It can name a new passkey for this account; that waits ${spanText(cfg.delaySecs)} and you can cancel it. It cannot spend.`}
    >
      {has ? (
        <p className="text-sm">
          Spare key:{" "}
          <Payee addr={spare!} nameable={false} />{" "}
          · {link("Change")}
        </p>
      ) : isAddr(rec?.spare_key) ? (
        <div className="space-y-2 text-sm">
          <SpareKeyProposal />
          <p>Or {link("use another wallet")}.</p>
        </div>
      ) : (
        <p className="text-sm">No spare key yet — connect a wallet or {link("set one")}.</p>
      )}
      {editing && (
        <div className="flex gap-2">
          <Input autoFocus placeholder="alice.eth, or paste the wallet" className="h-9 text-sm" value={next} onChange={(e) => setNext(e.target.value)} aria-label="Spare key: an ENS name or the wallet" />
          <Button variant="outline" className="h-9" disabled={state.kind === "working"} onClick={() => set(typedSpare.address ?? next)}>
            Set with my passkey
          </Button>
        </div>
      )}
      {editing && <TypedParty t={typedSpare} />}
      <Out state={state} />
    </Block>
  );
}

const SCOPES: [string, string, boolean][] = [
  ["read", "Read", true],
  ["run", "Run checks", true],
  ["prepare", "Prepare calls for my passkey", false],
  ["import", "Add bills", false],
  ["buy", "Buy paid APIs, inside the limits", false],
];

function AgentKeys() {
  const { inbox, tenant, cfg, post, refresh, pub } = useAccount();
  const mint = useAction();
  const revoke = useAction();
  const { confirm, dialog } = useConfirm();
  const [scopes, setScopes] = useState<string[]>(SCOPES.filter((s) => s[2]).map((s) => s[0]));
  const [label, setLabel] = useState("");
  const [days, setDays] = useState("90");
  const toks = inbox?.agentTokens ?? [];
  return (
    <Block
      title="Other agents"
      hint={
        toks.length
          ? `${toks.length} key${toks.length === 1 ? "" : "s"}; this account opens only to them.`
          : "Give another agent (your ops bot, Claude, a partner) a key to this account. It can look, run and prepare, never sign. Once a key exists, the account's records open only to its keys and to this device."
      }
    >
      {toks.map((t) => (
        <div key={t.id} className="flex items-center justify-between gap-3 rounded-lg border p-3 text-sm">
          <div className="min-w-0">
            <p className="font-medium">{t.label || "unnamed"}</p>
            <p className="text-muted-foreground">
              {t.scopes.join(", ")} · until {new Date(t.expires * 1000).toLocaleDateString()}
            </p>
          </div>
          <Button
            variant="outline"
            size="sm"
            disabled={revoke.state.kind === "working"}
            onClick={() =>
              revoke.run("Revoking…", async () => {
                const issued = nowSecs();
                const auth = await passkeyAssert(tokenRevokeChallenge(chainOf(cfg, tenant), tenant.policy, t.id, issued), pub);
                const r = await post<{ revoked: string; gated: boolean }>("/api/level1/token-revoke-signed", { policy: tenant.policy, id: t.id, issued, auth });
                if (!r.gated || store.get(`tameion.tokenid.${tenant.policy}`) === t.id) forgetKey(tenant.policy);
                setTimeout(refresh, 1500);
                return { text: `Revoked ${r.revoked}${r.gated ? "" : ". No keys left: the account is open again."}` };
              })
            }
          >
            Revoke
          </Button>
        </div>
      ))}
      <Out state={revoke.state} />
      <div className="grid gap-2">
        {SCOPES.map(([v, l]) => (
          <div key={v} className="flex items-center gap-2">
            <Checkbox id={`scope-${v}`} checked={scopes.includes(v)} onCheckedChange={(c) => setScopes((s) => (c === true ? [...s, v] : s.filter((x) => x !== v)))} />
            <Label htmlFor={`scope-${v}`} className="text-sm font-normal">
              {l}
            </Label>
          </div>
        ))}
        <div className="mt-1 flex gap-2">
          <Input placeholder="What it is (e.g. ops bot)" value={label} onChange={(e) => setLabel(e.target.value)} />
          <Input className="w-20" inputMode="numeric" value={days} onChange={(e) => setDays(e.target.value)} aria-label="Days the key lasts" />
        </div>
      </div>
      <Button
        disabled={mint.state.kind === "working"}
        onClick={() =>
          mint.run("Making the key…", async () => {
            // level1::token_challenge over what this page shows; the words are the page's too.
            const sc = SCOPES.map((x) => x[0]).filter((x) => scopes.includes(x));
            if (!sc.length) throw new Error("Pick at least one thing the key may do.");
            const lb = label.trim();
            const d = Math.min(365, Math.max(1, parseInt(days, 10) || 90));
            const issued = nowSecs();
            const expires = issued + d * 86_400;
            const words = SCOPES.filter((x) => sc.includes(x[0])).map((x) => x[1].toLowerCase());
            const body = `Give an agent${lb ? ` (${lb})` : ""} a key to this account that may: ${words.join(", ")} — for ${d} days. It can never sign; your passkey stays the only key that moves authority.`;
            if (!(await confirm({ title: "Make this key?", body, action: "Approve with passkey" }))) throw new Error("cancelled");
            const auth = await passkeyAssert(tokenChallenge(chainOf(cfg, tenant), tenant.policy, sc, lb, expires, issued), pub);
            const r = await post<{ token: string; id: string }>("/api/level1/token-signed", { policy: tenant.policy, scopes: sc, label: lb, issued, expires, auth });
            // A key that may buy is for another agent: this browser keeps no copy of it.
            const keep = !sc.includes("buy");
            if (keep) {
              store.set(`tameion.token.${tenant.policy}`, r.token);
              store.set(`tameion.tokenid.${tenant.policy}`, r.id);
            }
            setTimeout(refresh, 1500);
            return {
              text: `Key for ${lb || "your agent"} (shown once${keep ? "; this browser kept a copy" : "; this browser kept no copy"}): ${r.token} — for tameion-mcp: TAMEION_TOKEN=${r.token}`,
            };
          })
        }
      >
        Make a key with my passkey
      </Button>
      <Out state={mint.state} />
      {dialog}
    </Block>
  );
}

function AddBills() {
  const { tenant, sign, post, runPass } = useAccount();
  const { state, run } = useAction();
  const [text, setText] = useState("");
  return (
    <Block
      title="Add bills the agent should know about"
      hint={
        <>
          One per line: payee address, USDC amount, every N days, and the invoice&apos;s early-payment discount if it offers one (&ldquo;2%/10&rdquo; = 2% off when paid 10 days
          early). Or ask the vendor to send a signed invoice: it lands in Needs you.
        </>
      }
    >
      <Textarea rows={4} className="font-mono text-xs" placeholder="0xabc…, 1200, 30, 2%/10" value={text} onChange={(e) => setText(e.target.value)} />
      <Button
        disabled={state.kind === "working"}
        onClick={() =>
          run("Approving payees…", async () => {
            const bills = text
              .split("\n")
              .map((l) => l.trim())
              .filter(Boolean)
              .map((l) => {
                const [payee, amount, every, terms] = l.split(/[,;\t]+/).map((x) => x.trim());
                const m = /^([\d.]+)\s*%\s*\/\s*(\d+)/.exec(terms || "");
                return { payee, amount, everyDays: Number(every) || 30, ...(m ? { discountPercent: Number(m[1]), earlyDays: Number(m[2]) } : {}) };
              });
            if (!bills.length) throw new Error("Nothing to add.");
            const payees = [...new Set(bills.map((b) => b.payee.toLowerCase()))];
            const prep = await post<{ to: string; data: string }>("/api/level1/prepare-payees", { policy: tenant.policy, payees });
            const ex = await sign(prep.to, prep.data, { fn: PAYEES, attested: true, rows: payees.map((to) => ({ to })) });
            const r = await post<{ bills: unknown[]; refused: { why: string }[] }>("/api/level1/bills", { policy: tenant.policy, bills });
            setText("");
            void runPass(true);
            return { text: `${r.bills.length} added${r.refused.length ? `, ${r.refused.length} refused (${r.refused.map((x) => x.why).join("; ")})` : ""}.`, href: ex.explorer };
          })
        }
      >
        Approve payees &amp; add
      </Button>
      <Out state={state} />
    </Block>
  );
}
