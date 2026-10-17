"use client";
// The record stream (Agent view): every record of the account, newest first, from GET /api/level1/symbolon. Each line
// spells out its three slots; a line opens to the slots' alg, key id and whether the check passed, with the full leaf,
// root and the anchoring transaction. A decision the rule blocked is a refusal and carries its code.
import { Fragment, useState } from "react";
import type { Slot, Symbolon, SymbolonRecord } from "../../lib/types.ts";
import { money, short, when } from "./format.ts";

const KIND: Record<string, string> = {
  SETTLEMENT: "settlement",
  DECISION: "decision",
  FLOWCHECK: "flow check",
  METERCHECK: "meter check",
  PRICETABLE: "price table",
};

type SlotName = "agent" | "counterparty" | "witness";
const SLOTS: { name: SlotName; label: string; of: (r: SymbolonRecord) => Slot | null | undefined }[] = [
  { name: "agent", label: "Agent", of: (r) => r.agent },
  { name: "counterparty", label: "Seller", of: (r) => r.counterparty_slot },
  { name: "witness", label: "Witness", of: (r) => r.witness },
];

/** A slot as one mark: ✓ verified, – none, ? its anchoring could not be read this time (witness only). */
function mark(s: Slot | null | undefined, unread: boolean): { glyph: string; words: string } {
  if (s?.verified) return { glyph: "✓", words: `verified, ${s.alg}, ${s.keyId}` };
  if (unread) return { glyph: "?", words: "the anchoring could not be read this time" };
  return { glyph: "–", words: "none" };
}

const txLink = (explorer: string | null | undefined, tx: string | null | undefined) =>
  explorer && tx && /^0x[0-9a-fA-F]{64}$/.test(tx) && /^https:\/\//.test(explorer) ? `${explorer}/tx/${tx}` : null;

export function Stream({ data, now }: { data: Symbolon; now?: number }) {
  const [open, setOpen] = useState<string | null>(null);
  const unread = new Set(data.chainReadFailed.map((r) => r.toLowerCase()));
  const records = [...data.records].sort((a, b) => (b.at ?? 0) - (a.at ?? 0) || b.root.localeCompare(a.root) || b.seq - a.seq);
  const c = data.counts;

  return (
    <section aria-labelledby="records-title" className="flex flex-col gap-(--space-2)">
      <div className="flex flex-wrap items-baseline gap-x-(--space-5) gap-y-(--space-1)">
        <h2 id="records-title" className="mono m-0">
          Records {records.length}
        </h2>
        {c && (
          <p className="mono m-0" aria-label="Records by how many slots stand behind them">
            <span className="mr-(--space-4)">Single {c.single}</span>
            <span className="mr-(--space-4)">Double {c.double}</span>
            <span className="mr-(--space-4)">Triple {c.triple}</span>
            <span>Unsigned {c.unsigned}</span>
          </p>
        )}
        {unread.size > 0 && (
          <p className="meta m-0">
            Anchoring not read this time for {unread.size} {unread.size === 1 ? "root" : "roots"}; their witness shows ?.
          </p>
        )}
      </div>
      {data.locked && (
        <p className="meta m-0">
          This account is read with its owner&apos;s device or a read key. Anyone can check each record&apos;s leaf; its fields
          and slots stay with them.
        </p>
      )}
      {records.length === 0 ? (
        <p className="mono m-0">No records yet. The agent writes one on each pass.</p>
      ) : (
        <div role="region" aria-label="Records, newest first" tabIndex={0} className="overflow-x-auto">
          <table className="mono w-full border-collapse whitespace-nowrap text-left">
            <thead className="meta">
              <tr className="border-b border-(--rule)">
                {(data.locked
                  ? ["Seq", "Leaf", "Root"]
                  : ["When", "Seq", "Kind", "Verdict", "Counterparty", "Qty", "Unit", "Agent", "Seller", "Witness", "Leaf", "Anchor"]
                ).map((h) => (
                  <th key={h} scope="col" className="py-(--space-1) pr-(--space-4) font-normal">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {records.map((r) => {
                const id = `${r.root}:${r.seq}`;
                if (data.locked)
                  return (
                    <tr key={id} className="border-b border-(--rule)">
                      <td className="py-(--space-1) pr-(--space-4)">{r.seq}</td>
                      <td className="pr-(--space-4)" title={r.leaf}>
                        {r.leaf}
                      </td>
                      <td title={r.root}>{short(r.root)}</td>
                    </tr>
                  );
                const amount = money(r.amount);
                const isOpen = open === id;
                const refused = r.decision === "BLOCK";
                const anchor = txLink(data.explorer, r.anchorTx);
                return (
                  <Fragment key={id}>
                    <tr className={isOpen ? "" : "border-b border-(--rule)"}>
                      <td className="py-(--space-1) pr-(--space-4)">
                        <button
                          type="button"
                          aria-expanded={isOpen}
                          aria-controls={`slots-${id}`}
                          onClick={() => setOpen(isOpen ? null : id)}
                          className="min-h-(--touch) cursor-pointer border-0 bg-transparent p-0 text-left text-(--ink) [font:inherit]"
                        >
                          {r.at ? when(r.at, now) : "–"}
                        </button>
                      </td>
                      <td className="pr-(--space-4)">{r.seq}</td>
                      <td className="pr-(--space-4)">{KIND[r.kind ?? ""] ?? r.kind?.toLowerCase() ?? "–"}</td>
                      <td className="pr-(--space-4)">
                        {refused ? (
                          <>
                            <span className="text-(--red)">refused</span> <span className="meta">block</span>
                          </>
                        ) : (
                          (r.decision?.toLowerCase() ?? "–")
                        )}
                      </td>
                      <td className="pr-(--space-4)" title={r.counterparty}>
                        {r.counterparty ? short(r.counterparty) : "–"}
                      </td>
                      <td className="pr-(--space-4) text-right">{amount?.qty ?? "–"}</td>
                      <td className="pr-(--space-4)">{amount?.unit ?? ""}</td>
                      {SLOTS.map((s) => {
                        const m = mark(s.of(r), s.name === "witness" && unread.has(r.root.toLowerCase()));
                        return (
                          <td key={s.name} className="pr-(--space-4)" title={m.words}>
                            <span aria-hidden>{m.glyph}</span>
                            <span className="sr-only">
                              {s.label}: {m.words}
                            </span>
                          </td>
                        );
                      })}
                      <td className="pr-(--space-4)" title={r.leaf}>
                        {short(r.leaf)}
                      </td>
                      <td>
                        {anchor ? (
                          <a href={anchor} target="_blank" rel="noreferrer" className="text-(--accent) underline underline-offset-4" title={r.anchorTx ?? ""}>
                            {short(r.anchorTx)}
                          </a>
                        ) : (
                          (r.anchorTx && short(r.anchorTx)) || "–"
                        )}
                      </td>
                    </tr>
                    {isOpen && (
                      <tr id={`slots-${id}`} className="border-b border-(--rule)">
                        <td colSpan={12} className="pb-(--space-3)">
                          <Detail r={r} explorer={data.explorer} unread={unread.has(r.root.toLowerCase())} />
                        </td>
                      </tr>
                    )}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function Detail({ r, explorer, unread }: { r: SymbolonRecord; explorer?: string | null; unread: boolean }) {
  const anchor = txLink(explorer, r.anchorTx);
  const paid = txLink(explorer, r.tx);
  const line = "grid grid-cols-[14ch_1fr] gap-x-(--space-4)";
  return (
    <div className="flex flex-col gap-(--space-1) whitespace-normal break-all pl-(--space-4)">
      <div className={line}>
        <span className="meta">Root</span>
        <span>{r.root}</span>
      </div>
      <div className={line}>
        <span className="meta">Leaf</span>
        <span>{r.leaf}</span>
      </div>
      <table className="my-(--space-1) border-collapse whitespace-nowrap text-left">
        <thead className="meta">
          <tr>
            {["Slot", "Alg", "Key id", "Verified"].map((h) => (
              <th key={h} scope="col" className="pr-(--space-5) font-normal">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {SLOTS.map((s) => {
            const slot = s.of(r);
            const verified = slot?.verified ? "yes" : s.name === "witness" && unread ? "not read this time" : "no";
            return (
              <tr key={s.name}>
                <th scope="row" className="pr-(--space-5) font-normal">
                  {s.name}
                </th>
                <td className="pr-(--space-5)">{slot?.alg ?? "–"}</td>
                <td className="pr-(--space-5)">{slot?.keyId ?? "–"}</td>
                <td>{verified}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {r.anchorTx && (
        <div className={line}>
          <span className="meta">Anchor tx</span>
          {anchor ? (
            <a href={anchor} target="_blank" rel="noreferrer" className="text-(--accent) underline underline-offset-4">
              {r.anchorTx}
            </a>
          ) : (
            <span>{r.anchorTx}</span>
          )}
        </div>
      )}
      {r.tx && (
        <div className={line}>
          <span className="meta">Payment tx</span>
          {paid ? (
            <a href={paid} target="_blank" rel="noreferrer" className="text-(--accent) underline underline-offset-4">
              {r.tx}
            </a>
          ) : (
            <span>{r.tx}</span>
          )}
        </div>
      )}
      {r.note && (
        <div className={line}>
          <span className="meta">Note</span>
          <span className="break-normal">{r.note}</span>
        </div>
      )}
    </div>
  );
}
