// Names, not addresses (docs/81; D12 of docs/102; docs/105). A party is shown as `name@chain`, the shape of an
// ERC-7828 interoperable name; its address, the exact interoperable form and its checksum sit behind one tap. The
// checksum is eight characters a person can read over the phone to confirm a key: forty hex characters never are.
import { getAddress, keccak256, toBytes } from "viem";

/** A chain as a person reads it after the `@`. ERC-7828 resolves such labels under on.eth; the exact form a program
 *  takes is CAIP-350's `eip155:<chain id>` (`interopName`), which is also the label for a chain not listed here. */
const LABELS: Record<number, string> = {
  1: "ethereum",
  10: "optimism",
  8453: "base",
  42161: "arbitrum",
  421614: "arbitrum-sepolia",
  84532: "base-sepolia",
  11155111: "sepolia",
  11155420: "optimism-sepolia",
  5042002: "arc-testnet",
};
export const chainLabel = (chainId: number) => LABELS[chainId] ?? `eip155:${chainId}`;

/** A name as the service keeps it (`crates/operator/src/parties.rs` `clean_name`): NFKC, no invisible format characters
 *  (Cf: zero width spaces and joiners, direction marks), spaces collapsed — the same White_Space set as Rust's
 *  `split_whitespace`. A counterparty signs this string, so the signed bytes are the stored ones (docs/103 S2-2). */
export const cleanName = (s: string) =>
  s
    .normalize("NFKC")
    .replace(/\p{Cf}/gu, "")
    .split(/[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+/)
    .filter(Boolean)
    .join(" ");

/** `Supplier@arbitrum-sepolia`: what the desk shows for a party. */
export const nameAt = (name: string, chainId: number) => `${name}@${chainLabel(chainId)}`;

/** ERC-7828's checksum of `address` on `chainId`: the first 4 bytes of keccak256 over the ERC-7930 fields ChainType ‖
 *  ChainReferenceLength ‖ ChainReference ‖ AddressLength ‖ Address, in upper-case hex. For eip155 the ChainType is
 *  0x0000 and the reference is the chain id, big-endian, in the fewest bytes (CAIP-350). */
export function interopChecksum(address: string, chainId: number): string {
  const ref: number[] = [];
  for (let n = BigInt(chainId); n > 0n; n >>= 8n) ref.unshift(Number(n & 0xffn));
  const a = toBytes(getAddress(address));
  return keccak256(new Uint8Array([0, 0, ref.length, ...ref, a.length, ...a]))
    .slice(2, 10)
    .toUpperCase();
}

/** `0xFe89…D44b7@eip155:1#80B12379`: the exact form, for copying (ERC-7828, checksummed address). */
export const interopName = (address: string, chainId: number) => `${getAddress(address)}@eip155:${chainId}#${interopChecksum(address, chainId)}`;

/** The checksum in two groups of four, for reading aloud ("80B1 2379"). */
export function codeText(address: string, chainId: number) {
  const c = interopChecksum(address, chainId);
  return `${c.slice(0, 4)} ${c.slice(4)}`;
}

/** What a counterparty's key signs to give a desk its name and take up an invite (docs/105): EIP-712 `DeskJoin` on
 *  the desk's own domain — the one its steps use, version 2 with `verifyingContract` = the desk — so it cannot be shown
 *  to another trader. `crates/operator/src/desk_invite.rs` checks the same digest (the test vector is shared). */
export const deskJoin = (chainId: number, desk: string, name: string, role: string, invite: string, at: number) => ({
  domain: { name: "Tameion Desk", version: "2", chainId, verifyingContract: getAddress(desk) },
  types: {
    DeskJoin: [
      { name: "name", type: "string" },
      { name: "role", type: "string" },
      { name: "invite", type: "string" },
      { name: "at", type: "uint64" },
    ],
  },
  primaryType: "DeskJoin" as const,
  message: { name, role, invite, at: BigInt(at) },
});
