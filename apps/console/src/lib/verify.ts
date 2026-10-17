// "Check it yourself" (CLAUDE.md: every decision provable on-chain): the browser
// rebuilds the agent's record from its own events and asks two public nodes of the
// account's chain what was anchored, under the agent the account's policy names and
// in the pinned record contract. The service supplies the events, not the verdict.
// Limit: this code is served by the same site; `scripts/verify_tenant.sh` from a
// clone of the repository is the check that trusts nothing here.
// Same rules as `crates/operator/examples/audit.rs` and `scripts/verify_tenant.sh`.
import { concat, createPublicClient, decodeFunctionData, http, isAddressEqual, keccak256, parseAbi, type Hex } from "viem";

const ANCHOR_ABI = parseAbi(["function anchor(bytes32 root)", "function rootOf(address agent) view returns (bytes32)"]);
const POLICY_ABI = parseAbi(["function agent() view returns (address)", "function operator() view returns (address)"]);
/** A keyless account names this as its agent: no key can pay; the service's operator key writes the record. */
const NO_AGENT = "0x000000000000000000000000000000000000dEaD";

/** Two public nodes per chain, neither of them Tameion's (the same as `verify_tenant.sh`). */
export const NODES: Record<string, readonly [string, string]> = {
  "arc-testnet": ["https://rpc.testnet.arc.network", "https://explorer.testnet.arc.io/api/eth-rpc"],
  "arbitrum-sepolia": ["https://sepolia-rollup.arbitrum.io/rpc", "https://arbitrum-sepolia-rpc.publicnode.com"],
};

/** The record contract (MemoryAnchor) on each chain, pinned here rather than taken from the service: a
 *  lying service could otherwise name a contract of its own (review 2026-10-15). */
export const ANCHORS: Record<string, Hex> = {
  "arc-testnet": "0xDAb65A5380BCC2Eebc3A3A91d0AD11Eec95994dB",
  "arbitrum-sepolia": "0xe7C381AAFA077CC99359E8b297D2125efD76B8F5",
};

/** RFC 6962 over Keccak-256, as `ledger::log` builds it: 0x00 before a leaf, 0x01 before a node. */
const leaf = (canonical: Hex) => keccak256(concat(["0x00", canonical]));
const node = (l: Hex, r: Hex) => keccak256(concat(["0x01", l, r]));

export function merkleRoot(leaves: Hex[]): Hex {
  if (leaves.length === 0) return `0x${"0".repeat(64)}`;
  if (leaves.length === 1) return leaves[0];
  let k = 1; // the largest power of two strictly below n
  while (k * 2 < leaves.length) k *= 2;
  return node(merkleRoot(leaves.slice(0, k)), merkleRoot(leaves.slice(k)));
}

/** The root of an events file as the service serves it (`/api/level1/record`, one JSON event per line). On an
 *  account with agent keys an outsider gets each event's leaf hash instead of the event (docs/60 P-212); the root
 *  rebuilt from those is the same root. */
export function rootOfEvents(jsonl: string): { root: Hex; count: number } {
  const events = jsonl
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l) as { seq: string | number; canonical?: Hex; leaf?: Hex })
    .sort((a, b) => Number(a.seq) - Number(b.seq));
  return { root: merkleRoot(events.map((e) => e.leaf ?? leaf(e.canonical ?? "0x"))), count: events.length };
}

export type AnchoredRecord = {
  root: Hex;
  agent: Hex;
  anchor: Hex;
  anchorTx?: Hex | null;
  at: number;
  events: number;
};

export type NodeAnswer = { host: string; ok: boolean; how: string };

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** What one node says about `root`: the agent's latest root, or an earlier one carried by its anchoring
 *  transaction. The agent must be the one the account's own policy names on chain, and the record contract the
 *  pinned one: otherwise a service could anchor a made-up record from a key of its own (review 2026-10-15). */
export async function askNode(url: string, r: AnchoredRecord, root: Hex, policy: Hex, anchor: Hex): Promise<NodeAnswer> {
  const host = new URL(url).host;
  const client = createPublicClient({ transport: http(url, { timeout: 15_000, retryCount: 1 }) });
  try {
    if (!isAddressEqual(r.anchor, anchor)) return { host, ok: false, how: "the record names a different record contract than this chain's" };
    // The writer must be the key the account itself names: its agent, or — on a keyless account, whose
    // agent is the dead address — its operator (the service's key), and the verdict says which.
    let writer = await client.readContract({ address: policy, abi: POLICY_ABI, functionName: "agent" });
    let who = "the account's agent";
    if (isAddressEqual(writer, NO_AGENT)) {
      writer = await client.readContract({ address: policy, abi: POLICY_ABI, functionName: "operator" });
      who = "the operator key this account names (it has no agent key)";
    }
    if (!isAddressEqual(writer, r.agent)) return { host, ok: false, how: "it was written by a key this account does not name on chain" };
    const agent = writer;
    const latest = await client.readContract({ address: anchor, abi: ANCHOR_ABI, functionName: "rootOf", args: [agent] });
    if (same(latest, root)) return { host, ok: true, how: `the latest record ${who} put on chain` };
    if (!r.anchorTx) return { host, ok: false, how: "a later record is on chain and the service named no transaction for this one" };
    const [tx, receipt] = await Promise.all([
      client.getTransaction({ hash: r.anchorTx }),
      client.getTransactionReceipt({ hash: r.anchorTx }),
    ]);
    let carried: Hex | undefined;
    try {
      const call = decodeFunctionData({ abi: ANCHOR_ABI, data: tx.input });
      if (call.functionName === "anchor") carried = call.args[0];
    } catch {
      carried = undefined;
    }
    const ok =
      receipt.status === "success" &&
      !!tx.to &&
      isAddressEqual(tx.to, anchor) &&
      isAddressEqual(tx.from, agent) &&
      !!carried &&
      same(carried, root);
    return {
      host,
      ok,
      how: ok
        ? `put on chain by ${who}, in a transaction that succeeded`
        : `what ${who} put on chain in that transaction is something else`,
    };
  } catch (e) {
    return { host, ok: false, how: (e as Error).message.split("\n")[0] };
  }
}

export type CheckResult = { recomputed: Hex; count: number; matchesClaim: boolean; nodes: NodeAnswer[]; ok: boolean };

/** The whole check: the root rebuilt here equals the record's name, and both nodes hold it under the agent
 *  that `policy` (the account) names on `chainKey` (the account's chain, not the service's word for it). */
export async function checkRecord(r: AnchoredRecord, jsonl: string, chainKey: string, policy: Hex): Promise<CheckResult> {
  const { root, count } = rootOfEvents(jsonl);
  const matchesClaim = same(root, r.root);
  const urls = NODES[chainKey] ?? [];
  const anchor = ANCHORS[chainKey];
  const nodes = anchor ? await Promise.all(urls.map((u) => askNode(u, r, root, policy, anchor))) : [];
  return { recomputed: root, count, matchesClaim, nodes, ok: matchesClaim && nodes.length === 2 && nodes.every((n) => n.ok) };
}
