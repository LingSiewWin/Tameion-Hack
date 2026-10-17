"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { ApiError, fetchInbox, getJson, reserveToken } from "@/lib/tameion";
import type { AnchoredRecord } from "@/lib/verify";

/** A pass's job as `GET /api/watch/<id>` serves it: the pass itself writes down how long it ran (`progress.elapsedSecs`). */
type Job = { id: string; created: number; status: string; progress?: { elapsedSecs?: number } | null };

/** What an account's agents did, from the account's own record and not from an exchange's feed: the inbox (mandate, what was paid and
 *  refused, the last pass), the anchored records (the last fingerprint put on chain) and the last pass's own duration. Read every 15 s,
 *  and again the moment the event stream says something changed (`lastEvent` is the newest event's number). */
export function useAccountRecord(policy: string, lastEvent: number | undefined) {
  const qc = useQueryClient();
  const inbox = useQuery({
    queryKey: ["agent-inbox", policy],
    queryFn: () => fetchInbox(policy),
    refetchInterval: 15_000,
    retry: (n, e) => !(e instanceof ApiError && (e.status === 401 || e.status === 403)) && n < 1,
  });
  const records = useQuery({
    queryKey: ["agent-records", policy],
    queryFn: async () => {
      const device = reserveToken.get(policy);
      try {
        return (await getJson<{ records: AnchoredRecord[] }>(`/api/level1/records?policy=${policy}`, policy, device ? { "x-tameion-device": device } : undefined)).records;
      } catch (e) {
        // "no anchored records for that policy": the agent has not finished a pass yet. That is an answer, not a failure.
        if (e instanceof ApiError && e.status === 404) return [];
        throw e;
      }
    },
    refetchInterval: 30_000,
    retry: false,
  });
  const passId = inbox.data?.lastPass?.id;
  const job = useQuery({
    queryKey: ["agent-job", passId],
    // A pass is served only to a caller that may read the account (docs/60 P-252): this device's token or the agent key.
    queryFn: () => {
      const device = reserveToken.get(policy);
      return getJson<Job>(`/api/watch/${passId}`, policy, device ? { "x-tameion-device": device } : undefined);
    },
    enabled: !!passId,
    // a finished pass does not change
    staleTime: 3_600_000,
    retry: false,
  });
  useEffect(() => {
    if (lastEvent == null) return;
    void qc.invalidateQueries({ queryKey: ["agent-inbox", policy] });
    void qc.invalidateQueries({ queryKey: ["agent-records", policy] });
  }, [lastEvent, policy, qc]);
  return { inbox, records, job };
}
