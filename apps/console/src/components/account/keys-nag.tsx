"use client";

import { ShieldAlert } from "lucide-react";
import { useCallback } from "react";
import { Button } from "@/components/ui/button";
import { keysReady, keysSet, useKeys } from "@/lib/keys";
import type { Tenant } from "@/lib/types";
import { useConfirm } from "./confirm";

/** "1 of 3 keys set": shown until two guardians can replace the owner's key (docs/104), and — more urgent — a
 *  replacement key waiting out the delay, which only the daily key can cancel (docs/103 S1-2). Nothing while the
 *  chain has not answered. `onAdd` opens wherever the page keeps the Keys panel. */
export function KeysNag({ tenant, onAdd }: { tenant: Pick<Tenant, "chain" | "owner">; onAdd?: () => void }) {
  const q = useKeys(tenant);
  const k = q.data;
  if (k?.pending)
    return (
      <div role="alert" className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-destructive/40 px-4 py-3 text-sm text-destructive sm:px-5">
        <span className="flex items-start gap-2">
          <ShieldAlert className="mt-0.5 size-4 shrink-0" aria-hidden />
          <span>
            A new key is waiting to replace yours at {new Date(k.pending.eta * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}. If you did not
            ask for it, cancel it before then.
          </span>
        </span>
        {onAdd && (
          <Button size="sm" variant="destructive" onClick={onAdd}>
            Open Keys
          </Button>
        )}
      </div>
    );
  if (!k || keysReady(k)) return null;
  return (
    <div role="status" className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-attention/45 bg-attention-soft/40 px-4 py-3 text-sm sm:px-5">
      <span className="flex items-start gap-2">
        <ShieldAlert className="mt-0.5 size-4 shrink-0 text-attention" aria-hidden />
        <span>
          <span className="font-medium">{keysSet(k)} of 3 keys set</span> — add two guardians before raising a limit. Today one lost device loses
          this account&apos;s key for good.
        </span>
      </span>
      {onAdd && (
        <Button size="sm" onClick={onAdd}>
          Add guardians
        </Button>
      )}
    </div>
  );
}

/** Explain, not forbid: before a trust-widening action (a new counterparty, a higher limit or autonomy, a trade past
 *  the auto limit), `gate("raise the auto limit to 2,000 g")` resolves true at once when two guardians are set, and
 *  otherwise shows why that is risky and resolves with the owner's answer. While the first read is still on its way it
 *  waits for it, so "could not be read" means the chain did not answer (docs/103 S1-6). Render `dialog` once. */
export function useKeysGate(tenant: Pick<Tenant, "chain" | "owner">) {
  const q = useKeys(tenant);
  const { confirm, dialog } = useConfirm();
  const { data, refetch } = q;
  const gate = useCallback(
    async (what: string) => {
      const k = data ?? (await refetch()).data;
      if (k && keysReady(k)) return true;
      return confirm({
        title: k ? `${keysSet(k)} of 3 keys set` : "Your keys could not be read from the chain",
        body: (
          <>
            <p>You are about to {what}.</p>
            <p className="mt-2">
              {k
                ? "Add two guardians first: if you lose this device you cannot get the account back, and what you approve now stays approved. Your email wallet and a hardware wallet can be the two."
                : "This page could not check whether two guardians can replace your key. Add two guardians first — your email wallet and a hardware wallet."}{" "}
              Or go on.
            </p>
          </>
        ),
        action: "Go on anyway",
      });
    },
    [data, refetch, confirm],
  );
  return { gate, dialog, ready: !!data && keysReady(data) };
}
