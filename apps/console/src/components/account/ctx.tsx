"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ExternalLink } from "lucide-react";
import { createContext, useCallback, useContext, useState } from "react";
import { toast } from "sonner";
import { safeHref, type Expect } from "@/lib/guard";
import { getJson, post, sign as signCall, type Executed } from "@/lib/tameion";
import type { Cfg, Inbox, Pub, Tenant } from "@/lib/types";

export type AccountCtx = {
  tenant: Tenant;
  cfg: Cfg;
  inbox: Inbox | undefined;
  pub: Pub | null;
  /** this browser holds one of the account's owner keys; without it the page is read-only */
  owns: boolean;
  explorer: string;
  chainName: string;
  /** refetch the inbox now */
  refresh: () => void;
  /** wrap an owner action: polling pauses while any is in flight */
  track: <T>(p: Promise<T>) => Promise<T>;
  /** sign `data` for `target`, only if it is what `expect` says the button does */
  sign: (target: string, data: string, expect: Expect) => Promise<Executed>;
  post: <T = Record<string, unknown>>(path: string, body: unknown) => Promise<T>;
  /** ask the agent to check now; `say` tells the owner when the service answers
   *  with the last check instead (one check per account per ten minutes) */
  runPass: (say?: boolean) => Promise<void>;
};

const Ctx = createContext<AccountCtx | null>(null);
export const AccountProvider = Ctx.Provider;
export function useAccount() {
  const c = useContext(Ctx);
  if (!c) throw new Error("useAccount outside the account");
  return c;
}

export function makeCtx(
  base: { tenant: Tenant; cfg: Cfg; inbox: Inbox | undefined; pub: Pub | null; owns: boolean },
  refresh: () => void,
  track: AccountCtx["track"],
): AccountCtx {
  const chain = base.cfg.chains.find((c) => c.key === base.tenant.chain);
  const policy = base.tenant.policy;
  return {
    ...base,
    explorer: chain?.explorer ?? "",
    chainName: chain?.name ?? base.tenant.chain,
    refresh,
    track,
    sign: (target, data, expect) =>
      track(base.owns ? signCall(base, target, data, expect) : Promise.reject(new Error("Not signing: this browser holds no owner key of this account."))),
    post: (path, body) => track(post(path, body, policy)),
    runPass: async (say = false) => {
      try {
        const r = await track(post<{ cached?: boolean; status?: string }>("/api/level1/run", { policy }, policy));
        if (say && r.cached && r.status === "done") {
          const at = base.inbox?.lastPass?.at;
          const left = at ? Math.max(1, Math.ceil((at + 600 - Date.now() / 1000) / 60)) : 10;
          toast(`The agent checked a few minutes ago and can check again in about ${left} min. Anything new is picked up then.`);
        }
      } catch (e) {
        toast.error((e as Error).message);
      }
      setTimeout(refresh, 1500);
    },
  };
}

/** The busy counter the inbox poll reads: a row being tapped is never redrawn
 *  by a refresh mid-flight (the classic page's 15 s refresh wiped an in-flight
 *  Pay; docs/66). */
export function useBusy() {
  const [busy, setBusy] = useState(0);
  const track = useCallback(async <T,>(p: Promise<T>) => {
    setBusy((b) => b + 1);
    try {
      return await p;
    } finally {
      setBusy((b) => b - 1);
    }
  }, []);
  return { busy, track };
}

export function useInvalidate(policy: string) {
  const qc = useQueryClient();
  return useCallback(() => qc.invalidateQueries({ queryKey: ["inbox", policy] }), [qc, policy]);
}

/** The service's record of the account (name, connected wallets, spare key):
 *  fresher than the copy this browser kept when it opened the account. */
export function useRecord(policy: string) {
  return useQuery({
    queryKey: ["tenant", policy],
    queryFn: async () => (await getJson<{ tenant: Tenant }>(`/api/level1/tenant/${policy}`)).tenant,
    staleTime: 30_000,
  });
}

export type ActionState = { kind: "idle" } | { kind: "working"; label: string } | { kind: "done"; text: string; href?: string } | { kind: "error"; text: string };

/** One row's action: its state lives in the row, keyed by the row's stable id,
 *  so a refetch re-renders around it instead of replacing it. */
export function useAction() {
  const [state, setState] = useState<ActionState>({ kind: "idle" });
  const run = useCallback(async (label: string, fn: (step: (l: string) => void) => Promise<{ text: string; href?: string } | void>) => {
    setState({ kind: "working", label });
    try {
      const r = await fn((l) => setState({ kind: "working", label: l }));
      if (r) {
        setState({ kind: "done", ...r });
        const href = safeHref(r.href);
        toast.success(r.text, href ? { action: { label: "Receipt", onClick: () => window.open(href, "_blank", "noopener") } } : undefined);
      } else setState({ kind: "idle" });
    } catch (e) {
      const text = (e as Error).message;
      setState(text === "cancelled" ? { kind: "idle" } : { kind: "error", text });
    }
  }, []);
  return { state, run, reset: () => setState({ kind: "idle" }) };
}

export function Receipt({ href: raw, children = "Receipt" }: { href?: string; children?: React.ReactNode }) {
  const href = safeHref(raw);
  if (!href) return null;
  return (
    <a href={href} target="_blank" rel="noopener" className="inline-flex items-center gap-1 font-medium text-foreground underline-offset-4 hover:underline">
      {children}
      <ExternalLink className="size-3" aria-hidden />
    </a>
  );
}
