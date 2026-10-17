"use client";

import { useQuery } from "@tanstack/react-query";
import { Check, ChevronsUpDown, Link2, Pencil, Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { useSse } from "@/app/agent/live";
import { TopBar } from "@/components/brand";
import { Columns, Page, PageHeader } from "@/components/layout/page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { ApiError, fetchInbox, labelOf, moveLocalNames, num, ownerPub, post, store, tail, tenants } from "@/lib/tameion";
import type { Cfg, Inbox, Tenant } from "@/lib/types";
import { Activity } from "./activity";
import { AgentControls, Suggestions } from "./agent";
import { AccountProvider, makeCtx, useBusy, useInvalidate, useRecord } from "./ctx";
import { Hero, Status } from "./hero";
import { KeysNag } from "./keys-nag";
import { AccountMandate } from "./mandate";
import { Money } from "./money";
import { NeedsYou } from "./needs";
import { Owe } from "./owe";
import { SettingsSheet } from "./settings";
import { ProofDialog } from "./proof";

const cached = (policy: string): Inbox | undefined => {
  try {
    const c = JSON.parse(store.get(`tameion.inbox.${policy}`) || "null");
    return c && c.paid ? c : undefined;
  } catch {
    return undefined;
  }
};

export function Account({ tenant, cfg, onSwitch, onNew }: { tenant: Tenant; cfg: Cfg; onSwitch: (t: Tenant) => void; onNew: () => void }) {
  const { busy, track } = useBusy();
  const refresh = useInvalidate(tenant.policy);
  // The account's live stream, read with fetch so it carries this device's token (docs/60 P-173: `EventSource`
  // cannot send a header, and once an agent key exists the stream answers 401 without one). Named events:
  // `hello` on connect, `pass` as a check queues / runs / ends, `inbox` after any change to the account.
  const stream = useSse(`/api/level1/events?policy=${tenant.policy}`, tenant.policy, (e) => {
    if (e.event === "inbox" || e.event === "pass") void refresh();
  });
  const live = stream.link.kind === "open";
  // /desk's "Add guardians" links here as /?keys: open Settings → Owners, where the Keys panel is (docs/104).
  const keysLink = typeof location !== "undefined" && new URLSearchParams(location.search).has("keys");
  const [settingsOpen, setSettingsOpen] = useState(keysLink);
  const [settingsTab, setSettingsTab] = useState(keysLink ? "owners" : "reports");
  const [, bump] = useState(0);

  const q = useQuery({
    queryKey: ["inbox", tenant.policy],
    queryFn: async () => {
      const i = await fetchInbox(tenant.policy);
      i.policy = tenant.policy;
      i.readAt = Date.now();
      store.set(`tameion.inbox.${tenant.policy}`, JSON.stringify(i));
      return i;
    },
    // Paint the last inbox this browser saw at once; a public node answers in
    // seconds. A tap in flight stops the poll so its row is never redrawn.
    placeholderData: () => cached(tenant.policy),
    refetchInterval: (query) => (busy ? false : query.state.data?.running && !live ? 4_000 : live ? 60_000 : 15_000),
    retry: (n, e) => !(e instanceof ApiError && (e.status === 401 || e.status === 403)) && n < 1,
  });
  const inbox = q.data;
  const gated = q.error instanceof ApiError && (q.error.status === 401 || q.error.status === 403);
  const stale = q.isPlaceholderData;

  // docs/81: payee names this device kept move to the service, once.
  useEffect(() => {
    void moveLocalNames(tenant.policy).then((n) => {
      if (n > 0) void refresh();
    });
  }, [tenant.policy, refresh]);

  // Whose account this is, decided by this browser's own key (docs/60 P-72 H3): a
  // link to someone else's account opens read-only, never as "Your treasury".
  const { pub, owns } = ownerPub(tenant, inbox?.owners?.keys);
  const ctx = makeCtx({ tenant, cfg, inbox, pub, owns }, refresh, track);

  // The agent's first check starts by itself the moment money can be reached;
  // with no scheduler on the host, the page nudges a check once an hour.
  const nudged = useRef(0);
  useEffect(() => {
    if (!inbox || inbox.running || stale) return;
    const now = Date.now();
    const reservedFunded = inbox.deployed === false && !inbox.lastPass && num(inbox.balance) > 0 && now - nudged.current > 60_000;
    const hourly = inbox.lastPass && now / 1000 - inbox.lastPass.at > 3600 && !inbox.nextPassSecs && now - nudged.current > 600_000;
    if (reservedFunded || hourly) {
      nudged.current = now;
      void ctx.runPass();
    }
  }, [inbox, stale, ctx]);

  // A raise the owner signed (autopilot, resume, a higher limit) waits the
  // delay; anyone may apply it after. Applying is not a trust decision, so the
  // page does it the moment it is ready, also after a reload.
  // A chain clock behind the page's refuses the first try (TooEarly): it tries
  // again every 15 s while the change is still pending (P-49).
  // Never while an owner action is in flight (docs/60 P-171): a cancel the owner signed must reach the chain
  // before this page applies the change it cancels.
  const inFlight = useRef(0);
  useEffect(() => {
    inFlight.current = busy;
  }, [busy]);
  const eta = inbox?.pending?.eta;
  useEffect(() => {
    if (!eta) return;
    let t: ReturnType<typeof setTimeout>;
    let tries = 0;
    const apply = async () => {
      if (inFlight.current > 0) {
        t = setTimeout(apply, 3_000);
        return;
      }
      try {
        await post("/api/level1/mode", { policy: tenant.policy, action: "apply" }, tenant.policy);
      } catch {
        if (++tries < 20) t = setTimeout(apply, 15_000);
      }
      refresh();
    };
    t = setTimeout(apply, Math.max(0, eta * 1000 - Date.now()) + 3_000);
    return () => clearTimeout(t);
  }, [eta, tenant.policy, refresh]);

  // The account's name (docs/79), unless this device gave it its own label.
  const handle = useRecord(tenant.policy).data?.name ?? tenant.name;
  const name = labelOf(tenant) || handle || "";
  const all = tenants();
  const chain = cfg.chains.find((c) => c.key === tenant.chain);
  const reserved = inbox?.deployed === false && !inbox?.lastPass;
  // A device asking to join, or a change waiting for more owners: the
  // classic page opened its settings fold for these; here a notice opens them.
  const ownerWork = owns ? (inbox?.joinRequests?.length ?? 0) + (inbox?.approvals?.length ?? 0) : 0;
  // Nothing to decide: what was paid becomes the page's main content, and the
  // wide column is not left short beside the narrow one.
  const quiet = !inbox?.needsYou.length && !inbox?.sellersToApprove?.length && !inbox?.invoices?.length;

  return (
    <AccountProvider value={ctx}>
      <TopBar surface="human">
        <AccountMenu tenant={tenant} all={all} name={name} onSwitch={onSwitch} onNew={onNew} onRenamed={() => bump((x) => x + 1)} />
        <SettingsSheet
          open={settingsOpen}
          tab={settingsTab}
          onOpenChange={(o) => {
            setSettingsOpen(o);
            if (!o) setSettingsTab("reports");
          }}
        />
      </TopBar>
      <Page>
        <PageHeader
          title={owns ? name || "Your treasury" : `Viewing ${handle || `account ••${tail(tenant.policy)}`}`}
          meta={
            <>
              <Badge variant="outline">{chain?.name ?? tenant.chain}</Badge>
              {stale && !q.error && <Badge variant="secondary">Updating…</Badge>}
              {live && (
                <Badge variant="secondary" className="gap-1">
                  <span className="size-1.5 rounded-full bg-protect" /> Live
                </Badge>
              )}
            </>
          }
        >
          {inbox && !gated && <Status />}
        </PageHeader>

        {gated ? (
          <KeyPaste policy={tenant.policy} onSaved={refresh} />
        ) : !inbox && q.error ? (
          <p className="text-sm text-muted-foreground">Could not reach the network: {(q.error as Error).message}. Trying again.</p>
        ) : !inbox ? (
          <div className="space-y-6">
            <Skeleton className="h-40 w-full rounded-xl" />
            <Skeleton className="h-52 w-full rounded-xl" />
          </div>
        ) : (
          <div className="space-y-8">
            {!owns && (
              <p role="status" className="rounded-xl border px-4 py-3 text-sm leading-relaxed text-muted-foreground sm:px-5">
                Read-only. This browser holds no owner key of this account, so nothing here can be paid, funded or approved from it. If it is yours, open it on the
                device where you made it, or ask to join from Settings and approve this device there.
              </p>
            )}
            {/* P-202: "1 of 3 keys set" on the page, not only inside Settings → Owners. */}
            {owns && (
              <KeysNag
                tenant={tenant}
                onAdd={() => {
                  setSettingsTab("owners");
                  setSettingsOpen(true);
                }}
              />
            )}
            <AccountMandate />
            <Hero />
            {ownerWork > 0 && (
              <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-attention/45 bg-attention-soft/40 px-4 py-3 text-sm sm:px-5">
                <span>
                  {inbox.joinRequests?.length
                    ? `${inbox.joinRequests.length} device${inbox.joinRequests.length === 1 ? " asks" : "s ask"} to become an owner of this account.`
                    : `${inbox.approvals?.length} change${inbox.approvals?.length === 1 ? " waits" : "s wait"} for more owners to approve.`}
                </span>
                <Button
                  size="sm"
                  onClick={() => {
                    setSettingsTab("owners");
                    setSettingsOpen(true);
                  }}
                >
                  Review in Settings
                </Button>
              </div>
            )}
            {q.error && !gated && (
              <p className="text-sm text-attention">
                Could not reach the network just now; showing what was read at{" "}
                {new Date((stale ? inbox.readAt : q.dataUpdatedAt) || q.dataUpdatedAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}.
                Trying again.
              </p>
            )}
            {/* What the owner acts on in the wide column (the suggestions are
                taps too); what they glance at — the agent, the money, what was
                paid — in the narrow one (docs/71 D-F2). */}
            <Columns
              primary={
                <>
                  {reserved && <Money />}
                  {owns && reserved && <Owe />}
                  {owns && <NeedsYou />}
                  {owns && <Suggestions />}
                  {quiet && <Activity />}
                  {owns && !reserved && <Owe />}
                </>
              }
              side={
                <>
                <AgentControls />
                {!reserved && <Money />}
                {!quiet && <Activity />}
                <p className="px-1 text-caption text-muted-foreground">
                  Every decision the agent makes is written to a record anyone can check against the chain.{" "}
                  <ProofDialog>
                    <button type="button" className="inline-flex items-center gap-1 font-medium text-foreground underline-offset-4 hover:underline">
                      Check it yourself
                    </button>
                  </ProofDialog>
                </p>
                </>
              }
            />
          </div>
        )}
      </Page>
    </AccountProvider>
  );
}

function AccountMenu({
  tenant,
  all,
  name,
  onSwitch,
  onNew,
  onRenamed,
}: {
  tenant: Tenant;
  all: Tenant[];
  name: string;
  onSwitch: (t: Tenant) => void;
  onNew: () => void;
  onRenamed: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState("");
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" className="max-w-[46vw] gap-1.5 sm:max-w-xs">
            <span className="truncate">{name || `Account ••${tail(tenant.policy)}`}</span>
            <ChevronsUpDown className="size-3.5 opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-72">
          <DropdownMenuLabel>Accounts on this device</DropdownMenuLabel>
          {all.map((t) => (
            <DropdownMenuItem key={t.policy} onClick={() => t.policy !== tenant.policy && onSwitch(t)}>
              <span className="min-w-0 flex-1 truncate">
                {labelOf(t) || t.name || `Account ••${tail(t.policy)}`}
                <span className="ml-1 text-xs text-muted-foreground">{t.kind === "pull" ? "· money in your wallet" : ""}</span>
              </span>
              {t.policy === tenant.policy && <Check className="size-4" />}
            </DropdownMenuItem>
          ))}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            onClick={() => {
              setDraft(name);
              setOpen(true);
            }}
          >
            <Pencil className="size-4" /> {name ? "Rename this account" : "Name this account"}
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={async () => {
              const url = `${location.origin}/?policy=${tenant.policy}`;
              try {
                await navigator.clipboard.writeText(url);
                toast.success("Link copied. Open it on the other device and tap “Ask to join” in Settings there; approve it here.");
              } catch {
                toast(url);
              }
            }}
          >
            <Link2 className="size-4" /> Open on another device
          </DropdownMenuItem>
          <DropdownMenuItem onClick={onNew}>
            <Plus className="size-4" /> Start another account
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem asChild>
            <a href="/classic">Classic view</a>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Name this account</DialogTitle>
            <DialogDescription>Shown on this device only; the chain and the service never see it.</DialogDescription>
          </DialogHeader>
          <Input autoFocus value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="e.g. Ops payroll, EU entity" />
          <DialogFooter>
            <Button
              onClick={() => {
                if (draft.trim()) store.set(`tameion.label.${tenant.policy}`, draft.trim());
                else store.del(`tameion.label.${tenant.policy}`);
                setOpen(false);
                onRenamed();
              }}
            >
              Save
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

function KeyPaste({ policy, onSaved }: { policy: string; onSaved: () => void }) {
  const [k, setK] = useState("");
  return (
    <div className="max-w-lg rounded-xl border bg-card p-5 shadow-card">
      <h2 className="font-semibold">This account opens only to its keys</h2>
      <p className="mt-1 text-sm text-muted-foreground">Its owner made keys for the agents that may look at it. Paste the one you were given.</p>
      <div className="mt-4 flex gap-2">
        <Input value={k} onChange={(e) => setK(e.target.value)} placeholder="Agent key" className="font-mono" />
        <Button
          onClick={() => {
            if (k.trim()) {
              store.set(`tameion.token.${policy}`, k.trim());
              onSaved();
            }
          }}
        >
          Use it
        </Button>
      </div>
    </div>
  );
}
