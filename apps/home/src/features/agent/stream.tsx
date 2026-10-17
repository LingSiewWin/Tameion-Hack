"use client";
// The record stream (Agent view): every record of the account, newest first, from GET /api/level1/symbolon. Each line
// spells out its three slots; a line opens to the slots' alg, key id and whether the check passed, with the full leaf,
// root and the anchoring transaction. A decision the rule blocked is a refusal and carries its code.
import { Fragment, useState } from "react";
import type { Slot, Symbolon, SymbolonRecord } from "../../lib/types.ts";
import { copy } from "./copy.ts";
import { money, short, when } from "./format.ts";

const C = copy.stream;
type SlotName = keyof typeof C.slots;
const SLOTS: { name: SlotName; of: (r: SymbolonRecord) => Slot | null | undefined }[] = [
  { name: "agent", of: (r) => r.agent },
  { name: "counterparty", of: (r) => r.counterpartySlot },
  { name: "witness", of: (r) => r.witness },
];
const LINK = "text-(--ink) underline underline-offset-4";

/** A slot as one mark: ✓ verified, – none, ? its anchoring could not be read this time (witness only). */
function mark(s: Slot | null | undefined, unread: boolean): { glyph: string; words: string } {
  if (s?.verified) return { glyph: "✓", words: C.mark.verified(s.alg, s.keyId) };
  if (unread) return { glyph: "?", words: C.mark.unread };
  return { glyph: copy.none, words: C.mark.none };
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
        <h2 id="records-title" className="row-title m-0">
          {C.records(records.length)}
        </h2>
        {c && (
          <p className="mono-dense m-0" aria-label={C.countsLabel}>
            {(["single", "double", "triple", "unsigned"] as const).map((k) => (
              <span key={k} className="mr-(--space-4) inline-block last:mr-0">
                {C.counts[k]} {c[k]}
              </span>
            ))}
          </p>
        )}
        {unread.size > 0 && <p className="mono-read m-0 text-(--ink-2)">{C.unread(unread.size)}</p>}
      </div>
      {data.locked && <p className="mono-read m-0">{C.locked}</p>}
      {records.length === 0 ? (
        <p className="mono-read m-0">{C.empty}</p>
      ) : (
        <div role="region" aria-label={C.region} tabIndex={0} className="relative overflow-x-auto">
          <table className="mono-dense w-full border-collapse whitespace-nowrap text-left">
            <thead className="meta">
              <tr className="border-b border-(--rule)">
                {(data.locked ? C.headLocked : C.head).map((h) => (
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
                          {r.at ? when(r.at, now) : copy.none}
                        </button>
                      </td>
                      <td className="pr-(--space-4)">{r.seq}</td>
                      <td className="pr-(--space-4)">{C.kinds[r.kind ?? ""] ?? r.kind?.toLowerCase() ?? copy.none}</td>
                      <td className="pr-(--space-4)">
                        {r.decision === "BLOCK" ? (
                          <>
                            <span className="text-(--attention)">{C.refused}</span> <span className="meta">{C.refusedCode}</span>
                          </>
                        ) : (
                          (r.decision?.toLowerCase() ?? copy.none)
                        )}
                      </td>
                      <td className="pr-(--space-4)" title={r.counterparty}>
                        {r.counterparty ? short(r.counterparty) : copy.none}
                      </td>
                      <td className="pr-(--space-4) text-right">{amount?.qty ?? copy.none}</td>
                      <td className="pr-(--space-4)">{amount?.unit ?? ""}</td>
                      {SLOTS.map((s) => {
                        const m = mark(s.of(r), s.name === "witness" && unread.has(r.root.toLowerCase()));
                        return (
                          <td key={s.name} className="pr-(--space-4)" title={m.words}>
                            <span aria-hidden>{m.glyph}</span>
                            <span className="sr-only">
                              {C.slots[s.name]}: {m.words}
                            </span>
                          </td>
                        );
                      })}
                      <td className="pr-(--space-4)" title={r.leaf}>
                        {short(r.leaf)}
                      </td>
                      <td>
                        {anchor ? (
                          <a href={anchor} target="_blank" rel="noreferrer" className={LINK} title={r.anchorTx ?? ""}>
                            {short(r.anchorTx)}
                          </a>
                        ) : (
                          (r.anchorTx && short(r.anchorTx)) || copy.none
                        )}
                      </td>
                    </tr>
                    {isOpen && (
                      <tr id={`slots-${id}`} className="border-b border-(--rule)">
                        <td colSpan={C.head.length} className="pb-(--space-3)">
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
  const D = C.detail;
  const anchor = txLink(explorer, r.anchorTx);
  const paid = txLink(explorer, r.tx);
  const line = "grid grid-cols-[14ch_1fr] gap-x-(--space-4)";
  return (
    <div className="flex flex-col gap-(--space-1) whitespace-normal break-all pl-(--space-4)">
      <div className={line}>
        <span className="meta">{D.root}</span>
        <span>{r.root}</span>
      </div>
      <div className={line}>
        <span className="meta">{D.leaf}</span>
        <span>{r.leaf}</span>
      </div>
      <table className="my-(--space-1) self-start border-collapse whitespace-nowrap text-left">
        <thead className="meta">
          <tr>
            {D.head.map((h) => (
              <th key={h} scope="col" className="pr-(--space-5) font-normal">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {SLOTS.map((s) => {
            const slot = s.of(r);
            const verified = slot?.verified ? D.yes : s.name === "witness" && unread ? D.unread : D.no;
            return (
              <tr key={s.name}>
                <th scope="row" className="pr-(--space-5) font-normal">
                  {D.slotNames[s.name]}
                </th>
                <td className="pr-(--space-5)">{slot?.alg ?? copy.none}</td>
                <td className="pr-(--space-5)">{slot?.keyId ?? copy.none}</td>
                <td>{verified}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {r.anchorTx && (
        <div className={line}>
          <span className="meta">{D.anchorTx}</span>
          {anchor ? (
            <a href={anchor} target="_blank" rel="noreferrer" className={LINK}>
              {r.anchorTx}
            </a>
          ) : (
            <span>{r.anchorTx}</span>
          )}
        </div>
      )}
      {r.tx && (
        <div className={line}>
          <span className="meta">{D.paymentTx}</span>
          {paid ? (
            <a href={paid} target="_blank" rel="noreferrer" className={LINK}>
              {r.tx}
            </a>
          ) : (
            <span>{r.tx}</span>
          )}
        </div>
      )}
      {r.note && (
        <div className={line}>
          <span className="meta">{D.note}</span>
          <span className="mono-read break-normal">{r.note}</span>
        </div>
      )}
    </div>
  );
}
