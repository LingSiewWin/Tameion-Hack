"use client";
// `/a/<name>/receive` (H4): the account's address as a QR, its name under it the way a person reads it (`name@chain`,
// ERC-7828's shape, as apps/console/src/lib/names.ts shows a party), Copy and Share, and one line naming the chain.
// The QR is the address (that is the point of this page, ruled 2026-10-18); the Owner home's DOM never carries it, and
// here the address is text only in the Agent view (DESIGN Rule 6). The name resolves through the /api proxy
// (lib/api.ts `resolveAccount`); a shown skeleton holds at least 300 ms (`useSkeleton`). Share only where the browser has it.
import { useEffect, useState, useSyncExternalStore } from "react";
import { encode } from "uqr";
import { useSkeleton } from "../../components/skeleton.tsx";
import { ReceiveSkeleton } from "../../components/skeletons.tsx";
import { resolveAccount, type Account } from "../../lib/api.ts";
import { useView } from "../../lib/view.ts";
import { palette } from "../../tokens.ts";
import { receive as words } from "./copy.ts";
import { CTA, CTA_QUIET, T } from "./styles.ts";

/** The chain after the `@` (apps/console/src/lib/names.ts `LABELS`; CAIP-350's `eip155:<id>` for any other). */
const LABELS: Record<number, string> = { 421614: "arbitrum-sepolia", 5042002: "arc-testnet", 84532: "base-sepolia", 11155111: "sepolia", 42161: "arbitrum", 8453: "base", 1: "ethereum" };
export const nameAt = (name: string, chainId: number) => `${name}@${LABELS[chainId] ?? `eip155:${chainId}`}`;

/** The QR's dark modules as one path; drawn dark on light in both views (Owner tokens), so every camera reads it. */
function qrPath(text: string) {
  const { data, size } = encode(text, { ecc: "M", border: 2 });
  let d = "";
  data.forEach((row, y) => row.forEach((dark, x) => dark && (d += `M${x} ${y}h1v1h-1z`)));
  return { d, size };
}

export function Receive({ name }: { name: string }) {
  // undefined while reading; null when no account has the name; "down" when the service did not answer
  const [rec, setRec] = useState<Account | null | "down" | undefined>(undefined);
  const [tries, setTries] = useState(0);
  useEffect(() => {
    let live = true;
    resolveAccount(name).then(
      (r) => live && setRec(r),
      () => live && setRec("down"),
    );
    return () => {
      live = false;
    };
  }, [name, tries]);
  const loading = useSkeleton(rec === undefined);

  return (
    <div className="flex flex-col items-center gap-(--space-5)">
      <h1 className={`${T.title} self-start`}>{words.title}</h1>
      {loading ? (
        <div className="w-full">
          <ReceiveSkeleton />
        </div>
      ) : rec === "down" ? (
        <div role="alert" className="flex w-full flex-col gap-(--space-3)">
          <p className={T.attention}>{words.down}</p>
          <button
            type="button"
            className={CTA}
            onClick={() => {
              setRec(undefined);
              setTries((n) => n + 1);
            }}
          >
            {words.retry}
          </button>
        </div>
      ) : !rec ? (
        <p className={`${T.row} self-start`}>{words.unknown(name)}</p>
      ) : (
        <Address rec={rec} />
      )}
    </div>
  );
}

function Address({ rec }: { rec: Account }) {
  const view = useView();
  const [copied, setCopied] = useState(false);
  const canShare = useSyncExternalStore(
    () => () => {},
    () => typeof navigator.share === "function",
    () => false,
  );
  const chain = rec.chains.find((c) => c.deployed) ?? rec.chains[0];
  const address = chain?.account ?? rec.policy;
  const qr = qrPath(address);
  const handle = chain ? nameAt(rec.name, chain.chainId) : rec.name;
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
    <>
      <svg
        viewBox={`0 0 ${qr.size} ${qr.size}`}
        role="img"
        aria-label={words.qr(handle)}
        shapeRendering="crispEdges"
        className="size-60 rounded-(--radius-plate) p-(--space-3)"
        style={{ background: palette.owner.plate }}
      >
        <path d={qr.d} fill={palette.owner.ink} />
      </svg>
      <p className={`${T.rowTitle} text-center break-all`}>{handle}</p>
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
      {chain && <p className={`${T.row} self-start`}>{words.arrives(chain.name)}</p>}
    </>
  );
}
