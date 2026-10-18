// The owner's passkey in this browser (apps/console/src/lib/tameion.ts `createKey`, `passkeyAssert`, `remember`; the
// same WebAuthn options and the same localStorage keys, so the Owner home finds the key this stepper made).
import { b64u, derToRS, fromHex, hex, unb64u } from "./sign.ts";

export type Pub = { qx: string; qy: string; credentialId: string };
export type Auth = { r: string; s: string; challengeIndex: number; typeIndex: number; authenticatorData: string; clientDataJSON: string };

const PUB = "tameion.pub";
const store = {
  get: (k: string) => {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  },
  set: (k: string, v: string) => {
    try {
      localStorage.setItem(k, v);
    } catch {
      // storage blocked (a private window): the passkey still works for this visit
    }
  },
};

export function savedPub(): Pub | null {
  try {
    return JSON.parse(store.get(PUB) ?? "null") as Pub | null;
  } catch {
    return null;
  }
}

/** The account this browser opened, first in `tameion.tenants`, as the console keeps it. */
export function remember(t: { policy: string; owner: string; chain: string; name?: string; qx: string; qy: string }) {
  let all: { policy: string }[] = [];
  try {
    all = JSON.parse(store.get("tameion.tenants") ?? "[]");
  } catch {}
  store.set("tameion.tenants", JSON.stringify([t, ...all.filter((x) => x.policy !== t.policy)]));
  store.set("tameion.tenant", JSON.stringify(t));
}

/** Make the passkey (or return the one this browser already holds). A minute at most; `signal` cancels it. */
export async function createKey(signal?: AbortSignal): Promise<Pub> {
  const saved = savedPub();
  if (saved) return saved;
  const minute = AbortSignal.timeout(60_000);
  const cred = (await navigator.credentials.create({
    publicKey: {
      rp: { name: "Tami", id: location.hostname },
      user: { id: crypto.getRandomValues(new Uint8Array(16)), name: "treasurer", displayName: "Treasurer" },
      challenge: crypto.getRandomValues(new Uint8Array(32)),
      pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      authenticatorSelection: { userVerification: "required", residentKey: "preferred" },
      attestation: "none",
      timeout: 60_000,
    },
    signal: signal ? AbortSignal.any([signal, minute]) : minute,
  })) as PublicKeyCredential;
  const spki = new Uint8Array((cred.response as AuthenticatorAttestationResponse).getPublicKey() as ArrayBuffer);
  const point = spki.slice(spki.length - 65);
  if (point[0] !== 0x04) throw new Error("expected an uncompressed P-256 point");
  const pub = { qx: hex(point.slice(1, 33)), qy: hex(point.slice(33, 65)), credentialId: b64u(cred.rawId) };
  store.set(PUB, JSON.stringify(pub));
  return pub;
}

/** A passkey assertion over a 32-byte challenge, in the shape the contract and the service check. */
export async function assertKey(challengeHex: string, pub: Pub | null): Promise<Auth> {
  const assertion = (await navigator.credentials.get({
    publicKey: {
      challenge: fromHex(challengeHex),
      rpId: location.hostname,
      userVerification: "required",
      allowCredentials: pub?.credentialId ? [{ type: "public-key", id: unb64u(pub.credentialId) }] : [],
    },
  })) as PublicKeyCredential;
  const r = assertion.response as AuthenticatorAssertionResponse;
  const clientDataJSON = new TextDecoder().decode(r.clientDataJSON);
  const { r: rr, s } = derToRS(r.signature);
  return {
    r: rr,
    s,
    challengeIndex: clientDataJSON.indexOf('"challenge":"'),
    typeIndex: clientDataJSON.indexOf('"type":"webauthn.get"'),
    authenticatorData: hex(r.authenticatorData),
    clientDataJSON,
  };
}

/** The passkey prompt's end, in words (apps/console start.tsx `keyWords`): never a spinner. */
export function keyWords(e: unknown): string {
  const name = (e as Error)?.name;
  if (name === "AbortError") return "Cancelled. Nothing was made; tap again when you are ready.";
  if (name === "TimeoutError") return "No answer to the passkey prompt within a minute, so nothing was made. Tap again and answer it.";
  if (name === "NotAllowedError") return "The passkey prompt was closed, so no key was made. Tap again and answer it with your face, fingerprint or password manager.";
  if (name === "NotSupportedError" || name === "SecurityError") return `This browser cannot make a passkey on this page (${name}). Open it in Chrome or Safari.`;
  return (e as Error)?.message || String(e);
}
