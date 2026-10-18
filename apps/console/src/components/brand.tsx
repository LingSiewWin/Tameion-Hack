"use client";

import { Moon, Sun } from "lucide-react";
import { useTheme } from "next-themes";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export function Mark({ className = "size-6" }: { className?: string }) {
  return (
    <svg viewBox="0 0 32 32" className={className} aria-hidden>
      <circle cx="16" cy="16" r="14.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeDasharray="1.6 1.4" />
      <circle cx="16" cy="16" r="10" fill="none" stroke="var(--protect)" strokeWidth="3" strokeDasharray="47 63" transform="rotate(-90 16 16)" strokeLinecap="round" />
      <path d="M12 12.5h8M16 12.5v8" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}

export function ThemeToggle() {
  const { resolvedTheme, setTheme } = useTheme();
  return (
    <Button
      variant="ghost"
      size="icon"
      aria-label="Switch light or dark"
      onClick={() => setTheme(resolvedTheme === "dark" ? "light" : "dark")}
    >
      <Sun className="size-4 dark:hidden" />
      <Moon className="hidden size-4 dark:block" />
    </Button>
  );
}

// ---- the two surfaces (docs/102 D13, docs/107) ------------------------------------------------------------
// Human = the desk's decision inbox; Agent = the live ops view. One toggle in the header, beside light/dark, on
// the two owner pages only (a lender or a counterparty reading a link is neither).
export type Surface = "human" | "agent";
const SURFACE_KEY = "tameion.surface";

/** The surface the owner chose last. Storage can be blocked or absent (a private window): then it is Human. */
function lastSurface(): Surface {
  try {
    return localStorage.getItem(SURFACE_KEY) === "agent" ? "agent" : "human";
  } catch {
    return "human";
  }
}
function chose(s: Surface) {
  try {
    localStorage.setItem(SURFACE_KEY, s);
  } catch {}
}

/** Two links dressed as one segmented control: the page is the state, so each segment is a link (Tab reaches both,
 *  Enter follows, `aria-current` says where you are) and the choice is remembered when it is made, not when a page opens. */
export function SurfaceToggle({ active, humanHref = "/", fill = false }: { active: Surface; humanHref?: string; fill?: boolean }) {
  // Human is the account's own page, `/`, for every account; the desk's page only for an account that has a desk (an exchange connected).
  const surfaces: { id: Surface; label: string; href: string }[] = [
    { id: "human", label: "Human", href: humanHref },
    { id: "agent", label: "Agent", href: "/agent" },
  ];
  return (
    <nav aria-label="View" className={cn("items-center rounded-lg bg-muted p-[3px]", fill ? "flex w-full" : "mr-1 hidden md:flex")}>
      {surfaces.map((s) => (
        <Link
          key={s.id}
          href={s.href}
          aria-current={s.id === active ? "page" : undefined}
          onClick={() => chose(s.id)}
          className={cn(
            "inline-flex h-8 items-center rounded-md px-3 text-sm font-medium outline-ring transition-colors duration-100 focus-visible:outline-2 focus-visible:outline-offset-1 pointer-coarse:h-10",
            fill && "flex-1 justify-center",
            s.id === active ? "bg-background text-foreground shadow-card ring-1 ring-foreground/10" : "text-muted-foreground hover:text-foreground",
          )}
        >
          {s.label}
        </Link>
      ))}
    </nav>
  );
}

/** A tab opened straight onto the desk (nothing behind it to go Back to) opens on the surface chosen last. A link, Back
 *  or the toggle never redirects, so `/desk` stays reachable by its own link. */
export function useOpenOnLastSurface() {
  const router = useRouter();
  useEffect(() => {
    if (window.history.length <= 1 && lastSurface() === "agent") router.replace("/agent");
  }, [router]);
}

export function TopBar({ children, surface, humanHref }: { children?: React.ReactNode; surface?: Surface; humanHref?: string }) {
  return (
    <>
      <header className="sticky top-0 z-30 border-b border-border/70 bg-background/85 backdrop-blur supports-[backdrop-filter]:bg-background/70">
        <div className="mx-auto flex h-14 max-w-6xl items-center gap-3 px-4 sm:px-6">
          <Link href="/" className="flex items-center gap-2 font-semibold tracking-tight">
            <Mark />
            <span>Tami</span>
          </Link>
          <div className="ml-auto flex items-center gap-1">
            {children}
            {surface && <SurfaceToggle active={surface} humanHref={humanHref} />}
            <ThemeToggle />
          </div>
        </div>
      </header>
      {/* Below md the header row has no room for the two segments beside the account menu (a long account name overflows it at 640 px): they get a row of their own, full width, under it. */}
      {surface && (
        <div className="mx-auto w-full max-w-6xl px-4 pt-3 md:hidden">
          <SurfaceToggle active={surface} humanHref={humanHref} fill />
        </div>
      )}
    </>
  );
}
