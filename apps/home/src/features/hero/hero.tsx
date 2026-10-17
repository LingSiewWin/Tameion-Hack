"use client";
// The hero on `/` (DESIGN §The hero): one scroll-bound stage. The plate on the left holds the words; the canvas holds the
// strait becoming the book; the demo account's real records take the rows; the Owner home closes it. Never on /a/<name>.
// The canvas starts after first paint (the plate's words are the LCP); reduced motion shows three stills instead.
import dynamic from "next/dynamic";
import { useEffect, useRef, useState } from "react";
import { OwnerSkeleton } from "../../components/skeletons.tsx";
import { fetchDemo, fetchInbox } from "../../lib/api.ts";
import type { Inbox } from "../../lib/types.ts";
import { useView } from "../../lib/view.ts";
import { COPY as OWNER_WORDS, lines } from "../owner/words.ts";
import { CAPTION, FIGURES, HERO } from "./copy.ts";
import { clamp01, COLUMN, MODULE, phases, placeholderScene, seg, stillAt, STILLS, windowWords, type PortWatch, type Replay, type Scene } from "./model.ts";
import { createStage, drawChart, drawPlaceholder, type Layout } from "./stage.ts";

// H2's Owner home closes the hero; its code (passkey signing included) loads only once the hand-over begins
const OwnerHome = dynamic(() => import("../owner/owner-home.tsx").then((m) => m.OwnerHome), { ssr: false, loading: () => <OwnerSkeleton /> });

export type Treatment = "A" | "B" | "C";
type Row = { date: string; who: string; amount: string; why: string; state: string };
type Props = { treatment: Treatment; scene: Scene | null; replayUrl: string | null; portwatch: PortWatch | null; demo: string | null };

/** Columns of the book on the grid, in px (multiples of 8; the row module is 40): date, who, amount, why, state. */
const COLS_WIDE = [80, 160, 120, 240, 80];
const COLS_NARROW = [64, 112, 88, 0, 80];
/** The demo account's records as the book's rows, in the Owner home's own words (H2's lines()), so the hand-over reads
 *  exactly like the page it hands over to: payees by name, amounts trimmed, no hash (DESIGN Rule 1). */
export function rowsOf(inbox: Inbox, vendor?: string, nowSecs = Date.now() / 1000): Row[] {
  const k = vendor?.toLowerCase();
  const p = k ? inbox.parties?.[k] : undefined;
  // the demo's vendor reads "Demo Vendor" on every host, as the Owner home names it (owner-home.tsx withDemoVendor)
  const named = k && p?.kind === "new" ? { ...inbox, parties: { ...inbox.parties, [k]: { ...p, label: OWNER_WORDS.demoVendor, kind: "tameion" as const } } } : inbox;
  return lines(named, nowSecs)
    .slice(0, 6)
    .map((l) => ({ date: l.when, who: l.payee, amount: l.amount, why: l.why, state: l.state }));
}

const groupRange: [number, number][] = [[-1, 0.3], [0.3, 0.5], [0.5, 0.7], [0.7, 0.9], [0.9, 2]];
const groupOpacity = (i: number, p: number) => {
  const [a, b] = groupRange[i];
  return seg(p, a, a + 0.04) * (1 - seg(p, b - 0.04, b));
};

export function Hero({ treatment, scene: realScene, replayUrl, portwatch, demo }: Props) {
  const view = useView();
  const section = useRef<HTMLElement>(null);
  const stageEl = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const plate = useRef<HTMLDivElement>(null);
  const groups = useRef<(HTMLDivElement | null)[]>([]);
  const book = useRef<HTMLDivElement>(null);
  const table = useRef<HTMLTableElement>(null);
  const figs = useRef<HTMLDivElement>(null);
  const prows = useRef<HTMLDivElement>(null);
  const captionEl = useRef<HTMLParagraphElement>(null);
  const [active, setActive] = useState(0);
  const [layout, setLayout] = useState<Layout | null>(null);
  const [replay, setReplay] = useState<Replay | null>(null);
  const [rows, setRows] = useState<Row[] | "error" | null>(null);
  const [reduced, setReduced] = useState(false);
  const [bookOn, setBookOn] = useState(false);
  const scene = realScene ?? placeholderScene();

  useEffect(() => {
    const m = window.matchMedia("(prefers-reduced-motion: reduce)");
    const on = () => setReduced(m.matches);
    on();
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  }, []);

  // after first paint: the recording and the demo account's records (the LCP never waits for either)
  useEffect(() => {
    let live = true;
    const start = () => {
      if (replayUrl)
        fetch(replayUrl)
          .then((r) => (r.ok ? (r.json() as Promise<Replay>) : null))
          .then((r) => live && setReplay(r), () => live && setReplay(null));
      if (demo)
        Promise.all([fetchInbox(demo), fetchDemo().catch(() => null)]).then(
          ([i, d]) => live && setRows(rowsOf(i, d?.vendor)),
          () => live && setRows("error"),
        );
      else setRows("error");
    };
    const id = window.requestIdleCallback ? window.requestIdleCallback(start, { timeout: 1200 }) : window.setTimeout(start, 300);
    return () => {
      live = false;
      if (window.cancelIdleCallback) window.cancelIdleCallback(id);
      else window.clearTimeout(id);
    };
  }, [replayUrl, demo]);

  // the stage: layout from the plate's real box, a canvas frame per scroll frame
  useEffect(() => {
    const sec = section.current, st = stageEl.current, cv = canvas.current, pl = plate.current;
    if (!sec || !st || !cv || !pl) return;
    const stage = createStage(cv);
    let source: CanvasImageSource | null = null;
    let monoFamily = "monospace";
    let raf = 0;
    let L: Layout | null = null;

    const measure = () => {
      // full bleed: the class places it for a page without a classic scrollbar; this corrects for one (100vw counts it)
      const parent = sec.parentElement!;
      const left = parent.getBoundingClientRect().left + parseFloat(getComputedStyle(parent).paddingLeft || "0");
      sec.style.width = `${document.documentElement.clientWidth}px`;
      sec.style.marginLeft = `${-left}px`;
      const W = st.clientWidth, H = st.clientHeight;
      const b = pl.getBoundingClientRect(), s = st.getBoundingClientRect();
      const plateRect = { x: b.left - s.left, y: b.top - s.top, w: b.width, h: b.height };
      const wide = W >= 640;
      // the book's first column: right of the plate on a module column; on a phone two 8 px columns in, under the caption
      const x = wide ? Math.ceil((plateRect.x + plateRect.w + 32) / MODULE) * MODULE : 2 * COLUMN;
      const y = wide ? Math.round((H * 0.28) / MODULE) * MODULE : 3 * MODULE;
      L = { W, H, plate: plateRect, rows: { x, y } };
      stage.resize(L);
      setLayout(L);
    };
    const frame = () => {
      raf = 0;
      const r = sec.getBoundingClientRect();
      const raw = clamp01(-r.top / Math.max(1, r.height - window.innerHeight));
      const p = reduced ? stillAt(raw) : raw;
      const ph = phases(p);
      const set = (el: HTMLElement | null, o: number) => el && (el.style.opacity = String(o));
      set(table.current, seg(ph.hand, 0.5, 1));
      set(book.current, ph.book);
      set(captionEl.current, 1 - ph.book); // the book names itself; the caption leaves with the strait
      set(figs.current, 1 - seg(p, 0.26, 0.34));
      set(prows.current, seg(p, 0.3, 0.38) * (1 - seg(ph.hand, 0, 0.5)));
      prows.current?.querySelectorAll<HTMLElement>("[data-shift]").forEach((r) => {
        r.style.transform = `translateY(${Number(r.dataset.shift) * seg(p, 0.48, 0.6)}px)`;
      });
      groups.current.forEach((g, i) => set(g, groupOpacity(i, p)));
      setActive(groupRange.findIndex(([a, b]) => p >= a && p < b));
      if (p >= 0.7) setBookOn(true);
      if (treatment !== "C") stage.draw(p, { treatment, scene, source, replay, view, monoFont: monoFamily });
      else stage.draw(p, { treatment: "B", scene, source: null, replay: null, view, monoFont: monoFamily, ruled: true });
      cv.style.opacity = String(1 - ph.book);
    };
    const kick = () => {
      if (!raf) raf = requestAnimationFrame(frame);
    };
    measure();
    kick();
    let cancelled = false;
    const start = async () => {
      monoFamily = getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim() || "monospace";
      // the labels' face loads beside the picture, never before it; once it is in, labels are measured again
      document.fonts.load(`400 13px ${monoFamily}`).then(() => {
        if (!cancelled && L) stage.resize(L);
        kick();
      }, () => null);
      if (treatment !== "C" && !realScene?.src) {
        const coast = await fetch("/hero/coastline-hormuz.geojson").then((r) => r.json());
        if (cancelled) return;
        source = treatment === "A" ? drawPlaceholder(scene, coast) : drawChart(scene, coast, view);
      } else if (treatment === "A" && realScene?.src) {
        const img = st.querySelector<HTMLImageElement>("img[data-scene]");
        if (img) {
          await img.decode().catch(() => null);
          source = img;
        }
      }
      st.dataset.ready = "1";
      kick();
    };
    const id = window.setTimeout(start, 0);
    const ro = new ResizeObserver(() => {
      measure();
      kick();
    });
    ro.observe(st);
    const onResize = () => {
      measure();
      kick();
    };
    window.addEventListener("scroll", kick, { passive: true });
    window.addEventListener("resize", onResize);
    return () => {
      cancelled = true;
      window.clearTimeout(id);
      ro.disconnect();
      window.removeEventListener("scroll", kick);
      window.removeEventListener("resize", onResize);
      if (raf) cancelAnimationFrame(raf);
    };
    // scene is derived from realScene; a new placeholder object each render must not restart the stage
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [treatment, realScene, replay, view, reduced]);

  const wide = (layout?.W ?? 1280) >= 640;
  const cols = wide ? COLS_WIDE : COLS_NARROW;
  const grid = { gridTemplateColumns: cols.filter(Boolean).map((c) => `${c}px`).join(" ") };
  const win = replay ? windowWords(replay) : null;
  const latest = portwatch?.days[0] ?? null;
  const caption = [
    treatment === "A" ? (realScene?.date ? CAPTION.sceneA(realScene.date) : CAPTION.placeholderA) : treatment === "B" ? CAPTION.sceneB : latest ? CAPTION.sceneC(latest.date) : "",
    !replay ? CAPTION.aisNone : replay.synthetic ? CAPTION.aisSynthetic : CAPTION.ais(win!.from, win!.to),
    CAPTION.rows,
  ].join(" ");
  const cell = (r: Row) => [r.date, r.who, r.amount, r.why, r.state].filter((_, i) => cols[i] > 0);
  const portRows = latest
    ? (Object.keys(FIGURES.types) as (keyof typeof FIGURES.types)[]).map((k) => ({ type: FIGURES.types[k], n: latest[k] }))
    : [];
  const sortedIndex = (i: number) => [...portRows].map((r, j) => ({ ...r, j })).sort((a, b) => b.n - a.n || a.j - b.j).findIndex((r) => r.j === i);

  return (
    <section ref={section} data-hero aria-label={HERO.canvasLabel} className="relative -mt-(--space-5) ml-[calc(50%-50vw)] w-screen" style={{ height: reduced ? `${STILLS.length * 100}svh` : "500svh" }}>
      <div ref={stageEl} className="sticky top-0 h-svh w-full overflow-hidden" data-treatment={treatment}>
        {treatment === "A" && realScene?.src ? (
          // eslint-disable-next-line @next/next/no-img-element -- the scene is the LCP image, sized and placed by the stage
          <img data-scene src={realScene.src} alt="" fetchPriority="high" className="absolute inset-0 h-full w-full object-cover" />
        ) : null}
        <canvas ref={canvas} aria-hidden className="absolute inset-0 h-full w-full" />

        {treatment === "C" && latest && layout ? (
          <div ref={figs} className="absolute px-(--space-2)" style={{ left: layout.rows.x, top: wide ? layout.rows.y - 2 * MODULE : 2 * MODULE }}>
            <p className="hero-figure m-0">{latest.total}</p>
            <p className="meta m-0 mb-(--space-5)">{FIGURES.transits(latest.date)}</p>
            {replay ? (
              <>
                <p className="hero-figure m-0">{replay.vessels.length}</p>
                <p className="meta m-0">{FIGURES.window(replay.synthetic)}</p>
              </>
            ) : null}
          </div>
        ) : null}

        {layout ? (
          <div className="absolute" style={{ left: layout.rows.x, top: layout.rows.y - MODULE }}>
            {treatment === "C" ? (
              <div ref={prows} className="absolute left-0 top-0" style={{ opacity: 0 }}>
                {portRows.map((r, i) => (
                  <div key={r.type} data-shift={(sortedIndex(i) - i) * MODULE} className="row money absolute grid h-10 items-center" style={{ ...grid, top: MODULE * (1 + i) }}>
                    {[latest!.date.slice(5), r.type, String(r.n), wide ? "IMF PortWatch" : null, "transits"].filter((x) => x !== null).map((c, j) => (
                      <span key={j} className="truncate px-(--space-2)">{c}</span>
                    ))}
                  </div>
                ))}
              </div>
            ) : null}
            <table ref={table} className="border-collapse" style={{ opacity: 0 }}>
              <thead>
                <tr className="meta grid h-10 items-center text-left" style={grid}>
                  {HERO.columns.filter((_, i) => cols[i] > 0).map((c) => (
                    <th key={c} scope="col" className="truncate px-(--space-2) font-normal">{c}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rows === null ? null : rows === "error" || rows.length === 0 ? (
                  <tr className="row grid h-10 items-center" style={grid}>
                    <td className="px-(--space-2) whitespace-nowrap">{rows === "error" ? HERO.unreadBook : HERO.emptyBook}</td>
                  </tr>
                ) : (
                  rows.map((r, i) => (
                    <tr key={i} className="row money grid h-10 items-center" style={grid}>
                      {cell(r).map((c, j) => (
                        <td key={j} className="truncate px-(--space-2)">{c}</td>
                      ))}
                    </tr>
                  ))
                )}
              </tbody>
            </table>
          </div>
        ) : null}

        <div ref={book} className="absolute inset-0 bg-(--paper) px-(--gutter-narrow) pt-(--space-6) sm:px-(--gutter-wide)" style={{ opacity: 0, pointerEvents: active === 4 ? "auto" : "none" }} inert={active !== 4}>
          <div className="mx-auto max-w-(--content-max)">
            <p className="meta m-0 mb-(--space-4)">{HERO.bookLabel}</p>
            {demo && bookOn ? <OwnerHome name={demo} /> : <OwnerSkeleton />}
          </div>
        </div>

        {/* the plate: its box is where text may sit (labels keep clear of it); each stage's words carry their own paper */}
        <div ref={plate} className="absolute inset-x-(--gutter-narrow) bottom-(--gutter-narrow) sm:inset-x-auto sm:bottom-auto sm:left-(--gutter-wide) sm:top-1/2 sm:w-[480px] sm:-translate-y-1/2">
          {[0, 1, 2, 3].map((i) => (
            <div
              key={i}
              ref={(el) => {
                groups.current[i] = el;
              }}
              inert={active !== i}
              className={`rounded-(--radius-plate) bg-(--paper) p-(--space-5) sm:p-(--space-6) ${i === 0 ? "relative" : "absolute inset-x-0 bottom-0 sm:bottom-auto sm:top-0"}`}
              style={{ opacity: i === 0 ? 1 : 0 }}
            >
              {i === 0 ? (
                <>
                  <h1 className="hero-statement m-0">{HERO.statement}</h1>
                  <p className="row m-0 mt-(--space-4) max-w-(--measure) text-(--ink-2)">{HERO.body}</p>
                  <div className="mt-(--space-5) flex flex-col gap-(--space-3) sm:flex-row">
                    <a href="#guest" className="row-title inline-flex h-(--button) items-center justify-center rounded-(--radius-button) bg-(--ink) px-(--space-5) text-(--paper) no-underline">{HERO.open}</a>
                    <a href="/new" className="row-title inline-flex h-(--button) items-center justify-center rounded-(--radius-button) bg-(--plate-2) px-(--space-5) text-(--ink) no-underline">{HERO.tryIt}</a>
                  </div>
                </>
              ) : (
                <p className="hero-statement m-0">{HERO.stages[i - 1]}</p>
              )}
            </div>
          ))}
        </div>

        <p ref={captionEl} className="meta absolute left-(--gutter-narrow) right-(--gutter-narrow) top-(--space-3) m-0 rounded-(--radius-mark) bg-(--paper) px-(--space-2) py-(--space-1) sm:left-auto sm:bottom-(--space-4) sm:right-(--gutter-wide) sm:top-auto sm:max-w-[60ch]">{caption}</p>
      </div>
    </section>
  );
}
