import { AddMoney } from "../../../../features/add-money/add-money.tsx";
import { resolveAccount } from "../../../../lib/api.ts";

// `/a/<name>/add` add money (H5). The name becomes the account here, on the server; the page gets the address only to
// show it on the address plate with Copy and to address the move to it (PM ruling 2026-10-18).
export default async function AddMoneyPage({ params }: { params: Promise<{ name: string }> }) {
  const { name } = await params;
  let account: Awaited<ReturnType<typeof resolveAccount>>;
  try {
    account = await resolveAccount(name);
  } catch {
    return <Said title="The service did not answer" body="This page needs it to find the account. Reload the page to try again." />;
  }
  if (!account) return <Said title="No account has that name" body="Check the name in the link; an account lives on the service that opened it." />;
  const chain = account.chains[0]?.key ?? "";
  return <AddMoney account={{ name: account.name, address: account.policy as `0x${string}`, chain }} />;
}

function Said({ title, body }: { title: string; body: string }) {
  return (
    <section className="flex flex-col gap-(--space-2)">
      <h1 className="row-title">{title}</h1>
      <p className="row">{body}</p>
    </section>
  );
}
