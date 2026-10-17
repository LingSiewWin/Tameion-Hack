"use client";
// Copy and Share for Receive (H4). Share shows only where the browser has the Web Share API. The address is shown as
// text only in the Agent view (DESIGN Rule 6); the Owner view reads the name.
import { useState, useSyncExternalStore } from "react";
import { useView } from "../../lib/view.ts";
import { receive as words } from "./copy.ts";
import { CTA, CTA_QUIET, T } from "./ui.tsx";

export function ReceiveButtons({ address, handle }: { address: string; handle: string }) {
  const view = useView();
  const [copied, setCopied] = useState(false);
  const canShare = useSyncExternalStore(
    () => () => {},
    () => typeof navigator.share === "function",
    () => false,
  );
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(address);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
    }
  };
  return (
    <div className="flex w-full flex-col gap-(--space-3)">
      {view === "agent" && <p className={`${T.mono} break-all text-center`}>{address}</p>}
      <div className="flex w-full gap-(--space-3)">
        <button type="button" className={CTA} onClick={copy} aria-live="polite">
          {copied ? words.copied : words.copy}
        </button>
        {canShare && (
          <button type="button" className={CTA_QUIET} onClick={() => navigator.share({ title: handle, text: address }).catch(() => undefined)}>
            {words.share}
          </button>
        )}
      </div>
    </div>
  );
}
