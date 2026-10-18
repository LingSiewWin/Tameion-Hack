"use client";
// From another chain: one Circle CCTP V2 move into the account, tracked as three states — burned on the source chain,
// signed by Circle, arrived on the account's chain. The viewer's wallet burns (approve + depositForBurn) and mints
// (receiveMessage); Circle's attestation is polled through this app's route (/api/circle/attestation). The move is kept
// on this device the moment the burn has a hash, so a closed tab resumes from the chain and nothing is sent twice.
import { useEffect, useRef, useState } from "react";
import type { Address, Hex } from "viem";
import { useView } from "../../lib/view.ts";
import type { AddAccount } from "./add-money.tsx";
import {
  approveData,
  burnData,
  byDomain,
  IRIS,
  MESSAGE_TRANSMITTER,
  mintData,
  parseMove,
  readMessage,
  sourcesFor,
  TOKEN_MESSENGER,
  type Attestation,
  type CctpChain,
  type Domain,
  type Move,
} from "./cctp.ts";
import { asOfText, ledgerText, parseUsdc, usdcText, whenText, type Held } from "./format.ts";
import { Mark } from "./mark.tsx";
import { cta, explorer, FAUCET, inner, mark, plate, quiet, rowLink } from "./ui.ts";
import { allowed, codeOf, connect, gasHeld, injected, minted, receiptStatus, send, usdcHeld, type Phase } from "./wallet.ts";

const KEY = (address: string) => `tameion.home.move.${address.toLowerCase()}`;
const loadMove = (address: string) => {
  try {
    return parseMove(localStorage.getItem(KEY(address)));
  } catch {
    return null;
  }
};
const saveMove = (address: string, m: Move | null) => {
  try {
    if (m) localStorage.setItem(KEY(address), JSON.stringify(m));
    else localStorage.removeItem(KEY(address));
  } catch {
    // storage blocked: the move is still tracked while the page stays open
  }
};

/** What the chain and Circle say about a kept move. `minted` is the destination's own record (usedNonces). */
type Track = { burn: "checking" | "success" | "reverted"; att: Attestation | null; minted: boolean | null; gas: bigint | null };

function useTrack(move: Move | null, dest: CctpChain, who: Address | undefined, onLanded: () => void): Track | null {
  const [track, setTrack] = useState<{ for: Move; t: Track } | null>(null);
  const landed = useRef(onLanded);
  useEffect(() => {
    landed.current = onLanded;
  }, [onLanded]);
  useEffect(() => {
    if (!move) return;
    const from = byDomain(move.from)!;
    let cur: Track = { burn: "checking", att: null, minted: null, gas: null };
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      try {
        if (cur.burn === "checking") cur = { ...cur, burn: (await receiptStatus(from, move.burnTx)) ?? "checking" };
        if (cur.burn === "success" && cur.att?.state !== "signed") {
          const r = await fetch(`/api/circle/attestation?domain=${move.from}&tx=${move.burnTx}`, { cache: "no-store" });
          cur = { ...cur, att: (await r.json()) as Attestation };
        }
        if (cur.att?.state === "signed" && cur.minted !== true) {
          const burn = readMessage(cur.att.message);
          if (burn) cur = { ...cur, minted: await minted(dest, burn.nonce) };
          if (!cur.minted && who) cur = { ...cur, gas: await gasHeld(dest, who) };
        }
      } catch {
        // a read that failed keeps the last state; the next tick reads again
      }
      if (!live) return;
      setTrack({ for: move, t: cur });
      if (cur.minted) return landed.current();
      if (cur.burn === "reverted") return;
      // Arc attests in about half a second; Base and Arbitrum take minutes, so ask Circle less often there
      timer = setTimeout(tick, cur.att?.state === "signed" ? 15_000 : move.from === 26 ? 3_000 : 10_000);
    };
    tick();
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [move, dest, who]);
  return track && track.for === move ? track.t : null;
}

const phaseWord = (p: Phase, i: number, of: number) => `${p === "signing" ? "Signing" : p === "sent" ? "Sent" : "Confirmed"}${of > 1 ? ` ${i} of ${of}` : ""}`;

type Failure = { words: string; moved: string; detail: string };
/** A wallet's or chain's refusal as a plain sentence, with its own text kept for the disclosure. */
function failureOf(e: unknown, moved: string): Failure {
  const detail = String((e as { shortMessage?: string; message?: string })?.shortMessage ?? (e as Error)?.message ?? e);
  const code = codeOf(e);
  const words =
    code === 4001
      ? "You said no in the wallet."
      : /insufficient funds/i.test(detail)
        ? "The wallet could not pay the gas."
        : /failed on|revert/i.test(detail)
          ? "The chain refused the transaction."
          : "The wallet did not send it.";
  return { words, moved, detail };
}

/** Seconds since `since` (ms), ticking each second while `on`; DESIGN §States, Waits. */
function useElapsed(since: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (since === null) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [since]);
  return since === null ? 0 : Math.max(0, Math.floor((now - since) / 1000));
}
/** A wait in words: the verb, its seconds after 3 s, and after 15 s the promise that nothing is sent twice. */
const waitWords = (verb: string, secs: number) => (secs >= 15 ? "Still checking. It will not be sent twice." : secs >= 3 ? `${verb}, ${secs} s` : verb);

export function MovePlate({
  dest,
  account,
  figures,
  asOf,
  onLanded,
}: {
  dest: CctpChain;
  account: AddAccount;
  /** the account's balance and Gateway holdings as last read; `asOf` (ms) when they are stale */
  figures: Pick<Held, "balance" | "holdingsElsewhere"> | null;
  asOf: number | null;
  onLanded: () => void;
}) {
  const view = useView();
  const sources = sourcesFor(dest);
  const [fromDomain, setFromDomain] = useState<Domain>(sources[0].domain);
  const from = byDomain(fromDomain)!;
  const [amountText, setAmountText] = useState("");
  const amount = parseUsdc(amountText);
  const input = useRef<HTMLInputElement>(null);
  // This plate renders only in the browser (the page shows its skeleton until the account is read), so the first state
  // may read this device: the move it keeps, and whether it has a wallet. eth_accounts then says if this site is
  // connected, asking the person nothing.
  const [wallet, setWallet] = useState<{ has: boolean; address?: Address }>(() => ({ has: !!injected() }));
  const [held, setHeld] = useState<{ for: string; usdc: bigint; gas: bigint } | null>(null);
  const [move, setMove] = useState<Move | null>(() => loadMove(account.address));
  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  // when the wallet's current phase began, so a long "Sent" says how long and that nothing is re-sent
  const [phaseAt, setPhaseAt] = useState<number | null>(null);
  const elapsed = useElapsed(busy ? phaseAt : null);
  const label = busy && busy.startsWith("Sent") ? waitWords(busy, elapsed) : busy;

  useEffect(() => {
    const p = injected();
    if (!p) return;
    let live = true;
    p.request({ method: "eth_accounts" }).then(
      (a) => live && setWallet({ has: true, address: (a as Address[])[0] }),
      () => {},
    );
    return () => {
      live = false;
    };
  }, []);

  // the wallet's USDC and gas on the chosen source chain, so a short wallet is said before it is asked to sign
  const who = wallet.address;
  useEffect(() => {
    if (!who) return;
    let live = true;
    const key = `${who}:${fromDomain}`;
    Promise.all([usdcHeld(from, who), gasHeld(from, who)]).then(
      ([usdc, gas]) => live && setHeld({ for: key, usdc, gas }),
      () => {},
    );
    return () => {
      live = false;
    };
    // read again once a move is kept: the wallet holds less after a burn
  }, [who, fromDomain, from, move]);
  const heldNow = held && held.for === `${who}:${fromDomain}` ? held : null;

  const track = useTrack(move, dest, who, onLanded);
  const burn = track?.att?.state === "signed" ? readMessage(track.att.message) : null;
  const forThisAccount = !burn || (burn.recipient.toLowerCase() === account.address.toLowerCase() && burn.destination === dest.domain);
  const arrived = track?.minted === true;
  const inFlight = !!move && !arrived && track?.burn !== "reverted";

  const start = async () => {
    if (!wallet.has || busy) return;
    if (!who) {
      try {
        setWallet({ has: true, address: await connect() });
      } catch (e) {
        setFailure(failureOf(e, "Nothing was sent."));
      }
      return;
    }
    if (!amount) return input.current?.focus();
    if (heldNow && heldNow.usdc < amount) return void window.open(FAUCET, "_blank", "noopener,noreferrer");
    setFailure(null);
    // the burn's hash, kept the moment the wallet hands it over (before the receipt)
    const sent: { move: Move | null } = { move: null };
    try {
      const of = (await allowed(from, who)) < amount ? 2 : 1;
      if (of === 2)
        await send(from, from.usdc, approveData(amount), (p) => {
          setBusy(phaseWord(p, 1, 2));
          setPhaseAt(Date.now());
        });
      await send(from, TOKEN_MESSENGER, burnData(from, dest, account.address, amount), (p, hash) => {
        setBusy(phaseWord(p, of, of));
        setPhaseAt(Date.now());
        if (p === "sent" && hash) {
          sent.move = { from: from.domain, amount: amount.toString(), burnTx: hash, at: Math.floor(Date.now() / 1000) };
          saveMove(account.address, sent.move);
        }
      });
      setAmountText("");
    } catch (e) {
      // after the burn has a hash the chain answers for it (reverted, or still checking); before, nothing moved
      if (!sent.move) setFailure(failureOf(e, "Nothing was sent."));
    } finally {
      setBusy(null);
      if (sent.move) setMove(sent.move);
    }
  };

  const mint = async () => {
    if (!move || track?.att?.state !== "signed" || busy) return;
    if (!who) {
      try {
        setWallet({ has: true, address: await connect() });
      } catch (e) {
        setFailure(failureOf(e, "Nothing arrived yet. The USDC is burned and waits at Circle for the mint."));
      }
      return;
    }
    setFailure(null);
    const { message, attestation } = track.att;
    const sent: { move: Move | null } = { move: null };
    try {
      await send(dest, MESSAGE_TRANSMITTER, mintData(message, attestation), (p, hash) => {
        setBusy(phaseWord(p, 1, 1));
        setPhaseAt(Date.now());
        if (p === "sent" && hash) saveMove(account.address, (sent.move = { ...move, mintTx: hash }));
      });
    } catch (e) {
      // the tracker reads the destination again: a mint someone else sent, or one whose answer was lost, shows as arrived
      setFailure(failureOf(e, "Nothing arrived yet. The USDC is burned and waits at Circle for the mint."));
    } finally {
      setBusy(null);
      setMove(sent.move ?? { ...move });
    }
  };

  const forget = () => {
    saveMove(account.address, null);
    setMove(null);
  };

  const moveFrom = move ? byDomain(move.from)! : null;
  const moved = move ? usdcText(BigInt(move.amount)) : "";
  const mintTx = move?.mintTx ?? (track?.att?.state === "signed" ? track.att.mintTx : null) ?? null;

  return (
    <section aria-labelledby="move-title" className={plate}>
      <h2 id="move-title" className="row-title">
        From another chain
      </h2>

      {move && moveFrom && arrived && (
        <div className="flex flex-col gap-(--space-2)">
          <p className="row">
            Moved {moved} from {moveFrom.name}
          </p>
          <p className="meta">Arrived on {dest.name}</p>
          {view === "agent" && <Hashes from={moveFrom} dest={dest} move={move} mintTx={mintTx} nonce={burn?.nonce} />}
          <Witnesses dest={dest} figures={figures} asOf={asOf} />
        </div>
      )}

      {move && moveFrom && !arrived && (
        <ol className="flex flex-col" aria-label="The move">
          <Line
            title={track?.burn === "success" ? `Burned ${moved} on ${moveFrom.name}` : `Burn ${moved} on ${moveFrom.name}`}
            step={track?.burn === "success" ? "done" : track?.burn === "reverted" ? "failed" : "transit"}
            state={
              track?.burn === "success" ? `Done ${whenText(move.at)}` : track?.burn === "reverted" ? "Failed: nothing moved" : "Still checking. It will not be sent twice."
            }
            agent={
              view === "agent" && (
                <a className={rowLink} href={explorer(moveFrom, "tx", move.burnTx)} target="_blank" rel="noreferrer">
                  {move.burnTx}
                </a>
              )
            }
          />
          {track?.burn !== "reverted" && (
            <>
              <Line
                title={track?.att?.state === "signed" ? "Signed by Circle" : "Circle signs it"}
                step={track?.att?.state === "signed" ? (forThisAccount ? "done" : "failed") : track?.burn === "success" ? "transit" : "next"}
                state={
                  track?.att?.state === "signed"
                    ? forThisAccount
                      ? "Done"
                      : "Signed for another address: this page will not bring it here"
                    : track?.att?.state === "waiting"
                      ? `Waiting for ${moveFrom.name} to finalize, ${moveFrom.wait}${minutesSince(move.at)}${track.att.delay ? ` (Circle: ${track.att.delay})` : ""}`
                      : track?.att?.state === "unreadable"
                        ? `Not read: ${track.att.words}`
                        : track?.burn === "success"
                          ? "Circle has not seen the burn yet"
                          : "Next"
                }
                agent={
                  view === "agent" &&
                  track?.burn === "success" && (
                    <span className="flex flex-col">
                      <a className={rowLink} href={`${IRIS}/v2/messages/${move.from}?transactionHash=${move.burnTx}`} target="_blank" rel="noreferrer">
                        iris /v2/messages/{move.from}
                      </a>
                      {burn && <span>nonce {burn.nonce}</span>}
                    </span>
                  )
                }
              />
              <Line
                title={`Arrives on ${dest.name}`}
                step={track?.att?.state === "signed" && forThisAccount ? "transit" : "next"}
                state={track?.att?.state === "signed" && forThisAccount ? (label ?? "Waits for the mint") : "Next"}
                agent={
                  view === "agent" &&
                  mintTx && (
                    <a className={rowLink} href={explorer(dest, "tx", mintTx)} target="_blank" rel="noreferrer">
                      {mintTx}
                    </a>
                  )
                }
              />
            </>
          )}
        </ol>
      )}

      {track?.burn === "reverted" && (
        <button type="button" className={quiet} onClick={forget}>
          Clear the failed move
        </button>
      )}

      {inFlight && track?.att?.state === "signed" && forThisAccount && (
        <MintStep dest={dest} wallet={wallet} gas={track.gas} busy={label} onMint={mint} />
      )}

      {!inFlight && (
        <MoveForm
          sources={sources}
          from={from}
          dest={dest}
          setFrom={setFromDomain}
          amountText={amountText}
          setAmountText={setAmountText}
          input={input}
          wallet={wallet}
          held={heldNow}
          amount={amount}
          busy={label}
          onStart={start}
        />
      )}

      {failure && (
        <div role="alert" className="flex flex-col gap-(--space-1)">
          <p className="row text-(--attention)">{failure.words}</p>
          <p className="meta">{failure.moved}</p>
          <details className="meta">
            <summary className="min-h-(--touch) cursor-pointer py-(--space-3)">What the wallet said</summary>
            <p className="break-all">{failure.detail}</p>
          </details>
        </div>
      )}
    </section>
  );
}

/** ", 6 min so far" once a minute has passed since `at` (unix seconds); read at render, refreshed by the tracker's polls. */
function minutesSince(at: number) {
  const m = Math.floor((Date.now() / 1000 - at) / 60);
  return m >= 1 ? `; ${m} min so far` : "";
}

function Line({ title, state, step, agent }: { title: string; state: string; step: keyof typeof mark; agent?: React.ReactNode }) {
  return (
    <li className="grid grid-cols-[12px_minmax(0,1fr)] gap-x-(--space-3) border-t border-(--rule) py-(--space-3) first:border-t-0 first:pt-0">
      <span aria-hidden className={`mt-(--space-2) size-3 rounded-(--radius-mark) ${mark[step]}`} />
      <span className="flex flex-col gap-(--space-1)">
        <span className="row">{title}</span>
        <span className="meta">{state}</span>
        {agent && <span className="break-all">{agent}</span>}
      </span>
    </li>
  );
}

function MintStep({
  dest,
  wallet,
  gas,
  busy,
  onMint,
}: {
  dest: CctpChain;
  wallet: { has: boolean; address?: Address };
  gas: bigint | null;
  busy: string | null;
  onMint: () => void;
}) {
  if (!wallet.has)
    return <p className="row">Bringing it to {dest.name} needs a browser wallet with {dest.gas} on {dest.name} for the gas. This browser has none.</p>;
  if (wallet.address && gas === 0n)
    return (
      <div className="flex flex-col gap-(--space-1)">
        <p className="row">
          Your wallet holds no {dest.gas} on {dest.name} to pay the mint&apos;s gas.
        </p>
        {dest.gas === "USDC" && (
          <a className={`${rowLink} row`} href={FAUCET} target="_blank" rel="noreferrer">
            Get USDC on {dest.name} from Circle&apos;s faucet
          </a>
        )}
      </div>
    );
  return (
    <button type="button" className={cta} onClick={onMint} aria-live="polite">
      {busy ?? (wallet.address ? `Bring it to ${dest.name}` : "Connect your wallet")}
    </button>
  );
}

function MoveForm({
  sources,
  from,
  dest,
  setFrom,
  amountText,
  setAmountText,
  input,
  wallet,
  held,
  amount,
  busy,
  onStart,
}: {
  sources: readonly CctpChain[];
  from: CctpChain;
  dest: CctpChain;
  setFrom: (d: Domain) => void;
  amountText: string;
  setAmountText: (s: string) => void;
  input: React.RefObject<HTMLInputElement | null>;
  wallet: { has: boolean; address?: Address };
  held: { usdc: bigint; gas: bigint } | null;
  amount: bigint | null;
  busy: string | null;
  onStart: () => void;
}) {
  const view = useView();
  const short = held && amount && held.usdc < amount;
  const noGas = held && held.gas === 0n;
  const label = busy
    ? busy
    : !wallet.address
      ? "Connect your wallet"
      : !amount
        ? "Enter an amount"
        : short
          ? `Get USDC on ${from.name}`
          : `Move ${usdcText(amount)} from ${from.name}`;
  return (
    <div className="flex flex-col gap-(--space-3)">
      <fieldset className="flex flex-col gap-(--space-2)">
        <legend className="row mb-(--space-2)">From</legend>
        <div className="flex flex-wrap gap-(--space-2)">
          {sources.map((c) => {
            const chosen = c.domain === from.domain;
            return (
              <label
                key={c.domain}
                className={`flex min-h-(--touch) cursor-pointer items-center gap-(--space-2) rounded-(--radius-inner) px-(--space-3) ${chosen ? "bg-(--plate-2)" : ""}`}
              >
                <input type="radio" name="from" className="accent-(--ink)" value={c.domain} checked={chosen} onChange={() => setFrom(c.domain)} />
                {view === "owner" && <Mark src={c.mark} />}
                <span className={chosen ? "row-title" : "row"}>{c.name}</span>
              </label>
            );
          })}
        </div>
      </fieldset>
      <label className="flex flex-col gap-(--space-2)">
        <span className="row">Amount in USDC</span>
        <input
          ref={input}
          className="row money min-h-(--button-compact) rounded-(--radius-inner) bg-(--plate-2) px-(--space-3) text-(--ink)"
          inputMode="decimal"
          autoComplete="off"
          value={amountText}
          onChange={(e) => setAmountText(e.target.value)}
        />
      </label>
      {!wallet.has ? (
        <p className="row">
          Moving USDC here needs a browser wallet holding USDC on {from.name}. This browser has none; open this page where your wallet is.
        </p>
      ) : (
        <>
          {held && (
            <p className="meta">
              Your wallet holds {usdcText(held.usdc)} on {from.name}
              {noGas ? `, and no ${from.gas} for the gas` : ""}. It arrives on {dest.name} after Circle signs: {from.wait}.
            </p>
          )}
          <button type="button" className={cta} onClick={onStart} aria-live="polite">
            {label}
          </button>
        </>
      )}
    </div>
  );
}

function Hashes({ from, dest, move, mintTx, nonce }: { from: CctpChain; dest: CctpChain; move: Move; mintTx: Hex | null; nonce?: Hex }) {
  return (
    <span className="flex flex-col break-all">
      <a className={rowLink} href={explorer(from, "tx", move.burnTx)} target="_blank" rel="noreferrer">
        burn {move.burnTx}
      </a>
      {nonce && <span>nonce {nonce} used on {dest.name}</span>}
      {mintTx && (
        <a className={rowLink} href={explorer(dest, "tx", mintTx)} target="_blank" rel="noreferrer">
          mint {mintTx}
        </a>
      )}
    </span>
  );
}

/** When a move lands: the account's balance as the service reads it, beside Circle Gateway's (inbox.holdingsElsewhere);
 *  the last figures this device read, marked with their time, when the service does not answer. */
function Witnesses({ dest, figures, asOf }: { dest: CctpChain; figures: Pick<Held, "balance" | "holdingsElsewhere"> | null; asOf: number | null }) {
  const view = useView();
  const g = figures?.holdingsElsewhere as { total?: string; error?: string } | null | undefined;
  return (
    <dl className={`grid grid-cols-2 gap-(--space-3) ${inner}`}>
      <div className="flex flex-col gap-(--space-1)">
        <dt className="meta">The account on {dest.name}</dt>
        <dd className="row money m-0">{figures ? ledgerText(figures.balance) : "Not read"}</dd>
        {figures && asOf && <dd className="meta m-0">{asOfText(asOf)}</dd>}
      </div>
      <div className="flex flex-col gap-(--space-1)">
        <dt className="meta">Circle Gateway, other chains</dt>
        <dd className="row money m-0">{g?.total ? ledgerText(g.total) : "Not read"}</dd>
        {g?.total && asOf && <dd className="meta m-0">{asOfText(asOf)}</dd>}
        {view === "agent" && g?.error && <dd className="meta m-0">{g.error}</dd>}
      </div>
    </dl>
  );
}
