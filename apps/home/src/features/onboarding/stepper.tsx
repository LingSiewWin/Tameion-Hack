"use client";
// `/new` the stepper (H4): 1 name the account, 2 make the passkey, 3 the demo funds it on chain (a real progress line
// from the deploy's funding until the balance reads back), 4 one tap lets the agent pay the Demo Vendor bill, then the
// authority receipt. Request shapes are the console's (start.tsx `open`, judge.tsx `BillStep`); the device token stays
// on the server (actions.ts). Every trust decision is one tap; nothing else asks the person to do anything.
import { useRouter } from "next/navigation";
import { useCallback, useEffect, useEffectEvent, useRef, useState } from "react";
import type { Demo, Inbox } from "../../lib/types.ts";
import { openAccount, ownerPost, readInbox, type Opened } from "./actions.ts";
import { assertKey, createKey, keyWords, remember, savedPub, type Pub } from "./passkey.ts";
import { receiptOf, spanWords, usdc } from "./receipt.ts";
import { ReceiptView } from "./receipt-view.tsx";
import { challengeFor, checkPayeesCall } from "./sign.ts";
import { CTA, CTA_QUIET, FIELD, PLATE, Segments, useNow } from "./ui.tsx";

type Chain = { key: string; name: string; chainId: number; anchor: string | null };
type Cfg = { chains: Chain[] };
type Step = 0 | 1 | 2 | 3 | 4;
const STEPS = ["Name the account", "Your passkey", "Funded", "Allowance"] as const;
const isHandle = (s: string) => /^[a-z0-9][a-z0-9-]{1,30}[a-z0-9]$/.test(s);
/** A free-looking name, so the first tap needs no typing (console start.tsx `suggestName`). */
const suggestName = () => `acme-agents-${1000 + (crypto.getRandomValues(new Uint16Array(1))[0] % 9000)}`;
const num = (s?: string | null) => parseFloat((s ?? "").replace(" USDC", "")) || 0;

/** A POST through this app's /api proxy; a refusal keeps whether anything executed (the envelope's `executed`). */
class Refused extends Error {
  constructor(
    message: string,
    readonly executed: "yes" | "no" | "unknown",
  ) {
    super(message);
  }
}
async function post<T>(path: string, body: unknown, idempotencyKey?: string): Promise<T> {
  let r: Response;
  try {
    r = await fetch(path, {
      method: "POST",
      headers: { "content-type": "application/json", ...(idempotencyKey ? { "idempotency-key": idempotencyKey } : {}) },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Refused("The service did not answer.", "unknown");
  }
  const d = (await r.json().catch(() => null)) as { error?: string | { words?: string; executed?: "yes" | "no" | "unknown" } } | null;
  if (r.ok) return d as T;
  const e = d?.error;
  throw new Refused(typeof e === "string" ? e : (e?.words ?? r.statusText), typeof e === "object" && e?.executed ? e.executed : r.status === 502 ? "unknown" : "no");
}
const getJson = async <T,>(path: string): Promise<T> => {
  const r = await fetch(path, { cache: "no-store" });
  if (!r.ok) throw new Error(`HTTP ${r.status}`);
  return (await r.json()) as T;
};

export function Stepper() {
  const router = useRouter();
  const [step, setStep] = useState<Step>(0);
  const [cfg, setCfg] = useState<Cfg | null>(null);
  const [demo, setDemo] = useState<Demo | null>(null);
  const [loadError, setLoadError] = useState("");
  const [name, setName] = useState(suggestName);
  const [pub, setPub] = useState<Pub | null>(null);
  const [opened, setOpened] = useState<Opened | null>(null);
  const [inbox, setInbox] = useState<Inbox | null>(null);
  const [readAt, setReadAt] = useState(0);

  useEffect(() => {
    Promise.all([getJson<Cfg>("/api/level1/config"), getJson<Demo>("/api/level1/demo")]).then(
      ([c, d]) => {
        setCfg(c);
        setDemo(d);
      },
      () => setLoadError("The service did not answer, so no account can open yet. Try again in a moment."),
    );
  }, []);

  // The console's choice (start.tsx): Arbitrum Sepolia first, where the demo's USDC is a token the relayer can send.
  const anchored = cfg?.chains.filter((c) => c.anchor) ?? [];
  const chain = anchored.find((c) => c.key === "arbitrum-sepolia") ?? anchored[0] ?? null;

  if (loadError)
    return (
      <p role="alert" className="row text-(--red)">
        {loadError}
      </p>
    );
  if (!cfg || !demo || !chain) return <StepSkeleton />;
  if (!demo.enabled) return <p className="row">This host does not open demo accounts. The demo account on the first page still opens.</p>;

  return (
    <div className="flex flex-col gap-(--space-5)">
      {step !== 4 && <Segments labels={STEPS} at={step} label={`Step ${step + 1} of ${STEPS.length}: ${STEPS[step]}`} />}
      {step === 0 && <NameStep name={name} setName={setName} onDone={() => setStep(1)} />}
      {step === 1 && (
        <PasskeyStep
          onDone={(p) => {
            setPub(p);
            setStep(2);
          }}
        />
      )}
      {step === 2 && pub && (
        <FundStep
          name={name}
          pub={pub}
          chain={chain}
          onTaken={() => setStep(0)}
          onDone={(o, i) => {
            setOpened(o);
            setInbox(i);
            setStep(3);
          }}
        />
      )}
      {step === 3 && opened && inbox && pub && (
        <AllowStep
          opened={opened}
          inbox={inbox}
          pub={pub}
          chain={chain}
          demo={demo}
          onDone={(i) => {
            setInbox(i);
            setReadAt(Math.floor(Date.now() / 1000));
            setStep(4);
          }}
        />
      )}
      {step === 4 && opened && inbox && (
        <div className="flex flex-col gap-(--space-5)">
          <div className="flex flex-col gap-(--space-2)">
            <h1 className="statement-line">{opened.name} is open.</h1>
            <p className="row money">
              It holds {usdc(inbox.balance)} on {chain.name}.
            </p>
          </div>
          <ReceiptView receipt={receiptOf(inbox, readAt)} />
          <button type="button" className={CTA} onClick={() => router.push(`/a/${opened.name}`)}>
            Open the account
          </button>
        </div>
      )}
    </div>
  );
}

function StepSkeleton() {
  return (
    <div role="status" aria-busy="true" aria-label="Loading" className="flex flex-col gap-(--space-4)">
      <span aria-hidden className="skeleton block h-1 w-full" />
      <span aria-hidden className="skeleton block h-12 w-3/4" />
      <span aria-hidden className="skeleton block h-12 w-full" />
      <span aria-hidden className="skeleton block h-(--cta) w-full" />
    </div>
  );
}

function NameStep({ name, setName, onDone }: { name: string; setName: (n: string) => void; onDone: () => void }) {
  const [checked, setChecked] = useState<{ name: string; taken: boolean | null }>({ name: "", taken: null });
  const [tried, setTried] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  // Is the name free? Asked once typing pauses (GET /api/level1/name/<handle>: 404 is free).
  useEffect(() => {
    if (!isHandle(name)) return;
    let live = true;
    const t = setTimeout(async () => {
      const r = await fetch(`/api/level1/name/${encodeURIComponent(name)}`, { cache: "no-store" }).catch(() => null);
      if (live) setChecked({ name, taken: r ? (r.status === 404 ? false : r.ok ? true : null) : null });
    }, 350);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [name]);

  const status = !isHandle(name) ? "invalid" : checked.name !== name ? "checking" : checked.taken === null ? "unknown" : checked.taken ? "taken" : "free";
  const words = {
    invalid: "3 to 32 lowercase letters, numbers or dashes, not starting or ending with a dash.",
    checking: `Checking ${name}.`,
    unknown: "The name could not be checked just now; opening still tells you.",
    taken: `${name} is taken. Try another.`,
    free: `${name} is free.`,
  }[status];
  const go = () => {
    setTried(true);
    if (status === "invalid" || status === "taken") return input.current?.focus();
    onDone();
  };

  return (
    <form
      className="flex flex-col gap-(--space-5)"
      onSubmit={(e) => {
        e.preventDefault();
        go();
      }}
    >
      <h1 id="name-title" className="statement-line">
        Name the account
      </h1>
      <div className="flex flex-col gap-(--space-2)">
        <input
          ref={input}
          aria-labelledby="name-title"
          aria-describedby="name-status"
          aria-invalid={tried && (status === "invalid" || status === "taken")}
          value={name}
          onChange={(e) => setName(e.target.value.toLowerCase().replace(/\s+/g, "-"))}
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          maxLength={32}
          className={FIELD}
        />
        <p id="name-status" aria-live="polite" className={tried && (status === "invalid" || status === "taken") ? "row text-(--red)" : "meta"}>
          {words}
        </p>
      </div>
      <button type="submit" className={CTA}>
        Continue
      </button>
    </form>
  );
}

function PasskeyStep({ onDone }: { onDone: (p: Pub) => void }) {
  const [waiting, setWaiting] = useState(false);
  const [error, setError] = useState("");
  const prompt = useRef<AbortController | null>(null);
  const make = async () => {
    setError("");
    setWaiting(true);
    prompt.current = new AbortController();
    try {
      onDone(await createKey(prompt.current.signal));
    } catch (e) {
      setError(keyWords(e));
    } finally {
      prompt.current = null;
      setWaiting(false);
    }
  };
  return (
    <div className="flex flex-col gap-(--space-5)">
      <h1 className="statement-line">Your passkey</h1>
      {/* The brief's line said "the only key that can change the rules"; the operator key may also propose a raise
          (BudgetPolicy.setLimits is onlyPrincipalOrOperator), so the line names what only the passkey can do. */}
      <p className="row">Your face or fingerprint is the only key that can approve a payee or move the money. We never have it.</p>
      <button type="button" className={CTA} onClick={make} aria-busy={waiting}>
        {waiting ? "Waiting for your passkey" : savedPub() ? "Use my passkey" : "Make my passkey"}
      </button>
      {waiting && (
        <button type="button" className={CTA_QUIET} onClick={() => prompt.current?.abort()}>
          Cancel
        </button>
      )}
      {error && (
        <p role="alert" className="row text-(--red)">
          {error}
        </p>
      )}
    </div>
  );
}

type FundPhase = "opening" | "sent" | "arrived";
const FUND_STOPS = ["Opened", "Sent", "Arrived"] as const;

function FundStep({ name, pub, chain, onTaken, onDone }: { name: string; pub: Pub; chain: Chain; onTaken: () => void; onDone: (o: Opened, i: Inbox) => void }) {
  const [phase, setPhase] = useState<FundPhase>("opening");
  const [opened, setOpened] = useState<Opened | null>(null);
  const [error, setError] = useState<{ words: string; sent: boolean } | null>(null);
  const [startedAt, setStartedAt] = useState(0);
  const [attempt, setAttempt] = useState(0);
  const now = useNow(!error);
  // Open once per attempt: React runs effects twice in development, and the open must not be asked twice at once.
  const started = useRef(-1);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const run = useEffectEvent(async () => {
    setStartedAt(Math.floor(Date.now() / 1000));
    setError(null);
    setPhase("opening");
    const o = await openAccount({ name, qx: pub.qx, qy: pub.qy, chain: chain.key });
    if (!mounted.current) return;
    if ("error" in o) {
      if (o.taken) return onTaken();
      return setError({ words: o.error, sent: false });
    }
    setOpened(o);
    remember({ policy: o.policy, owner: o.owner, chain: o.chain, name: o.name, qx: pub.qx, qy: pub.qy });
    if (!o.funding.sent) return setError({ words: o.funding.error ?? "The demo did not fund the account.", sent: false });
    setPhase("sent");
    // The funding is confirmed when the deploy answers; this waits until the account's own balance reads it back.
    for (let i = 0; i < 60 && mounted.current; i++) {
      const r = await readInbox(o.policy);
      if ("inbox" in r && num((r.inbox as Inbox).balance) > 0) {
        setPhase("arrived");
        return onDone(o, r.inbox as Inbox);
      }
      await new Promise((ok) => setTimeout(ok, 2000));
    }
    if (mounted.current) setError({ words: "The funding was sent, but the balance has not read back after two minutes.", sent: true });
  });
  useEffect(() => {
    if (started.current === attempt) return;
    started.current = attempt;
    void run();
  }, [attempt]);

  // Filled only once true: "Sent" means the deploy answered with the funding confirmed, which also means "Opened".
  const at: number = phase === "opening" ? -1 : phase === "sent" ? 1 : 2;
  const amount = opened?.funding.usdc ? usdc(opened.funding.usdc) : null;
  const sentence = {
    opening: `Opening ${name} on ${chain.name}.`,
    sent: `Sent ${amount ?? "the demo's USDC"} to the account; reading the balance back.`,
    arrived: `The account holds ${amount ?? "the demo's USDC"}.`,
  }[phase];

  return (
    <div className="flex flex-col gap-(--space-5)">
      <h1 className="statement-line">Funded on {chain.name}</h1>
      <div className={`${PLATE} flex flex-col gap-(--space-3)`}>
        <Segments labels={FUND_STOPS} at={at} label={at < 0 ? "Funding: opening" : `Funding: ${FUND_STOPS[at] ?? ""}`} show />
        <p className="row money" aria-live="polite">
          {sentence}
        </p>
        {!error && startedAt > 0 && <p className="meta money">{Math.max(0, now - startedAt)} s</p>}
      </div>
      {error && (
        <div role="alert" className={`${PLATE} flex flex-col gap-(--space-3)`}>
          <p className="row-title">{error.sent ? "Sent; the balance is still being read." : "Nothing was sent."}</p>
          <p className="row">{error.words}</p>
          <button type="button" className={CTA} onClick={() => setAttempt((a) => a + 1)}>
            Try again
          </button>
        </div>
      )}
    </div>
  );
}

type AllowPhase = "idle" | "Signing" | "Sent" | "Confirmed";

function AllowStep({ opened, inbox, pub, chain, demo, onDone }: { opened: Opened; inbox: Inbox; pub: Pub; chain: Chain; demo: Demo; onDone: (i: Inbox) => void }) {
  const [phase, setPhase] = useState<AllowPhase>("idle");
  const [failure, setFailure] = useState<{ title: string; words: string } | null>(null);
  const [tappedAt, setTappedAt] = useState(0);
  const now = useNow(phase !== "idle");
  const vendor = demo.vendor.toLowerCase();
  const every = spanWords(demo.bill.everySecs).replace(/^1 /, "");
  const label = `Let the agent pay Demo Vendor ${usdc(demo.bill.usdc)} every ${every}`;

  const tap = useCallback(async () => {
    if (phase !== "idle") return;
    setFailure(null);
    setPhase("Signing");
    setTappedAt(Math.floor(Date.now() / 1000));
    const policy = opened.policy;
    let approved = (inbox.payees ?? []).some((p) => p.address.toLowerCase() === vendor);
    try {
      // Ask first whether this device may save bills here, so a refusal costs no passkey tap (docs/60 P-225).
      const check = await ownerPost(policy, "bills", { check: true });
      if ("error" in check) throw new Refused(check.error, "no");
      if (!approved) {
        // The contract pays only payees it approved: the one passkey tap approves Demo Vendor.
        const prep = await ownerPost(policy, "prepare-payees", { payees: [vendor] });
        if ("error" in prep) throw new Refused(prep.error, "no");
        const { to, data } = prep.body as { to: string; data: string };
        checkPayeesCall(policy, to, data, [vendor]);
        const ch = await post<{ nonce: string; challenge: string }>("/api/level1/challenge", { chain: opened.chain, owner: opened.owner, target: to, data });
        if (challengeFor(chain.chainId, opened.owner, ch.nonce, to, data).toLowerCase() !== ch.challenge.toLowerCase())
          throw new Refused("Not signing: the service's challenge differs from the one this page computed.", "no");
        const auth = await assertKey(ch.challenge, pub);
        setPhase("Sent");
        const keyIndex = Math.max(0, (inbox.owners?.keys ?? []).findIndex((k) => k.qx.toLowerCase() === pub.qx.toLowerCase()));
        const body = { chain: opened.chain, owner: opened.owner, target: to, data, auth, keyIndex };
        // One id per tap: a lost answer is asked again under the same id, and the service hands back the reply it kept.
        const id = crypto.randomUUID();
        const ex = await post<{ pending?: boolean }>("/api/level1/execute", body, id).catch(async (e: unknown) => {
          if (!(e instanceof Refused) || e.executed !== "unknown") throw e;
          await new Promise((ok) => setTimeout(ok, 2000));
          return post<{ pending?: boolean }>("/api/level1/execute", body, id);
        });
        if (ex.pending) throw new Refused("Approved by you; another owner key must approve too, on its own device.", "no");
        approved = true;
      }
      // First due one cycle from now; `everyDays: 0` so the signed bytes never rest on the service's default.
      if (inbox.agentTokens?.length) throw new Refused("This account has agent keys, so its bills are saved from the account page with the passkey.", "no");
      const bill = { payee: vendor, amount: demo.bill.usdc, everyDays: 0, everySecs: demo.bill.everySecs, nextDue: Math.floor(Date.now() / 1000) + demo.bill.everySecs };
      const saved = await ownerPost(policy, "bills", { bills: [bill] });
      if ("error" in saved) throw new Refused(saved.error, saved.executed);
      const refused = (saved.body as { refused?: { why: string }[] }).refused;
      if (refused?.length) throw new Refused(refused[0].why, "no");
      await ownerPost(policy, "contacts", { address: vendor, name: "Demo Vendor" });
      setPhase("Confirmed");
      // Read back until the account lists the payee (the receipt is built from what the chain says).
      for (let i = 0; i < 30; i++) {
        const r = await readInbox(policy);
        const next = "inbox" in r ? (r.inbox as Inbox) : null;
        if (next?.payees?.some((p) => p.address.toLowerCase() === vendor)) return onDone(next);
        await new Promise((ok) => setTimeout(ok, 2000));
      }
      throw new Refused("Demo Vendor is approved and the bill is saved, but the account has not read them back yet.", "yes");
    } catch (e) {
      const words = e instanceof Error ? (e.name === "NotAllowedError" || e.name === "AbortError" ? keyWords(e) : e.message) : String(e);
      const lost = e instanceof Refused && e.executed === "unknown";
      setFailure({
        title: lost ? "Still checking: the approval may have gone through. Nothing was paid." : approved ? "Demo Vendor is approved; nothing was paid." : "Nothing was sent.",
        words,
      });
      setPhase("idle");
    }
  }, [phase, opened, inbox, pub, chain, demo, vendor, onDone]);

  return (
    <div className="flex flex-col gap-(--space-5)">
      <h1 className="statement-line">Allowance</h1>
      <button type="button" className={CTA} onClick={tap} aria-busy={phase !== "idle"} aria-live="polite">
        {phase === "idle" ? label : phase}
      </button>
      {/* The service can take a while (a check of the account may be running); the seconds say it is still going. */}
      {phase !== "idle" && tappedAt > 0 && <p className="meta money">{Math.max(0, now - tappedAt)} s</p>}
      {failure && (
        <div role="alert" className={`${PLATE} flex flex-col gap-(--space-2)`}>
          <p className="row-title">{failure.title}</p>
          <p className="row">{failure.words}</p>
        </div>
      )}
    </div>
  );
}
