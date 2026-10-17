"use client";
// Add money (`/a/<name>/add`, H5): what the account holds, its address on its own chain with Copy, the testnet faucet
// (what the demo sent, and Circle's faucet for more), and a Circle CCTP V2 move from another chain (move-plate.tsx).
// A card top-up is not built, so it has no plate. The Owner view shows no hash and no explorer link (DESIGN Rules 1, 6);
// the Agent view adds them. Figures come from the service (`inbox`) or the chain, never computed here (Rule 3).
import { useCallback, useEffect, useState } from "react";
import type { Address, Hex } from "viem";
import { AddMoneySkeleton } from "../../components/skeletons.tsx";
import { useSkeleton } from "../../components/skeleton.tsx";
import { fetchDemo, fetchInbox, fetchTenant } from "../../lib/api.ts";
import type { Demo, Inbox, Tenant } from "../../lib/types.ts";
import { useView } from "../../lib/view.ts";
import { byKey, type CctpChain } from "./cctp.ts";
import { grouped, ledgerText, usdcText, whenText } from "./format.ts";
import { MovePlate } from "./move-plate.tsx";
import { explorer, FAUCET, link, plate, quiet } from "./ui.ts";
import { demoFunding } from "./wallet.ts";

/** The account as the server resolved it from its name: the address is for this page's Copy, calldata and chain reads. */
export type AddAccount = { name: string; address: Address; chain: string };

/** What the page read from the service. `null` while reading; an `Error` when that read failed. */
type Reads = { inbox: Inbox | Error | null; tenant: Tenant | Error | null; demo: Demo | Error | null };

const asValue = <T,>(r: PromiseSettledResult<T>) => (r.status === "fulfilled" ? r.value : r.reason instanceof Error ? r.reason : new Error(String(r.reason)));
async function readAll(name: string): Promise<Reads> {
  const [inbox, tenant, demo] = await Promise.allSettled([fetchInbox(name), fetchTenant(name), fetchDemo()]);
  return { inbox: asValue(inbox), tenant: asValue(tenant), demo: asValue(demo) };
}

export function AddMoney({ account }: { account: AddAccount }) {
  const dest = byKey(account.chain);
  const [reads, setReads] = useState<Reads>({ inbox: null, tenant: null, demo: null });
  useEffect(() => {
    let live = true;
    readAll(account.name).then((r) => live && setReads(r));
    return () => {
      live = false;
    };
  }, [account.name]);
  // Try again, and a move that lands: read the account again, keeping what is shown until the answer comes
  const reload = useCallback(() => {
    readAll(account.name).then(setReads);
  }, [account.name]);

  const showSkeleton = useSkeleton(reads.inbox === null);
  if (!dest) return <Unsupported chain={account.chain} />;
  if (showSkeleton) return <AddMoneySkeleton />;
  const inbox = reads.inbox instanceof Error ? null : reads.inbox;
  const tenant = reads.tenant instanceof Error ? null : reads.tenant;
  const demo = reads.demo instanceof Error ? null : reads.demo;

  return (
    <div className="flex flex-col gap-(--space-5)">
      <section aria-labelledby="add-title" className="flex flex-col gap-(--space-1)">
        <h1 id="add-title" className="row-title">
          Add money
        </h1>
        {inbox ? (
          <>
            <p className="balance money">{ledgerText(inbox.balance)}</p>
            <p className="meta">In the account on {dest.name}</p>
          </>
        ) : (
          <div className="flex flex-col items-start gap-(--space-2)">
            <p className="row">The service did not answer, so the balance is not shown. Moving money below still works.</p>
            <button type="button" className={quiet} onClick={reload}>
              Try again
            </button>
          </div>
        )}
      </section>
      <AddressPlate dest={dest} address={account.address} />
      <FaucetPlate dest={dest} address={account.address} tenant={tenant} demo={demo} />
      <MovePlate key={account.address} dest={dest} account={account} inbox={inbox} onLanded={reload} />
    </div>
  );
}

function Unsupported({ chain }: { chain: string }) {
  return (
    <section className="flex flex-col gap-(--space-2)">
      <h1 className="row-title">Add money</h1>
      <p className="row">This account lives on {chain}, where this page cannot move USDC yet. Send USDC to it there from any wallet.</p>
    </section>
  );
}

/** The account's address on its own chain, as text with Copy (PM ruling 2026-10-18). Grouped by four in the Owner view. */
function AddressPlate({ dest, address }: { dest: CctpChain; address: Address }) {
  const view = useView();
  const [copied, setCopied] = useState<"yes" | "no" | null>(null);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(address);
      setCopied("yes");
    } catch {
      setCopied("no");
    }
    setTimeout(() => setCopied(null), 2000);
  };
  return (
    <section aria-labelledby="address-title" className={plate}>
      <h2 id="address-title" className="row-title">
        The account&apos;s address on {dest.name}
      </h2>
      <p className="row money break-all">{view === "agent" ? address : grouped(address)}</p>
      <div className="flex flex-wrap items-center gap-(--space-3)">
        <button type="button" className={quiet} onClick={copy}>
          Copy
        </button>
        <span role="status" className="meta">
          {copied === "yes" ? "Copied" : copied === "no" ? "Not copied: select the address above" : ""}
        </span>
        {view === "agent" && (
          <a className={link} href={explorer(dest, "address", address)} target="_blank" rel="noreferrer">
            explorer
          </a>
        )}
      </div>
      <p className="meta">
        USDC sent on any other chain does not reach the account.
      </p>
    </section>
  );
}

type Funding = { state: "reading" } | { state: "found"; tx: Hex; amount: bigint; at: number } | { state: "none" } | { state: "unread" };

/** The testnet faucet: what the demo sent when the account opened (read from the chain), and Circle's faucet for more.
 *  The service pays a demo account once (demo.rs `fund_once`); it has no top-up route. */
function FaucetPlate({ dest, address, tenant, demo }: { dest: CctpChain; address: Address; tenant: Tenant | null; demo: Demo | null }) {
  const view = useView();
  // the service's own rule for a demo account (demo.rs `fund_once`): on chain, with the demo's delay
  const isDemo = !!demo?.enabled && tenant?.deployed === true && tenant.delay_secs === demo.delaySecs;
  const relayer = typeof tenant?.operator === "string" ? (tenant.operator as Address) : null;
  const from = typeof tenant?.created_block === "number" ? BigInt(tenant.created_block) : null;
  const [funding, setFunding] = useState<Funding>({ state: "reading" });
  const read = useCallback(() => {
    if (!relayer || from === null) return;
    demoFunding(dest, relayer, address, from).then(
      (f) => setFunding(f ? { state: "found", ...f } : { state: "none" }),
      () => setFunding({ state: "unread" }),
    );
  }, [dest, relayer, address, from]);
  useEffect(() => {
    if (isDemo) read();
  }, [isDemo, read]);
  const again = () => {
    setFunding({ state: "reading" });
    read();
  };

  return (
    <section aria-labelledby="faucet-title" className={plate}>
      <h2 id="faucet-title" className="row-title">
        Testnet faucet
      </h2>
      {isDemo && (
        <div className="flex flex-col gap-(--space-1)">
          {funding.state === "reading" && <span aria-hidden className="skeleton block h-[22px] w-2/3" />}
          {funding.state === "found" && (
            <>
              <p className="row">Funded by the demo: {usdcText(funding.amount)}</p>
              <p className="meta">Arrived {whenText(funding.at)}</p>
              {view === "agent" && (
                <a className={`${link} break-all`} href={explorer(dest, "tx", funding.tx)} target="_blank" rel="noreferrer">
                  {funding.tx}
                </a>
              )}
            </>
          )}
          {funding.state === "none" && (
            <>
              <p className="row">The demo&apos;s funding is not on {dest.name}.</p>
              <p className="meta">The demo pays an account once, when it opens.</p>
            </>
          )}
          {funding.state === "unread" && (
            <div className="flex flex-col items-start gap-(--space-2)">
              <p className="row">{dest.name} did not answer, so the demo&apos;s funding is not shown.</p>
              <button type="button" className={quiet} onClick={again}>
                Try again
              </button>
            </div>
          )}
        </div>
      )}
      <div className="flex flex-col gap-(--space-1)">
        <a className={`${link} row`} href={FAUCET} target="_blank" rel="noreferrer">
          Get 20 USDC from Circle&apos;s faucet
        </a>
        <p className="meta">Choose {dest.name} there and paste the account&apos;s address. Once every 2 hours.</p>
      </div>
    </section>
  );
}
