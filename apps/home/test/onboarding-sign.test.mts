// What the stepper's passkey signs (H4): the payee approval is held against the payee the button names, and the
// challenge is the contract's formula, byte for byte the describer the service serves (apps/classic/passkey/call.js).
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";
import { encodeFunctionData, parseAbi } from "viem";
import { challengeFor, checkPayeesCall, derToRS } from "../src/features/onboarding/sign.ts";

const policy = "0x907a9f4fe960017ee9c31550d2f1956e07e2df4f";
const vendor = "0xe5e2d8dcc2038fd0a010a6e7845378b646e33854";
const abi = parseAbi([
  "function setPayees(address[] payees, bool attested)",
  "function setPayeesIf(address[] payees, bool attested, bytes32 terms, uint64 deadline)",
]);
const approve = encodeFunctionData({ abi, functionName: "setPayees", args: [[vendor], true] });

test("the challenge is PasskeyPrincipal.challengeFor, the same bytes as the service's describer", () => {
  const call = createRequire(import.meta.url)("../../classic/passkey/call.js") as {
    challengeFor: (a: { chainId: number; owner: string; nonce: bigint; target: string; data: string }) => string;
  };
  const owner = "0xf31d2ab18b605eb85337154e0229d076dca33144";
  for (const nonce of [0n, 1n, 77n])
    assert.equal(challengeFor(421614, owner, nonce, policy, approve), call.challengeFor({ chainId: 421614, owner, nonce, target: policy, data: approve }));
});

test("only an approval of exactly the named payee, on this account, is signed", () => {
  assert.doesNotThrow(() => checkPayeesCall(policy, policy, approve, [vendor.toUpperCase().replace("0X", "0x")]));
  const other = encodeFunctionData({ abi, functionName: "setPayees", args: [["0x0000000000000000000000000000000000000bad"], true] });
  assert.throws(() => checkPayeesCall(policy, policy, other, [vendor]), /different payee/);
  const revoke = encodeFunctionData({ abi, functionName: "setPayees", args: [[vendor], false] });
  assert.throws(() => checkPayeesCall(policy, policy, revoke, [vendor]), /stop the payee/);
  assert.throws(() => checkPayeesCall(policy, vendor, approve, [vendor]), /not this account/);
  const bound = encodeFunctionData({ abi, functionName: "setPayeesIf", args: [[vendor], true, `0x${"11".repeat(32)}`, 1n] });
  assert.throws(() => checkPayeesCall(policy, policy, bound, [vendor]), /cannot read/);
});

test("a DER signature becomes the low-s (r, s) the contract checks", () => {
  const N = 0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551n;
  const high = N - 5n;
  const be = (n: bigint) => {
    const h = n.toString(16).padStart(64, "0");
    const b = Uint8Array.from(h.match(/../g)!.map((x) => parseInt(x, 16)));
    return b[0] & 0x80 ? Uint8Array.from([0, ...b]) : b;
  };
  const r = be(7n), s = be(high);
  const der = Uint8Array.from([0x30, 4 + r.length + s.length, 0x02, r.length, ...r, 0x02, s.length, ...s]);
  const out = derToRS(der);
  assert.equal(BigInt(out.r), 7n);
  assert.equal(BigInt(out.s), 5n);
});
