// Circle CCTP V2 on the three testnets this app moves USDC between. Every shape below is from Circle's own pages, read
// 2026-10-18 06:2x +08 (the H5 report quotes them):
//   developers.circle.com/cctp/references/contract-addresses: TokenMessengerV2 and MessageTransmitterV2 have one address on
//     every EVM testnet; domains: Arbitrum Sepolia 3, Base Sepolia 6, Arc Testnet 26.
//   developers.circle.com/cctp/references/contract-interfaces: depositForBurn(amount, destinationDomain, mintRecipient,
//     burnToken, destinationCaller, maxFee, minFinalityThreshold); destinationCaller bytes32(0) lets any address call
//     receiveMessage(message, attestation); 1000 is a Fast Transfer, 2000 a Standard one.
//   developers.circle.com/cctp/concepts/fees: Standard Transfers are free (0 bps), so maxFee is 0.
//   developers.circle.com/cctp/concepts/finality-and-block-confirmations: a Standard attestation takes ~15-19 minutes from
//     Base and Arbitrum (~65 Ethereum blocks), ~0.5 s from Arc.
//   developers.circle.com/api-reference/cctp/all/get-messages-v2: GET {iris}/v2/messages/{sourceDomainId}?transactionHash=
//     → { messages: [{ message ("0x" until signed), attestation ("PENDING" until signed), status: "complete" |
//     "pending_confirmations", delayReason }] }; 404 until Circle has seen the burn.
//   github.com/circlefin/evm-cctp-contracts src/messages/v2/MessageV2.sol and BurnMessageV2.sol: the nonce is bytes32 at
//     byte 12 of the message, the body starts at 148; in the body mintRecipient sits at 36, amount at 68, feeExecuted at
//     164. src/v2/BaseMessageTransmitter.sol: usedNonces(bytes32) is 1 once the message was received (the mint happened).
import { encodeFunctionData, getAddress, hexToBigInt, hexToNumber, pad, parseAbi, sliceHex, type Address, type Chain, type Hex } from "viem";
import { arbitrumSepolia, arcTestnet, baseSepolia } from "viem/chains";

export const TOKEN_MESSENGER: Address = "0x8FE6B999Dc680CcFDD5Bf7EB0974218be2542DAA";
export const MESSAGE_TRANSMITTER: Address = "0xE737e5cEBEEBa77EFE34D4aa090756590b1CE275";
export const IRIS = "https://iris-api-sandbox.circle.com";
/** A Standard Transfer: attested at hard finality, no fee. */
export const STANDARD = 2000;
const ANY_CALLER: Hex = `0x${"0".repeat(64)}`;

export const abi = parseAbi([
  "function approve(address spender, uint256 amount) returns (bool)",
  "function allowance(address owner, address spender) view returns (uint256)",
  "function balanceOf(address account) view returns (uint256)",
  "function depositForBurn(uint256 amount, uint32 destinationDomain, bytes32 mintRecipient, address burnToken, bytes32 destinationCaller, uint256 maxFee, uint32 minFinalityThreshold)",
  "function receiveMessage(bytes message, bytes attestation) returns (bool)",
  "function usedNonces(bytes32 nonce) view returns (uint256)",
  "event Transfer(address indexed from, address indexed to, uint256 value)",
]);

export type Domain = 3 | 6 | 26;
/** One chain as this page names it: `key` is the service's chain key where it has one; `gas` is what the wallet pays fees in. */
export type CctpChain = { domain: Domain; key: string; chain: Chain; name: string; usdc: Address; gas: "ETH" | "USDC"; wait: string };

export const CHAINS: readonly CctpChain[] = [
  { domain: 3, key: "arbitrum-sepolia", chain: arbitrumSepolia, name: "Arbitrum Sepolia", usdc: "0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d", gas: "ETH", wait: "about 15 to 19 minutes" },
  { domain: 6, key: "base-sepolia", chain: baseSepolia, name: "Base Sepolia", usdc: "0x036CbD53842c5426634e7929541eC2318f3dCF7e", gas: "ETH", wait: "about 15 to 19 minutes" },
  { domain: 26, key: "arc-testnet", chain: arcTestnet, name: "Arc testnet", usdc: "0x3600000000000000000000000000000000000000", gas: "USDC", wait: "a few seconds" },
];

export const byKey = (key: string | undefined) => CHAINS.find((c) => c.key === key);
export const byDomain = (domain: number) => CHAINS.find((c) => c.domain === domain);
/** Where USDC can come from: every chain but the account's own. */
export const sourcesFor = (dest: CctpChain) => CHAINS.filter((c) => c.domain !== dest.domain);

/** An address as CCTP's bytes32: left-padded with zeros. */
export const toBytes32 = (a: string): Hex => pad(getAddress(a), { size: 32 }).toLowerCase() as Hex;

export const approveData = (amount: bigint) => encodeFunctionData({ abi, functionName: "approve", args: [TOKEN_MESSENGER, amount] });

/** Burn `amount` of the source chain's USDC for `recipient` on `dest`: Standard Transfer, no fee, anyone may mint it. */
export const burnData = (from: CctpChain, dest: CctpChain, recipient: string, amount: bigint) =>
  encodeFunctionData({
    abi,
    functionName: "depositForBurn",
    args: [amount, dest.domain, toBytes32(recipient), from.usdc, ANY_CALLER, 0n, STANDARD],
  });

export const mintData = (message: Hex, attestation: Hex) => encodeFunctionData({ abi, functionName: "receiveMessage", args: [message, attestation] });

/** The fields of a signed V2 burn message this page checks before it shows a move as the account's. */
export type BurnMessage = { source: number; destination: number; nonce: Hex; recipient: Address; amount: bigint; fee: bigint };

const BODY = 148;
export function readMessage(message: Hex): BurnMessage | null {
  // header 148 bytes + the burn body's fixed 228 bytes
  if (!/^0x[0-9a-fA-F]*$/.test(message) || (message.length - 2) / 2 < BODY + 228) return null;
  const at = (start: number, len: number) => sliceHex(message, start, start + len);
  return {
    source: hexToNumber(at(4, 4)),
    destination: hexToNumber(at(8, 4)),
    nonce: at(12, 32),
    recipient: getAddress(sliceHex(at(BODY + 36, 32), 12)),
    amount: hexToBigInt(at(BODY + 68, 32)),
    fee: hexToBigInt(at(BODY + 164, 32)),
  };
}

/** What Circle says about one burn, as the route hands it to the page: not seen yet, seen and waiting, signed (ready to
 *  mint; `mintTx` is Circle's `destinationMintTxHash` once someone minted it, a field its answers carry beyond the API page),
 *  or an answer this page cannot read. The page decodes `message` itself with `readMessage`. */
export type Attestation =
  | { state: "unseen" }
  | { state: "waiting"; delay: string | null }
  | { state: "signed"; message: Hex; attestation: Hex; mintTx: Hex | null }
  | { state: "unreadable"; words: string };

/** Circle's answer to `GET /v2/messages/{domain}?transactionHash=` (the 200 body), normalised. One burn, one message. */
export function attestationOf(body: unknown): Attestation {
  const m = (body as { messages?: unknown[] } | null)?.messages?.[0] as
    | { message?: string; attestation?: string | null; status?: string; delayReason?: string | null; destinationMintTxHash?: string | null }
    | undefined;
  if (!m) return { state: "unseen" };
  if (m.status !== "complete" || !m.attestation || m.attestation === "PENDING" || !m.message || m.message === "0x")
    return { state: "waiting", delay: m.delayReason ?? null };
  if (!readMessage(m.message as Hex) || !/^0x[0-9a-fA-F]+$/.test(m.attestation)) return { state: "unreadable", words: "Circle's answer is not a CCTP V2 burn message." };
  const mintTx = /^0x[0-9a-fA-F]{64}$/.test(m.destinationMintTxHash ?? "") ? (m.destinationMintTxHash as Hex) : null;
  return { state: "signed", message: m.message as Hex, attestation: m.attestation as Hex, mintTx };
}

/** One move as this device keeps it (`tameion.home.move.<address>` in localStorage); amounts in USDC minor units. */
export type Move = { from: Domain; amount: string; burnTx: Hex; at: number; mintTx?: Hex };

/** A kept move read back; null unless every field has its shape (storage is the device's, not ours). */
export function parseMove(raw: string | null): Move | null {
  try {
    const m = JSON.parse(raw ?? "null") as Partial<Move> | null;
    if (!m || !byDomain(Number(m.from)) || !/^\d+$/.test(String(m.amount)) || !/^0x[0-9a-fA-F]{64}$/.test(String(m.burnTx)) || typeof m.at !== "number") return null;
    if (m.mintTx !== undefined && !/^0x[0-9a-fA-F]{64}$/.test(String(m.mintTx))) return null;
    return m as Move;
  } catch {
    return null;
  }
}
