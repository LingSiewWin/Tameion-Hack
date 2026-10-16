"use client";

import { Layers, Loader2, PenLine, Send, UserCheck, X } from "lucide-react";
import { useEffect, useEffectEvent, useState } from "react";
import { toast } from "sonner";
import { Identicon } from "@/components/account/payee";
import { CopyRow, Section, Why } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { deskBaseChallenge, deskPartyChallengeV2, deskStepChallenge, deskVenueChallenge, type DeskKind } from "@/lib/guard";
import { useQuery } from "@tanstack/react-query";
import { shareLink } from "@/lib/links";
import { chainLabel, codeText, interopName } from "@/lib/names";
import {
  deskPost,
  fetchDeskVenue,
  getJson,
  gramsText,
  isAddr,
  ozText,
  passkeyAssert,
  reserveToken,
  tail,
} from "@/lib/tameion";
import { cn } from "@/lib/utils";
import type { DeskBook, DeskDeal, DeskMilestone, Pub } from "@/lib/types";

/** A party as the desk shows it (D12 of docs/102, docs/105): its picture and `name@chain`. One tap shows the key's
 *  last four, the ERC-7828 code to read over the phone and the exact interoperable form to copy; the address itself is
 *  never in the sentence (docs/81). */
export function NameAt({ name, address, chainId, className }: { name: string; address?: string | null; chainId: number; className?: string }) {
  const label = (
    <>
      {name}
      <span className="text-muted-foreground">@{chainLabel(chainId)}</span>
    </>
  );
  if (!address || !isAddr(address)) return <span className={className}>{label}</span>;
  return (
    <Popover>
      <PopoverTrigger
        className={cn(
          "inline-flex items-center gap-1 rounded-sm text-left underline decoration-dotted underline-offset-4 outline-ring hover:decoration-solid focus-visible:outline-2",
          className,
        )}
      >
        <Identicon addr={address} size={14} />
        <span>{label}</span>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-80 max-w-[calc(100vw-2rem)] space-y-2 text-sm">
        <p>
          Key ••{tail(address)} · code <span className="font-mono font-semibold tracking-wider">{codeText(address, chainId)}</span>
        </p>
        <p className="text-caption text-muted-foreground">On a call, {name} can read you the same code from their phone.</p>
        <CopyRow value={interopName(address, chainId)} label={`${name}'s key`} mono />
      </PopoverContent>
    </Popover>
  );
}

/** Connect this desk to the exchange account that approved this Mac's trade-only key, from the
 *  page instead of the service's environment: a trust decision, signed with the owner's passkey;
 *  disconnecting is one tap and instant. The account comes from Hyperliquid itself (`userRole`). */
export function VenueConnect({ policy, chainId, pub, onDone }: { policy: string; chainId: number; pub: Pub | null; onDone: () => void }) {
  const q = useQuery({ queryKey: ["desk-venue", policy], queryFn: () => fetchDeskVenue(policy) });
  const [busy, setBusy] = useState(false);
  const v = q.data;
  async function connect() {
    if (!v?.account) return;
    const account = v.account;
    setBusy(true);
    try {
      const issued = nowSecs();
      const auth = await passkeyAssert(deskVenueChallenge(chainId, policy, account, issued), pub, { kind: "venue", chainId, desk: policy, issued, account });
      await deskPost("/api/desk/venue", policy, { account, issued, auth });
      toast.success("Connected: the desk now offsets on that exchange account.");
      await q.refetch();
      onDone();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function disconnect() {
    setBusy(true);
    try {
      await deskPost("/api/desk/venue/disconnect", policy, {});
      toast.success("Disconnected: the desk books every step and trades nothing.");
      await q.refetch();
      onDone();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  if (!v || v.byEnvironment) return null;
  if (v.connected)
    return (
      <Button size="sm" variant="ghost" className="text-muted-foreground" onClick={disconnect} disabled={busy}>
        Disconnect the exchange account
      </Button>
    );
  return (
    <div className="mb-8 grid gap-2 rounded-lg border bg-background p-3 text-sm">
      {v.account ? (
        <>
          <p>
            This Mac&apos;s trade-only key (••{tail(v.agent ?? "")}) was approved by your Hyperliquid testnet{" "}
            <Why label={`account ••${tail(v.account)}`}>
              <CopyRow value={v.account} label="the exchange account" mono />
            </Why>
            . Connected, the desk offsets on it by itself; the key can trade on that account and cannot withdraw from it.
          </p>
          <Button size="sm" onClick={connect} disabled={busy} className="justify-self-start">
            {busy && <Loader2 className="size-4 animate-spin" />} Connect it
          </Button>
        </>
      ) : (
        <p className="text-muted-foreground">
          {v.error ?? `No exchange account has approved this Mac's trade-only key (••${tail(v.agent ?? "")}) yet.`} Approve it as an API
          wallet on Hyperliquid testnet, then come back here.
        </p>
      )}
    </div>
  );
}

/** A link a counterparty opens to sign: on the public copy of these pages when there is one, carrying where to send the
 *  signed step back (this desk) and who asks (docs/95 P3). `extra` prefills the step. */
export function signingLink(publicUrl: string | null | undefined, policy: string, chainId: number, trader?: string, extra: Record<string, string> = {}) {
  const here = typeof location === "undefined" ? "" : location.origin;
  const q = new URLSearchParams({ desk: policy, chain: String(chainId), ...extra, back: here, ...(trader ? { from: trader } : {}) });
  // In the fragment: deal, grams and names stay out of the host's logs (docs/96 V9).
  return `${publicUrl || here}/sign#${q}`;
}

/** The next step each approved counterparty may sign on a deal, as prefilled links (its role's first kind not yet booked). */
export function asksFor(deal: DeskDeal, parties: DeskBook["parties"], publicUrl: string | null | undefined, policy: string, chainId: number, trader?: string) {
  const booked = new Set(deal.milestones.map((m) => m.kind));
  return parties
    .filter((p) => !p.stopped && p.kinds?.length)
    .flatMap((p) => {
      // "returned" only after work or consignment went out; a purchase is the supplier's own first step.
      const out = booked.has("to_fabrication") || booked.has("consigned");
      const next = p.kinds!.find((k) => !booked.has(k as DeskMilestone["kind"]) && k !== "bought" && (k !== "returned" || out));
      return next
        ? [{ label: `Ask ${p.name}: ${next.replace("_", " ")}`, url: signingLink(publicUrl, policy, chainId, trader, { deal: deal.id, kind: next, as: p.name, qty: deal.held.replace(" g gold", "") }) }]
        : [];
    });
}

/** Seconds now, for a challenge's `issued` (called from handlers only). */
const nowSecs = () => Math.floor(Date.now() / 1000);

/** What each counterparty vouches for (docs/91 R7): a vault signs what comes into
 *  stock, not a sale. "Any step" is the old, unlimited approval. */
const ROLES: { role: string; kinds: DeskKind[] | null; words: string }[] = [
  { role: "Supplier", kinds: ["bought"], words: "the purchase" },
  { role: "Carrier", kinds: ["shipped", "arrived"], words: "shipping and delivery into stock" },
  { role: "Vault", kinds: ["arrived", "returned"], words: "what comes into stock" },
  { role: "Workshop", kinds: ["to_fabrication", "returned"], words: "work sent out and returned" },
  { role: "Retailer", kinds: ["consigned", "returned"], words: "goods on consignment and returned" },
  { role: "Buyer", kinds: ["sold"], words: "the sale" },
  { role: "Any step", kinds: null, words: "any step" },
];

/** An answer to an invite, waiting for the owner's tap (`GET /api/desk/party/proposals`, docs/105). */
type Proposal = { id: string; role: string; name: string; address: string; at: number; invited?: number };

/** Which invite a row answers (docs/103 S2-3): an invite link is a bearer capability, so the row says which one it used. */
const invitedWords = (p: Proposal) =>
  p.invited
    ? ` It answers the ${p.role.toLowerCase()} invite you made at ${new Date(p.invited * 1000).toLocaleString(undefined, { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" })}.`
    : "";

/** The answers to this desk's invites, and the two taps on each: approve (the owner's passkey, `POST /api/desk/party`)
 *  or turn down (the device; trust does not grow). `chainId` unknown: nothing to sign yet. */
function useAnswers({ policy, chainId, pub, onDone }: { policy: string; chainId?: number; pub: Pub | null; onDone: () => void }) {
  const [busy, setBusy] = useState<string | null>(null);
  const device = reserveToken.get(policy);
  const asked = useQuery({
    queryKey: ["desk-proposals", policy],
    queryFn: () => getJson<{ proposals: Proposal[] }>(`/api/desk/party/proposals?policy=${policy}`, policy, device ? { "x-tameion-device": device } : undefined),
    // A counterparty answers from its own phone: look every 10 s, like the book.
    refetchInterval: 10_000,
  });
  async function approve(p: Proposal) {
    const role = ROLES.find((r) => r.role === p.role);
    if (!role?.kinds || !chainId) return toast.error(`This page does not know the role ${p.role}.`);
    const kinds = role.kinds;
    setBusy(p.id);
    try {
      const issued = nowSecs();
      const auth = await passkeyAssert(deskPartyChallengeV2(chainId, policy, p.name, p.address, kinds, issued), pub, {
        kind: "party",
        chainId,
        desk: policy,
        issued,
        name: p.name,
        address: p.address,
        kinds,
      });
      await deskPost("/api/desk/party", policy, { name: p.name, address: p.address, kinds, issued, auth });
      toast.success(`${p.role} approved: ${p.name} may now sign ${role.words} for your desk.`);
      onDone();
      await asked.refetch();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  }
  async function dismiss(p: Proposal) {
    setBusy(p.id);
    try {
      await deskPost("/api/desk/party/dismiss", policy, { id: p.id });
      toast.success(`Turned down: ${p.name} is not on your desk.`);
      await asked.refetch();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  }
  return { rows: chainId ? (asked.data?.proposals ?? []) : [], busy, approve, dismiss, refetch: asked.refetch };
}

/** The answers as rows for the desk's Needs-you inbox (`app/desk/inbox.tsx`, `Need`): one row each, "Approve Kim Seng ••8904
 *  as Supplier?", one tap. Turning one down is under "Who may sign" (`Parties` with `inInbox`). */
export function usePartyNeeds(args: { policy: string; chainId?: number; pub: Pub | null; onDone: () => void }) {
  const { rows, busy, approve } = useAnswers(args);
  return rows.map((p) => {
    const role = ROLES.find((r) => r.role === p.role);
    return {
      key: `party-${p.id}`,
      title: `Approve ${p.name} ••${tail(p.address)} as ${p.role}?`,
      why: `They will sign ${role?.words ?? "their steps"}${role?.kinds?.some((k) => k === "bought" || k === "sold") ? ", and what they sign moves your offset" : ""}. Their phone shows code ${codeText(p.address, args.chainId!)}; on a call they can read it to you.${invitedWords(p)}`,
      action: { label: `Approve ${p.name}`, onClick: () => void approve(p), busy: busy === p.id },
    };
  });
}

/** Who may sign milestones for this desk. Approving one is a trust decision, like adding a payee, so the owner's passkey
 *  signs it (docs/89 M5) over a challenge this page computes itself; nothing else is typed (D11 of docs/102, docs/105):
 *  the owner sends an invite for a role, the counterparty's phone answers it with its own key and the name it chose, and
 *  the answer is one row here — "Approve Kim Seng ••8904 as Supplier?" — one tap. Stopping one shrinks trust, so it is a
 *  tap and takes effect at once (docs/91 R4). */
export function Parties({
  policy,
  chainId,
  parties,
  pub,
  onDone,
  publicUrl,
  trader,
  inInbox,
}: {
  policy: string;
  chain: string;
  chainId: number;
  parties: DeskBook["parties"];
  pub: Pub | null;
  onDone: () => void;
  publicUrl?: string | null;
  trader?: string;
  /** The page's Needs-you inbox shows the approve rows (`usePartyNeeds`): here only "Not them" is left. */
  inInbox?: boolean;
}) {
  const [mine, setBusy] = useState<string | null>(null);
  const [stopping, setStopping] = useState<string | null>(null);
  // The last invite made here, shown too: a copy that did not reach the clipboard is not lost.
  const [last, setLast] = useState<{ role: string; url: string } | null>(null);
  const answers = useAnswers({ policy, chainId, pub, onDone });
  const rows = answers.rows;
  const busy = mine ?? answers.busy;

  // A counterparty on a public copy of /sign cannot reach this desk: its answer comes back as a link to it (`#join=`),
  // which this page hands to the service to check, like a signed step (`#step=`).
  const onJoin = useEffectEvent(async (packed: string) => {
    try {
      // base64url of UTF-8 bytes (docs/60 P-145): a Chinese name survives.
      const bytes = Uint8Array.from(atob(packed.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
      const answer = JSON.parse(new TextDecoder().decode(bytes)) as Record<string, unknown>;
      if (String(answer.policy ?? "").toLowerCase() !== policy.toLowerCase()) throw new Error("This answer is for another desk.");
      const { data } = await deskPost<{ proposed: { name: string; role: string } }>("/api/desk/party/propose", policy, answer);
      toast.success(`${data.proposed.name} answered your invite: approve them as ${data.proposed.role} with one tap.`);
      await answers.refetch();
    } catch (e) {
      // The parser's words are not the owner's (docs/103 S2-6).
      const m = (e as Error).message;
      toast.error(/signature|JSON|atob|base64|character/i.test(m) ? "This answer is not signed properly; ask them to open your invite again." : m);
    }
  });
  useEffect(() => {
    const take = () => {
      const m = /^#join=(.+)$/.exec(location.hash);
      if (!m) return;
      history.replaceState(null, "", location.pathname + location.search);
      void onJoin(m[1]);
    };
    take();
    window.addEventListener("hashchange", take);
    return () => window.removeEventListener("hashchange", take);
  }, []);

  async function invite(role: string) {
    setBusy(role);
    try {
      const { data } = await deskPost<{ invite: string; role: string; expires: number }>("/api/desk/party/invite", policy, { role });
      const url = signingLink(publicUrl, policy, chainId, trader, { join: data.role, inv: data.invite });
      setLast({ role: data.role, url });
      const how = await shareLink(url, "Sign for my desk", `${trader ?? "A trader"} asks you to confirm your steps on their desk as their ${data.role}. Open this on your phone:`);
      if (how === "copied") toast.success(`Invite link copied: send it to your ${data.role.toLowerCase()}. It works once, for a week.`);
      if (how === "shared") toast.success(`Invite sent. When your ${data.role.toLowerCase()} answers, they appear here for one tap.`);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  async function stop(party: string) {
    if (stopping !== party) return setStopping(party);
    setBusy(party);
    try {
      await deskPost("/api/desk/party/revoke", policy, { name: party });
      toast.success(`${party} is stopped: its signature books nothing until you approve it again.`);
      onDone();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setStopping(null);
      setBusy(null);
    }
  }

  return (
    <Section
      title="Who may sign"
      icon={UserCheck}
      description="A step they sign is booked, and the offset follows it by itself up to your limit; above it, it waits for your tap."
    >
      {rows.length > 0 && inInbox && (
        <ul className="mb-3 grid gap-1 text-sm" aria-label="Waiting for your tap">
          {rows.map((p) => (
            <li key={p.id} className="flex flex-wrap items-center gap-2">
              <Identicon addr={p.address} size={14} />
              <span>
                {p.name} asks to sign as {p.role}: approve it under Needs you.
              </span>
              <Button size="sm" variant="ghost" className="h-7" onClick={() => void answers.dismiss(p)} disabled={!!busy}>
                Not them
              </Button>
            </li>
          ))}
        </ul>
      )}
      {rows.length > 0 && !inInbox && (
        <div className="mb-4 grid gap-3 rounded-lg border border-attention/40 bg-background p-3" aria-label="Needs you">
          <p className="text-sm font-medium">Needs you</p>
          {rows.map((p) => {
            const role = ROLES.find((r) => r.role === p.role);
            return (
              <div key={p.id} className="grid gap-2 text-sm">
                <p className="flex flex-wrap items-center gap-1.5">
                  <Identicon addr={p.address} size={16} />
                  <span>
                    Approve <strong>{p.name}</strong> <span className="figures text-muted-foreground">••{tail(p.address)}</span> as {p.role}?
                  </span>
                </p>
                <p className="text-caption text-muted-foreground">
                  They will sign {role?.words ?? "their steps"}
                  {role?.kinds?.some((k) => k === "bought" || k === "sold") ? ", and what they sign moves your offset" : ""}. Their phone shows code{" "}
                  <span className="font-mono font-medium text-foreground">{codeText(p.address, chainId)}</span>; on a call they can read it to you.
                  {invitedWords(p)}
                </p>
                <div className="flex flex-wrap gap-2">
                  <Button size="sm" onClick={() => void answers.approve(p)} disabled={!!busy}>
                    {busy === p.id && <Loader2 className="size-4 animate-spin" />} Approve {p.name}
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => void answers.dismiss(p)} disabled={!!busy}>
                    Not them
                  </Button>
                </div>
              </div>
            );
          })}
        </div>
      )}
      {parties.length > 0 && (
        <ul className="mb-3 flex flex-wrap gap-2">
          {parties.map((p) => (
            <li key={p.address + p.name} className="flex items-center gap-1 rounded-lg border bg-background py-1 pl-2.5 pr-1 text-sm">
              <NameAt name={p.name} address={p.address} chainId={chainId} className={p.stopped ? "text-muted-foreground line-through" : undefined} />
              <span className="text-caption text-muted-foreground">
                · {p.kinds ? (ROLES.find((r) => r.kinds?.join() === p.kinds?.join())?.words ?? p.kinds.join(", ").replaceAll("_", " ")) : "any step"}
              </span>
              {p.stopped ? (
                <span className="px-1.5 text-caption text-muted-foreground">stopped</span>
              ) : (
                <Button size="sm" variant="ghost" className="h-6 px-1.5 text-caption" onClick={() => stop(p.name)} disabled={!!busy} aria-label={`Stop ${p.name}`}>
                  {stopping === p.name ? `Stop ${p.name}?` : <X className="size-3.5" aria-hidden />}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
      <p className="mb-2 text-caption text-muted-foreground">
        Invite who signs: they open the link on their phone, which makes their key and sends it to you with their name; you approve them here with one tap
        {publicUrl ? "" : " (the link opens only on this Mac until the desk has a public address)"}.
      </p>
      <div className="flex flex-wrap gap-2">
        {ROLES.filter((r) => r.kinds).map((r) => (
          <Button key={r.role} size="sm" variant="outline" onClick={() => void invite(r.role)} disabled={!!busy}>
            {busy === r.role ? <Loader2 className="size-3.5 animate-spin" /> : <Send className="size-3.5" />} Invite a {r.role.toLowerCase()}
          </Button>
        ))}
      </div>
      {last && (
        <div className="mt-3 space-y-1">
          <p className="text-caption text-muted-foreground">The {last.role.toLowerCase()} invite, to send again if it did not go:</p>
          <CopyRow value={last.url} label={`the ${last.role.toLowerCase()} invite`} shown={`Invite for your ${last.role.toLowerCase()} (works once, for a week)`} />
        </div>
      )}
      <details className="mt-4">
        <summary className="cursor-pointer text-caption font-medium text-muted-foreground">The signing link for counterparties you already approved</summary>
        <div className="mt-2">
          <CopyRow value={signingLink(publicUrl, policy, chainId, trader)} label="the signing link" shown="Signing link for your counterparties" />
        </div>
      </details>
    </Section>
  );
}

/** Stock the trader replaces every day anyway (docs/84 §1): kept unoffset, so
 *  the offset covers only what is above it. A trust decision: the owner's device. */
export function BaseStock({
  policy,
  chainId,
  pub,
  base,
  onDone,
}: {
  policy: string;
  chainId: number;
  pub: Pub | null;
  base: string;
  onDone: () => void;
}) {
  const [grams, setGrams] = useState("");
  const [busy, setBusy] = useState(false);
  async function save() {
    const g = parseFloat(grams);
    if (!(g >= 0)) return toast.error("How many grams do you keep unoffset?");
    setBusy(true);
    try {
      // A trust decision: the owner's passkey signs it (docs/89 M5).
      const issued = Math.floor(Date.now() / 1000);
      const auth = await passkeyAssert(deskBaseChallenge(chainId, policy, "gold", g.toFixed(3), issued), pub, {
        kind: "base",
        chainId,
        desk: policy,
        issued,
        goods: "gold",
        grams: g.toFixed(3),
      });
      const { data } = await deskPost<{
        unhedgedBase: string;
        offsetTarget?: string | null;
      }>("/api/desk/base", policy, { goods: "gold", grams: g.toFixed(3), issued, auth });
      toast.success(
        `${gramsText(parseFloat(data.unhedgedBase))} kept unoffset; the offset now aims at ${ozText(parseFloat(data.offsetTarget ?? "0"))}.`,
      );
      setGrams("");
      onDone();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Section
      title="Stock you keep unoffset"
      icon={Layers}
      description={`Now ${gramsText(parseFloat(base) || 0)}: what you replace every day anyway. The offset covers what is above it.`}
    >
      <details>
        <summary className="cursor-pointer text-sm font-medium">
          Change it
        </summary>
        <div className="mt-3 flex flex-wrap items-end gap-2">
          <div className="grid gap-1.5">
            <Label htmlFor="base-g">Grams kept unoffset</Label>
            <Input
              id="base-g"
              inputMode="decimal"
              className="w-36"
              placeholder="0.000"
              value={grams}
              onChange={(e) => setGrams(e.target.value)}
            />
          </div>
          <Button size="sm" variant="outline" onClick={save} disabled={busy}>
            {busy && <Loader2 className="size-4 animate-spin" />} Set it
          </Button>
        </div>
      </details>
    </Section>
  );
}

const KINDS: { kind: DeskMilestone["kind"]; label: string }[] = [
  { kind: "bought", label: "Bought (priced purchase)" },
  { kind: "shipped", label: "Shipped" },
  { kind: "arrived", label: "Into stock (vault or safe)" },
  { kind: "to_fabrication", label: "Sent to fabrication" },
  { kind: "consigned", label: "Consigned to a retailer" },
  { kind: "returned", label: "Back into stock" },
  { kind: "sold", label: "Sold" },
];
const NEW = "__new__";

/** The owner's own entry: booked as unsigned, and every statement says so. */
export function EnterStep({
  policy,
  chainId,
  pub,
  deals,
  onDone,
}: {
  policy: string;
  chainId: number;
  pub: Pub | null;
  deals: DeskBook["deals"];
  onDone: () => void;
}) {
  const [deal, setDeal] = useState(
    deals.find((d) => parseFloat(d.held) > 0)?.id ?? NEW,
  );
  const [newId, setNewId] = useState("");
  const [kind, setKind] = useState<DeskMilestone["kind"]>("arrived");
  const [qty, setQty] = useState("");
  const [usdc, setUsdc] = useState("");
  const [party, setParty] = useState("");
  const [doc, setDoc] = useState("");
  const [busy, setBusy] = useState(false);
  const priced = kind === "bought" || kind === "sold";

  async function enter() {
    const id =
      deal === NEW
        ? // A new deal is named after its document when there is one (docs/95 P8), not a clock.
          newId.trim() || doc.trim() || `GOLD-${Math.floor(Date.now() / 1000)}`
        : deal;
    const g = parseFloat(qty);
    if (!(g > 0)) return toast.error("How many grams?");
    if (priced && !(parseFloat(usdc) > 0))
      return toast.error("A purchase or a sale needs its USDC amount.");
    if (!party.trim() || !doc.trim())
      return toast.error("Name the other side and the document.");
    setBusy(true);
    try {
      const milestone = {
        deal: id,
        kind,
        goods: "gold",
        quantity: g.toFixed(3),
        ...(priced ? { usdc: parseFloat(usdc).toFixed(2) } : {}),
        counterparty: party.trim(),
        document: doc.trim(),
        at: Math.floor(Date.now() / 1000),
      };
      // The owner's own step: the passkey signs its EIP-712 digest for this desk (docs/89 M5).
      const issued = Math.floor(Date.now() / 1000);
      const auth = await passkeyAssert(deskStepChallenge(chainId, policy, milestone, issued), pub, {
        kind: "step",
        chainId,
        desk: policy,
        issued,
        milestone,
      });
      await deskPost("/api/desk/milestone", policy, { milestone, issued, auth });
      toast.success(
        priced
          ? "Entered, unsigned. The offset follows it."
          : "Entered, unsigned.",
      );
      setQty("");
      setUsdc("");
      setDoc("");
      onDone();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Section
      title="Enter a step yourself"
      icon={PenLine}
      description="Unsigned: no counterparty vouches for it, and your lender's statement says so."
    >
      <details>
        <summary className="cursor-pointer text-sm font-medium">
          Enter an unsigned step
        </summary>
        <div className="mt-3 grid gap-3">
          <div className="grid gap-1.5">
            <Label>Deal</Label>
            <Select value={deal} onValueChange={setDeal}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {deals.map((d) => (
                  <SelectItem key={d.id} value={d.id}>
                    {d.id}
                  </SelectItem>
                ))}
                <SelectItem value={NEW}>A new deal</SelectItem>
              </SelectContent>
            </Select>
            {deal === NEW && (
              <Input
                aria-label="New deal's name"
                placeholder="e.g. the supplier's PO number (left empty: the document's number)"
                value={newId}
                onChange={(e) => setNewId(e.target.value)}
              />
            )}
          </div>
          <div className="grid gap-1.5">
            <Label>What happened</Label>
            <Select
              value={kind}
              onValueChange={(v) => setKind(v as DeskMilestone["kind"])}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {KINDS.map((k) => (
                  <SelectItem key={k.kind} value={k.kind}>
                    {k.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="grid gap-1.5">
              <Label htmlFor="step-g">Grams</Label>
              <Input
                id="step-g"
                inputMode="decimal"
                placeholder="10.000"
                value={qty}
                onChange={(e) => setQty(e.target.value)}
              />
            </div>
            {priced && (
              <div className="grid gap-1.5">
                <Label htmlFor="step-usdc">USDC</Label>
                <Input
                  id="step-usdc"
                  inputMode="decimal"
                  placeholder="1334.45"
                  value={usdc}
                  onChange={(e) => setUsdc(e.target.value)}
                />
              </div>
            )}
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="grid gap-1.5">
              <Label htmlFor="step-party">With whom</Label>
              <Input
                id="step-party"
                placeholder="Workshop"
                value={party}
                onChange={(e) => setParty(e.target.value)}
              />
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="step-doc">Document</Label>
              <Input
                id="step-doc"
                placeholder="JOB-0142"
                value={doc}
                onChange={(e) => setDoc(e.target.value)}
              />
            </div>
          </div>
          <Button
            size="sm"
            variant="outline"
            onClick={enter}
            disabled={busy}
            className="justify-self-start"
          >
            {busy && <Loader2 className="size-4 animate-spin" />} Enter it,
            unsigned
          </Button>
        </div>
      </details>
    </Section>
  );
}
