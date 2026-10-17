"use client";
// `/new` the stepper (H4): 1 name the account, 2 make the passkey, 3 the demo funds it on chain (a real progress line
// from the deploy's funding until the balance reads back), 4 one tap lets the agent pay the Demo Vendor bill, then the
// authority receipt. Request shapes are the console's (start.tsx `open`, judge.tsx `BillStep`); the device token stays
// on the server (actions.ts). Every trust decision is one tap; nothing else asks the person to do anything.
import { useRouter } from "next/navigation";
import { useCallback, useEffect, useEffectEvent, useRef, useState } from "react";
import type { Demo, Inbox } from "../../lib/types.ts";
import { openAccount, ownerPost, readInbox, type Opened } from "./actions.ts";
import * as copy from "./copy.ts";
import { assertKey, createKey, keyWords, remember, savedPub, type Pub } from "./passkey.ts";
import { receiptOf, spanWords, usdc } from "./receipt.ts";
import { ReceiptView } from "./receipt-view.tsx";
import { challengeFor, checkPayeesCall } from "./sign.ts";
import { CTA, CTA_QUIET, FIELD, Failure, PLATE, Segments, T, useNow, Waited } from "./ui.tsx";

type Chain = { key: string; name: string; chainId: number; anchor: string | null };
type Cfg = { chains: Chain[] };
type Step = 0 | 1 | 2 | 3 | 4;
const isHandle = (s: string) => /^[a-z0-9][a-z0-9-]{1,30}[a-z0-9]$/.test(s);
/** A free-looking name, so the first tap needs no typing (console start.tsx `suggestName`). */
const suggestName = () => `acme-agents-${1000 + (crypto.getRandomValues(new Uint16Array(1))[0] % 9000)}`;
const num = (s?: string | null) => parseFloat((s ?? "").replace(" USDC", "")) || 0;
const nowSecs = () => Math.floor(Date.now() / 1000);
const pause = (ms: number) => new Promise((ok) => setTimeout(ok, ms));

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
    throw new Refused(copy.server.down, "unknown");
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
  const [loadError, setLoadError] = useState(false);
  const [loads, setLoads] = useState(0);
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
        setLoadError(false);
      },
      () => setLoadError(true),
    );
  }, [loads]);

  // The console's choice (start.tsx): Arbitrum Sepolia first, where the demo's USDC is a token the relayer can send.
  const anchored = cfg?.chains.filter((c) => c.anchor) ?? [];
  const chain = anchored.find((c) => c.key === "arbitrum-sepolia") ?? anchored[0] ?? null;

  if (loadError)
    return (
      <div role="alert" className={`${PLATE} flex flex-col gap-(--space-3)`}>
        <p className={T.rowTitle}>{copy.load.down}</p>
        <button type="button" className={CTA} onClick={() => setLoads((n) => n + 1)}>
          {copy.load.retry}
        </button>
      </div>
    );
  if (!cfg || !demo || !chain) return <StepSkeleton />;
  if (!demo.enabled) return <p className={T.row}>{copy.load.demoOff}</p>;

  return (
    <div className="flex max-w-[560px] flex-col gap-(--space-5)">
      {step !== 4 && <Segments labels={copy.steps} done={step + 1} current={step} label={copy.stepOf(step, copy.steps.length, copy.steps[step])} />}
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
            setReadAt(nowSecs());
            setStep(4);
          }}
        />
      )}
      {step === 4 && opened && inbox && (
        <div className="flex flex-col gap-(--space-5)">
          <div className="flex flex-col gap-(--space-2)">
            <h1 className={T.title}>{copy.receipt.title(opened.name)}</h1>
            <p className={`${T.row} ${T.money}`}>{copy.receipt.holds(usdc(inbox.balance), chain.name)}</p>
          </div>
          <ReceiptView receipt={receiptOf(inbox, readAt)} />
          <button type="button" className={CTA} onClick={() => router.push(`/a/${opened.name}`)}>
            {copy.receipt.open}
          </button>
        </div>
      )}
    </div>
  );
}

function StepSkeleton() {
  return (
    <div role="status" aria-busy="true" aria-label={copy.load.loading} className="flex max-w-[560px] flex-col gap-(--space-4)">
      <span aria-hidden className="skeleton block h-1 w-full" />
      <span aria-hidden className="skeleton block h-8 w-3/4" />
      <span aria-hidden className="skeleton block h-(--button) w-full" />
      <span aria-hidden className="skeleton block h-(--button) w-full" />
    </div>
  );
}

function NameStep({ name, setName, onDone }: { name: string; setName: (n: string) => void; onDone: () => void }) {
  const [checked, setChecked] = useState<{ name: string; taken: boolean | null }>({ name: "", taken: null });
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
  const blocked = status === "invalid" || status === "taken";
  const words = { invalid: copy.name.invalid, checking: copy.name.checking(name), unknown: copy.name.unknown, taken: copy.name.taken(name), free: copy.name.free(name) }[status];

  return (
    <form
      className="flex flex-col gap-(--space-5)"
      onSubmit={(e) => {
        e.preventDefault();
        // A blocked action names its fix and does it: back to the name.
        if (blocked) return input.current?.select();
        onDone();
      }}
    >
      <h1 id="name-title" className={T.title}>
        {copy.name.title}
      </h1>
      <div className="flex flex-col gap-(--space-2)">
        <input
          ref={input}
          aria-labelledby="name-title"
          aria-describedby="name-status"
          aria-invalid={blocked}
          value={name}
          onChange={(e) => setName(e.target.value.toLowerCase().replace(/\s+/g, "-"))}
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          maxLength={32}
          className={FIELD}
        />
        <p id="name-status" aria-live="polite" className={blocked ? T.attention : T.meta}>
          {words}
        </p>
      </div>
      <button type="submit" className={CTA}>
        {blocked ? copy.name.fix : copy.name.go}
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
      <h1 className={T.title}>{copy.passkey.title}</h1>
      <p className={T.row}>{copy.passkey.primer}</p>
      <button type="button" className={CTA} onClick={make} aria-busy={waiting}>
        {waiting ? copy.passkey.waiting : savedPub() ? copy.passkey.use : copy.passkey.make}
      </button>
      {waiting && (
        <button type="button" className={CTA_QUIET} onClick={() => prompt.current?.abort()}>
          {copy.passkey.cancel}
        </button>
      )}
      {error && (
        <p role="alert" className={T.attention}>
          {error}
        </p>
      )}
    </div>
  );
}

type FundPhase = "opening" | "sent" | "arrived";

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
    setStartedAt(nowSecs());
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
    if (!o.funding.sent) return setError({ words: o.funding.error ?? copy.fund.notFunded, sent: false });
    setPhase("sent");
    // The funding is confirmed when the deploy answers; this waits until the account's own balance reads it back.
    for (let i = 0; i < 60 && mounted.current; i++) {
      const r = await readInbox(o.policy);
      if ("inbox" in r && num((r.inbox as Inbox).balance) > 0) {
        setPhase("arrived");
        return onDone(o, r.inbox as Inbox);
      }
      await pause(2000);
    }
    if (mounted.current) setError({ words: copy.fund.slow, sent: true });
  });
  useEffect(() => {
    if (started.current === attempt) return;
    started.current = attempt;
    void run();
  }, [attempt]);

  // Filled only once true: "Sent" means the deploy answered with the funding confirmed, which also means "Opened".
  const done = phase === "opening" ? 0 : phase === "sent" ? 2 : 3;
  const amount = opened?.funding.usdc ? usdc(opened.funding.usdc) : copy.fund.demoAmount;
  const sentence = { opening: copy.fund.opening(name, chain.name), sent: copy.fund.sent(amount), arrived: copy.fund.arrived(amount) }[phase];

  return (
    <div className="flex flex-col gap-(--space-5)">
      <h1 className={T.title}>{copy.fund.title(chain.name)}</h1>
      <div className={`${PLATE} flex flex-col gap-(--space-3)`}>
        <Segments labels={copy.fund.stops} done={done} current={done} transit={!error} label={copy.fund.label(copy.fund.stops[Math.min(done, 2)])} show />
        <p className={`${T.row} ${T.money}`} aria-live="polite">
          {sentence}
        </p>
        {!error && <Waited since={startedAt} now={now} />}
      </div>
      {error && (
        <Failure
          title={error.sent ? copy.fund.slowTitle : copy.fund.failedTitle}
          words={error.words}
          action={
            <button type="button" className={CTA} onClick={() => setAttempt((a) => a + 1)}>
              {copy.fund.retry}
            </button>
          }
        />
      )}
    </div>
  );
}

type AllowPhase = "idle" | keyof typeof copy.allow.phases;

function AllowStep({ opened, inbox, pub, chain, demo, onDone }: { opened: Opened; inbox: Inbox; pub: Pub; chain: Chain; demo: Demo; onDone: (i: Inbox) => void }) {
  const [phase, setPhase] = useState<AllowPhase>("idle");
  const [failure, setFailure] = useState<{ title: string; words: string } | null>(null);
  const [tappedAt, setTappedAt] = useState(0);
  const now = useNow(phase !== "idle");
  const vendor = demo.vendor.toLowerCase();
  const label = copy.allow.go(usdc(demo.bill.usdc), spanWords(demo.bill.everySecs).replace(/^1 /, ""));

  const tap = useCallback(async () => {
    if (phase !== "idle") return;
    setFailure(null);
    setPhase("Signing");
    setTappedAt(nowSecs());
    const policy = opened.policy;
    // Read the account again first: a tap after a failure must not approve a payee the first tap already approved.
    const fresh = await readInbox(policy);
    const payees = ("inbox" in fresh ? (fresh.inbox as Inbox) : inbox).payees ?? [];
    let approved = payees.some((p) => p.address.toLowerCase() === vendor);
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
        if (challengeFor(chain.chainId, opened.owner, ch.nonce, to, data).toLowerCase() !== ch.challenge.toLowerCase()) throw new Refused(copy.allow.challengeDiffers, "no");
        const auth = await assertKey(ch.challenge, pub);
        setPhase("Sent");
        const keyIndex = Math.max(0, (inbox.owners?.keys ?? []).findIndex((k) => k.qx.toLowerCase() === pub.qx.toLowerCase()));
        const body = { chain: opened.chain, owner: opened.owner, target: to, data, auth, keyIndex };
        // One id per tap: a lost answer is asked again under the same id, and the service hands back the reply it kept.
        const id = crypto.randomUUID();
        const ex = await post<{ pending?: boolean }>("/api/level1/execute", body, id).catch(async (e: unknown) => {
          if (!(e instanceof Refused) || e.executed !== "unknown") throw e;
          await pause(2000);
          return post<{ pending?: boolean }>("/api/level1/execute", body, id);
        });
        if (ex.pending) throw new Refused(copy.allow.moreOwners, "no");
        approved = true;
      }
      // First due one cycle from now; `everyDays: 0` so the signed bytes never rest on the service's default.
      if (inbox.agentTokens?.length) throw new Refused(copy.allow.agentKeys, "no");
      const bill = { payee: vendor, amount: demo.bill.usdc, everyDays: 0, everySecs: demo.bill.everySecs, nextDue: nowSecs() + demo.bill.everySecs };
      const saved = await ownerPost(policy, "bills", { bills: [bill] });
      if ("error" in saved) throw new Refused(saved.error, saved.executed);
      const refused = (saved.body as { refused?: { why: string }[] }).refused;
      if (refused?.length) throw new Refused(refused[0].why, "no");
      await ownerPost(policy, "contacts", { address: vendor, name: copy.allow.vendor });
      setPhase("Confirmed");
      // Read back until the account lists the payee (the receipt is built from what the chain says).
      for (let i = 0; i < 30; i++) {
        const r = await readInbox(policy);
        const next = "inbox" in r ? (r.inbox as Inbox) : null;
        if (next?.payees?.some((p) => p.address.toLowerCase() === vendor)) return onDone(next);
        await pause(2000);
      }
      throw new Refused(copy.allow.notReadBack, "yes");
    } catch (e) {
      const words = e instanceof Error ? (e.name === "NotAllowedError" || e.name === "AbortError" ? keyWords(e) : e.message) : String(e);
      const lost = e instanceof Refused && e.executed === "unknown";
      setFailure({ title: lost ? copy.allow.lostTitle : approved ? copy.allow.approvedTitle : copy.allow.failedTitle, words });
      setPhase("idle");
    }
  }, [phase, opened, inbox, pub, chain, demo, vendor, onDone]);

  return (
    <div className="flex flex-col gap-(--space-5)">
      <h1 className={T.title}>{copy.allow.title}</h1>
      <button type="button" className={CTA} onClick={tap} aria-busy={phase !== "idle"} aria-live="polite">
        {phase === "idle" ? label : copy.allow.phases[phase]}
      </button>
      {phase !== "idle" && <Waited since={tappedAt} now={now} />}
      {failure && <Failure title={failure.title} words={failure.words} />}
    </div>
  );
}
