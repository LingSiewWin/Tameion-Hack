"use client";

import { CheckCircle2, Download, History, ReceiptText, ShoppingBag } from "lucide-react";
import { EmptyState, Section } from "@/components/layout/section";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ago, labelOf, usd } from "@/lib/tameion";
import { Receipt, useAccount } from "./ctx";
import { partyOf, Payee } from "./payee";

function download(name: string, rows: (string | number)[][]) {
  const csv = rows.map((r) => r.map((c) => (/[",\n]/.test(String(c)) ? `"${String(c).replace(/"/g, '""')}"` : String(c))).join(",")).join("\n") + "\n";
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([csv], { type: "text/csv" }));
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
}

export function Activity() {
  const { inbox, tenant } = useAccount();
  if (!inbox) return null;
  const paid = inbox.paid;
  const base = `tameion-${(labelOf(tenant) || tenant.policy.slice(0, 10)).replace(/[^\w.-]+/g, "_")}`;
  return (
    <Section
      title="Recent"
      icon={History}
      flush
      action={
        paid.length > 0 && (
          <>
            <Button
              variant="ghost"
              size="sm"
              onClick={() =>
                download(`${base}-statement.csv`, [
                  ["date_utc", "payee", "amount_usdc", "paid_by", "transaction", "explorer", "account"],
                  ...paid.map((p) => [
                    p.at ? new Date(p.at * 1000).toISOString() : "",
                    // A file for the accountant: the name, and the address it stands for.
                    `${partyOf(inbox, p.counterparty)?.label} (${p.counterparty})`,
                    usd(p.amount),
                    p.by === "you" ? "owner" : "agent",
                    p.tx || "",
                    p.explorer || "",
                    tenant.policy,
                  ]),
                ])
              }
            >
              <Download className="size-3.5" /> Statement
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() =>
                // Xero / QuickBooks bank-statement import: money out is negative.
                download(`${base}-bank-statement.csv`, [
                  ["*Date", "*Amount", "Payee", "Description", "Reference"],
                  ...paid.map((p) => [
                    p.at ? new Date(p.at * 1000).toISOString().slice(0, 10) : "",
                    "-" + usd(p.amount).replace(/,/g, ""),
                    partyOf(inbox, p.counterparty)?.label ?? p.counterparty,
                    `USDC payment to ${p.counterparty} by ${p.by === "you" ? "the owner" : "the agent"} from ${tenant.policy}`,
                    p.tx || "",
                  ]),
                ])
              }
            >
              For Xero / QuickBooks
            </Button>
          </>
        )
      }
    >
      {paid.length === 0 ? (
        <EmptyState icon={ReceiptText} title="Nothing paid yet">
          Every payment shows here with its receipt.
        </EmptyState>
      ) : (
        <ol className="divide-y">
          {paid.slice(0, 12).map((p, i) => (
            <li key={(p.tx ?? "") + i} className="flex gap-3 px-4 py-3 sm:px-5">
              {p.bought ? <ShoppingBag className="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden /> : <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-protect" aria-hidden />}
              <div className="min-w-0 flex-1 text-sm">
                <p className="leading-snug">
                  {p.bought ? (
                    <>
                      Bought from <span className="font-medium">{p.bought}</span>
                    </>
                  ) : (
                    <>
                      Paid <Payee addr={p.counterparty} />
                    </>
                  )}
                  {p.by === "you" && (
                    <Badge variant="secondary" className="ml-2 align-middle">
                      by you
                    </Badge>
                  )}
                </p>
                <p className="mt-0.5 text-xs text-muted-foreground">
                  {p.bought && p.reason ? `${p.reason} · ` : ""}
                  {p.early ? `Paid early for the invoice's discount: saved ${usd(p.saved)} USDC · ` : ""}
                  {p.at ? ago(p.at) : ""} {p.explorer && <>· <Receipt href={p.explorer} /></>}
                </p>
              </div>
              <span className="figures shrink-0 text-sm font-semibold">{usd(p.amount)}</span>
            </li>
          ))}
        </ol>
      )}
    </Section>
  );
}
