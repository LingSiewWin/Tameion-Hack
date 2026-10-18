import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { redirect } from "next/navigation";
import { HERO } from "../features/hero/copy.ts";
import type { Treatment } from "../features/hero/hero.tsx";
import type { PortWatch, Scene } from "../features/hero/model.ts";
import { HeroSlot } from "../features/hero/slot.tsx";
import { GuestCard } from "../features/onboarding/guest.tsx";
import { accountHref } from "../lib/api.ts";

// `/`: the guest card (the invite word opens the seeded demo account read-only, or starts the stepper, H4) under the
// statement. The hero ships behind HOME_HERO=1 until the five-second test passes (founder): with it, H7's hero (the strait
// becomes the book, treatments A, B, C) sits above the card; without it, the statement is plain text, the page's one h1,
// and nothing of the hero renders or loads. The judges' path, /a/<name>, never loads the hero.
const pub = (f: string) => join(process.cwd(), "public", f);
const readJson = <T,>(f: string): T | null => (existsSync(pub(f)) ? (JSON.parse(readFileSync(pub(f), "utf8")) as T) : null);

export default async function GuestPage({ searchParams }: { searchParams: Promise<{ treatment?: string; policy?: string; view?: string }> }) {
  // read first: it keeps the page rendered per request, so the flag is read at run time, not frozen at build
  const sp = await searchParams;
  const t = sp.treatment;
  // An old console link (`/?policy=0x…`, and `/agent?policy=…` through next.config): the account's page, by its name.
  const to = typeof sp.policy === "string" ? await accountHref(sp.policy, sp.view) : null;
  if (to) redirect(to);
  if (process.env.HOME_HERO !== "1")
    return (
      <>
        <h1 className="authority m-0 max-w-(--measure)">{HERO.statement}</h1>
        <div className="pt-(--space-6)">
          <GuestCard />
        </div>
      </>
    );
  const treatment: Treatment = t === "B" || t === "C" ? t : "A";
  // the prebuilt illustrative tracks (scripts/hero-data.mjs tracks)
  const replayUrl = existsSync(pub("hero/ais.json")) ? "/hero/ais.json" : null;
  return (
    <>
      <HeroSlot
        treatment={treatment}
        scene={readJson<Scene>("hero/scene.json")}
        replayUrl={replayUrl}
        portwatch={readJson<PortWatch>("hero/portwatch.json")}
        demo={process.env.HOME_DEMO_NAME ?? null}
      />
      <div className="pt-(--space-7)">
        <GuestCard />
      </div>
    </>
  );
}
