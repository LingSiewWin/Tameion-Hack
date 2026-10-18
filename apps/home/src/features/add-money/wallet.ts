// The viewer's own wallet (window.ethereum, EIP-1193) and public reads of the three testnets, with viem. The page holds
// no key: every transaction is the viewer's wallet signing what it shows them.
import { createPublicClient, createWalletClient, custom, getAbiItem, http, type Address, type EIP1193Provider, type Hex } from "viem";
import { abi, MESSAGE_TRANSMITTER, TOKEN_MESSENGER, type CctpChain } from "./cctp.ts";

export const injected = (): EIP1193Provider | undefined => (globalThis as { ethereum?: EIP1193Provider }).ethereum;

const readers = new Map<number, ReturnType<typeof createPublicClient>>();
export function reader(c: CctpChain) {
  let r = readers.get(c.domain);
  if (!r) readers.set(c.domain, (r = createPublicClient({ chain: c.chain, transport: http() })));
  return r;
}

/** The EIP-1193 code anywhere in viem's error chain (4001 = the person said no, 4902 = the wallet lacks the chain). */
export function codeOf(e: unknown): number | undefined {
  for (let x = e as { code?: unknown; cause?: unknown } | undefined, i = 0; x && i < 8; x = x.cause as typeof x, i++)
    if (typeof x.code === "number") return x.code;
  return undefined;
}

function wallet() {
  const p = injected();
  if (!p) throw new Error("No wallet in this browser.");
  return createWalletClient({ transport: custom(p) });
}

export async function connect(): Promise<Address> {
  const [a] = await wallet().requestAddresses();
  return a;
}

/** Put the wallet on `c`: switch, or add the chain first when the wallet does not know it (EIP-3085), then check. */
async function onChain(c: CctpChain) {
  const w = wallet();
  if ((await w.getChainId()) === c.chain.id) return;
  try {
    await w.switchChain({ id: c.chain.id });
  } catch (e) {
    if (codeOf(e) !== 4902) throw e;
    await w.addChain({ chain: c.chain });
  }
  if ((await w.getChainId()) !== c.chain.id) throw new Error(`The wallet is not on ${c.name}.`);
}

export type Phase = "signing" | "sent" | "confirmed";

/** One transaction from the viewer's wallet on `c`. `onPhase("sent", hash)` fires before the receipt, so the caller keeps
 *  the hash: an answer lost after that is read back from the chain, never sent again. Throws when it reverts. */
export async function send(c: CctpChain, to: Address, data: Hex, onPhase: (p: Phase, hash?: Hex) => void): Promise<Hex> {
  const w = wallet();
  const [account] = await w.requestAddresses();
  await onChain(c);
  onPhase("signing");
  const hash = await w.sendTransaction({ account, chain: c.chain, to, data });
  onPhase("sent", hash);
  const receipt = await reader(c).waitForTransactionReceipt({ hash, timeout: 300_000 });
  if (receipt.status !== "success") throw new Error(`The transaction failed on ${c.name}.`);
  onPhase("confirmed", hash);
  return hash;
}

export const usdcHeld = (c: CctpChain, who: Address) => reader(c).readContract({ address: c.usdc, abi, functionName: "balanceOf", args: [who] });
export const allowed = (c: CctpChain, who: Address) => reader(c).readContract({ address: c.usdc, abi, functionName: "allowance", args: [who, TOKEN_MESSENGER] });
export const gasHeld = (c: CctpChain, who: Address) => reader(c).getBalance({ address: who });
/** Whether the destination already received this message (the mint happened, whoever sent it). */
export const minted = async (c: CctpChain, nonce: Hex) =>
  (await reader(c).readContract({ address: MESSAGE_TRANSMITTER, abi, functionName: "usedNonces", args: [nonce] })) === 1n;
/** "success" | "reverted", or null while the chain does not know the receipt (pending, or the read failed). */
export const receiptStatus = (c: CctpChain, hash: Hex) =>
  reader(c)
    .getTransactionReceipt({ hash })
    .then((r) => r.status, () => null);

/** The demo relayer's USDC transfer to the account: the first one after the account opened (the demo pays once). */
export async function demoFunding(c: CctpChain, relayer: Address, account: Address, fromBlock: bigint) {
  const r = reader(c);
  const head = await r.getBlockNumber();
  // ponytail: one read of 200,000 blocks after the open (about 14 h on Arbitrum Sepolia); the relayer funds within seconds
  const to = fromBlock + 200_000n < head ? fromBlock + 200_000n : head;
  const [log] = await r.getLogs({ address: c.usdc, event: getAbiItem({ abi, name: "Transfer" }), args: { from: relayer, to: account }, fromBlock, toBlock: to });
  if (!log) return null;
  const block = await r.getBlock({ blockNumber: log.blockNumber });
  return { tx: log.transactionHash, amount: log.args.value ?? 0n, at: Number(block.timestamp) };
}
