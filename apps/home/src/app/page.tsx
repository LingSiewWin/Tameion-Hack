import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { Hero, type Treatment } from "../features/hero/hero.tsx";
import type { PortWatch, Scene } from "../features/hero/model.ts";
import { GuestCard } from "../features/onboarding/guest.tsx";

// `/`: H7's hero (the strait becomes the book) above, then the guest card: the invite word opens the seeded demo account
// read-only, or starts the stepper (H4). The judges' path, /a/<name>, never loads the hero.
const pub = (f: string) => join(process.cwd(), "public", f);
const readJson = <T,>(f: string): T | null => (existsSync(pub(f)) ? (JSON.parse(readFileSync(pub(f), "utf8")) as T) : null);

export default async function GuestPage({ searchParams }: { searchParams: Promise<{ treatment?: string }> }) {
  const t = (await searchParams).treatment;
  const treatment: Treatment = t === "B" || t === "C" ? t : "A";
  // the recording once it exists; the synthetic replay only in development, never in a production build
  const replayUrl = existsSync(pub("hero/ais.json"))
    ? "/hero/ais.json"
    : process.env.NODE_ENV !== "production" && existsSync(pub("hero/ais.synthetic.json"))
      ? "/hero/ais.synthetic.json"
      : null;
  return (
    <>
      <Hero
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
