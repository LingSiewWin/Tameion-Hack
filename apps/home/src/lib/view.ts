"use client";
// The view (Owner | Agent) lives on <html data-view>: tokens.css reads it, components read it through useView().
// VIEW_BOOT (tokens.ts) sets it from localStorage before first paint, so a remembered Agent view never flashes Owner.
import { useSyncExternalStore } from "react";
import { flushSync } from "react-dom";
import { VIEW_KEY, type View } from "../tokens.ts";

export const parseView = (v: unknown): View => (v === "agent" ? "agent" : "owner");
export const otherView = (v: View): View => (v === "owner" ? "agent" : "owner");

const listeners = new Set<() => void>();
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
};
const read = (): View => parseView(document.documentElement.dataset.view);

export const useView = (): View => useSyncExternalStore(subscribe, read, () => "owner");

/** Switch the whole surface; the browser crossfades old and new (globals.css sets 200 ms). Remembered per device. */
export function setView(next: View) {
  try {
    localStorage.setItem(VIEW_KEY, next);
  } catch {
    // storage blocked: the switch still happens, it is just not remembered
  }
  const apply = () =>
    flushSync(() => {
      document.documentElement.dataset.view = next;
      listeners.forEach((l) => l());
    });
  // ponytail: no fallback animation where View Transitions are missing; the switch is then instant
  if (typeof document.startViewTransition === "function") document.startViewTransition(apply);
  else apply();
}

/** `v` flips the view unless the person is typing or holding a modifier. */
export const isViewKey = (e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "target">) => {
  if (e.key !== "v" || e.metaKey || e.ctrlKey || e.altKey) return false;
  const t = e.target as HTMLElement | null;
  return !(t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName)));
};
