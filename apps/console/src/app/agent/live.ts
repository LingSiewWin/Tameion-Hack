"use client";

import { useEffect, useEffectEvent, useState, useSyncExternalStore } from "react";
import { ApiError, authHeaders, reserveToken } from "@/lib/tameion";
import { createSseParser, describeEvent, readFrame, venueSeen, type EventRow, type Frame, type SseEvent } from "./feed";

// ---- the clock: one shared tick a second, for "3 s ago" and for noticing a feed gone quiet ----------------
// `useNow` is called once, by the page, and the time goes down as a prop: every call would start its own timer.
let tick = 0;
const subscribeClock = (notify: () => void) => {
  tick = Date.now();
  const id = setInterval(() => {
    tick = Date.now();
    notify();
  }, 1000);
  return () => clearInterval(id);
};
/** Milliseconds now, ticking each second; 0 until the browser has taken over from the static page. */
export const useNow = () =>
  useSyncExternalStore(
    subscribeClock,
    () => tick,
    () => 0,
  );

// ---- a Server-Sent Events route, read with fetch ----------------------------------------------------------
// Not `EventSource`: it cannot send a header, and the desk's routes open to this device's token in
// `x-tameion-device` (tameion-app.rs `desk_owner`) or to an agent key's bearer.
export type Link = { kind: "connecting" } | { kind: "open" } | { kind: "down"; status: number; message: string };

const headersFor = (policy: string): Record<string, string> => {
  const tok = reserveToken.get(policy);
  return { accept: "text/event-stream", ...authHeaders(policy), ...(tok ? { "x-tameion-device": tok } : {}) };
};

async function readSse(path: string, policy: string, signal: AbortSignal, onOpen: () => void, onEvent: (e: SseEvent) => void) {
  const res = await fetch(path, { headers: headersFor(policy), signal, cache: "no-store" });
  if (!res.ok || !res.body) {
    // The service answers an error as JSON, and a route it does not have as plain text.
    const d = (await res.json().catch(() => ({}))) as { error?: string };
    throw new ApiError(d.error ?? (res.statusText || `answered ${res.status}`), res.status);
  }
  onOpen();
  const reader = res.body.getReader();
  const text = new TextDecoder();
  const push = createSseParser(onEvent);
  for (;;) {
    const { done, value } = await reader.read();
    if (done) return;
    push(text.decode(value, { stream: true }));
  }
}

const sleep = (ms: number, signal: AbortSignal) =>
  new Promise<void>((done) => {
    const t = setTimeout(done, ms);
    signal.addEventListener(
      "abort",
      () => {
        clearTimeout(t);
        done();
      },
      { once: true },
    );
  });

/** Keeps `path` open and calls `onEvent` for each event; when the stream fails or ends it comes back after
 *  1 s, 2 s, 4 s … never more than 15 s apart, so a service restarted under the page is picked up again. */
export function useSse(path: string, policy: string, onEvent: (e: SseEvent) => void) {
  const [link, setLink] = useState<Link>({ kind: "connecting" });
  const [again, setAgain] = useState(0);
  const handle = useEffectEvent((e: SseEvent) => onEvent(e));
  useEffect(() => {
    const stop = new AbortController();
    void (async () => {
      let tries = 0;
      while (!stop.signal.aborted) {
        try {
          await readSse(
            path,
            policy,
            stop.signal,
            () => {
              tries = 0;
              setLink({ kind: "open" });
            },
            (e) => handle(e),
          );
          throw new Error("The service closed the stream.");
        } catch (e) {
          if (stop.signal.aborted) return;
          setLink({ kind: "down", status: e instanceof ApiError ? e.status : 0, message: e instanceof Error ? e.message : String(e) });
          await sleep(Math.min(15_000, 1000 * 2 ** tries++), stop.signal);
        }
      }
    })();
    return () => stop.abort();
  }, [path, policy, again]);
  return { link, retry: () => setAgain((n) => n + 1) };
}

// ---- the two streams the page reads ----------------------------------------------------------------------
/** `GET /api/desk/live`: a frame a second. Keeps the last good frame, when this page got it (ms), and when this page last saw
 *  the venue's own reading change (ms; null while the venue's numbers are not sent). */
export function useLiveFeed(policy: string) {
  const [last, setLast] = useState<{ frame: Frame; seenAt: number; venueSeenAt: number | null } | null>(null);
  const [rejected, setRejected] = useState<string | null>(null);
  const sse = useSse(`/api/desk/live?policy=${policy}`, policy, (e) => {
    if (e.event === "hello") return;
    let json: unknown;
    try {
      json = JSON.parse(e.data);
    } catch {
      setRejected("the feed sent text that is not JSON");
      return;
    }
    const r = readFrame(json);
    if ("frame" in r) {
      const now = Date.now();
      const frame = r.frame;
      setLast((prev) => ({ frame, seenAt: now, venueSeenAt: venueSeen(prev && { venueTime: prev.frame.venueTime, venueSeenAt: prev.venueSeenAt }, frame.venueTime, now) }));
      setRejected(null);
    } else setRejected(r.error);
  });
  return { ...sse, last, rejected };
}

const KEEP = 60;
/** `GET /api/level1/events`: what happened on the account, newest first, the last 60 of this visit. */
export function useEvents(policy: string) {
  const [rows, setRows] = useState<EventRow[]>([]);
  const sse = useSse(`/api/level1/events?policy=${policy}`, policy, (e) => {
    let data: unknown = null;
    try {
      data = JSON.parse(e.data);
    } catch {}
    const at = Date.now();
    const said = describeEvent(e.event, data);
    setRows((prev) => [{ id: (prev[0]?.id ?? 0) + 1, at, ...said }, ...prev].slice(0, KEEP));
  });
  return { ...sse, rows };
}
