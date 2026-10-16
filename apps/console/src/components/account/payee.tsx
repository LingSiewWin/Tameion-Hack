"use client";

import { BadgeCheck, ExternalLink, Loader2, Pencil, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { keccak256, toBytes } from "viem";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { isAddr, resolveName, saveName, tail, type Resolved } from "@/lib/tameion";
import type { Inbox, Party } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useAccount, useInvalidate } from "./ctx";

/** A party as a person reads it (docs/81): the service's name for it, or — for
 *  an address the service has not named — its last four, like a card. */
export function partyOf(inbox: Inbox | undefined, addr?: string | null): Party | undefined {
  if (!addr) return undefined;
  return inbox?.parties?.[addr.toLowerCase()] ?? { label: `Payee ••${tail(addr)}`, kind: "new", verified: false, note: "", tail: tail(addr) };
}

export function usePayeeName(addr?: string | null) {
  const { inbox } = useAccount();
  return partyOf(inbox, addr)?.label ?? "";
}

/** A small picture drawn from the address itself, so one party always looks the
 *  same. Drawn here, from the address the page is about to sign for — not from
 *  anything the service says — and from the hash of all of it: a look-alike
 *  address (same first and last characters, the poisoning trick) looks
 *  nothing alike. */
export function Identicon({ addr, size = 16, className }: { addr: string; size?: number; className?: string }) {
  const h = keccak256(toBytes(addr.toLowerCase())).slice(2);
  const hue = parseInt(h.slice(0, 4), 16) % 360;
  const bg = (hue + 150 + (parseInt(h.slice(4, 6), 16) % 60)) % 360;
  const cells: [number, number][] = [];
  for (let r = 0; r < 5; r++)
    for (let c = 0; c < 3; c++)
      if (parseInt(h[6 + r * 3 + c], 16) % 2) {
        cells.push([c, r]);
        if (c < 2) cells.push([4 - c, r]);
      }
  return (
    <svg width={size} height={size} viewBox="0 0 5 5" aria-hidden className={cn("shrink-0 rounded-[3px]", className)} shapeRendering="crispEdges">
      <rect width="5" height="5" fill={`hsl(${bg} 55% 88%)`} />
      {cells.map(([x, y]) => (
        <rect key={`${x}-${y}`} x={x} y={y} width="1" height="1" fill={`hsl(${hue} 55% 38%)`} />
      ))}
    </svg>
  );
}

/** What a person typed in a payee or wallet field, as an address: typed as one,
 *  or a name the service resolved (Tameion name, ENS, a domain's own list). */
export function useTyped(text: string, chain: string) {
  const [r, setR] = useState<{ q: string; ok?: Resolved; error?: string }>();
  const q = text.trim();
  useEffect(() => {
    if (!q || isAddr(q)) return;
    const id = setTimeout(async () => {
      try {
        setR({ q, ok: await resolveName(q, chain) });
      } catch (e) {
        setR({ q, error: (e as Error).message });
      }
    }, 400);
    return () => clearTimeout(id);
  }, [q, chain]);
  const found = r?.q === q ? r : undefined;
  const address = isAddr(q) ? q : found?.ok?.address;
  return { q, address, label: isAddr(q) ? "" : (found?.ok?.label ?? ""), found, looking: !!q && !isAddr(q) && !found };
}

const KIND_WORDS: Record<Resolved["kind"], string> = {
  tameion: "a Tameion account",
  ens: "its ENS name, checked both ways",
  domain: "listed by the domain itself",
  address: "",
};

/** The line under a typed payee: who it resolved to, or why not. */
export function TypedParty({ t }: { t: ReturnType<typeof useTyped> }) {
  if (!t.q) return null;
  if (t.looking) return <p className="text-caption text-muted-foreground">Looking up {t.q}…</p>;
  if (t.found?.error) return <p className="text-caption text-destructive">{t.found.error}</p>;
  if (!t.address) return null;
  const k = t.found?.ok?.kind;
  return (
    <p className="flex items-center gap-1.5 text-caption text-muted-foreground">
      <Identicon addr={t.address} size={14} />
      <span className="font-medium text-foreground">{t.label || `••${tail(t.address)}`}</span>
      {k && k !== "address" && <BadgeCheck className="size-3.5 text-primary" aria-hidden />}
      {k && KIND_WORDS[k] ? ` ${KIND_WORDS[k]} · ` : " "}ends ••{tail(t.address)}
    </p>
  );
}

const SOURCE: Record<Party["kind"], string> = {
  owner: "You named it.",
  you: "",
  tameion: "A Tameion account, by its own name.",
  domain: "Proved by its web domain.",
  ens: "Its ENS name, checked both ways.",
  x402: "The site you bought from gave this address.",
  invoice: "Named by its own invoice; not checked.",
  clash: "",
  new: "No name yet.",
};

/** A party in a sentence: its picture and name. Tap for what it is, how the
 *  name is known, and — only there — the address and the explorer. */
export function Payee({ addr, nameable = true }: { addr: string; nameable?: boolean }) {
  const { tenant, inbox, explorer, owns } = useAccount();
  const invalidate = useInvalidate(tenant.policy);
  const p = partyOf(inbox, addr)!;
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const clash = p.kind === "clash";
  const checked = p.verified && p.kind !== "owner" && p.kind !== "you";
  return (
    <span className="inline-flex items-center gap-1 align-baseline">
      <Popover>
        <PopoverTrigger asChild>
          <button
            type="button"
            className={cn(
              "inline-flex items-center gap-1.5 rounded-sm font-medium underline-offset-4 hover:underline focus-visible:outline-2",
              clash && "text-destructive",
            )}
          >
            <Identicon addr={addr} />
            {p.label}
            {checked && <BadgeCheck className="size-3.5 text-primary" aria-label="checked" />}
            {p.kind === "invoice" && <span className="font-normal text-muted-foreground">(not checked)</span>}
            {clash && <TriangleAlert className="size-3.5" aria-label="warning" />}
          </button>
        </PopoverTrigger>
        <PopoverContent className="w-72 space-y-2 text-sm">
          <p className="flex items-center gap-2 font-medium">
            <Identicon addr={addr} size={20} />
            {p.label}
          </p>
          {p.note && <p className={clash ? "text-destructive" : "text-muted-foreground"}>{p.note}</p>}
          {SOURCE[p.kind] && <p className="text-muted-foreground">{SOURCE[p.kind]}</p>}
          {p.domain && p.kind !== "domain" && (
            <p className="text-muted-foreground">
              Says it is <span className="font-medium text-foreground">{p.domain}</span>.
            </p>
          )}
          <details className="text-xs">
            <summary className="cursor-pointer text-muted-foreground">Address</summary>
            <p className="mt-1 break-all font-mono">{addr}</p>
          </details>
          {explorer && (
            <a
              href={`${explorer}/address/${addr}`}
              target="_blank"
              rel="noopener"
              className="inline-flex items-center gap-1 text-xs font-medium underline-offset-4 hover:underline"
            >
              View on the explorer <ExternalLink className="size-3" aria-hidden />
            </a>
          )}
        </PopoverContent>
      </Popover>
      {nameable && owns && (
        <button
          type="button"
          aria-label={`Name ${p.label}`}
          className="rounded p-0.5 text-muted-foreground hover:text-foreground focus-visible:outline-2"
          onClick={() => {
            setDraft(p.kind === "owner" ? p.label : "");
            setOpen(true);
          }}
        >
          <Pencil className="size-3" />
        </button>
      )}
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <Identicon addr={addr} size={20} /> Name this payee
            </DialogTitle>
            <DialogDescription>
              Ends in ••{p.tail}. Everyone who opens this account sees this name; only the device that opened the account can change it.
            </DialogDescription>
          </DialogHeader>
          <Input autoFocus value={draft} maxLength={40} onChange={(e) => setDraft(e.target.value)} placeholder="e.g. Hosting, Auditor, Alice (design)" />
          <DialogFooter>
            <Button
              disabled={saving}
              onClick={async () => {
                setSaving(true);
                try {
                  await saveName(tenant.policy, addr, draft);
                  await invalidate();
                  setOpen(false);
                } catch (e) {
                  toast.error((e as Error).message);
                } finally {
                  setSaving(false);
                }
              }}
            >
              {saving && <Loader2 className="size-4 animate-spin" />}
              {draft.trim() ? "Save name" : "Forget the name"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </span>
  );
}
