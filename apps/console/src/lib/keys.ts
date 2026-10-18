// The owner's three keys (docs/104, docs/102 D10): key 0 — the passkey that signs every decision — and two guardians
// who together can replace it after the account's recovery delay. Read from two public nodes of the account's chain,
// never from our service: the panel shows what the contract will do, not what we say it will do.
import { useQuery } from "@tanstack/react-query";
import { createPublicClient, getAddress, http, isAddressEqual, parseAbi, toHex, verifyMessage, type Hex } from "viem";
import { eth } from "@/lib/tameion";
import type { Tenant } from "@/lib/types";
import { NODES } from "@/lib/verify";

const OWNER_ABI = parseAbi([
  "function qx() view returns (bytes32)",
  "function qy() view returns (bytes32)",
  "function guardianCount() view returns (uint256)",
  "function guardians(uint256 i) view returns (address)",
  "function guardianThreshold() view returns (uint8)",
  "function recovery() view returns (address)",
  "function recoveryDelay() view returns (uint256)",
  "function pendingKey() view returns (bytes32 qx, bytes32 qy, uint64 eta)",
]);

export const ZERO_ADDRESS = "0x0000000000000000000000000000000000000000";

export type Keys = {
  /** false while the account is reserved: its owner contract goes on chain with the first passkey tap */
  onChain: boolean;
  guardians: Hex[];
  threshold: number;
  /** the one address that may propose a new key alone (zero: nobody) */
  recovery: Hex;
  delaySecs: number;
  /** a replacement key waiting out the delay */
  pending: { qx: Hex; qy: Hex; eta: number } | null;
};

const client = (url: string) => createPublicClient({ transport: http(url, { timeout: 15_000, retryCount: 1 }) });

async function readFrom(url: string, owner: Hex): Promise<Keys> {
  const c = client(url);
  const code = await c.getCode({ address: owner });
  if (!code || code === "0x") return { onChain: false, guardians: [], threshold: 0, recovery: ZERO_ADDRESS, delaySecs: 0, pending: null };
  const at = { address: owner, abi: OWNER_ABI } as const;
  const [n, threshold, recovery, delay, p] = await Promise.all([
    c.readContract({ ...at, functionName: "guardianCount" }),
    c.readContract({ ...at, functionName: "guardianThreshold" }),
    c.readContract({ ...at, functionName: "recovery" }),
    c.readContract({ ...at, functionName: "recoveryDelay" }),
    c.readContract({ ...at, functionName: "pendingKey" }),
  ]);
  const guardians = await Promise.all(
    Array.from({ length: Math.min(Number(n), 16) }, (_, i) => c.readContract({ ...at, functionName: "guardians", args: [BigInt(i)] })),
  );
  const [pqx, pqy, eta] = p;
  return { onChain: true, guardians, threshold, recovery, delaySecs: Number(delay), pending: eta > 0n ? { qx: pqx, qy: pqy, eta: Number(eta) } : null };
}

/** The keys as the chain holds them; the second public node answers when the first does not. */
export async function readKeys(chain: string, owner: string): Promise<Keys> {
  const nodes = NODES[chain];
  if (!nodes) throw new Error(`This page knows no public node for ${chain}.`);
  let last: unknown;
  for (const url of nodes) {
    try {
      return await readFrom(url, owner as Hex);
    } catch (e) {
      last = e;
    }
  }
  throw new Error(`The chain did not answer: ${(last as Error)?.message?.split("\n")[0] ?? "no node reachable"}`);
}

/** Waits until a public node has the transaction in a block (up to two minutes), so the panel re-reads the chain
 *  after the change is there rather than on a timer (docs/103 S1-8). A failed transaction says so. */
export async function landed(chain: string, tx?: string) {
  const nodes = NODES[chain];
  if (!nodes || !tx) return;
  const r = await client(nodes[0])
    .waitForTransactionReceipt({ hash: tx as Hex, timeout: 120_000, pollingInterval: 2_000 })
    .catch(() => null);
  if (r?.status === "reverted") throw new Error("The transaction failed on chain; nothing changed.");
}

export const useKeys = (t: Pick<Tenant, "chain" | "owner">) =>
  useQuery({
    queryKey: ["keys", t.chain, t.owner.toLowerCase()],
    queryFn: () => readKeys(t.chain, t.owner),
    staleTime: 15_000,
    refetchInterval: 60_000,
  });

/** Key 0 always exists; each guardian is a key, up to the two the panel asks for. */
export const keysSet = (k: Keys) => 1 + Math.min(k.guardians.length, 2);
/** Two guardians who must agree: a lost key 0 can be replaced, and no single other key can take it. */
export const keysReady = (k: Keys) => k.guardians.length >= 2 && k.threshold >= 2;

/** What a guardian signs when it agrees to be one: no gas, and proof that the wallet can sign at all
 *  (a watch-only account would otherwise be a dead slot found only on the day of recovery). */
export const guardianConsent = (owner: string, chainId: number) =>
  `Tami: I agree to be a guardian of account ${getAddress(owner)} on chain ${chainId}. ` +
  "Together with the other guardian I can replace its owner's passkey after a delay the owner can cancel. A guardian cannot spend.";

/** A guardian from a wallet in this browser (a hardware wallet through MetaMask, Rabby or Frame; an email wallet):
 *  its address comes from the wallet, never typed, and the wallet signs the consent above before it is used. */
export async function connectGuardian(
  acct: { owner: string; chain: string },
  chainId: number,
  refuse: { address?: string | null; why: string }[],
): Promise<Hex> {
  const w = eth();
  if (!w) throw new Error("This browser has no wallet. Open this page where your hardware wallet or your email wallet is connected.");
  const cancelled = (e: unknown) => (e as { code?: number }).code === 4001;
  // The account picker, so the second guardian can be another account; a wallet without it goes straight on.
  await w.request({ method: "wallet_requestPermissions", params: [{ eth_accounts: {} }] }).catch((e) => {
    if (cancelled(e)) throw new Error("cancelled");
  });
  const accounts = (await w.request({ method: "eth_requestAccounts" }).catch((e) => {
    throw new Error(cancelled(e) ? "cancelled" : (e as Error).message);
  })) as string[];
  const address = getAddress(accounts[0] ?? ZERO_ADDRESS);
  if (address === ZERO_ADDRESS) throw new Error("The wallet named no account.");
  for (const r of refuse) if (r.address && isAddressEqual(address, r.address as Hex)) throw new Error(r.why);
  const message = guardianConsent(acct.owner, chainId);
  const signature = (await w.request({ method: "personal_sign", params: [toHex(message), address] }).catch((e) => {
    throw new Error(cancelled(e) ? "cancelled" : (e as Error).message);
  })) as Hex;
  // An ordinary wallet is checked here; a smart-account guardian (ERC-1271) by the chain's own rules.
  const nodes = NODES[acct.chain];
  const ok =
    (await verifyMessage({ address, message, signature }).catch(() => false)) ||
    (!!nodes && (await client(nodes[0]).verifyMessage({ address, message, signature }).catch(() => false)));
  if (!ok) throw new Error("That wallet's signature does not match its address; it cannot be a guardian.");
  return address;
}
