"use client";

import { BadgeCheck, CheckCircle2, KeyRound, Languages, Loader2, PenLine, Smartphone, UserPlus } from "lucide-react";
import { useSearchParams } from "next/navigation";
import { Suspense, useEffect, useState } from "react";
import { TopBar } from "@/components/brand";
import { Page } from "@/components/layout/page";
import { CopyRow, KeyValue, Section } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cleanName, codeText, deskJoin, nameAt } from "@/lib/names";
import { eth, tail } from "@/lib/tameion";
import { bytesToHex, hexToBytes, type Hex } from "viem";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";

/** The words of this page, in English and Chinese (docs/97: the first firms are Chinese-owned and phone-first). */
const WORDS = {
  en: {
    kinds: {
      bought: "sold to the trader (a priced purchase)",
      shipped: "shipped (bill of lading, air waybill or courier receipt)",
      arrived: "received into stock or a vault",
      to_fabrication: "taken in for fabrication",
      consigned: "placed on consignment",
      returned: "returned into stock",
      sold: "bought from the trader (a priced sale)",
    } as Record<string, string>,
    title: "Confirm your step",
    intro:
      "A trader asked you to confirm a step on their book. You sign a message: no transaction, no fee, no money moves. Their desk records it as confirmed by you, and their lender can check your signature.",
    step: "The step",
    as: "You confirm as",
    what: "What happened",
    grams: "Grams of gold",
    cost: "Price, USDC",
    proceeds: "Proceeds, USDC",
    doc: "Your document number",
    deal: "Deal",
    desk: "The trader's desk",
    deskAccount: "Desk account",
    chain: "Chain id",
    confirms: (who: string, g: string, verb: string, price: string, doc: string, deal: string) =>
      `${who} confirms that ${g} g of gold ${verb}${price ? ` for ${price} USDC` : ""}, document ${doc || "…"}, deal ${deal || "…"}.`,
    signWallet: "Sign with my wallet",
    signPhone: "Sign with the key on this phone",
    orPhone: "No wallet? Sign with a key on this phone",
    makeKey: "Make a signing key on this phone",
    keyTitle: "Your signing key on this phone",
    keyNote: (trader: string) =>
      `It signs your steps for ${trader}. It stays in this browser: if you clear its data, ask ${trader} for a new invite and make a new key.`,
    yourKey: (t4: string, code: string) => `Your key ••${t4} · code ${code}`,
    locked: "Kept sealed behind this phone's passkey: its fingerprint, face or passcode opens it, and every signature asks.",
    unlocked: "This browser cannot seal the key behind a fingerprint, so it is kept in the browser as it is.",
    noWallet: "Open this page in a browser with your wallet, or sign with a key on this phone.",
    notApproved: (as: string, addr: string) =>
      `This key (••${addr.slice(-4).toUpperCase()}) is not the one the trader approved for ${as}. Ask them for an invite link: it sends them this key with your name, for one tap.`,
    backTitle: (trader: string) => `Send this back to ${trader}`,
    backNote: (trader: string) =>
      `Your signature is ready. Send this link to ${trader}; when they open it on their desk, the step is booked. It carries only the step you signed — nothing was sent anywhere yet.`,
    theLink: "The signed step, as a link",
    share: "Send it with WhatsApp or another app",
    recorded: "Recorded on the trader's desk",
    booked: "Booked",
    moved: "The trader's offset was brought in line with this step.",
    willMove: "The trader will bring the offset in line with this step.",
    forDesk: (desk: string, code: string) => `For the desk ${desk} · desk code ${code}.`,
    goesTo: (host: string) => `Your signature goes back to ${host}.`,
    newDesk: "This key has not signed for this desk before: ask the trader to read you their desk code and check it matches the one above before you sign.",
    checkedName: (n: string) => `${n} is that desk's name on this service.`,
    joinTitle: (trader: string) => `Sign for ${trader}'s desk`,
    joinCard: "Your name and your signing key",
    joinIntro: (trader: string, role: string) =>
      `${trader} asks you to confirm your steps on their book as their ${role}. This page makes a signing key on your phone and sends it to them with your name — no app, no fee, no money moves. They approve it with one tap.`,
    yourName: "Your name on their desk",
    joinMake: "Make my key and send it",
    joinSend: "Send my name and key",
    joinWallet: "Send with my wallet's key",
    joinSent: (trader: string, name: string, role: string) =>
      `Sent. ${trader} now sees "Approve ${name} as ${role}?" and approves it with one tap; the steps they then ask you to sign are booked as yours.`,
    codeNote: "If they call to check, read them this code: their screen shows the same.",
    joinBackNote: (trader: string) =>
      `Your answer is signed. Send this link to ${trader}; when they open it on their desk, they see your name and key to approve. Nothing was sent anywhere yet.`,
    badRole: "This invite names a role this page does not know: ask the trader for a new one.",
    roles: { Supplier: "Supplier", Carrier: "Carrier", Vault: "Vault", Workshop: "Workshop", Retailer: "Retailer", Buyer: "Buyer" } as Record<string, string>,
    inApp: "Open this link in Chrome or Safari: inside this app the key on your phone may not be kept.",
    refused: "The trader's desk refused this step: ",
    pickKind: "Choose what happened",
    other: "中文",
  },
  zh: {
    kinds: {
      bought: "卖给交易商(有价格的采购)",
      shipped: "已发货(提单、空运单或快递单)",
      arrived: "已入库或存入金库",
      to_fabrication: "已收进加工",
      consigned: "已寄售",
      returned: "已退回库存",
      sold: "从交易商买入(有价格的销售)",
    } as Record<string, string>,
    title: "确认您的这一步",
    intro: "一位交易商请您确认他账本上的一步。您签的是一条信息:不是交易,没有手续费,不动任何钱。他的账本会记为由您确认,他的贷款方可以核对您的签名。",
    step: "这一步",
    as: "您确认的身份",
    what: "发生了什么",
    grams: "黄金克数",
    cost: "价格(USDC)",
    proceeds: "货款(USDC)",
    doc: "您的单据号码",
    deal: "交易编号",
    desk: "交易商的账本",
    deskAccount: "账本账户",
    chain: "链 ID",
    confirms: (who: string, g: string, verb: string, price: string, doc: string, deal: string) =>
      `${who} 确认:${g} 克黄金${verb}${price ? `,金额 ${price} USDC` : ""},单据 ${doc || "…"},交易 ${deal || "…"}。`,
    signWallet: "用我的钱包签名",
    signPhone: "用这部手机上的密钥签名",
    orPhone: "没有钱包?用这部手机上的密钥签名",
    makeKey: "在这部手机上生成签名密钥",
    keyTitle: "您在这部手机上的签名密钥",
    keyNote: (trader: string) =>
      `它为${trader === "the trader" ? "交易商" : trader}签您的步骤。密钥只存在这个浏览器里:如果清除了浏览器数据,请向${trader === "the trader" ? "交易商" : trader}要一个新的邀请链接,再生成新密钥。`,
    yourKey: (t4: string, code: string) => `您的密钥 ••${t4} · 核对码 ${code}`,
    locked: "密钥加密保存在这部手机的通行密钥后面:用指纹、面容或手机密码打开,每次签名都会要求确认。",
    unlocked: "这个浏览器不能用指纹保护密钥,所以密钥直接保存在浏览器里。",
    noWallet: "请在装有钱包的浏览器里打开此页,或者用这部手机上的密钥签名。",
    notApproved: (as: string, addr: string) =>
      `这个密钥(••${addr.slice(-4).toUpperCase()})不是交易商为「${as}」登记的那个。请向他要一个邀请链接:它会把这个密钥连同您的名字发给他,他按一下就能批准。`,
    backTitle: (trader: string) => `把这个发回给${trader === "the trader" ? "交易商" : trader}`,
    backNote: (trader: string) =>
      `签名完成。把这个链接发给${trader === "the trader" ? "交易商" : trader};他在自己的账本上打开时,这一步就记上了。链接里只有您签的这一步——目前还没有发送到任何地方。`,
    theLink: "已签名的这一步(链接)",
    share: "用 WhatsApp 或其他应用发送",
    recorded: "已记入交易商的账本",
    booked: "已记录",
    moved: "交易商的对冲已按这一步调整。",
    willMove: "交易商会按这一步调整对冲。",
    forDesk: (desk: string, code: string) => `签给这个账本:${desk} · 账本码 ${code}。`,
    goesTo: (host: string) => `签名会发回到 ${host}。`,
    newDesk: "这个密钥从未给这个账本签过名:签名前请让交易商念一下他的账本码,核对和上面的一样。",
    checkedName: (n: string) => `${n} 是这个账本在本服务上的名字。`,
    joinTitle: (trader: string) => `为${trader === "the trader" ? "交易商" : trader}的账本签名`,
    joinCard: "您的名字和签名密钥",
    joinIntro: (trader: string, role: string) =>
      `${trader === "the trader" ? "交易商" : trader}请您以「${role}」的身份确认他账本上的步骤。这个页面会在您的手机上生成签名密钥,连同您的名字发给他——不用装应用,没有手续费,不动任何钱。他按一下就能批准。`,
    yourName: "您在他账本上的名字",
    joinMake: "生成密钥并发送",
    joinSend: "发送我的名字和密钥",
    joinWallet: "用我的钱包密钥发送",
    joinSent: (trader: string, name: string, role: string) =>
      `已发送。${trader === "the trader" ? "交易商" : trader}现在会看到"批准 ${name} 作为${role}?",按一下就能批准;之后他请您签的步骤,都会记在您名下。`,
    codeNote: "如果他打电话来核对,把这个码念给他听:他屏幕上显示的应该一样。",
    joinBackNote: (trader: string) =>
      `您的回复已签名。把这个链接发给${trader === "the trader" ? "交易商" : trader};他在自己的账本上打开,就能看到您的名字和密钥并批准。目前还没有发送到任何地方。`,
    badRole: "这个邀请里的身份本页不认识:请向交易商要一个新的邀请。",
    roles: { Supplier: "供应商", Carrier: "承运人", Vault: "金库", Workshop: "加工厂", Retailer: "零售商", Buyer: "买家" } as Record<string, string>,
    inApp: "请用 Chrome 或 Safari 打开这个链接:在这个应用里,手机上的密钥可能保存不住。",
    refused: "交易商的账本拒绝了这一步:",
    pickKind: "请选择发生了什么",
    other: "English",
  },
};
const PRICED: Record<string, "cost" | "proceeds"> = { bought: "cost", sold: "proceeds" };
// The phone key (docs/97: counterparties are WhatsApp-first, few hold a wallet). A secp256k1 key made in this browser signs the
// same EIP-712 step a wallet would, so the desk and every verifier treat it alike. Where the phone's passkeys offer the PRF
// extension (Chrome, Android, iOS 18), the key is kept encrypted under a secret only the phone's fingerprint or face releases,
// asked for on every signature (docs/100 X7); elsewhere it is kept in the browser as it is. The address is public and kept plain.
const PHONE_KEY = "tameion.sign.key";
const PHONE_SEALED = "tameion.sign.sealed";
const PHONE_DESKS = "tameion.sign.desks";
/** The name this phone took on each desk when it answered an invite: its later steps there are signed under it. */
const PHONE_NAMES = "tameion.sign.names";
function namesOnDesks(): Record<string, string> {
  try {
    const v = JSON.parse(localStorage.getItem(PHONE_NAMES) ?? "{}") as unknown;
    return v && typeof v === "object" ? (v as Record<string, string>) : {};
  } catch {
    return {};
  }
}
const PRF_SALT = new TextEncoder().encode("tameion.sign.key/v1");
type Sealed = { addr: string; cred: Hex; iv: Hex; ct: Hex };
type Phone = { addr: string; key?: Hex; sealed?: Sealed };
function readPhone(): Phone | null {
  try {
    const sealed = JSON.parse(localStorage.getItem(PHONE_SEALED) ?? "null") as Sealed | null;
    if (sealed?.addr && sealed.cred && sealed.iv && sealed.ct) return { addr: sealed.addr, sealed };
    const k = localStorage.getItem(PHONE_KEY);
    return k && /^0x[0-9a-f]{64}$/.test(k) ? { addr: privateKeyToAccount(k as Hex).address, key: k as Hex } : null;
  } catch {
    return null;
  }
}
const rnd = (n: number) => crypto.getRandomValues(new Uint8Array(n));
type PrfResults = { prf?: { enabled?: boolean; results?: { first?: ArrayBuffer } } };
/** The AES key the phone's passkey releases for this credential (one fingerprint or face). */
async function prfKey(cred: Uint8Array): Promise<CryptoKey> {
  const a = (await navigator.credentials.get({
    publicKey: {
      challenge: rnd(32),
      allowCredentials: [{ type: "public-key", id: cred as BufferSource }],
      userVerification: "required",
      timeout: 60_000,
      extensions: { prf: { eval: { first: PRF_SALT } } } as AuthenticationExtensionsClientInputs,
    },
  })) as PublicKeyCredential | null;
  const first = (a?.getClientExtensionResults() as PrfResults | undefined)?.prf?.results?.first;
  if (!first) throw new Error("prf");
  return crypto.subtle.importKey("raw", first, "AES-GCM", false, ["encrypt", "decrypt"]);
}
/** A new key sealed under a new passkey's PRF secret; null when this phone's passkeys do not offer PRF. */
async function sealedKey(): Promise<{ sealed: Sealed } | null> {
  if (typeof PublicKeyCredential === "undefined") return null;
  const created = (await navigator.credentials.create({
    publicKey: {
      rp: { name: "Tameion Desk" },
      user: { id: rnd(16), name: "signing key", displayName: "Tameion signing key" },
      challenge: rnd(32),
      pubKeyCredParams: [
        { type: "public-key", alg: -7 },
        { type: "public-key", alg: -257 },
      ],
      authenticatorSelection: { residentKey: "preferred", userVerification: "required" },
      timeout: 60_000,
      extensions: { prf: {} } as AuthenticationExtensionsClientInputs,
    },
  })) as PublicKeyCredential | null;
  if (!created || (created.getClientExtensionResults() as PrfResults).prf?.enabled !== true) return null;
  const cred = new Uint8Array(created.rawId);
  const aes = await prfKey(cred);
  const key = generatePrivateKey();
  const iv = rnd(12);
  const addr = privateKeyToAccount(key).address;
  // The address is the additional data: a blob whose shown address is changed does not open (docs/101 Y3).
  const ct = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: new TextEncoder().encode(addr) }, aes, hexToBytes(key) as BufferSource),
  );
  return { sealed: { addr, cred: bytesToHex(cred), iv: bytesToHex(iv), ct: bytesToHex(ct) } };
}
async function unseal(s: Sealed): Promise<Hex> {
  const aes = await prfKey(hexToBytes(s.cred));
  const pt = new Uint8Array(
    await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: hexToBytes(s.iv) as BufferSource, additionalData: new TextEncoder().encode(s.addr) },
      aes,
      hexToBytes(s.ct) as BufferSource,
    ),
  );
  const key = bytesToHex(pt);
  // The key that signs is the address shown (docs/101 Y3).
  if (privateKeyToAccount(key).address !== s.addr) throw new Error("sealed");
  return key;
}

/** JSON as base64url of its UTF-8 bytes: a Chinese name survives the trip (plain `btoa` throws on it). */
const packed = (v: unknown) => {
  let bin = "";
  for (const b of new TextEncoder().encode(JSON.stringify(v))) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};
const ROLE_NAMES = ["Supplier", "Carrier", "Vault", "Workshop", "Retailer", "Buyer"];
/** Seconds now, for what is signed (called from handlers only). */
const nowSecs = () => Math.floor(Date.now() / 1000);

const TYPES = {
  EIP712Domain: [
    { name: "name", type: "string" },
    { name: "version", type: "string" },
    { name: "chainId", type: "uint256" },
    { name: "verifyingContract", type: "address" },
  ],
  ShipmentEvent: [
    { name: "deal", type: "string" },
    { name: "kind", type: "string" },
    { name: "goods", type: "string" },
    { name: "quantity", type: "string" },
    { name: "usdc", type: "string" },
    { name: "counterparty", type: "string" },
    { name: "document", type: "string" },
    { name: "at", type: "uint64" },
  ],
};

/** `/sign#desk=<policy>&chain=<chainId>&deal=…&kind=…&qty=…&usdc=…&as=…&doc=…` (docs/83 W5; the query form still works):
 *  a supplier, a vault, a carrier or a buyer confirms its own step on a trader's desk
 *  with its own wallet, or with a key this page makes on their phone (docs/97). It signs a message (EIP-712), not a transaction: no gas, no
 *  money moves. The desk books it only if the trader approved this wallet under that name. */
export default function SignPage() {
  return (
    <Suspense fallback={<TopBar />}>
      <Sign />
    </Suspense>
  );
}

function Sign() {
  const query = useSearchParams();
  // Prefilled from the fragment, which never reaches a server or its logs (docs/96 V9); older links used the query.
  const [hash] = useState(() => new URLSearchParams(typeof location === "undefined" ? "" : location.hash.slice(1)));
  const q = { get: (k: string) => hash.get(k) ?? query.get(k) };
  const [f, setF] = useState({
    desk: q.get("desk") ?? "",
    chain: q.get("chain") ?? "421614",
    deal: q.get("deal") ?? "",
    kind: q.get("kind") ?? "arrived",
    qty: q.get("qty") ?? "",
    usdc: q.get("usdc") ?? "",
    as: q.get("as") ?? (typeof window === "undefined" ? "" : (namesOnDesks()[(q.get("desk") ?? "").trim().toLowerCase()] ?? "")),
    doc: q.get("doc") ?? "",
  });
  // Chinese when the link or the phone says so; one tap switches.
  const [lang, setLang] = useState<"en" | "zh">(() =>
    q.get("lang") === "zh" || (q.get("lang") !== "en" && typeof navigator !== "undefined" && navigator.language.startsWith("zh")) ? "zh" : "en",
  );
  const t = WORDS[lang];
  // Where the trader's desk runs, and its name: a signed step goes back there as a link when this page is a public copy
  // that cannot reach the desk itself (docs/95 P3).
  const back = q.get("back");
  const trader = q.get("from") || "the trader";
  // An invite (docs/105): `join=<role>&inv=<secret>`. This phone answers it with its own key and the name it chooses.
  const joinRole = q.get("join");
  const invite = q.get("inv") ?? "";
  const [joinName, setJoinName] = useState(() => q.get("join") ?? "");
  const [joined, setJoined] = useState<{ name: string; address: string } | null>(null);
  const deskOk = /^0x[0-9a-fA-F]{40}$/.test(f.desk.trim()) && !/^0x0{40}$/.test(f.desk.trim());
  const chainNum = Number(f.chain) || 0;
  // The desk by its name and its 8-character code (ERC-7828), never its 40 hex characters (docs/105).
  const deskName = q.get("from") || "desk";
  const deskWords = deskOk ? t.forDesk(nameAt(deskName, chainNum), codeText(f.desk.trim(), chainNum)) : "";
  // Another link opened in this tab changes only the fragment: start again from it (the form reads it once).
  useEffect(() => {
    const again = () => location.reload();
    window.addEventListener("hashchange", again);
    return () => window.removeEventListener("hashchange", again);
  }, []);
  // The trader's name, checked against this service's names (docs/81): ticked only when it names this very desk.
  const [checked, setChecked] = useState(false);
  useEffect(() => {
    const desk = f.desk.trim().toLowerCase();
    if (trader === "the trader" || !/^0x[0-9a-f]{40}$/.test(desk)) return;
    let live = true;
    fetch(`/api/level1/name/${encodeURIComponent(trader)}`)
      .then((r) => (r.ok ? r.json() : null))
      .then((n: { chains?: { chainId: number; account: string }[] } | null) => {
        if (live) setChecked(!!n?.chains?.some((c) => c.chainId === chainNum && c.account.toLowerCase() === desk));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [trader, f.desk, chainNum]);
  const [phone, setPhone] = useState<Phone | null>(() => readPhone());
  const phoneAddr = phone?.addr ?? null;
  const hasWallet = typeof window !== "undefined" && !!eth();
  const [returnLink, setReturnLink] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<{ booked: string; offset?: { readBack?: string | null } | null } | null>(null);
  const [error, setError] = useState("");
  const [note, setNote] = useState("");
  const price = PRICED[f.kind];
  const verb = t.kinds[f.kind];
  // Any change is a new step to sign; a signed one is not signed again by a second tap (docs/100 X4).
  const set = (key: keyof typeof f) => (e: React.ChangeEvent<HTMLInputElement | HTMLSelectElement>) => {
    setF({ ...f, [key]: e.target.value });
    setDone(null);
    setReturnLink("");
  };
  // The desks this phone key has signed for: a new one is said before signing (docs/100 X3).
  const [signedFor, setSignedFor] = useState<string[]>(() => {
    try {
      const v = JSON.parse(localStorage.getItem(PHONE_DESKS) ?? "[]") as unknown;
      return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
    } catch {
      return [];
    }
  });
  const newDesk = !!phone && !signedFor.includes(f.desk.trim().toLowerCase());
  const backHost = (() => {
    try {
      const u = back ? new URL(back) : null;
      return !u ? "" : u.protocol === "https:" || u.protocol === "http:" ? u.host : "?";
    } catch {
      return "?";
    }
  })();
  const inApp = typeof navigator !== "undefined" && /FBAN|FBAV|Instagram|Line\/|MicroMessenger|WhatsApp/i.test(navigator.userAgent);

  /** A new key on this phone, sealed behind its passkey where it can be; null when none was made (said in `error`). */
  async function makeKey(): Promise<Phone | null> {
    setError("");
    try {
      // Sealed behind the phone's fingerprint or face where its passkeys offer PRF; kept as it is elsewhere.
      let made: { sealed: Sealed } | null = null;
      try {
        made = await sealedKey();
      } catch (e) {
        if ((e as Error).name === "NotAllowedError") {
          setError(lang === "zh" ? "没有完成指纹/面容确认,密钥没有生成。" : "The fingerprint or face check was not completed; no key was made.");
          return null;
        }
        // A page at an IP address cannot hold a passkey (WebAuthn needs a host name), nor can some in-app browsers: say so
        // and keep the key in the browser rather than fail (docs/101 Y4).
        setNote(lang === "zh" ? `这个页面不能用指纹保护密钥(${(e as Error).name}),密钥直接保存在浏览器里。` : `This page cannot seal the key behind a fingerprint (${(e as Error).name}); it is kept in the browser.`);
      }
      let next: Phone;
      if (made) {
        localStorage.setItem(PHONE_SEALED, JSON.stringify(made.sealed));
        next = { addr: made.sealed.addr, sealed: made.sealed };
      } else {
        const k = generatePrivateKey();
        localStorage.setItem(PHONE_KEY, k);
        next = { addr: privateKeyToAccount(k).address, key: k };
      }
      setPhone(next);
      return next;
    } catch {
      setError(lang === "zh" ? "这个浏览器不允许保存密钥(无痕模式?)。请用普通窗口打开。" : "This browser will not keep a key (a private window?). Open the page in a normal window.");
      return null;
    }
  }

  /** A signing failure in the signer's words: a closed fingerprint prompt signs nothing, a lost passkey opens no key. */
  function failed(e: unknown) {
    const name = (e as Error).name;
    const msg = (e as Error).message;
    setError(
      name === "NotAllowedError"
        ? lang === "zh"
          ? "没有完成指纹/面容确认,没有签名。"
          : "The fingerprint or face check was not completed; nothing was signed."
        : name === "OperationError" || msg === "prf" || msg === "sealed"
          ? lang === "zh"
            ? "这个密钥打不开了(手机上的通行密钥可能已删除)。请重新生成密钥,并向交易商要一个新的邀请。"
            : "This key cannot be opened (the phone's passkey may have been deleted). Make a new key and ask the trader for a new invite."
          : msg,
    );
  }

  /** The invite's answer: this key, the chosen name and the role, signed for this desk (`DeskJoin`, docs/105). */
  async function join(withPhone: boolean) {
    setError("");
    const w = eth();
    if (!withPhone && !w) return setError(t.noWallet);
    setBusy(true);
    try {
      const key = withPhone ? (phone ?? (await makeKey())) : null;
      if (withPhone && !key) return;
      // The name as the desk will keep it (docs/103 S2-2): what is signed is what is stored.
      const name = cleanName(joinName);
      const at = nowSecs();
      const typed = deskJoin(chainNum, f.desk.trim(), name, joinRole ?? "", invite, at);
      let from: string;
      let signature: string;
      if (key) {
        const account = privateKeyToAccount(key.key ?? (await unseal(key.sealed!)));
        from = account.address;
        signature = await account.signTypedData(typed);
      } else {
        [from] = (await w!.request({ method: "eth_requestAccounts" })) as string[];
        const json = { types: { EIP712Domain: TYPES.EIP712Domain, ...typed.types }, primaryType: typed.primaryType, domain: typed.domain, message: { ...typed.message, at } };
        signature = (await w!.request({ method: "eth_signTypedData_v4", params: [from, JSON.stringify(json)] })) as string;
      }
      const answer = { policy: f.desk.trim(), invite, name, address: from, at, signature };
      if (back && back !== location.origin) {
        // A public copy cannot reach the desk: the signed answer goes back as a link the trader opens (like a step).
        if (backHost === "?") throw new Error(lang === "zh" ? "链接里的回传地址无效。" : "The link's return address is not valid.");
        setReturnLink(`${back}/desk#join=${packed(answer)}`);
      } else {
        const r = await fetch("/api/desk/party/propose", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(answer) });
        const body = (await r.json().catch(() => ({}))) as { error?: string };
        if (!r.ok) throw new Error(t.refused + (body.error ?? `the desk answered ${r.status}`));
      }
      setJoined({ name, address: from });
      if (key) remember();
      try {
        localStorage.setItem(PHONE_NAMES, JSON.stringify({ ...namesOnDesks(), [f.desk.trim().toLowerCase()]: name }));
      } catch {}
    } catch (e) {
      failed(e);
    } finally {
      setBusy(false);
    }
  }

  function remember() {
    const d = f.desk.trim().toLowerCase();
    if (signedFor.includes(d)) return;
    const next = [...signedFor, d];
    setSignedFor(next);
    try {
      localStorage.setItem(PHONE_DESKS, JSON.stringify(next));
    } catch {}
  }

  async function sign(withPhone: boolean) {
    setError("");
    setDone(null);
    const w = eth();
    if (!withPhone && !w) return setError(t.noWallet);
    setBusy(true);
    let from = "";
    try {
      const milestone = {
        deal: f.deal.trim(),
        kind: f.kind,
        goods: "gold",
        quantity: f.qty.trim(),
        ...(price ? { usdc: f.usdc.trim() } : {}),
        counterparty: f.as.trim(),
        document: f.doc.trim(),
        at: nowSecs(),
      };
      // Version 2: the step is bound to this desk's account, so it cannot be replayed on another trader's desk (docs/89 H3).
      const domain = { name: "Tameion Desk", version: "2", chainId: Number(f.chain), verifyingContract: f.desk.trim() as Hex };
      const message = { ...milestone, usdc: price ? (milestone.usdc ?? "") : "" };
      let signature: string;
      if (withPhone && phone) {
        const account = privateKeyToAccount(phone.key ?? (await unseal(phone.sealed!)));
        from = account.address;
        signature = await account.signTypedData({
          domain,
          types: { ShipmentEvent: TYPES.ShipmentEvent },
          primaryType: "ShipmentEvent",
          message: { ...message, at: BigInt(message.at) },
        });
      } else {
        [from] = (await w!.request({ method: "eth_requestAccounts" })) as string[];
        const typed = { types: TYPES, primaryType: "ShipmentEvent", domain, message };
        signature = (await w!.request({ method: "eth_signTypedData_v4", params: [from, JSON.stringify(typed)] })) as string;
      }
      if (back && back !== location.origin) {
        // The signature is the authority: the trader's desk books it when the link is opened there; nothing goes to a server here.
        if (backHost === "?") throw new Error(lang === "zh" ? "链接里的回传地址无效。" : "The link's return address is not valid.");
        setReturnLink(`${back}/desk#step=${packed({ policy: f.desk.trim(), milestone, signature })}`);
        if (withPhone) remember();
        return;
      }
      const r = await fetch("/api/desk/milestone", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ policy: f.desk.trim(), milestone, signature }),
      });
      const body = (await r.json()) as { booked?: string; error?: string; offset?: { readBack?: string | null } | null };
      if (!r.ok || !body.booked) {
        const why = body.error ?? `The desk answered ${r.status}`;
        // The desk names addresses; the person signing gets a sentence.
        const role = /^(.+) may sign (.+) on this desk, not (\w+)$/.exec(why);
        throw new Error(
          /signs as|not an approved counterparty/.test(why)
            ? t.notApproved(f.as.trim(), from)
            : role && lang === "zh"
              ? `「${role[1]}」只能签:${role[2].split(", ").map((k) => t.kinds[k] ?? k).join("、")};不能签:${t.kinds[role[3]] ?? role[3]}。`
              : t.refused + why,
        );
      }
      setDone({ booked: body.booked, offset: body.offset });
      if (withPhone) remember();
    } catch (e) {
      // A fingerprint or face check that was cancelled or timed out signs nothing.
      failed(e);
    } finally {
      setBusy(false);
    }
  }

  const ready = /^0x[0-9a-fA-F]{40}$/.test(f.desk.trim()) && !/^0x0{40}$/.test(f.desk.trim()) && backHost !== "?" && !!verb && f.deal.trim() && f.qty.trim() && f.as.trim() && f.doc.trim() && (!price || f.usdc.trim());

  return (
    <>
      <TopBar />
      <Page className="max-w-xl space-y-6 lg:pt-12">
        <div>
          <div className="flex items-start justify-between gap-3">
            <h1 className="flex items-center gap-2 text-headline font-semibold">
              <PenLine className="size-6 text-protect" aria-hidden /> {joinRole ? t.joinTitle(trader) : t.title}
            </h1>
            <Button variant="ghost" size="sm" onClick={() => setLang(lang === "en" ? "zh" : "en")} aria-label="Language">
              <Languages className="size-4" /> {t.other}
            </Button>
          </div>
          {!joinRole && <p className="mt-2 text-muted-foreground">{t.intro}</p>}
        </div>

        {joinRole ? (
          <Section title={t.joinCard} icon={UserPlus}>
            {!ROLE_NAMES.includes(joinRole) || !invite ? (
              <p className="text-sm text-destructive">{t.badRole}</p>
            ) : (
              <>
                <p className="text-sm text-muted-foreground">{t.joinIntro(trader, t.roles[joinRole] ?? joinRole)}</p>
                {deskOk && (
                  <div className="mt-3 space-y-1 rounded-md bg-muted px-3 py-2 text-sm">
                    <p>{deskWords}</p>
                    {checked && (
                      <p className="flex items-center gap-1 text-xs text-protect">
                        <BadgeCheck className="size-3.5" aria-hidden /> {t.checkedName(deskName)}
                      </p>
                    )}
                    {back && back !== (typeof location === "undefined" ? "" : location.origin) && <p className="text-xs">{t.goesTo(backHost)}</p>}
                  </div>
                )}
                <div className="mt-4 max-w-sm space-y-1.5">
                  <Label htmlFor="join-name">{t.yourName}</Label>
                  <Input id="join-name" value={joinName} onChange={(e) => setJoinName(e.target.value)} maxLength={40} disabled={!!joined || !!returnLink} />
                </div>
                {inApp && <p className="mt-3 text-sm font-medium text-attention">{t.inApp}</p>}
                <div className="mt-4 flex flex-wrap items-center gap-2">
                  <Button onClick={() => void join(true)} disabled={!deskOk || !joinName.trim() || busy || !!joined || !!returnLink}>
                    {busy ? <Loader2 className="size-4 animate-spin" /> : <Smartphone className="size-4" />} {phone ? t.joinSend : t.joinMake}
                  </Button>
                  {hasWallet && (
                    <Button variant="outline" onClick={() => void join(false)} disabled={!deskOk || !joinName.trim() || busy || !!joined || !!returnLink}>
                      {t.joinWallet}
                    </Button>
                  )}
                </div>
                {error && <p className="mt-3 text-sm text-destructive">{error}</p>}
                {note && <p className="mt-3 text-sm text-attention">{note}</p>}
              </>
            )}
          </Section>
        ) : (
        <Section title={t.step}>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field id="as" label={t.as} value={f.as} onChange={set("as")} placeholder="Vault" />
            <div className="space-y-1.5">
              <Label htmlFor="kind">{t.what}</Label>
              <select id="kind" value={verb ? f.kind : ""} onChange={set("kind")} className="h-10 w-full rounded-md border bg-background px-3 text-sm">
                {/* A kind the link names that this page does not know is not shown as another one (docs/100 X8). */}
                <option value="" disabled>
                  {t.pickKind}
                </option>
                {Object.entries(t.kinds).map(([key, v]) => (
                  <option key={key} value={key}>
                    {v}
                  </option>
                ))}
              </select>
            </div>
            <Field id="qty" label={t.grams} value={f.qty} onChange={set("qty")} placeholder="10.000" />
            {price && <Field id="usdc" label={price === "cost" ? t.cost : t.proceeds} value={f.usdc} onChange={set("usdc")} placeholder="1334.45" />}
            <Field id="doc" label={t.doc} value={f.doc} onChange={set("doc")} placeholder="VR-1042" />
            <Field id="deal" label={t.deal} value={f.deal} onChange={set("deal")} placeholder="GOLD-…" />
          </div>
          <details className="mt-3 text-xs text-muted-foreground">
            <summary className="cursor-pointer">{t.desk}</summary>
            <div className="mt-2 grid gap-2 sm:grid-cols-2">
              <Field id="desk" label={t.deskAccount} value={f.desk} onChange={set("desk")} placeholder="0x…" mono />
              <Field id="chain" label={t.chain} value={f.chain} onChange={set("chain")} placeholder="421614" mono />
            </div>
          </details>
          {verb && f.qty && f.as && (
            <div className="mt-4 space-y-1 rounded-md bg-muted px-3 py-2 text-sm">
              <p>{t.confirms(f.as, f.qty, verb.split(" (")[0].split("(")[0], price ? f.usdc : "", f.doc, f.deal)}</p>
              {/* Where the signature binds and where it goes, in sight before the tap (docs/100 X3). */}
              <p className="text-xs">{deskWords}</p>
              {checked && (
                <p className="flex items-center gap-1 text-xs text-protect">
                  <BadgeCheck className="size-3.5" aria-hidden /> {t.checkedName(deskName)}
                </p>
              )}
              {back && back !== (typeof location === "undefined" ? "" : location.origin) && <p className="text-xs">{t.goesTo(backHost)}</p>}
              {newDesk && <p className="text-xs font-medium text-attention">{t.newDesk}</p>}
            </div>
          )}
          {inApp && <p className="mt-3 text-sm font-medium text-attention">{t.inApp}</p>}
          <div className="mt-4 flex flex-wrap items-center gap-2">
            {hasWallet && (
              <Button onClick={() => void sign(false)} disabled={!ready || busy || !!done || !!returnLink}>
                {busy && <Loader2 className="size-4 animate-spin" />} {t.signWallet}
              </Button>
            )}
            {phone ? (
              <Button variant={hasWallet ? "outline" : "default"} onClick={() => void sign(true)} disabled={!ready || busy || !!done || !!returnLink}>
                {busy && !hasWallet && <Loader2 className="size-4 animate-spin" />} <Smartphone className="size-4" /> {hasWallet ? t.orPhone : t.signPhone}
              </Button>
            ) : (
              <Button
                variant={hasWallet ? "outline" : "default"}
                onClick={async () => {
                  setBusy(true);
                  await makeKey();
                  setBusy(false);
                }}
                disabled={busy}
              >
                <KeyRound className="size-4" /> {t.makeKey}
              </Button>
            )}
          </div>
          {error && <p className="mt-3 text-sm text-destructive">{error}</p>}
          {note && <p className="mt-3 text-sm text-attention">{note}</p>}
        </Section>
        )}

        {joined && !returnLink && (
          <Section title={lang === "zh" ? "已发送" : "Sent"} icon={CheckCircle2} tone="protect">
            <p className="text-sm">{t.joinSent(trader, joined.name, t.roles[joinRole ?? ""] ?? joinRole ?? "")}</p>
            <p className="mt-3 text-sm">
              {t.yourKey(tail(joined.address), "")}
              <span className="figures rounded-md bg-muted px-2 py-0.5 font-mono font-semibold tracking-wider">{codeText(joined.address, chainNum)}</span>
            </p>
            <p className="mt-1 text-caption text-muted-foreground">{t.codeNote}</p>
          </Section>
        )}

        {phoneAddr && (
          <Section title={t.keyTitle} icon={Smartphone}>
            <p className="text-sm text-muted-foreground">{t.keyNote(trader)}</p>
            <p className="mt-1 text-sm text-muted-foreground">{phone?.sealed ? t.locked : t.unlocked}</p>
            <div className="mt-3 grid gap-2">
              {/* The key by its last four and its code; the address itself only on copy (docs/105). */}
              <CopyRow value={phoneAddr} label="your key's address" shown={t.yourKey(tail(phoneAddr), deskOk || chainNum ? codeText(phoneAddr, chainNum) : "")} />
            </div>
          </Section>
        )}

        {returnLink && (
          <Section title={t.backTitle(trader)} icon={CheckCircle2} tone="protect">
            <p className="text-sm text-muted-foreground">{joinRole ? t.joinBackNote(trader) : t.backNote(trader)}</p>
            <div className="mt-3 grid gap-2">
              <CopyRow value={returnLink} label="the link" shown={t.theLink} />
              {/* The system share sheet hands the link to WhatsApp or mail directly; no web page sees it on the way (docs/96 V9). */}
              {typeof navigator !== "undefined" && "share" in navigator && (
                <Button
                  variant="outline"
                  size="sm"
                  className="w-fit"
                  onClick={() =>
                    void navigator
                      .share({ text: joinRole ? `${joinName.trim()} answers your invite: ${returnLink}` : `${f.as.trim()} signed ${f.kind.replace("_", " ")} on ${f.deal.trim()}: ${returnLink}` })
                      .catch(() => {})
                  }
                >
                  {t.share}
                </Button>
              )}
            </div>
          </Section>
        )}

        {done && (
          <Section title={t.recorded} icon={CheckCircle2} tone="protect">
            <KeyValue label={t.booked} value={done.booked.replace(/\(0x[0-9a-fA-F]{40}\)/, "").trim()} />
            {/* Only a read-back says the offset moved (docs/91 R16); a step that waits or fails is the trader's to finish. */}
            {done.offset ? <p className="mt-2 text-caption text-muted-foreground">{done.offset.readBack != null ? t.moved : t.willMove}</p> : null}
          </Section>
        )}
      </Page>
    </>
  );
}

function Field({ id, label, value, onChange, placeholder, mono }: { id: string; label: string; value: string; onChange: (e: React.ChangeEvent<HTMLInputElement>) => void; placeholder?: string; mono?: boolean }) {
  return (
    <div className="space-y-1.5">
      <Label htmlFor={id}>{label}</Label>
      <Input id={id} value={value} onChange={onChange} placeholder={placeholder} className={mono ? "font-mono text-xs" : undefined} />
    </div>
  );
}
