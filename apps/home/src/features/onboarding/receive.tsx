// `/a/<name>/receive` (H4): the account's address as a QR, its name under it the way a person reads it (`name@chain`,
// ERC-7828's shape, as apps/console/src/lib/names.ts shows a party), Copy and Share, and one line naming the chain.
// The QR is the address (that is the point of this page, ruled 2026-10-18); the Owner home's DOM never carries it.
// A server component: the name resolves here (lib/api.ts `resolveAccount`) and the QR is drawn here, as an SVG.
import { encode } from "uqr";
import { resolveAccount, type Account } from "../../lib/api.ts";
import { palette } from "../../tokens.ts";
import { receive as words } from "./copy.ts";
import { ReceiveButtons } from "./receive-buttons.tsx";
import { T } from "./ui.tsx";

/** The chain after the `@` (apps/console/src/lib/names.ts `LABELS`; CAIP-350's `eip155:<id>` for any other). */
const LABELS: Record<number, string> = { 421614: "arbitrum-sepolia", 5042002: "arc-testnet", 84532: "base-sepolia", 11155111: "sepolia", 42161: "arbitrum", 8453: "base", 1: "ethereum" };
export const nameAt = (name: string, chainId: number) => `${name}@${LABELS[chainId] ?? `eip155:${chainId}`}`;

async function resolve(name: string): Promise<Account | null | "down"> {
  try {
    return await resolveAccount(name);
  } catch {
    return "down";
  }
}

/** The QR's dark modules as one path; drawn dark on light in both views (Owner tokens), so every camera reads it. */
function qrPath(text: string) {
  const { data, size } = encode(text, { ecc: "M", border: 2 });
  let d = "";
  data.forEach((row, y) => row.forEach((dark, x) => dark && (d += `M${x} ${y}h1v1h-1z`)));
  return { d, size };
}

export async function Receive({ name }: { name: string }) {
  const rec = await resolve(name);
  if (rec === "down")
    return (
      <p role="alert" className={T.attention}>
        {words.down}
      </p>
    );
  if (!rec) return <p className={T.row}>{words.unknown(name)}</p>;
  const chain = rec.chains.find((c) => c.deployed) ?? rec.chains[0];
  const address = chain?.account ?? rec.policy;
  const qr = qrPath(address);
  const handle = chain ? nameAt(rec.name, chain.chainId) : rec.name;
  return (
    <div className="flex flex-col items-center gap-(--space-5)">
      <h1 className={`${T.title} self-start`}>{words.title}</h1>
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
      <ReceiveButtons address={address} handle={handle} />
      {chain && <p className={`${T.row} self-start`}>{words.arrives(chain.name)}</p>}
    </div>
  );
}
