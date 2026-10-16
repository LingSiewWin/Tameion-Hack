"use client";

import { CheckCircle2, ChevronRight, Landmark, Loader2, PhoneCall, XCircle } from "lucide-react";
import { useEffect, useEffectEvent, useState } from "react";
import { TopBar } from "@/components/brand";
import { Page } from "@/components/layout/page";
import { KeyValue, Section, Stat, Stats } from "@/components/layout/section";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { bookError, checkAge, checkAnchor, checkFunding, checkHistory, checkOffline, checkPrice, checkTogether, checkVenue, parseParties, type Check, type LenderKit } from "@/lib/desk-verify";
import { kitFromFragment } from "@/lib/links";
import { codeText, nameAt } from "@/lib/names";
import { noAddr, tail } from "@/lib/tameion";

const usd = (s?: string) => {
  const n = Number(s ?? 0);
  return Number.isFinite(n) ? n.toLocaleString("en-US", { maximumFractionDigits: 2, minimumFractionDigits: 2 }) : s;
};
const grams = (s?: string) => (s ?? "").replace(" g gold", " g");
const PINS_KEY = "tameion.lender.pins";
type Setup = { signer: string; venuePin: string; partiesText: string };
/** The set-ups saved in this browser, by desk account (lowercase). An older single set-up has no desk and is dropped. */
function savedSetups(): Record<string, Setup> {
  try {
    const v = JSON.parse(localStorage.getItem(PINS_KEY) ?? "{}") as Record<string, unknown>;
    return Object.fromEntries(Object.entries(v).filter(([k, x]) => /^0x[0-9a-f]{40}$/.test(k) && typeof x === "object" && x)) as Record<string, Setup>;
  } catch {
    return {};
  }
}
/** The set-up a statement itself names: what a lender saves once it confirmed the desk by phone (TOFU, docs/105). */
function setupOf(k: LenderKit): Setup {
  return {
    signer: k.signed.signer,
    venuePin: k.signed.statement.venueAccount ?? "",
    partiesText: (k.parties ?? []).map((p) => `${p.name}=${p.address}`).join("\n"),
  };
}
const STAGE: Record<string, string> = { bought: "not delivered", in_transit: "in transit", in_stock: "in stock", in_fabrication: "in fabrication", consigned: "consigned" };

/** `/lender` (docs/83 W7): a lender opens the link (or the file) a trader sent and checks it here,
 *  against two public nodes and Hyperliquid's public API — not against the desk.
 *  The same checks as `scripts/verify_desk.py`; nothing is sent anywhere but those.
 *  A link (`#s=…`, docs/105) is checked the moment it opens, verdict first; the first statement from a desk
 *  asks the lender to confirm the desk by phone, then pins what it names, in this browser (TOFU). */
export default function LenderPage() {
  const [text, setText] = useState("");
  const [signer, setSigner] = useState("");
  const [venuePin, setVenuePin] = useState("");
  const [deskPin, setDeskPin] = useState("");
  const [partiesText, setPartiesText] = useState("");
  const [previous, setPrevious] = useState<LenderKit | null>(null);
  const [others, setOthers] = useState<LenderKit[]>([]);
  // The lender's own floor for the maximum loss, in percent (docs/92 T3).
  const [stressPct, setStressPct] = useState("30");
  const [basisPct, setBasisPct] = useState("2");
  // The lender's own price and advance rate (docs/95 P7): the figure is computed here, never sent.
  const [rmPerGram, setRmPerGram] = useState("");
  const [advancePct, setAdvancePct] = useState("75");
  const [pinnedAll, setPinnedAll] = useState(false);
  const [kit, setKit] = useState<LenderKit | null>(null);
  const [checks, setChecks] = useState<Check[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  // A whole-stock file (every open deal at one moment, docs/94 U1): each deal's checks, and which one is shown.
  const [book, setBook] = useState<{ kit: LenderKit; checks: Check[] }[] | null>(null);
  // The desk name the link claims (the trader's own word), once a link was opened.
  const [linked, setLinked] = useState<{ from: string | null } | null>(null);

  // A link checks itself as it opens, with what this browser already confirmed for that desk.
  const onLink = useEffectEvent(async (got: { text: string; from: string | null }) => {
    setLinked({ from: got.from });
    setText(got.text);
    let desk: string | undefined;
    try {
      desk = (JSON.parse(got.text) as LenderKit).desk;
    } catch {}
    const saved = desk ? savedSetups()[desk.toLowerCase()] : undefined;
    await run(got.text, saved && desk ? { ...saved, desk } : undefined);
  });
  useEffect(() => {
    const open = () =>
      void kitFromFragment(location.hash).then(
        (got) => got && onLink(got),
        (e: Error) => setError(e.message),
      );
    open();
    window.addEventListener("hashchange", open);
    return () => window.removeEventListener("hashchange", open);
  }, []);

  /** `use`: the set-up to check with instead of the fields below (a saved or just-confirmed desk); it fills them too. */
  async function run(raw: string, use?: Setup & { desk: string }) {
    setError("");
    setChecks(null);
    setBook(null);
    let kits: LenderKit[];
    // A whole-stock file is checked as one even with one deal in it (docs/99 W1).
    let isBook = false;
    try {
      const parsed = JSON.parse(raw) as LenderKit | { kits: LenderKit[] };
      isBook = "kits" in parsed && Array.isArray(parsed.kits);
      kits = isBook ? (parsed as { kits: LenderKit[] }).kits : [parsed as LenderKit];
      if (!kits.length || kits.some((k) => !k?.signed?.statement || !k.disclosure || !k.anchor)) throw new Error("this is not a desk statement file");
      const notBook = isBook ? bookError(kits) : null;
      if (notBook) throw new Error(notBook);
    } catch (e) {
      setKit(null);
      setError((e as Error).message);
      return;
    }
    const k = kits[0];
    setKit(k);
    setBusy(true);
    try {
      const isAddr = (x: string) => /^0x[0-9a-fA-F]{40}$/.test(x.trim());
      const v = use ?? { signer, venuePin, partiesText, desk: deskPin };
      if (use) {
        setSigner(use.signer);
        setVenuePin(use.venuePin);
        setPartiesText(use.partiesText);
        setDeskPin(use.desk);
      }
      const parties = parseParties(v.partiesText);
      const pins = {
        signer: isAddr(v.signer) ? v.signer.trim() : undefined,
        desk: isAddr(v.desk) ? v.desk.trim() : undefined,
        venue: isAddr(v.venuePin) ? v.venuePin.trim() : undefined,
        parties: Object.values(parties).every((v) => isAddr(v.split(":")[0])) && Object.keys(parties).length ? parties : undefined,
        stressBps: Math.round((Number(stressPct) || 30) * 100),
        basisBps: Math.round((Number(basisPct) || 2) * 100),
      };
      // Only pinned values make a PASS (docs/89 H4): the trader holds the statement key.
      setPinnedAll(!!pins.signer && !!pins.desk && !!pins.venue && !!pins.parties);
      // Remember this trader's set-up in this browser only (docs/95 P6), under its desk (docs/96 V3); a private window may refuse.
      try {
        if (pins.signer && pins.venue && pins.desk)
          localStorage.setItem(PINS_KEY, JSON.stringify({ ...savedSetups(), [pins.desk.toLowerCase()]: { signer: v.signer, venuePin: v.venuePin, partiesText: v.partiesText } }));
      } catch {}
      const checksFor = async (kk: LenderKit, rest: LenderKit[]) => {
        const [offline, price, anchor, venue, funding] = await Promise.all([checkOffline(kk, pins), checkPrice(kk), checkAnchor(kk), checkVenue(kk, pins.venue), checkFunding(kk, pins.venue)]);
        const history = [...(previous ? [checkHistory(kk, previous)] : []), ...(rest.length ? [checkTogether(kk, rest, venue.shortNow)] : [])];
        const byId = Object.fromEntries([...offline, price, anchor, venue, ...(funding ? [funding] : []), ...history, checkAge(kk)].map((c) => [c.id, c]));
        return ["a", "b", "c", "f", "h", "m", "p", "d", "e", "g", "n", "w", "t"].map((id) => byId[id]).filter(Boolean);
      };
      // Each deal of a whole-stock file is checked with the others (one short, one desk total); a single file with any extra files given.
      const results = await Promise.all(kits.map(async (kk, i) => ({ kit: kk, checks: await checksFor(kk, isBook ? kits.filter((_, j) => j !== i) : others) })));
      if (isBook) setBook(results);
      setChecks(results[0].checks);
    } finally {
      setBusy(false);
    }
  }

  const st = kit?.signed.statement;
  // TOFU (docs/105): what this browser confirmed for the desk this statement names, and what the statement adds.
  const chainId = kit?.anchor.chainId ?? 0;
  const deskOf = kit?.desk;
  const confirmed = deskOf ? savedSetups()[deskOf.toLowerCase()] : undefined;
  const named = kit ? setupOf(kit) : undefined;
  const pinned = parseParties(confirmed?.partiesText ?? "");
  const added = kit && confirmed ? (kit.parties ?? []).filter((p) => (pinned[p.name] ?? "").split(":")[0].toLowerCase() !== p.address.toLowerCase()) : [];
  const keyChanged =
    !!confirmed && !!named && (confirmed.signer.toLowerCase() !== named.signer.toLowerCase() || confirmed.venuePin.toLowerCase() !== named.venuePin.toLowerCase());
  const firstTime = !!kit && !!deskOf && !confirmed;
  const bookOk = !book || book.every((r) => r.checks.every((c) => c.ok));
  const allOk = !!checks && checks.every((c) => c.ok) && bookOk;
  const failed = (checks ?? []).filter((c) => !c.ok);
  const goneOffset = failed.some((c) => c.id === "e" && c.lines.some((l) => /not there now/.test(l)));
  // Failed only because a sale or a smaller offset came after the file (docs/101 Y2): out of date, ask for the newest.
  const allFailed = [...failed, ...(book ?? []).flatMap((r) => r.checks.filter((c) => !c.ok))];
  const outOfDate = allFailed.length > 0 && allFailed.every((c) => c.outdated);
  const when = st ? new Date(st.at * 1000).toLocaleString() : "";
  // One sentence first, in the lender's terms (docs/95 P4).
  const bookBad = (book ?? []).filter((r) => !r.checks.every((c) => c.ok)).map((r) => r.kit.signed.statement.deal);
  const bookEligibleG = (book ?? []).reduce((a, r) => a + (Number(grams(r.kit.signed.statement.eligible?.grams).split(" ")[0]) || 0), 0);
  const bookAfter = (book ?? []).reduce((a, r) => a + Number(r.kit.signed.statement.eligibleUsdc), 0);
  const verdict = !checks
    ? "Checking…"
    : outOfDate
      ? `Out of date: the trader sold gold or reduced the offset after this ${book ? "file" : "statement"} (issued ${when}). Ask for the newest one; do not lend on this one.`
    : book && bookBad.length
      ? `Do not lend on this file: ${bookBad.join(", ")} failed. Open each below to see why.`
      : book && pinnedAll
        ? `The whole stock holds as of ${when}: ${book.length} deals, ${bookEligibleG.toFixed(3)} g eligible; after each deal's maximum loss, ${usd(String(bookAfter))} USDC in all.`
    : goneOffset
      ? "Do not lend on this statement. The offset it counts was closed after it was issued: the gold was sold or the hedge was taken off. Ask the trader which; if the gold is still held, ask for a statement issued today."
      : !allOk
        ? `Do not lend on this statement: ${failed.map((c) => c.title.toLowerCase()).join("; ")} — failed.`
        : !pinnedAll
          ? firstTime
            ? "Consistent so far. Confirm this desk with the trader by phone (below), then Save: until then nothing in it is pinned."
            : "Consistent, but not pinned: confirm the desk below, or fill in the set-up values you confirmed yourself, then check again."
          : `These figures hold as of ${when}: ${grams(st?.eligible?.grams)} eligible, ${usd(st?.eligible?.valueUsdc)} USDC; after the maximum loss, ${usd(st?.eligibleUsdc)} USDC.${
              // docs/96 V1 (3): alone, a sold deal's offset can be covered by the desk's other stock.
              !others.length && Number(st?.deskHeldContracts ?? 0) > Number(st?.heldContracts ?? 0) + 2e-6
                ? " The desk holds other stock under the same short, so a sale of this deal would not show here: ask for the whole-stock file."
                : ""
            }`;
  const stages = (st?.byStage ?? []).map((l) => `${grams(l.grams)} ${STAGE[l.stage] ?? l.stage}`).join(", ");
  // The advance at the lender's own price per gram and rate, from the checked grams and the statement's own loss formula.
  const price = Number(rmPerGram);
  const heldG = Number(grams(st?.held).split(" ")[0]) || 0;
  const eligG = Number(grams(st?.eligible?.grams).split(" ")[0]) || 0;
  const unoffset = 1 - Number(st?.offsetShareBps ?? 0) / 10000;
  const lossRm = st && price > 0 ? heldG * price * ((unoffset * st.stressBps) / 10000 + st.basisBps / 10000) : 0;
  const advanceRm = price > 0 ? (Number(advancePct) / 100) * Math.max(0, eligG * price - lossRm) : 0;
  const rm = (n: number) => n.toLocaleString("en-MY", { minimumFractionDigits: 2, maximumFractionDigits: 2 });

  /** Pin what the statement names (first time), or add its new counterparties to what was confirmed; then check. */
  async function confirmDesk() {
    if (!kit?.desk || !named) return;
    const base = confirmed ?? named;
    const parties = { ...parseParties(base.partiesText), ...Object.fromEntries((kit.parties ?? []).map((p) => [p.name, p.address])) };
    await run(text, { signer: base.signer, venuePin: base.venuePin, partiesText: Object.entries(parties).map(([n, a]) => `${n}=${a}`).join("\n"), desk: kit.desk });
  }
  // A check's lines name keys as this page knows them; the addresses stay in the set-up below (docs/81).
  const label = (a: string) => {
    const x = a.toLowerCase();
    if (named && x === named.signer.toLowerCase()) return `the statement key ••${tail(a)}`;
    if (deskOf && x === deskOf.toLowerCase()) return `the desk ••${tail(a)}`;
    if (named && x === named.venuePin.toLowerCase()) return `exchange account ••${tail(a)}`;
    const p = kit?.parties?.find((q) => q.address.toLowerCase() === x);
    return p ? `${nameAt(p.name, chainId)} ••${tail(a)}` : `••${tail(a)}`;
  };
  const words = (l: string) => l.replace(/0x[0-9a-fA-F]{40}\b/g, label);

  return (
    <>
      <TopBar />
      <Page className="max-w-3xl space-y-6 lg:pt-12">
        <div>
          <h1 className="flex items-center gap-2 text-headline font-semibold">
            <Landmark className="size-6 text-protect" aria-hidden /> Check a trader&apos;s statement
          </h1>
          <p className="mt-2 text-muted-foreground">
            {linked ? "The statement in the link the trader sent you" : "Open the link the trader sent you. The statement in it"} is checked here against the chain
            and the exchange&apos;s public record, not against the trader&apos;s desk: who signed each step, whether the book was fixed on chain, and whether the
            offset they claim really exists.
          </p>
          {error && <p className="mt-3 text-sm text-destructive">{error}</p>}
        </div>

        {(busy || checks) && (
          <Section title={busy ? "Checking…" : allOk && pinnedAll ? "You can lend against these figures" : allOk ? "Not pinned yet" : outOfDate ? "Ask for the newest statement" : "Do not lend on this statement"} tone={busy ? "default" : allOk && pinnedAll ? "protect" : "attention"}>
            {!busy && <p className="mb-4 text-sm font-medium">{verdict}</p>}
            {book && (
              <div className="mb-4 flex flex-wrap gap-2">
                {book.map((r) => {
                  const good = r.checks.every((c) => c.ok);
                  return (
                    <Button
                      key={r.kit.signed.statement.deal}
                      size="sm"
                      variant={r.kit === kit ? "default" : "outline"}
                      onClick={() => {
                        setKit(r.kit);
                        setChecks(r.checks);
                      }}
                    >
                      {good ? <CheckCircle2 className="size-3.5" /> : <XCircle className="size-3.5" />} {r.kit.signed.statement.deal}
                    </Button>
                  );
                })}
              </div>
            )}
            <details open={!allOk}>
              <summary className="cursor-pointer text-caption font-medium text-muted-foreground">Every check ({(checks ?? []).length})</summary>
            <ul className="mt-3 space-y-3">
              {busy && !checks && (
                <li className="flex items-center gap-2 text-sm text-muted-foreground">
                  <Loader2 className="size-4 animate-spin" /> Asking two public nodes and the exchange…
                </li>
              )}
              {checks?.map((c) => (
                <li key={c.id} className="flex gap-2.5">
                  {c.ok ? <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-protect" aria-label="passed" /> : <XCircle className="mt-0.5 size-4 shrink-0 text-destructive" aria-label="failed" />}
                  <div className="min-w-0 text-sm">
                    <p className="font-medium">{c.title}</p>
                    {c.lines.map((l, i) => (
                      <p key={i} className="break-words text-muted-foreground">
                        {words(l)}
                      </p>
                    ))}
                  </div>
                </li>
              ))}
            </ul>
            </details>
            {allOk && !pinnedAll && !firstTime && (
              <p className="mt-4 text-sm">
                The statement agrees with itself, the chain and the exchange — but the trader holds the statement key and names the
                counterparties. Confirm them as <em>you</em> know them, then check again.
              </p>
            )}
          </Section>
        )}

        {/* Trust on first use, per desk (docs/105): the lender confirms the desk once by phone; later statements are held to it. */}
        {kit && checks && !busy && deskOf && (firstTime || keyChanged || added.length > 0) && (
          <Section title={firstTime ? "A desk you have not confirmed" : keyChanged ? "Not the desk you confirmed" : "New counterparties"} icon={PhoneCall} tone="attention">
            {keyChanged ? (
              <p className="text-sm">
                The statement key or the exchange account in this statement is not the one you confirmed for desk ••{tail(deskOf)}. Do not lend until the trader
                explains; to confirm again, change the set-up under &quot;…or open a file&quot;.
              </p>
            ) : (
              <>
                <p className="text-sm font-medium">
                  {firstTime ? (
                    <>
                      This is desk ••{tail(deskOf)}
                      {linked?.from ? ` (${linked.from})` : ""}. Confirm with them by phone, then Save.
                    </>
                  ) : (
                    <>Desk ••{tail(deskOf)} now names counterparties you have not confirmed. Confirm them with the trader by phone, then Save.</>
                  )}
                </p>
                {/* Two codes (docs/103 S2-1): the desk's address is public (it is in every link), the statement key is what signs. */}
                <p className="mt-2 text-sm">
                  Ask them to read you the two codes on their statement card — desk{" "}
                  <span className="figures rounded-md bg-muted px-2 py-0.5 font-mono text-base font-semibold tracking-wider">{codeText(deskOf, chainId)}</span> and
                  statement key{" "}
                  <span className="figures rounded-md bg-muted px-2 py-0.5 font-mono text-base font-semibold tracking-wider">
                    {named ? codeText(named.signer, chainId) : "?"}
                  </span>
                  . If either is different, do not save.
                </p>
                <ul className="mt-3 space-y-1 text-caption text-muted-foreground">
                  {firstTime && named && (
                    <>
                      <li>Statement key ••{tail(named.signer)} — the key that signs this desk&apos;s statements</li>
                      <li>Exchange account ••{tail(named.venuePin)} — where the offset is held</li>
                    </>
                  )}
                  {(firstTime ? (kit.parties ?? []) : added).map((p) => (
                    <li key={p.name + p.address}>
                      {nameAt(p.name, chainId)} ••{tail(p.address)} · code {codeText(p.address, chainId)}
                      {pinned[p.name] ? ` (was ••${tail(pinned[p.name].split(":")[0])})` : ""}
                    </li>
                  ))}
                </ul>
                <p className="mt-2 text-caption text-muted-foreground">Saved in this browser only. Later statements from this desk are checked against it.</p>
                <Button className="mt-3" onClick={() => void confirmDesk()} disabled={busy}>
                  Save and check again
                </Button>
              </>
            )}
          </Section>
        )}

        <details open={!linked} className="group rounded-xl border bg-card p-4 shadow-card sm:p-5">
          <summary className="flex cursor-pointer list-none select-none items-center gap-1.5 text-sm font-medium [&::-webkit-details-marker]:hidden">
            <ChevronRight className="size-4 transition-transform duration-150 group-open:rotate-90" aria-hidden />…or open a file
          </summary>
          <div className="mt-4 space-y-3">
            <Input
              type="file"
              accept="application/json,.json"
              aria-label="Statement file"
              onChange={async (e) => {
                const f = e.target.files?.[0];
                if (!f) return;
                const t = await f.text();
                setText(t);
                await run(t);
              }}
            />
            {st && <p className="text-sm">Statement {st.deal}, issued {when} — loaded.</p>}
            <details>
              <summary className="cursor-pointer text-caption text-muted-foreground">…or paste its contents</summary>
              <Textarea
                value={text}
                onChange={(e) => setText(e.target.value)}
                className="mt-2 max-h-28 min-h-20 font-mono text-xs"
                aria-label="Statement contents"
              />
            </details>
            <div className="grid gap-3 rounded-lg border bg-background p-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <p className="text-sm font-medium">Set up this trader once</p>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={!kit?.desk || !savedSetups()[kit.desk.toLowerCase()]}
                  onClick={() => {
                    // Only the set-up saved for the desk this file names; its key and parties still decide (docs/96 V3).
                    const v = kit?.desk ? savedSetups()[kit.desk.toLowerCase()] : undefined;
                    if (!v || !kit?.desk) return;
                    setDeskPin(kit.desk);
                    setSigner(v.signer);
                    setVenuePin(v.venuePin);
                    setPartiesText(v.partiesText);
                  }}
                >
                  Use the values saved for this desk
                </Button>
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="pin-signer">Statement key</Label>
                <Input id="pin-signer" value={signer} onChange={(e) => setSigner(e.target.value)} placeholder="0x…" aria-label="Expected statement key" className="font-mono text-xs" />
                <p className="text-caption text-muted-foreground">From the trader when the facility starts, on their letterhead or in person — never from the same message as a statement.</p>
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="pin-desk">Desk account</Label>
                <Input id="pin-desk" value={deskPin} onChange={(e) => setDeskPin(e.target.value)} placeholder="0x…" aria-label="Desk account" className="font-mono text-xs" />
                <p className="text-caption text-muted-foreground">From the trader at set-up; ask each counterparty to confirm it is the desk they sign for.</p>
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="pin-venue">Exchange account that holds the offset</Label>
                <Input id="pin-venue" value={venuePin} onChange={(e) => setVenuePin(e.target.value)} placeholder="0x…" aria-label="Venue account" className="font-mono text-xs" />
                <p className="text-caption text-muted-foreground">From the trader at set-up; from your custodian if you hold its master key.</p>
              </div>
            <div className="flex flex-wrap items-center gap-2 text-caption text-muted-foreground">
              Size the maximum loss at no less than a
              <Input value={stressPct} onChange={(e) => setStressPct(e.target.value)} inputMode="decimal" aria-label="Price fall, percent" className="h-8 w-16 text-xs" />
              % fall and
              <Input value={basisPct} onChange={(e) => setBasisPct(e.target.value)} inputMode="decimal" aria-label="Basis, percent" className="h-8 w-16 text-xs" />
              % basis
            </div>
              <div className="grid gap-1.5">
                <Label htmlFor="pin-parties">Counterparties</Label>
                <Textarea
                  id="pin-parties"
                  value={partiesText}
                  onChange={(e) => setPartiesText(e.target.value)}
                  placeholder={"Supplier=0x…:bought\nVault=0x…:arrived,returned\nBuyer=0x…:sold"}
                  className="min-h-20 font-mono text-xs"
                  aria-label="Counterparty addresses"
                />
                <p className="text-caption text-muted-foreground">Each signing address from the supplier, vault and buyer themselves, with what each may sign.</p>
              </div>
            </div>
            <label className="grid gap-1.5 text-caption text-muted-foreground">
              An earlier statement from the same desk, if you kept one: the book must have only grown since
              <Input
                type="file"
                accept="application/json,.json"
                aria-label="Earlier statement file"
                onChange={async (e) => {
                  const f = e.target.files?.[0];
                  try {
                    setPrevious(f ? (JSON.parse(await f.text()) as LenderKit) : null);
                  } catch {
                    setPrevious(null);
                    setError("the earlier file is not a desk statement");
                  }
                }}
              />
            </label>
            <label className="grid gap-1.5 text-caption text-muted-foreground">
              The desk&apos;s other statements made at the same time, if you finance more than one of its deals: they must share one
              short, not each claim it
              <Input
                type="file"
                multiple
                accept="application/json,.json"
                aria-label="Other statement files"
                onChange={async (e) => {
                  try {
                    setOthers(await Promise.all([...(e.target.files ?? [])].map(async (f) => JSON.parse(await f.text()) as LenderKit)));
                  } catch {
                    setOthers([]);
                    setError("one of the other files is not a desk statement");
                  }
                }}
              />
            </label>
            <Button onClick={() => void run(text)} disabled={!text.trim() || busy}>
              {busy && <Loader2 className="size-4 animate-spin" />} Check it
            </Button>
            <p className="text-caption text-muted-foreground">
              To trust nothing served by this site, run the same checks from the source, with the values you confirmed:{" "}
              <code className="break-all">
                python3 scripts/verify_desk.py statement.json --signer {signer.trim() || "0x…"} --desk {deskPin.trim() || "0x…"} --venue {venuePin.trim() || "0x…"}
                {Object.entries(parseParties(partiesText)).map(([n, a]) => ` --party '${`${n}=${a}`.replace(/'/g, "'\\''")}'`).join("") || " --party Name=0x…"}
              </code>
            </p>
          </div>
        </details>

        {st && (
          <Section
            title={`Deal ${st.deal}: ${grams(st.held)} of ${st.goods}${stages ? ` — ${stages}` : ""}`}
            description={`At ${new Date(st.at * 1000).toLocaleString()} · ${st.market} mark ${usd(st.markUsdc)} USDC per troy ounce`}
            tone={checks ? (allOk ? "protect" : "attention") : "default"}
            className={checks && !allOk ? "opacity-60" : undefined}
          >
            <Stats>
              <Stat label="Value at the mark" value={`${usd(st.valueUsdc)}`} sub="USDC" />
              <Stat label="Offset" value={`${(Number(st.offsetShareBps) / 100).toFixed(1)}%`} sub={`${st.offsetContracts} ${st.market} short`} />
              <Stat label="Maximum loss" value={`${usd(st.maxLossUsdc)}`} sub={`USDC at a ${st.stressBps / 100}% fall`} tone="attention" />
            </Stats>
            <div className="mt-4 grid gap-2 text-sm">
              {st.eligible && <KeyValue label="Eligible (in stock, in transit)" value={`${grams(st.eligible.grams)} · ${usd(st.eligible.valueUsdc)} USDC`} />}
              {st.consigned && <KeyValue label="Consigned" value={`${grams(st.consigned.grams)} · ${usd(st.consigned.valueUsdc)} USDC`} />}
              {st.ineligible && <KeyValue label="Not eligible (not delivered, in fabrication)" value={`${grams(st.ineligible.grams)} · ${usd(st.ineligible.valueUsdc)} USDC`} />}
              <KeyValue label="Eligible value after the maximum loss" value={<span className="font-semibold">{usd(st.eligibleUsdc)} USDC</span>} />
              <KeyValue label="Bought for" value={st.costUsdc} />
              {typeof st.fundingUsdc === "string" && (
                <KeyValue
                  label="Cost of the offset so far"
                  value={Number(st.fundingUsdc) >= 0 ? `the market paid ${usd(st.fundingUsdc)} USDC` : `${usd(st.fundingUsdc.replace("-", ""))} USDC`}
                />
              )}
              {st.hedgeControl && (
                <div className="pt-1">
                  <p className="text-caption font-medium text-muted-foreground">Who controls the offset — the trader states, not checked</p>
                  <p className="text-sm break-words">{noAddr(st.hedgeControl)}</p>
                </div>
              )}
            </div>
            {/* This page's own words from the values it checked, not the trader's (docs/94 U4). */}
            <p className="mt-3 text-caption text-muted-foreground">
              Valued at the exchange&apos;s {st.market} price {st.markUsdc} per ounce, checked against its 1-minute candles. Eligible:
              stock and goods in transit; consigned goods are their own class; goods not delivered and work in fabrication are not
              eligible. Maximum loss: a {st.stressBps / 100}% fall on the part not offset plus {st.basisBps / 100}% of the whole for
              the goods not tracking {st.market}; the eligible value after it is the eligible class less that loss. The offset is
              this deal&apos;s share of the short the statement read; the desk&apos;s total holding is the trader&apos;s word. Apply your own
              advance rates to these classes.
            </p>
            <div className="mt-4 grid gap-2 rounded-lg border bg-background p-3 text-sm">
              <div className="flex flex-wrap items-center gap-2 text-caption text-muted-foreground">
                At your price of RM
                <Input value={rmPerGram} onChange={(e) => setRmPerGram(e.target.value)} inputMode="decimal" placeholder="per gram" aria-label="Your gold price, RM per gram" className="h-8 w-24 text-xs" />
                per gram and an advance rate of
                <Input value={advancePct} onChange={(e) => setAdvancePct(e.target.value)} inputMode="decimal" aria-label="Your advance rate, percent" className="h-8 w-16 text-xs" />%
              </div>
              {price > 0 && (
                <p>
                  Up to <strong className="figures">RM {rm(advanceRm)}</strong> — {advancePct}% of ({eligG.toFixed(3)} g × RM {rm(price)} less the maximum loss at your
                  price, RM {rm(lossRm)}).
                </p>
              )}
            </div>
            <div className="mt-4 rounded-lg border border-attention/40 p-3 text-caption">
              <p className="font-medium text-foreground">What this file does not prove</p>
              <ul className="mt-1 list-disc space-y-0.5 pl-4 text-muted-foreground">
                <li>That the metal exists: only the vault&apos;s signature says so; no audit.</li>
                <li>Your charge over it, or over the exchange account.</li>
                <li>That the exchange account is the trader&apos;s: it is the account they named, until you hold its master key.</li>
                <li>That the offset stays: the trader can close it at any time; ask for a statement each time you advance.</li>
                <li>Your reference price: the figures use {st.market} on Hyperliquid, and the offset is in US dollars — a ringgit move is not offset.</li>
                <li>That the desk&apos;s total holding is complete: one file cannot show deals it leaves out.</li>
                <li>Hyperliquid is not an approved broker; this runs on its test network today.</li>
              </ul>
            </div>
          </Section>
        )}

      </Page>
    </>
  );
}
