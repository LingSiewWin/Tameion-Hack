// The statement as a link (D14 of docs/102, docs/105). The lender's kit, byte for byte as the service signed it, is
// gzipped into the URL's fragment, which no server and no log sees; `/lender` unpacks it in the browser and checks it
// at once. Nothing is uploaded and nothing is hosted.

const toB64u = (b: Uint8Array) => {
  let s = "";
  for (const x of b) s += String.fromCharCode(x);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};
const fromB64u = (s: string) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));

/** A kit unpacks to far less than this; a link that unpacks to more is refused rather than read (a gzip bomb). */
const MAX_TEXT = 2_000_000;

/** `<base>/lender#s=<gzip, base64url>&from=<desk name>`: the link a trader sends. `from` is the trader's own word,
 *  shown as such; the kit inside is what the page checks. */
export async function lenderLink(kitText: string, base: string, from?: string): Promise<string> {
  const gz = await new Response(new Blob([kitText]).stream().pipeThrough(new CompressionStream("gzip"))).arrayBuffer();
  const q = new URLSearchParams({ s: toB64u(new Uint8Array(gz)), ...(from ? { from } : {}) });
  return `${base.replace(/\/+$/, "")}/lender#${q}`;
}

/** The kit text and the claimed desk name from a `/lender` fragment; null when the fragment carries no kit. */
export async function kitFromFragment(fragment: string): Promise<{ text: string; from: string | null } | null> {
  const q = new URLSearchParams(fragment.replace(/^#/, ""));
  const s = q.get("s");
  if (!s) return null;
  let bytes: Uint8Array;
  try {
    bytes = fromB64u(s);
  } catch {
    throw new Error("the link is cut or changed: ask the trader to send it again");
  }
  const reader = new Blob([bytes as BlobPart]).stream().pipeThrough(new DecompressionStream("gzip")).getReader();
  const parts: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > MAX_TEXT) throw new Error("the link holds far more than a statement; not opened");
      parts.push(value);
    }
  } catch (e) {
    await reader.cancel().catch(() => {});
    throw (e as Error).message.startsWith("the link") ? e : new Error("the link is cut or changed: ask the trader to send it again");
  }
  const all = new Uint8Array(size);
  let at = 0;
  for (const p of parts) {
    all.set(p, at);
    at += p.length;
  }
  return { text: new TextDecoder().decode(all), from: q.get("from") };
}

/** Hand `url` to the system share sheet (WhatsApp, Mail, AirDrop…), or copy it where there is none. "cancelled" when
 *  the person closed the sheet: nothing was sent and nothing is copied behind their back. */
export async function shareLink(url: string, title: string, text: string): Promise<"shared" | "copied" | "cancelled"> {
  if (typeof navigator.share === "function") {
    try {
      await navigator.share({ title, text, url });
      return "shared";
    } catch (e) {
      if ((e as Error).name === "AbortError") return "cancelled";
      // No share target here (a desktop without one, a headless browser): copy instead.
    }
  }
  await navigator.clipboard.writeText(url);
  return "copied";
}
