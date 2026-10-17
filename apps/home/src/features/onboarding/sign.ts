// What the owner's passkey signs in the stepper, decided in the page, never taken from the service (the console's rule,
// apps/console/src/lib/guard.ts). The one call this flow signs is the payee approval; it is decoded and held against
// the payee the button names, and the challenge is recomputed here from the contract's own formula before the passkey
// sees it. Pure (viem only), so `node --test` runs it.
import { decodeFunctionData, encodeAbiParameters, keccak256, parseAbi, parseAbiParameters, type Hex } from "viem";

/** The payee calls the owner's passkey may make (guard.ts `PAYEES`). `setPayeesIf` is left out on purpose: it binds
 *  terms this flow does not check, so it is refused, never signed. */
const PAYEE_CALLS = parseAbi(["function setPayee(address payee, bool attested)", "function setPayees(address[] payees, bool attested)"]);

/** Throws, in words, unless `data` approves exactly `payees` on `policy` (guard.ts `checkCall` with `attested: true`). */
export function checkPayeesCall(policy: string, target: string, data: string, payees: readonly string[]): void {
  let c: ReturnType<typeof decodeFunctionData<typeof PAYEE_CALLS>>;
  try {
    c = decodeFunctionData({ abi: PAYEE_CALLS, data: data as Hex });
  } catch {
    throw new Error("Not signing: this page cannot read the call it was handed.");
  }
  if (target.toLowerCase() !== policy.toLowerCase()) throw new Error("Not signing: the call goes to a contract that is not this account's.");
  const [named, attested] = c.functionName === "setPayee" ? [[c.args[0]], c.args[1]] : [[...c.args[0]], c.args[1]];
  if (attested !== true) throw new Error("Not signing: the call would stop the payee, not approve it.");
  const a = named.map((x) => x.toLowerCase()).sort();
  const b = payees.map((x) => x.toLowerCase()).sort();
  if (a.length !== b.length || a.some((x, i) => x !== b[i])) throw new Error("Not signing: the call approves a different payee than the one on this page.");
}

/** PasskeyPrincipal.challengeFor: keccak256(abi.encode(chainid, owner, n, target, value, keccak256(data))). */
export const challengeFor = (chainId: number, owner: string, nonce: string | number | bigint, target: string, data: string) =>
  keccak256(
    encodeAbiParameters(parseAbiParameters("uint256, address, uint256, address, uint256, bytes32"), [
      BigInt(chainId),
      owner as Hex,
      BigInt(nonce),
      target as Hex,
      0n,
      keccak256(data as Hex),
    ]),
  );

// ---- bytes (apps/console/src/lib/tameion.ts) --------------------------------------
export const hex = (buf: ArrayBuffer | Uint8Array) => "0x" + [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
export const fromHex = (h: string) => new Uint8Array((h.replace(/^0x/, "").match(/../g) ?? []).map((x) => parseInt(x, 16)));
export const b64u = (buf: ArrayBuffer | Uint8Array) =>
  btoa(String.fromCharCode(...new Uint8Array(buf)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
export const unb64u = (s: string) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));

const N = 0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551n;
/** A WebAuthn DER signature as the low-s (r, s) the contract checks. */
export function derToRS(der: ArrayBuffer | Uint8Array) {
  const b = new Uint8Array(der);
  let i = 2;
  const read = () => {
    if (b[i++] !== 0x02) throw new Error("bad DER");
    const len = b[i++];
    let v = 0n;
    for (let k = 0; k < len; k++) v = (v << 8n) | BigInt(b[i++]);
    return v;
  };
  const r = read();
  let s = read();
  if (s > N / 2n) s = N - s;
  const to32 = (n: bigint) => "0x" + n.toString(16).padStart(64, "0");
  return { r: to32(r), s: to32(s) };
}
