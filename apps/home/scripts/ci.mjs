// Lane preflight (seat H6): the gate for apps/home, and the evidence for it. Static steps first (tokens.css against DESIGN.md,
// scripts/contrast.mjs, the zero-hash test, colour literals, the hex lint and `21st review` once they exist), then every
// route x state x view x width in a headless Chrome: axe-core, DESIGN's rules (founder, 2026-10-18 07:01) read from computed
// styles, the focus ring, reduced motion. The numbers it checks against are SPEC below. Needs the app running
// (APP_URL, default http://localhost:3010; the lane service's origin check wants "localhost", not 127.0.0.1).
//
//   npm run ci                                  the gate; exits 1 on any FAIL
//   npm run ci -- --shots --log                 also write proof/home-2026-10-18/<route>-<state>-<view>-<width>.png and append preflight.txt
//   npm run ci -- --route account --state loading,error --view owner --width 390
//   npm run ci -- --no-browser                  the static steps only
//   npm run ci -- --dry                         list the matrix, run nothing
//   npm run ci -- --fast                        use the Owner home's ?state= flag for loading, empty, error, offline (the real offline path waits ~20 s)
//   npm run ci -- --strict                      a feature that is not built yet (no sheet, no account) counts as FAIL
//
// env: POLICY            an account with paid rows, by NAME (routes are /a/<name>); default northwind-agents, the lane service's
//      POLICY_EMPTY      an account with no bills        } without these the service reply is patched in the page and the
//      POLICY_NEEDS      an account with a waiting item  } log says "mocked"; the screenshot is then a fixture, not a real account
//      SHOTS_DIR         where the PNGs go (default PROOF_DIR): a seat's tree before it is merged has its own folder
//      PROOF_DIR         default <main checkout>/proof/home-2026-10-18      PW_CHANNEL  default chrome (PW_CHANNEL= for bundled chromium)
//      APP_TREE          where the server runs, when lsof cannot say (only for the commit line in the log)
//      APP_DIR           check another tree (a seat's apps/home) instead of this one: its tokens, contrast, test, literals
//
// Read-only: it loads pages, flips the capsule, presses Tab, opens a row. It never presses Stop, Approve, Decline or Raise.
import { execFileSync, spawnSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const APP = process.env.APP_DIR ? resolve(process.env.APP_DIR) : resolve(dirname(fileURLToPath(import.meta.url)), ".."); // APP_DIR: run the gate on another seat's tree

// ---------------------------------------------------------------------------------------------------------------- pure helpers
export const HASH = /0x[0-9a-fA-F]{8,}/;
export const VIEW_KEY = "tameion.home.view";
export const HEIGHT = { 1280: 800, 390: 844 };
export const ROUTES = {
  guest: { path: () => "/", states: ["rest", "loading", "error", "offline"] },
  // rest is the owner's browser (the passkey's public half seeded, nothing signed); guest is the same page read-only
  account: { path: (p) => `/a/${p}`, states: ["rest", "guest", "loading", "empty", "error", "offline", "needs-you", "sealed", "detail"] },
  new: { path: () => "/new", states: ["rest", "error"] },
  add: { path: (p) => `/a/${p}/add`, states: ["rest", "loading", "error", "offline"] },
  receive: { path: (p) => `/a/${p}/receive`, states: ["rest", "loading", "error", "offline"] },
};
const CORE = ["paper", "plate", "plate-2", "ink", "ink-2", "rule", "attention", "transit"];
// The numbers the checks use, from DESIGN.md (founder, 2026-10-18 07:01). When the founder changes a value, change it here.
export const SPEC = {
  contentMax: 1120, gutter: { narrow: 20, wide: 32 }, touch: 44, button: 52, buttonCompact: 44, buttonRadius: 12,
  authority: [31, 38], heroStatement: [49, 56], heroNarrow: [31, 38], heroNarrowUnder: 640,
  mono: [[13, 20], [15, 22]], weights: { sans: [400, 500, 600], mono: [400, 500] }, drawer: 400, crossfadeMs: 200, skeletonMinMs: 300,
};

export function parseArgs(argv) {
  const o = {};
  for (let i = 0; i < argv.length; i++) {
    const m = /^--([\w-]+)(?:=(.*))?$/.exec(argv[i]);
    if (!m) continue;
    if (m[2] !== undefined) o[m[1]] = m[2];
    else if (argv[i + 1] && !argv[i + 1].startsWith("--")) o[m[1]] = argv[++i];
    else o[m[1]] = true;
  }
  return o;
}

/** "#abc" | "#AABBCC" | "rgb(1, 2, 3)" -> "#AABBCC"; anything else -> null. */
export function hexNorm(v) {
  if (typeof v !== "string") return null;
  const s = v.trim();
  let m = /^#([0-9a-f]{3})$/i.exec(s);
  if (m) return "#" + [...m[1]].map((c) => c + c).join("").toUpperCase();
  m = /^#([0-9a-f]{6}|[0-9a-f]{8})$/i.exec(s);
  if (m) return "#" + m[1].toUpperCase();
  m = /^rgb\(\s*(\d+)[,\s]+(\d+)[,\s]+(\d+)\s*\)$/.exec(s);
  if (m) return "#" + [m[1], m[2], m[3]].map((x) => (+x).toString(16).padStart(2, "0")).join("").toUpperCase();
  return null;
}

export function contrast(a, b) {
  const lum = (h) => {
    const n = parseInt(h.slice(1), 16);
    return [n >> 16, (n >> 8) & 255, n & 255].map((c) => c / 255).map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)).reduce((s, c, i) => s + c * [0.2126, 0.7152, 0.0722][i], 0);
  };
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** DESIGN.md's palette table -> { owner: {"--paper": "#EEF2EA", ...}, agent: {...} }. A "same" cell takes the Owner value. */
export function designPalette(md) {
  const out = { owner: {}, agent: {} };
  for (const line of md.split("\n")) {
    if (!/^\|/.test(line)) continue;
    const cells = line.split("|").slice(1, -1).map((c) => c.trim());
    if (cells.length < 3 || !/^[`\w-]/.test(cells[0]) || /^[-: ]+$/.test(cells[0])) continue;
    // a name cell is "paper", "`--paper`", "up / down"; the value cells hold hex in backticks
    const names = cells[0].replace(/`/g, "").split("/").map((n) => n.trim()).filter(Boolean).map((n) => "--" + n.replace(/^--/, "").replace(/\s+/g, "-"));
    const HEX = /#[0-9A-Fa-f]{8}\b|#[0-9A-Fa-f]{6}\b/g;
    const own = cells[1]?.match(HEX) ?? [];
    const night = /same/i.test(cells[2] ?? "") ? own : (cells[2]?.match(HEX) ?? []);
    names.forEach((n, i) => {
      if (own[i]) out.owner[n] = own[i].toUpperCase();
      if (night[i]) out.agent[n] = night[i].toUpperCase();
    });
  }
  return out;
}

/** tokens.css -> { owner: {"--paper": "#EEF2EA", ...}, agent: {...} }; a plain :root block counts for Owner. */
export function cssTokens(css) {
  const out = { owner: {}, agent: {} };
  for (const m of css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const view = /data-view=["']?agent/.test(m[1]) ? "agent" : /data-view=["']?owner|:root/.test(m[1]) ? "owner" : null;
    if (!view) continue;
    for (const d of m[2].matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) out[view][d[1]] = d[2].trim();
  }
  return out;
}

export const shotName = (sc) => `${sc.route}-${sc.state}-${sc.view}-${sc.width}.png`;

const list = (v, all) => (v === undefined || v === true ? all : String(v).split(",").map((s) => s.trim()).filter(Boolean));
export function matrix(args, have = { policy: true }) {
  const out = [];
  for (const route of list(args.route, Object.keys(ROUTES)))
    for (const state of list(args.state, ROUTES[route]?.states ?? []))
      for (const view of list(args.view, ["owner", "agent"]))
        for (const width of list(args.width, ["1280", "390"]).map(Number)) {
          if (!ROUTES[route]?.states.includes(state) || (state === "guest" && view === "agent")) continue;
          out.push({ route, state, view, width, needsPolicy: route !== "guest" && route !== "new", skip: route !== "guest" && route !== "new" && !have.policy });
        }
  return out;
}

/** Findings are keyed so two runs can be compared: what is new since the last run, what was fixed. */
export const keyOf = (name, f) => [name, f.rule, f.sel, f.detail].join("|");
export function diffRuns(prev, cur) {
  const a = new Set(prev), b = new Set(cur);
  return { added: [...b].filter((k) => !a.has(k)), fixed: [...a].filter((k) => !b.has(k)) };
}

// ---------------------------------------------------------------------------------------------------------------- static steps
function stepTokens() {
  const css = join(APP, "src/tokens.css"), md = join(APP, "DESIGN.md");
  if (!existsSync(css)) return { name: "tokens", sev: "fail", lines: ["src/tokens.css is missing"] };
  if (!existsSync(md)) return { name: "tokens", sev: "note", lines: ["DESIGN.md is not in this tree; token values were not compared with it"] };
  const want = designPalette(readFileSync(md, "utf8")), have = cssTokens(readFileSync(css, "utf8"));
  const lines = [];
  let sev = "pass";
  for (const view of ["owner", "agent"])
    for (const [name, hex] of Object.entries(want[view])) {
      const raw = have[view][name], got = hexNorm(raw);
      if (got === hex) continue;
      const core = CORE.includes(name.slice(2));
      // --up / --down: DESIGN says "same" at night, which reads 2.3 to 2.5 : 1 there; a lifted value that passes is the right call.
      const lifted = !core && got && view === "agent" && contrast(hex, want.agent["--paper"]) < 4.5 && contrast(got, have.agent["--paper"] ? hexNorm(have.agent["--paper"]) : want.agent["--paper"]) >= 4.5;
      lines.push(`${view} ${name}: DESIGN ${hex}, tokens.css ${raw ?? "missing"}${lifted ? " (deliberate: DESIGN's value fails 4.5:1 on night paper)" : ""}`);
      if (lifted) continue;
      if (core || raw === undefined) sev = "fail";
      else if (sev !== "fail") sev = "warn";
    }
  return { name: "tokens", sev, lines };
}

function run(name, args, missing) {
  if (!existsSync(join(APP, args.find((a) => !a.startsWith("-")) ?? ""))) return { name, sev: "fail", lines: [missing] };
  const r = spawnSync(process.execPath, args, { cwd: APP, encoding: "utf8" });
  const out = `${r.stdout ?? ""}${r.stderr ?? ""}`.trim().split("\n");
  if (r.status === 0) return { name, sev: "pass", lines: [out.filter((l) => /^(ℹ (tests|pass|fail)|contrast\/tokens)/.test(l)).join("  ")].filter(Boolean) };
  return { name, sev: "fail", lines: out.filter((l) => l.trim() && !/^ℹ /.test(l)).slice(-12) };
}

function stepZeroHash() {
  const pkg = JSON.parse(readFileSync(join(APP, "package.json"), "utf8"));
  if (pkg.scripts?.["test:zero-hash"]) {
    const r = spawnSync("npm", ["run", "--silent", "test:zero-hash"], { cwd: APP, encoding: "utf8" });
    const out = `${r.stdout ?? ""}${r.stderr ?? ""}`.trim().split("\n");
    return { name: "zero-hash", sev: r.status === 0 ? "pass" : "fail", lines: r.status === 0 ? [out.filter((l) => /^ℹ (tests|pass|fail)/.test(l)).join("  ")] : out.filter((l) => l.trim() && !/^ℹ /.test(l)).slice(-12) };
  }
  return run("zero-hash", ["--test", "test/owner-no-hash.test.mts"], "test/owner-no-hash.test.mts is missing (H2: DESIGN Rule 1, a seeded Owner page with no /0x[0-9a-fA-F]{8,}/ in the DOM)");
}

// Steps another seat builds, wired in as soon as the npm script exists (DESIGN §Acceptance: the tokens-only hex lint, `21st review` with zero errors).
function stepScripts(name, keys, missing, strict) {
  const pkg = JSON.parse(readFileSync(join(APP, "package.json"), "utf8"));
  const key = keys.find((k) => pkg.scripts?.[k]);
  if (!key) return { name, sev: strict ? "fail" : "note", lines: [missing] };
  const r = spawnSync("npm", ["run", "--silent", key], { cwd: APP, encoding: "utf8" });
  const out = `${r.stdout ?? ""}${r.stderr ?? ""}`.trim().split("\n").filter((l) => l.trim());
  return { name, sev: r.status === 0 ? "pass" : "fail", lines: r.status === 0 ? out.slice(-2) : out.slice(-12) };
}

function stepLiterals() {
  const walk = (d) => readdirSync(d, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(join(d, e.name)) : [join(d, e.name)]));
  const hits = [];
  for (const f of walk(join(APP, "src")).filter((p) => /\.(tsx?|css)$/.test(p) && !/tokens\.(css|ts)$/.test(p)))
    readFileSync(f, "utf8").split("\n").forEach((line, i) => {
      if (/^\s*(\/\/|\*|\/\*)/.test(line)) return;
      const m = /(?<=[\s:"'(,])#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})\b|\b(?:rgba?|hsla?|oklch|oklab)\(/.exec(line);
      if (m) hits.push(`${relative(APP, f)}:${i + 1}  ${m[0]}`);
    });
  return { name: "literals", sev: hits.length ? "warn" : "pass", lines: hits.slice(0, 10).concat(hits.length > 10 ? [`... ${hits.length - 10} more`] : []) };
}

// ---------------------------------------------------------------------------------------------------------------- in the page
// These run inside the browser: they close over nothing from this module, only over window.__h6 (install) and their argument.
function install({ key, view, pub }) {
  try {
    if (localStorage.getItem(key) === null) localStorage.setItem(key, view);
    if (pub && localStorage.getItem("tameion.pub") === null) localStorage.setItem("tameion.pub", JSON.stringify(pub));
  } catch {
    // storage blocked: the page will show Owner
  }
  const h = (window.__h6 = { skel: [] });
  h.vis = (el) => {
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height) return false;
    const s = getComputedStyle(el);
    return s.visibility !== "hidden" && s.display !== "none";
  };
  h.own = (el) => [...el.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join(" ").replace(/\s+/g, " ").trim();
  h.sel = (el) => {
    const p = [];
    for (let e = el, n = 0; e && e.nodeType === 1 && n < 3; e = e.parentElement, n++) {
      let s = e.tagName.toLowerCase();
      if (e.id) { p.unshift(`${s}#${e.id}`); break; }
      const c = [...e.classList].slice(0, 2).join(".");
      if (c) s += "." + c;
      p.unshift(s);
    }
    return p.join(" > ");
  };
  h.capsule = (label) => [...document.querySelectorAll("header button, header [role=tab], header [role=radio], header a")].find((e) => e.textContent.trim().toLowerCase() === label.toLowerCase() && h.vis(e));
  // a skeleton, once shown, stays >= 300 ms (DESIGN §States): record each stretch it was on screen
  const SK = '[aria-busy="true"],[data-skeleton],.skeleton,[class*="skeleton"]';
  let on = null;
  const tick = () => {
    const now = !!document.querySelector(SK);
    if (now && on === null) on = performance.now();
    if (!now && on !== null) { h.skel.push({ at: Math.round(on), ms: Math.round(performance.now() - on) }); on = null; }
  };
  new MutationObserver(tick).observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ["aria-busy", "class", "data-skeleton"] });
  document.addEventListener("DOMContentLoaded", tick);
}

/** DESIGN's rules (founder, 2026-10-18 07:01), read from the rendered page. Returns { findings, metrics }. spec is SPEC from this file. */
function domRules({ view, width, route, palette, spec }) {
  const h = window.__h6;
  const F = [];
  const add = (sev, rule, el, detail) => F.push({ sev, rule, sel: el ? h.sel(el) : "", text: el ? (el.innerText || el.textContent || "").replace(/\s+/g, " ").trim().slice(0, 50) : "", detail });
  const SKIP = /^(SCRIPT|STYLE|NOSCRIPT|TEMPLATE|NEXTJS-PORTAL|NEXT-ROUTE-ANNOUNCER)$/;
  const all = [...document.body.querySelectorAll("*")].filter((e) => !SKIP.test(e.tagName) && !e.closest("nextjs-portal") && h.vis(e));
  const texts = all.filter((e) => h.own(e));
  const st = (e) => getComputedStyle(e);
  const px = (v) => parseFloat(v);
  const isSheet = (e) => !!e.closest("dialog, [role=dialog], [data-sheet]");
  const near = (a, b, t = 0.6) => Math.abs(a - b) <= t;
  const metrics = {};
  metrics.busy = !!document.querySelector('[aria-busy="true"]'); // the page is a skeleton: no hero yet
  const tok = (n) => (palette[`--${n}`] ?? "").toUpperCase();

  // Hashes live in the Agent view only: none in the Owner DOM (text and every attribute, hrefs included), none in any route
  if (view === "owner") {
    const HASH = /0x[0-9a-fA-F]{8,}/;
    const t = document.body.innerText.match(HASH);
    if (t) add("fail", "zero-hash", null, `text: ${t[0].slice(0, 14)}...`);
    for (const e of document.body.querySelectorAll("*")) {
      if (SKIP.test(e.tagName)) continue;
      for (const a of e.attributes) if (HASH.test(a.value)) add("fail", "zero-hash", e, `${a.name}="${a.value.slice(0, 40)}"`);
    }
    for (const e of document.head.querySelectorAll("link[href],meta[content]")) {
      const v = e.getAttribute("href") ?? e.getAttribute("content");
      if (HASH.test(v)) add("warn", "zero-hash-head", e, v.slice(0, 40));
    }
  }
  // routes are by account name (ruled 2026-10-18 06:32): the 0x address lives in API calls, never in the URL
  if (/0x[0-9a-fA-F]{8,}/.test(location.pathname)) add("fail", "route-has-hash", null, `the route ${location.pathname.slice(0, 30)}... carries a 0x address; routes are /a/<name>`);

  // Typography: IBM Plex Sans in Owner, Plex Mono (the machine's eye) in Agent; only the weights DESIGN names; the roles' sizes
  const mono = (e) => /mono|courier|consolas|menlo|monospace/i.test(st(e).fontFamily);
  if (view === "owner") for (const e of texts) if (mono(e)) add("fail", "owner-mono", e, st(e).fontFamily.slice(0, 50));
  if (view === "agent") {
    const non = texts.filter((e) => !mono(e));
    if (non.length) add("warn", "agent-not-mono", non[0], `${non.length} text elements are not Plex Mono`);
    const off = texts.filter((e) => !spec.mono.some(([s, l]) => near(px(st(e).fontSize), s, 0.3) && near(px(st(e).lineHeight), l)));
    if (off.length) add("warn", "agent-size", off[0], `${off.length} text elements are neither monoDense 13/20 nor monoRead 15/22 (first: ${st(off[0]).fontSize}/${st(off[0]).lineHeight})`);
  }
  const okW = view === "agent" ? spec.weights.mono : spec.weights.sans;
  const badW = texts.filter((e) => !okW.includes(+st(e).fontWeight));
  if (badW.length) add("fail", "font-weight", badW[0], `${badW.length} text elements use weights outside ${okW.join("/")} (first: ${st(badW[0]).fontWeight})`);
  const loaded = [...document.fonts].filter((f) => f.status === "loaded").map((f) => f.family);
  const want = view === "agent" ? /plex[_ ]?mono/i : /plex[_ ]?sans/i;
  if (!loaded.some((f) => want.test(f))) add("fail", "font-not-loaded", null, `${view === "agent" ? "IBM Plex Mono" : "IBM Plex Sans"} is not loaded (loaded: ${[...new Set(loaded)].join(", ").slice(0, 80) || "none"}); the page is in a fallback face`);
  const h1s = all.filter((e) => e.tagName === "H1");
  if (!metrics.busy && h1s.length !== 1) add("fail", "one-hero", h1s[0] ?? null, `${h1s.length} h1 on the page (DESIGN: one hero per surface)`);
  if (h1s[0]) {
    const s = st(h1s[0]);
    metrics.h1 = `${s.fontSize}/${s.lineHeight}/${s.fontWeight}`;
    const fits = (pair) => near(px(s.fontSize), pair[0], 0.6) && near(px(s.lineHeight), pair[1], 0.6);
    if (route === "account" && view === "owner") {
      if (!fits(spec.authority)) add("fail", "type-authority", h1s[0], `h1 is ${metrics.h1}; the Owner hero is the authority role ${spec.authority.join("/")}`);
      if (!/\s/.test(h1s[0].innerText) || /^[\d.,\s]*(USDC)?$/.test(h1s[0].innerText.trim())) add("warn", "hero-not-a-sentence", h1s[0], "the Owner hero is a sentence, not a number");
    }
    if (route === "guest" && !fits(width < spec.heroNarrowUnder ? spec.heroNarrow : spec.heroStatement)) add("warn", "type-heroStatement", h1s[0], `h1 is ${metrics.h1}; the landing hero is heroStatement ${spec.heroStatement.join("/")} (${spec.heroNarrow.join("/")} under ${spec.heroNarrowUnder} px)`);
  }
  // one accented word inside a headline is banned
  for (const hd of all.filter((e) => /^H[1-3]$/.test(e.tagName))) {
    const hs = st(hd);
    for (const c of hd.querySelectorAll("*")) {
      if (!h.own(c)) continue;
      const cs = st(c);
      if (cs.color !== hs.color || cs.fontWeight !== hs.fontWeight || cs.fontStyle !== hs.fontStyle) { add("fail", "accented-word", c, `"${h.own(c).slice(0, 20)}" differs from its headline in colour, weight or style`); break; }
    }
  }
  // measure <= 68 ch: average characters per line (a 68ch line holds about 75 average characters)
  for (const e of all.filter((x) => /^(P|LI|H1|H2|H3)$/.test(x.tagName))) {
    const t = (e.innerText || "").replace(/\s+/g, " ").trim(), lh = px(st(e).lineHeight);
    if (t.length < 150 || !(lh > 0)) continue;
    const lines = Math.max(1, Math.round(e.getBoundingClientRect().height / lh));
    if (t.length / lines > 80) add("warn", "measure", e, `about ${Math.round(t.length / lines)} characters per line (DESIGN: measure <= 68 ch)`);
  }

  // Banned list and copy rules, element by element
  const money = /(\d+(?:\.\d+)?)\s*USDC/g, clock = /\b\d{1,2}:\d{2}\b/;
  for (const e of texts) {
    const own = h.own(e), s = st(e);
    const w = /\b(submit|soon|quote only|guarantee[ds]?|locked|yield|delta-neutral)\b/i.exec(own);
    if (w) add("fail", "banned-word", e, `"${w[0]}"`);
    if (/\$\s?\d/.test(own)) add("fail", "no-dollar-sign", e, "money is 0.05 USDC, never $");
    if (/\s·\s/.test(own)) add("fail", "middle-dot-meta", e, "meta strings joined with middle dots are banned");
    if (s.textTransform === "uppercase") add("fail", "tracked-uppercase", e, "text-transform: uppercase");
    else if (/^[A-Z][A-Z0-9 ]{3,}$/.test(own) && !/^(USDC|EURC|USD|ERC|CCTP|MCP|API|UTC|HTTP|AIS|MMSI)$/.test(own)) add("warn", "all-caps", e, `"${own.slice(0, 20)}"`);
    if (view === "owner" && (/\b\d{4}-\d{2}-\d{2}\b|T\d{2}:\d{2}:\d{2}|\b\d{1,2}\/\d{1,2}\/\d{2,4}\b/.test(own))) add("warn", "date-not-in-words", e, "dates read Today 05:41, Yesterday, 12 Oct");
    for (const m of own.matchAll(money)) {
      const frac = (m[1].split(".")[1] ?? "");
      if (frac.length > 6 || /0$/.test(frac)) add("warn", "money-not-shortest", e, `"${m[0]}": six decimals trimmed to the shortest that is exact`);
    }
    if ((own.match(money) || clock.test(own)) && !/tabular-nums/.test(s.fontVariantNumeric) && !mono(e)) add("fail", "figures-not-tabular", e, `"${(own.match(money)?.[0] ?? own.match(clock)[0])}": tabular lining figures wherever money or time appears`);
  }
  for (const hd of all.filter((e) => /^H[1-6]$/.test(e.tagName) || e.getAttribute("role") === "heading")) if (/\s[—–]\s/.test(hd.textContent)) add("fail", "word-dash-heading", hd, "no WORD — fragment headings");
  for (const b of all.filter((e) => e.matches("button, a[href], [role=button]"))) {
    if (/[→←↗↘↑↓➔➜➝⟶]/.test(b.textContent) || b.querySelector('svg[class*="arrow" i], svg[data-lucide*="arrow" i]')) add("fail", "arrow-on-button", b, "no arrows in links or buttons");
    else if (b.querySelector('svg[class*="chevron" i], svg[class*="caret" i]')) add("warn", "chevron-on-button", b, "a chevron on a button: an arrow by another name?");
  }

  // Visual bans: gradients, glow, shadows (the sheet's own excepted), spinner pages, borders on content, radii off the scale
  const RADII = [0, 4, 8, 12, 16, 24];
  const ledgerRow = (e, sides) => (e.matches("tr, td, th, thead, tbody, tfoot") || !!e.closest("table")) || (e.matches("li, [role=listitem]") && sides.every((x) => x === "Bottom" || x === "Top"));
  const radii = new Map();
  for (const e of all) {
    const s = st(e);
    if (/gradient/.test(s.backgroundImage)) add("fail", "gradient", e, s.backgroundImage.slice(0, 50));
    if (s.boxShadow !== "none" && !isSheet(e)) add("fail", "shadow", e, s.boxShadow.slice(0, 50));
    if (s.textShadow !== "none") add("fail", "glow", e, s.textShadow.slice(0, 50));
    if (/blur|drop-shadow/.test(s.filter) || /blur/.test(s.backdropFilter ?? "")) add("fail", "glow", e, `filter ${s.filter}`);
    if (/spin/i.test(s.animationName) || (e.getAttribute("role") === "progressbar" && px(s.width) > innerWidth * 0.5)) add("fail", "spinner", e, "no spinner pages");
    const sides = ["Top", "Right", "Bottom", "Left"].filter((x) => px(s[`border${x}Width`]) > 0 && s[`border${x}Style`] !== "none" && !/rgba\(\s*\d+,\s*\d+,\s*\d+,\s*0\s*\)/.test(s[`border${x}Color`]));
    const control = e.matches("button, [role=button], a[href], input, select, textarea, summary, [role=tab], [role=radio]") || !!e.closest("button, [role=button], a[href]");
    if (sides.length && !control && !ledgerRow(e, sides) && !sides.every((x) => s[`border${x}Style`] === "dashed") && !isSheet(e)) add("fail", "border-on-content", e, `${sides.join("+")} ${s.borderTopStyle} ${s.borderTopWidth}; depth is one step of fill, never a border (rule is for table rows only)`);
    const r = px(s.borderTopLeftRadius);
    if (r > 0 && r < 99 && !RADII.includes(Math.round(r))) radii.set(Math.round(r), e);
  }
  for (const [r, e] of radii) add("warn", "radius-off-scale", e, `${r}px; the scale is ${RADII.filter(Boolean).join(" / ")}`);

  // Colour: every colour on the page is a token; transit only as a mark; up and down never alone; attention only for Needs you, Stop, refused, failures
  const allowed = new Set(), allowedA = [];
  for (const x of Object.values(palette)) {
    const v = x.toUpperCase();
    if (v.length === 9) allowedA.push({ hex: v.slice(0, 7), a: parseInt(v.slice(7), 16) / 255 });
    else allowed.add(v);
  }
  const red = tok("attention") || tok("red");
  const rgb = (v) => {
    const m = /^rgba?\(\s*(\d+)[,\s]+(\d+)[,\s]+(\d+)(?:[,\s/]+([\d.]+%?))?\s*\)$/.exec(v);
    if (!m) return null;
    return { r: +m[1], g: +m[2], b: +m[3], a: m[4] === undefined ? 1 : m[4].endsWith("%") ? parseFloat(m[4]) / 100 : +m[4] };
  };
  const hex = (c) => "#" + [c.r, c.g, c.b].map((x) => x.toString(16).padStart(2, "0")).join("").toUpperCase();
  const used = new Map(), reds = new Set();
  const note = (raw, e, prop) => {
    const c = rgb(raw);
    if (c && c.a === 0) return;
    const key = c ? (c.a < 1 ? `${hex(c)}@${c.a}` : hex(c)) : raw;
    if (key === red) reds.add(e);
    if (prop === "color" && view === "owner" && key === tok("transit")) add("fail", "transit-as-text", e, "transit is for marks only, never text on light");
    if (prop === "color" && (key === tok("up") || key === tok("down")) && !/[+\-−–]|\b(up|down|in|out|more|less|received|paid|spent|refused|days?)\b/i.test(e.innerText || "")) add("warn", "direction-alone", e, "up and down colour a direction; with a sign or a word, never alone");
    const u = used.get(key) ?? { n: 0, e, prop, raw };
    u.n++;
    used.set(key, u);
  };
  for (const e of [document.documentElement, document.body, ...all]) {
    const s = st(e);
    if (e === document.body || h.own(e)) note(s.color, e, "color"); // <html> paints no text of its own
    note(s.backgroundColor, e, "background");
    for (const side of ["Top", "Right", "Bottom", "Left"]) if (px(s[`border${side}Width`]) > 0 && s[`border${side}Style`] !== "none") note(s[`border${side}Color`], e, "border");
    if (s.outlineStyle !== "none" && px(s.outlineWidth) > 0) note(s.outlineColor, e, "outline");
    if (e instanceof SVGElement) {
      if (s.fill !== "none") note(s.fill, e, "fill");
      if (s.stroke !== "none") note(s.stroke, e, "stroke");
    }
  }
  for (const [key, u] of used) {
    if (allowed.has(key)) continue;
    if (key.includes("@")) { const [hx, al] = key.split("@"); if (allowedA.some((t) => t.hex === hx && Math.abs(t.a - +al) < 0.012)) continue; }
    const c = rgb(u.raw);
    let sev = "warn", why = "a colour literal, not a token";
    if (c) {
      const mx = Math.max(c.r, c.g, c.b), mn = Math.min(c.r, c.g, c.b), l = (mx + mn) / 510;
      const sat = mx === mn ? 0 : (mx - mn) / 255 / (1 - Math.abs(2 * l - 1));
      if (sat > 0.3 && l > 0.1 && l < 0.9) { sev = "fail"; why = "a saturated colour that is no token"; }
    }
    add(sev, "off-token-colour", u.e, `${u.prop} ${key} x${u.n}: ${why}`);
  }
  for (const e of reds) {
    const around = `${e.innerText || ""} ${e.getAttribute("aria-label") || ""} ${e.parentElement?.parentElement?.innerText?.slice(0, 120) || ""}`;
    if (/^\s*[−–-]\s?\d/.test(e.innerText || "")) continue; // a negative amount is --down, which shares the red
    if (!e.closest("[data-needs-you],[data-stop],[data-refusal],[role=alert]") && !/needs you|stop|refus|declin|fail|did not answer|nothing was sent|not sent|cannot|can.t|error/i.test(around)) add("warn", "attention-outside-allowed", e, "attention red is for Needs you, Stop, refused and failures only");
  }
  metrics.reds = reds.size;

  // Layout: no sideways scroll, content max, gutter, touch targets >= 44 px, buttons 52 (44 compact)
  const de = document.documentElement;
  if (de.scrollWidth > de.clientWidth + 1) {
    const off = all.filter((e) => e.getBoundingClientRect().right > de.clientWidth + 1 && !e.closest("[style*='overflow']"))[0];
    add("fail", "h-scroll", off ?? null, `scrollWidth ${de.scrollWidth} > clientWidth ${de.clientWidth}`);
  }
  const main = document.querySelector("main");
  if (main) {
    const r = main.getBoundingClientRect();
    metrics.main = `${Math.round(r.left)}..${Math.round(r.right)} (${Math.round(r.width)} wide)`;
    if (r.width > spec.contentMax + 0.5) add("fail", "content-max", main, `main is ${Math.round(r.width)} px wide; content max is ${spec.contentMax}`);
    const pad = px(st(main).paddingLeft), want = width >= 768 ? spec.gutter.wide : spec.gutter.narrow;
    if (!near(pad, want) && !(width > spec.contentMax + 2 * want)) metrics.gutter = `main padding-left ${pad}px (DESIGN ${want}px)`;
  }
  if (width < 768) {
    const IA = 'button, a[href], input:not([type=hidden]), select, textarea, summary, [role=button], [role=tab], [role=radio], [role=switch], [role=checkbox]';
    for (const e of all.filter((x) => x.matches(IA))) {
      const r = e.getBoundingClientRect();
      if (r.width >= spec.touch - 0.5 && r.height >= spec.touch - 0.5) continue;
      const label = e.closest("label");
      if (label && label.getBoundingClientRect().height >= spec.touch - 0.5 && /^(radio|checkbox)$/.test(e.type)) continue;
      const inline = e.tagName === "A" && e.parentElement && e.parentElement.innerText.length > e.innerText.length + 12;
      add(inline ? "warn" : "fail", "touch-target", e, `${Math.round(r.width)}x${Math.round(r.height)} px; DESIGN touch targets are >= ${spec.touch}`);
    }
  }

  // The shell: Testnet is a word and a dashed capsule; the capsule says which view it is
  const header = document.querySelector("header");
  const pill = header && [...header.querySelectorAll("*")].find((e) => /^testnet$/i.test(h.own(e)) && h.vis(e));
  if (!pill) add("fail", "network-word", header, 'the header does not say "Testnet"');
  else {
    let b = pill;
    while (b && px(st(b).borderTopWidth) === 0 && b !== header) b = b.parentElement;
    const bs = b ? st(b) : null;
    if (!bs || px(bs.borderTopWidth) === 0 || rgb(bs.backgroundColor)?.a !== 0) add("fail", "network-pill", pill, "Testnet must be a capsule outline with no fill");
    else if (bs.borderTopStyle !== "dashed") add("fail", "network-pill-dashed", pill, `Testnet is a dashed capsule; the border is ${bs.borderTopStyle}`);
  }
  const seg = ["Owner", "Agent"].map((l) => h.capsule(l));
  if (!seg[0] || !seg[1]) add("fail", "capsule", header, "the header has no Owner | Agent capsule");
  else {
    const active = seg.filter((e) => ["true", "page"].includes(e.getAttribute("aria-pressed") ?? e.getAttribute("aria-selected") ?? e.getAttribute("aria-checked") ?? e.getAttribute("aria-current")));
    if (active.length !== 1 || active[0].textContent.trim().toLowerCase() !== view) add("fail", "capsule-state", header, `the capsule marks ${active.map((a) => a.textContent.trim()).join("+") || "nothing"} but the view is ${view}`);
  }
  if (document.documentElement.dataset.view !== view) add("fail", "data-view", null, `html[data-view]=${document.documentElement.dataset.view}, expected ${view}`);
  metrics.skeleton = h.skel;
  return { findings: F, metrics };
}

/** Every focusable thing the page offers right now, for the "is it reachable by Tab" count. */
function interactive() {
  const h = window.__h6;
  return [...document.querySelectorAll('a[href], button, input:not([type=hidden]), select, textarea, summary, [tabindex], [role=button]')]
    .filter((e) => !e.disabled && e.tabIndex >= 0 && h.vis(e) && !e.closest("[inert], [aria-hidden=true], nextjs-portal"))
    .map((e) => { const r = e.getBoundingClientRect(); return `${h.sel(e)}@${Math.round(r.x)},${Math.round(r.y + window.scrollY)}`; });
}

function focusInfo(e) {
  const h = window.__h6, s = getComputedStyle(e), r = e.getBoundingClientRect();
  return {
    key: `${h.sel(e)}@${Math.round(r.x)},${Math.round(r.y + window.scrollY)}`, y: r.y + window.scrollY,
    sel: h.sel(e), name: (e.getAttribute("aria-label") || e.innerText || e.value || e.title || "").replace(/\s+/g, " ").trim().slice(0, 30),
    portal: !!e.closest("nextjs-portal") || e.tagName === "NEXTJS-PORTAL",
    visible: e.matches(":focus-visible"), outline: `${s.outlineStyle} ${s.outlineWidth} ${s.outlineColor} +${s.outlineOffset}`, shadow: s.boxShadow === "none" ? "" : s.boxShadow.slice(0, 40),
    ow: parseFloat(s.outlineWidth), os: s.outlineStyle, oc: s.outlineColor, tab: e.tabIndex,
  };
}

/** Samples document.getAnimations() on every frame for `ms`, after `action` ran; returns the union and what still runs at the end. */
async function sampleMotion({ action, ms }) {
  const h = window.__h6;
  const seen = new Map();
  const grab = () => document.getAnimations().map((a) => {
    const t = a.effect?.getTiming?.() ?? {};
    const props = [...new Set((a.effect?.getKeyframes?.() ?? []).flatMap((k) => Object.keys(k)).filter((k) => !["offset", "computedOffset", "easing", "composite"].includes(k)))];
    return { id: `${a.constructor.name}|${a.animationName ?? a.transitionProperty ?? ""}|${a.effect?.pseudoElement ?? ""}|${a.effect?.target ? h.sel(a.effect.target) : ""}`, name: a.animationName ?? a.transitionProperty ?? "", pseudo: a.effect?.pseudoElement ?? "", props, duration: t.duration, infinite: t.iterations === Infinity, running: a.playState === "running" };
  });
  const before = document.documentElement.dataset.view;
  if (action) {
    const b = h.capsule(action === "agent" ? "Agent" : "Owner");
    if (!b) return { error: `capsule segment ${action} not found` };
    b.click();
  }
  const t0 = performance.now();
  while (performance.now() - t0 < ms) {
    for (const a of grab()) seen.set(a.id, a);
    await new Promise((r) => requestAnimationFrame(r));
  }
  return { before, after: document.documentElement.dataset.view, seen: [...seen.values()], restRunning: grab().filter((a) => a.running) };
}

// ---------------------------------------------------------------------------------------------------------------- browser
const SEV = { fail: 3, warn: 2, note: 1, pass: 0 };
const worst = (xs) => xs.reduce((m, x) => (SEV[x.sev] > SEV[m] ? x.sev : m), "pass");
const AXE_TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"];
const DENY = /^\s*(stop|approve|decline|raise|pay now|send|sign|confirm|delete|revoke|start|resume|cancel|withdraw|add money|make your own|close|try again)\b/i;

async function newPage(browser, sc, { reduced = false, seed = true, pub = null } = {}) {
  const ctx = await browser.newContext({
    viewport: { width: sc.width, height: sc.height ?? HEIGHT[sc.width] ?? 800 }, deviceScaleFactor: 1, isMobile: sc.width < 768, hasTouch: sc.width < 768,
    reducedMotion: reduced ? "reduce" : "no-preference", colorScheme: "light", locale: "en-GB", timezoneId: "Asia/Kuala_Lumpur",
  });
  await ctx.addInitScript(install, { key: VIEW_KEY, view: seed ? sc.view : "owner", pub });
  const page = await ctx.newPage();
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text().replace(/\s+/g, " ").slice(0, 700)); });
  page.on("pageerror", (e) => errors.push(`pageerror: ${String(e.message).replace(/\s+/g, " ").slice(0, 700)}`));
  page.on("response", (r) => { if (r.status() >= 400) errors.push(`${r.status()} ${new URL(r.url()).pathname}`); });
  return { ctx, page, errors };
}

async function settle(page, quick = false) {
  await page.waitForLoadState("domcontentloaded").catch(() => {});
  if (quick) return;
  await page.evaluate(() => document.fonts?.ready).catch(() => {});
  await page.waitForLoadState("networkidle", { timeout: 5000 }).catch(() => {});
  await page.waitForFunction(() => !document.querySelector('[aria-busy="true"],[data-skeleton],.skeleton'), null, { timeout: 1500 }).catch(() => {});
  await page.waitForTimeout(300);
}

async function setView(page, view) {
  const cur = () => page.evaluate(() => document.documentElement.dataset.view);
  if ((await cur()) === view) return true;
  await page.evaluate((l) => window.__h6.capsule(l)?.click(), view === "agent" ? "Agent" : "Owner");
  await page.waitForFunction((v) => document.documentElement.dataset.view === v, view, { timeout: 3000 }).catch(() => {});
  await page.waitForTimeout(350);
  return (await cur()) === view;
}

async function patchJson(page, re, mutate, R) {
  await page.route(re, async (route) => {
    const res = await route.fetch();
    let body;
    try { body = await res.json(); } catch { return route.fulfill({ response: res }); }
    R.mocked = true;
    return route.fulfill({ response: res, json: mutate(body) ?? body });
  });
}
const fail503 = (route) => route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ ok: false, error: "the service did not answer", executed: false, audience: "user" }) });
const API = /\/api\//;
const FLAGS = { loading: "skeleton", empty: "empty", error: "error", offline: "offline" }; // H2's ?state= values

// Arrange one scenario's state. Real data where an account for it was given; otherwise the reply is patched in the page and R.mocked says so.
async function arrange(page, ctx, sc, cfg, R) {
  const { state } = sc;
  if (R.flag) return; // the page itself shows the state (H2's ?state=, development only)
  if (state === "loading") {
    await page.route(API, async (route) => { await R.hold; await route.continue().catch(() => {}); });
  } else if (state === "error") {
    await page.route(API, fail503); // the service never answers: nothing was read, nothing is cached
  } else if (state === "empty" && !cfg.policyEmpty) {
    await patchJson(page, /\/api\/level1\/inbox/, (b) => ({ ...b, paid: [], waiting: [], waitingCount: 0, needsYou: [], paidTotal: "0", purchases: [], refused: [], invoices: [] }), R);
  } else if (state === "needs-you" && !cfg.policyNeeds) {
    const item = { kind: "bill", what: "Pay Demo Vendor 0.002 USDC for the hosting bill", counterparty: "0x" + "11".repeat(20), amount: "0.002", why: "hosting bill, due today" };
    await patchJson(page, /\/api\/level1\/inbox/, (b) => ({ ...b, waiting: [item], waitingCount: 1, needsYou: [{ counterparty: item.counterparty, amount: item.amount, why: item.why, to: item.counterparty, data: "0x" }] }), R);
  }
}

async function afterLoad(page, ctx, sc, R) {
  if (R.flag) return;
  if (sc.state === "offline") {
    // the connection drops after a good read: the next read fails and the page keeps what it had, each figure marked stale with its time
    if (await page.evaluate(() => !!document.querySelector('[aria-busy="true"]'))) { R.pending = "the route is still a skeleton: nothing to mark stale"; return; }
    await ctx.setOffline(true);
    await page.evaluate(() => { window.dispatchEvent(new Event("offline")); window.dispatchEvent(new Event("focus")); document.dispatchEvent(new Event("visibilitychange")); }).catch(() => {});
    const t0 = Date.now();
    const seen = await page.waitForFunction(() => /\bas of \d{1,2}:\d{2}\b/i.test(document.body.innerText), null, { timeout: 30000 }).then(() => true, () => false);
    R.waited = Math.round((Date.now() - t0) / 1000);
    if (!seen) R.findings.push({ sev: "warn", rule: "offline-no-stale-marker", sel: "", text: "", detail: `no "as of HH:MM" on the page ${R.waited} s after the connection dropped (DESIGN: cached values at full ink, each marked stale with its time)` });
    await page.waitForTimeout(300);
  } else if (sc.state === "detail") {
    const ok = await openDetail(page);
    if (!ok) R.pending = "no row disclosure to open yet";
  }
}

async function openDetail(page) {
  const handle = await page.evaluateHandle((deny) => {
    const re = new RegExp(deny, "i");
    const c = [...document.querySelectorAll('main [aria-haspopup="dialog"], main li button, main [role=listitem] button, main ul button, main table button')];
    return c.find((b) => window.__h6.vis(b) && !re.test(b.innerText || b.getAttribute("aria-label") || "")) ?? null;
  }, DENY.source);
  const el = handle.asElement();
  if (!el) return false;
  await el.click();
  await page.waitForSelector("dialog[open], [role=dialog]", { timeout: 2500 }).catch(() => {});
  await page.waitForTimeout(350);
  return !!(await page.$("dialog[open], [role=dialog]"));
}

async function axeFindings(page, AxeBuilder, R) {
  const res = await new AxeBuilder({ page }).withTags(AXE_TAGS).exclude("nextjs-portal").analyze();
  const out = [];
  for (const v of res.violations)
    for (const n of v.nodes)
      out.push({ sev: "fail", rule: `axe:${v.id}`, sel: n.target.join(" ").slice(0, 90), text: "", detail: `${v.impact}: ${(n.any[0]?.message ?? n.all[0]?.message ?? n.none[0]?.message ?? v.help).slice(0, 120)}` });
  R.axeIncomplete = res.incomplete.map((v) => `${v.id}x${v.nodes.length}`).join(" ");
  return out;
}

async function scenario(browser, AxeBuilder, sc, cfg) {
  const R = { name: `${sc.route}/${sc.state}/${sc.view}/${sc.width}`, findings: [], mocked: false, pending: null };
  // --fast: the Owner home's own development flag shows loading, empty, error and offline at once; otherwise the state is made for real
  R.flag = cfg.fast && sc.route === "account" && sc.view === "owner" ? FLAGS[sc.state] : undefined;
  const policy = sc.state === "empty" ? (cfg.policyEmpty ?? cfg.policy) : sc.state === "needs-you" ? (cfg.policyNeeds ?? cfg.policy) : cfg.policy;
  const { ctx, page, errors } = await newPage(browser, sc, { pub: sc.state === "guest" ? null : cfg.pub });
  let release;
  R.hold = new Promise((r) => (release = r));
  const quiet = ["loading", "error", "offline"].includes(sc.state);
  try {
    await arrange(page, ctx, sc, cfg, R);
    await page.goto(cfg.url + ROUTES[sc.route].path(policy) + (R.flag ? `?state=${R.flag}` : ""), { waitUntil: sc.state === "loading" && !R.flag ? "commit" : "domcontentloaded", timeout: 60000 });
    if (sc.state === "loading") {
      await page.waitForSelector('[aria-busy="true"], .skeleton, [data-skeleton]', { timeout: 5000 }).catch(() => R.findings.push({ sev: "warn", rule: "no-skeleton", sel: "", text: "", detail: "no skeleton on screen while the service reply is held" }));
      await page.waitForTimeout(450);
    } else await settle(page);
    if (!(await setView(page, sc.view))) R.findings.push({ sev: "fail", rule: "set-view", sel: "", text: "", detail: `could not switch to ${sc.view}` });
    await afterLoad(page, ctx, sc, R);
    await page.addStyleTag({ content: "nextjs-portal{display:none!important}" }).catch(() => {});

    if (sc.state === "loading") {
      // while the reply is held: no fake zero, no spinner, and the pulse is an opacity pulse
      const z = await page.evaluate(() => [...document.body.querySelectorAll("*")].filter((e) => window.__h6.vis(e) && /^0(\.0+)?( USDC)?$/.test(window.__h6.own(e))).map((e) => window.__h6.sel(e)));
      if (z.length) R.findings.push({ sev: "fail", rule: "fake-zero", sel: z[0], text: "", detail: `${z.length} element(s) read 0 while loading` });
    }
    const { findings, metrics } = await page.evaluate(domRules, { view: sc.view, width: sc.width, route: sc.route, palette: cfg.palette[sc.view], spec: SPEC });
    R.findings.push(...findings);
    R.metrics = metrics;
    // a skeleton has no hero; the heading rule waits for the content
    if (!cfg.noAxe) R.findings.push(...(await axeFindings(page, AxeBuilder, R)).filter((f) => !(metrics.busy && f.rule === "axe:page-has-heading-one")));
    if (metrics.busy && sc.state === "rest") R.pending = "the route still shows its skeleton after settling: no content yet";
    if (sc.state === "rest" || sc.state === "detail") {
      const flash = (metrics.skeleton ?? []).filter((s) => s.ms < 300);
      if (flash.length) R.findings.push({ sev: "fail", rule: "skeleton-flash", sel: "", text: "", detail: `skeleton on screen for ${flash.map((f) => f.ms + " ms").join(", ")} (DESIGN: >= 300 ms once shown)` });
    }
    if (!quiet) for (const e of errors.filter((x) => !/^Failed to load resource/.test(x) && !/^404 \/favicon/.test(x) && !/_next\/(webpack-)?hmr/.test(x))) {
      // the symbolon projection is merged after the tag: this service answering 404 for it is expected, the page then marks nothing sealed
      if (/^404 \/api\/level1\/symbolon/.test(e)) { R.symbolon404 = true; continue; }
      R.findings.push({ sev: /hydrat|pageerror/i.test(e) ? "fail" : "warn", rule: "console", sel: "", text: "", detail: e });
    }
    if (cfg.shots) {
      mkdirSync(cfg.shotsDir, { recursive: true });
      await page.screenshot({ path: join(cfg.shotsDir, shotName(sc)), fullPage: true, animations: "disabled", caret: "hide" });
      R.shot = shotName(sc);
    }
  } catch (e) {
    R.findings.push({ sev: "fail", rule: "scenario-error", sel: "", text: "", detail: String(e.message).split("\n")[0].slice(0, 160) });
  } finally {
    release();
    await ctx.close().catch(() => {});
  }
  if (R.pending) R.findings.push({ sev: cfg.strict ? "fail" : "note", rule: "not-built-yet", sel: "", text: "", detail: R.pending });
  if (R.mocked) R.findings.push({ sev: "note", rule: "mocked", sel: "", text: "", detail: "the service reply was patched in the page (no real account for this state)" });
  delete R.hold;
  return R;
}

// Tab through a page: every stop must show a ring (pixels differ with and without focus), in the accent, and every control must be a stop.
async function focusWalk(browser, sc, cfg) {
  const R = { name: `focus/${sc.route}/${sc.view}/${sc.width}`, findings: [], stops: 0 };
  const { ctx, page } = await newPage(browser, sc, { pub: cfg.pub });
  try {
    await page.goto(cfg.url + ROUTES[sc.route].path(cfg.policy), { waitUntil: "domcontentloaded", timeout: 60000 });
    await settle(page);
    await setView(page, sc.view);
    await page.addStyleTag({ content: "nextjs-portal{display:none!important}" }).catch(() => {});
    const want = await page.evaluate(interactive);
    const seen = new Set();
    let prevY = -1;
    mkdirSync(join(cfg.shotsDir, "focus"), { recursive: true });
    await page.evaluate(() => { document.activeElement?.blur?.(); window.scrollTo(0, 0); });
    for (let i = 0; i < 80; i++) {
      await page.keyboard.press("Tab");
      const h = await page.evaluateHandle(() => (document.activeElement && document.activeElement !== document.body && document.activeElement !== document.documentElement ? document.activeElement : null));
      const el = h.asElement();
      if (!el) break;
      const info = await el.evaluate(focusInfo);
      if (info.portal) continue;
      if (seen.has(info.key)) break;
      seen.add(info.key);
      R.stops++;
      const box = await el.boundingBox();
      if (!box) { R.findings.push({ sev: "warn", rule: "focus-no-box", sel: info.sel, text: info.name, detail: "focused element has no box" }); continue; }
      const pad = 10, vw = sc.width, vh = HEIGHT[sc.width];
      const x = Math.max(0, box.x - pad), y = Math.max(0, box.y - pad);
      const clip = { x, y, width: Math.min(vw - x, box.width + 2 * pad), height: Math.min(vh - y, box.height + 2 * pad) };
      const on = await page.screenshot({ clip, animations: "disabled", caret: "hide" });
      await el.evaluate((e) => e.blur());
      const off = await page.screenshot({ clip, animations: "disabled", caret: "hide" });
      await el.evaluate((e) => e.focus());
      if (cfg.shots) writeFileSync(join(cfg.shotsDir, "focus", `${sc.route}-${sc.view}-${sc.width}-${String(R.stops).padStart(2, "0")}.png`), on);
      if (on.equals(off)) R.findings.push({ sev: "fail", rule: "focus-ring-missing", sel: info.sel, text: info.name, detail: `no visible change when focused (outline ${info.outline}; shadow ${info.shadow || "none"})` });
      else if (!(info.ow >= 2) && !info.shadow) R.findings.push({ sev: "warn", rule: "focus-ring-thin", sel: info.sel, text: info.name, detail: `outline ${info.outline}` });
      if (info.tab > 0) R.findings.push({ sev: "warn", rule: "tabindex-positive", sel: info.sel, text: info.name, detail: `tabindex=${info.tab} bends the order` });
      if (info.y < prevY - 40) R.findings.push({ sev: "warn", rule: "focus-order", sel: info.sel, text: info.name, detail: "focus jumped up the page" });
      prevY = info.y;
      if (cfg.shots && R.stops === 1) {
        await page.screenshot({ path: join(cfg.shotsDir, `focus-${sc.route}-${sc.view}-${sc.width}.png`), animations: "disabled", caret: "hide" });
      }
    }
    const missed = want.filter((k) => !seen.has(k));
    if (missed.length) R.findings.push({ sev: "warn", rule: "not-reachable-by-tab", sel: missed[0].split("@")[0], text: "", detail: `${missed.length} interactive element(s) were never a Tab stop` });
  } catch (e) {
    R.findings.push({ sev: "fail", rule: "scenario-error", sel: "", text: "", detail: String(e.message).split("\n")[0].slice(0, 160) });
  } finally {
    await ctx.close().catch(() => {});
  }
  return R;
}

// The capsule flip, with and without reduced motion; the `v` key; the remembered view; Esc on the sheet.
async function motionAndShell(browser, sc, cfg) {
  const out = [];
  for (const reduced of [false, true]) {
    const R = { name: `motion/${reduced ? "reduced" : "normal"}/${sc.route}/${sc.width}`, findings: [] };
    const { ctx, page } = await newPage(browser, { ...sc, view: "owner" }, { reduced, pub: cfg.pub });
    try {
      await page.goto(cfg.url + ROUTES[sc.route].path(cfg.policy), { waitUntil: "domcontentloaded", timeout: 60000 });
      await settle(page);
      const media = await page.evaluate(() => matchMedia("(prefers-reduced-motion: reduce)").matches);
      if (media !== reduced) R.findings.push({ sev: "fail", rule: "emulation", sel: "", text: "", detail: `prefers-reduced-motion is ${media}, expected ${reduced}` });
      const idle = await page.evaluate(sampleMotion, { action: null, ms: 400 });
      const loops = idle.restRunning.filter((a) => !/pulse|skeleton/i.test(a.name) && (a.infinite || a.props.some((p) => !["opacity", "mixBlendMode"].includes(p))));
      if (loops.length) R.findings.push({ sev: "fail", rule: "moves-at-rest", sel: loops[0].id.split("|").pop(), text: "", detail: `${loops.length} animation(s) still run on a settled page (${loops.map((a) => a.name || a.id.split("|")[0]).join(", ")}); nothing moves that is not a state change` });
      for (const to of ["agent", "owner"]) {
        const m = await page.evaluate(sampleMotion, { action: to, ms: 700 });
        if (m.error) { R.findings.push({ sev: "fail", rule: "capsule", sel: "", text: "", detail: m.error }); break; }
        if (m.after !== to) R.findings.push({ sev: "fail", rule: "capsule-flip", sel: "", text: "", detail: `clicked ${to}, html[data-view] is ${m.after}` });
        // the UA's ::view-transition-group / image-pair animate size and position from the old snapshot to the new one; for the root they do not change
        const moving = m.seen.filter((a) => !/view-transition-(group|image-pair)/.test(a.pseudo) && a.props.some((p) => !["opacity", "mixBlendMode"].includes(p)));
        if (moving.length) R.findings.push({ sev: "fail", rule: "flip-not-opacity", sel: moving[0].pseudo || moving[0].id.split("|").pop(), text: "", detail: `the flip animates ${[...new Set(moving.flatMap((a) => a.props))].filter((p) => p !== "mixBlendMode").join(", ")}; DESIGN: crossfade only, no slide, no scale` });
        const fade = m.seen.filter((a) => a.props.includes("opacity"));
        if (!reduced && !fade.some((a) => Math.abs(a.duration - 200) <= 25)) R.findings.push({ sev: "warn", rule: "flip-200ms", sel: "", text: "", detail: `no 200 ms opacity crossfade seen on the flip to ${to} (saw ${m.seen.map((a) => `${a.name || "?"}:${a.duration}`).join(", ") || "nothing"})` });
        if (reduced && fade.length) R.findings.push({ sev: "warn", rule: "reduced-motion-crossfade", sel: fade[0].pseudo || fade[0].id.split("|").pop(), text: "", detail: `a ${fade[0].duration} ms crossfade still runs under reduced motion; DESIGN (founder 07:01): the capsule swaps instantly (the lane brief had said crossfade only: a ruling is needed)` });
        R.crossfade = m.seen.map((a) => `${a.pseudo || a.name}:${a.duration}`).join(" ");
      }
      if (!reduced) {
        // `v` flips, is ignored while typing, and the view is remembered per device
        const v0 = await page.evaluate(() => document.documentElement.dataset.view);
        await page.keyboard.press("v");
        await page.waitForTimeout(400);
        const v1 = await page.evaluate(() => document.documentElement.dataset.view);
        if (v0 === v1) R.findings.push({ sev: "fail", rule: "v-key", sel: "", text: "", detail: "pressing v did not flip the view" });
        const stored = await page.evaluate((k) => localStorage.getItem(k), VIEW_KEY);
        if (stored !== v1) R.findings.push({ sev: "fail", rule: "view-remembered", sel: "", text: "", detail: `localStorage ${VIEW_KEY} is ${stored}, the view is ${v1}` });
        await page.reload({ waitUntil: "domcontentloaded" });
        await settle(page, true);
        const v2 = await page.evaluate(() => document.documentElement.dataset.view);
        if (v2 !== v1) R.findings.push({ sev: "fail", rule: "view-not-restored", sel: "", text: "", detail: `reloaded as ${v2}, was ${v1}` });
      }
    } catch (e) {
      R.findings.push({ sev: "fail", rule: "scenario-error", sel: "", text: "", detail: String(e.message).split("\n")[0].slice(0, 160) });
    } finally {
      await ctx.close().catch(() => {});
    }
    out.push(R);
  }
  return out;
}

// The sheet: opens at the right size, takes focus, keeps it, Esc closes it and gives focus back.
async function sheetCheck(browser, sc, cfg) {
  const R = { name: `sheet/${sc.route}/${sc.view}/${sc.width}`, findings: [] };
  const { ctx, page } = await newPage(browser, sc, { pub: cfg.pub });
  try {
    await page.goto(cfg.url + ROUTES[sc.route].path(cfg.policy), { waitUntil: "domcontentloaded", timeout: 60000 });
    await settle(page);
    await setView(page, sc.view);
    if (!(await openDetail(page))) { R.findings.push({ sev: cfg.strict ? "fail" : "note", rule: "not-built-yet", sel: "", text: "", detail: "no row disclosure to open" }); return R; }
    const d = await page.evaluate(() => {
      const el = document.querySelector("dialog[open], [role=dialog]");
      const r = el.getBoundingClientRect();
      return { left: r.left, right: r.right, top: r.top, bottom: r.bottom, w: innerWidth, h: innerHeight, modal: el.matches(":modal") || el.getAttribute("aria-modal") === "true", inside: el.contains(document.activeElement), named: !!(el.getAttribute("aria-label") || el.getAttribute("aria-labelledby")) };
    });
    if (!d.modal) R.findings.push({ sev: "fail", rule: "sheet-not-modal", sel: "dialog", text: "", detail: "the sheet is not modal (page behind stays reachable)" });
    if (!d.named) R.findings.push({ sev: "fail", rule: "sheet-unnamed", sel: "dialog", text: "", detail: "the sheet has no accessible name" });
    if (!d.inside) R.findings.push({ sev: "fail", rule: "sheet-focus", sel: "dialog", text: "", detail: "focus did not move into the sheet" });
    if (sc.width >= 1024 && !(d.right >= d.w - 1 && d.left > d.w * 0.4)) R.findings.push({ sev: "fail", rule: "sheet-drawer", sel: "dialog", text: "", detail: `>= 1024 px must be a right drawer; sheet spans ${Math.round(d.left)}..${Math.round(d.right)} of ${d.w}` });
    else if (sc.width >= 1024 && Math.abs(d.right - d.left - SPEC.drawer) > 4) R.findings.push({ sev: "fail", rule: "sheet-drawer-width", sel: "dialog", text: "", detail: `the drawer is ${Math.round(d.right - d.left)} px wide; DESIGN: a ${SPEC.drawer} px right drawer` });
    if (sc.width < 1024 && !(d.bottom >= d.h - 1 && d.top > d.h * 0.1)) R.findings.push({ sev: "warn", rule: "sheet-size", sel: "dialog", text: "", detail: `mobile sheet opens at content size from the bottom; spans ${Math.round(d.top)}..${Math.round(d.bottom)} of ${d.h}` });
    let escaped = true;
    for (let i = 0; i < 12; i++) {
      await page.keyboard.press("Tab");
      // focus may leave for the browser (activeElement = body) but never lands on the page behind
      if (await page.evaluate(() => { const a = document.activeElement; return !!a && a !== document.body && !a.closest("dialog[open], [role=dialog]"); })) { escaped = false; break; }
    }
    if (!escaped) R.findings.push({ sev: "fail", rule: "sheet-focus-trap", sel: "dialog", text: "", detail: "Tab left the sheet" });
    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);
    if (await page.$("dialog[open], [role=dialog]:not(dialog)")) R.findings.push({ sev: "fail", rule: "sheet-esc", sel: "dialog", text: "", detail: "Esc did not close the sheet" });
    else if (!(await page.evaluate(() => document.activeElement !== document.body))) R.findings.push({ sev: "warn", rule: "sheet-focus-return", sel: "dialog", text: "", detail: "focus is on <body> after Esc, not back on the row" });
  } catch (e) {
    R.findings.push({ sev: "fail", rule: "scenario-error", sel: "", text: "", detail: String(e.message).split("\n")[0].slice(0, 160) });
  } finally {
    await ctx.close().catch(() => {});
  }
  return R;
}

// ---------------------------------------------------------------------------------------------------------------- log
function git(dir, ...a) {
  try { return execFileSync("git", ["-C", dir, ...a], { encoding: "utf8" }).trim(); } catch { return ""; }
}
function treeInfo(url) {
  let dir = process.env.APP_TREE || "";
  try {
    const port = new URL(url).port || "80";
    const pid = execFileSync("lsof", ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN", "-t"], { encoding: "utf8" }).split("\n")[0];
    const cwd = execFileSync("lsof", ["-a", "-p", pid, "-d", "cwd", "-Fn"], { encoding: "utf8" }).split("\n").find((l) => l.startsWith("n"))?.slice(1);
    if (cwd) dir = cwd;
  } catch { /* nothing listens, or lsof is not allowed */ }
  dir ||= APP;
  const root = git(dir, "rev-parse", "--show-toplevel") || dir;
  const dirty = git(root, "status", "--porcelain").split("\n").filter(Boolean).length;
  return `${git(root, "rev-parse", "--abbrev-ref", "HEAD") || "?"} ${git(root, "rev-parse", "--short", "HEAD") || "?"}${dirty ? ` (+${dirty} uncommitted)` : ""} at ${root}`;
}
const clock = () => `${new Date().toLocaleString("sv-SE", { timeZone: "Asia/Kuala_Lumpur" })} +08`;

// Inputs and the CTA fit 375 x 812 with the keyboard open (about 336 px of the height gone): both stay on screen while the field has focus.
async function keyboardFit(browser, route, cfg) {
  const R = { name: `keyboard/${route}/375`, findings: [] };
  const { ctx, page } = await newPage(browser, { route, state: "rest", view: "owner", width: 375, height: 812 - 336 });
  try {
    await page.goto(cfg.url + ROUTES[route].path(cfg.policy), { waitUntil: "domcontentloaded", timeout: 60000 });
    await settle(page);
    const input = await page.$("main input:not([type=hidden]):not([type=radio]):not([type=checkbox]), main textarea");
    if (!input) { R.findings.push({ sev: cfg.strict ? "fail" : "note", rule: "not-built-yet", sel: "", text: "", detail: `no text input on /${route} yet` }); return R; }
    await input.focus();
    const m = await page.evaluate(() => {
      const h = window.__h6, a = document.activeElement.getBoundingClientRect();
      const area = (b) => b.getBoundingClientRect().width * b.getBoundingClientRect().height;
      const cta = [...document.querySelectorAll("main button, main [type=submit], main a[role=button]")].filter((b) => h.vis(b) && !b.closest("dialog")).sort((x, y) => area(y) - area(x))[0];
      const c = cta?.getBoundingClientRect();
      return { h: innerHeight, input: { top: a.top, bottom: a.bottom }, cta: c ? { top: c.top, bottom: c.bottom, w: c.width, hgt: c.height, text: cta.innerText.replace(/\s+/g, " ").trim().slice(0, 30) } : null };
    });
    if (m.input.bottom > m.h + 0.5 || m.input.top < -0.5) R.findings.push({ sev: "fail", rule: "keyboard-input", sel: "input", text: "", detail: `input spans ${Math.round(m.input.top)}..${Math.round(m.input.bottom)} of ${m.h} px with the keyboard open` });
    if (!m.cta) R.findings.push({ sev: "warn", rule: "keyboard-cta", sel: "", text: "", detail: "no CTA button found on the page" });
    else {
      if (m.cta.bottom > m.h + 0.5) R.findings.push({ sev: "fail", rule: "keyboard-cta", sel: "button", text: m.cta.text, detail: `CTA spans ${Math.round(m.cta.top)}..${Math.round(m.cta.bottom)} of ${m.h} px with the keyboard open` });
      if (![SPEC.button, SPEC.buttonCompact].some((h) => Math.abs(m.cta.hgt - h) <= 0.6) || m.cta.w < 375 - 2 * SPEC.gutter.narrow - 1) R.findings.push({ sev: "warn", rule: "cta-plate", sel: "button", text: m.cta.text, detail: `CTA is ${Math.round(m.cta.w)}x${Math.round(m.cta.hgt)} px; DESIGN: a ${SPEC.button} px plate (${SPEC.buttonCompact} compact), full width on mobile` });
    }
  } catch (e) {
    R.findings.push({ sev: "fail", rule: "scenario-error", sel: "", text: "", detail: String(e.message).split("\n")[0].slice(0, 160) });
  } finally {
    await ctx.close().catch(() => {});
  }
  return R;
}

// `v` flips the view, but not while someone is typing.
async function typingV(browser, cfg) {
  const sc = { route: "new", state: "rest", view: "owner", width: 390 };
  const R = { name: "typing-v/new/owner/390", findings: [] };
  const { ctx, page } = await newPage(browser, sc);
  try {
    await page.goto(cfg.url + ROUTES.new.path(), { waitUntil: "domcontentloaded", timeout: 60000 });
    await settle(page);
    const input = await page.$("main input:not([type=hidden]):not([type=radio]):not([type=checkbox]), main textarea");
    if (!input) { R.findings.push({ sev: cfg.strict ? "fail" : "note", rule: "not-built-yet", sel: "", text: "", detail: "no text input on /new yet" }); return R; }
    await input.focus();
    await page.keyboard.type("v");
    const view = await page.evaluate(() => document.documentElement.dataset.view), value = await input.inputValue();
    if (view !== "owner" || value !== "v") R.findings.push({ sev: "fail", rule: "v-while-typing", sel: "input", text: "", detail: `typed v: view ${view}, field reads "${value}"` });
  } catch (e) {
    R.findings.push({ sev: "fail", rule: "scenario-error", sel: "", text: "", detail: String(e.message).split("\n")[0].slice(0, 160) });
  } finally {
    await ctx.close().catch(() => {});
  }
  return R;
}

// ---------------------------------------------------------------------------------------------------------------- main
function proofDir() {
  if (process.env.PROOF_DIR) return process.env.PROOF_DIR;
  const common = git(APP, "rev-parse", "--path-format=absolute", "--git-common-dir"); // the main checkout, from any worktree
  return join(common ? dirname(common) : resolve(APP, "../.."), "proof", "home-2026-10-18");
}

const group = (arr) => {
  const m = new Map();
  for (const f of arr) {
    const k = [f.sev, f.rule, f.sel, f.detail].join("|");
    if (!m.has(k)) m.set(k, { ...f, names: [] });
    m.get(k).names.push(f.name);
  }
  return [...m.values()];
};
const showGroup = (g) => `  ${g.sev.toUpperCase().padEnd(4)} ${g.rule}  ${g.sel}${g.text ? ` "${g.text}"` : ""}  ${g.detail.length > 230 ? g.detail.slice(0, 230) + "..." : g.detail}\n       in ${g.names.length}: ${g.names.slice(0, 4).join(", ")}${g.names.length > 4 ? ", ..." : ""}`;

// The owner's browser keeps the passkey's public half in localStorage `tameion.pub`; the page compares it with the account's owner keys and shows
// Stop / Approve / Decline only on a match. The public coordinates are in the inbox; seeding them signs nothing (the private half never leaves the phone).
async function ownerPub(url, name) {
  try {
    const acct = (await (await fetch(`${url}/api/level1/name/${encodeURIComponent(name)}`)).json()).account;
    const k = (await (await fetch(`${url}/api/level1/inbox?policy=${acct}`)).json()).owners?.keys?.[0];
    return k?.qx && k?.qy ? { qx: k.qx, qy: k.qy, credentialId: "" } : null;
  } catch {
    return null;
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const policy = process.env.POLICY || args.policy || "northwind-agents"; // the lane's populated demo account, by name
  const cfg = {
    url: (process.env.APP_URL ?? "http://localhost:3010").replace(/\/$/, ""),
    policy, policyEmpty: process.env.POLICY_EMPTY || undefined, policyNeeds: process.env.POLICY_NEEDS || undefined,
    proof: proofDir(), shotsDir: process.env.SHOTS_DIR ?? proofDir(), shots: !!args.shots, strict: !!args.strict, noAxe: !!args["no-axe"], fast: !!args.fast,
  };
  const plan = matrix(args, { policy: !!policy });
  if (args.dry) {
    for (const s of plan) console.log(`${s.skip ? "SKIP (no POLICY) " : ""}${s.route}  ${s.state}  ${s.view}  ${s.width}  ${shotName(s)}`);
    console.log(`${plan.length} scenarios; proof dir ${cfg.proof}`);
    return 0;
  }
  const steps = [
    stepTokens(), run("contrast", ["scripts/contrast.mjs"], "scripts/contrast.mjs is missing (H1: WCAG 4.5:1 for every pair in both views)"), stepZeroHash(), stepLiterals(),
    stepScripts("hex-lint", ["lint:tokens", "lint:hex", "tokens:lint", "lint-tokens"], "no tokens-only hex lint script yet (H1 writes it: no literal colour outside src/tokens.*; npm script lint:tokens)", cfg.strict),
    stepScripts("21st-review", ["review:21st", "21st:review", "21st-review", "design:review"], "no `21st review` script yet (H1 finds the CLI; npm script review:21st)", cfg.strict),
  ];
  const md = join(APP, "DESIGN.md"), tk = join(APP, "src/tokens.css");
  const pal = existsSync(md) ? designPalette(readFileSync(md, "utf8")) : {};
  const css = existsSync(tk) ? cssTokens(readFileSync(tk, "utf8")) : { owner: {}, agent: {} };
  // the colours a view may use are DESIGN's table; with no DESIGN.md in the tree, what tokens.css says
  const palette = Object.fromEntries(["owner", "agent"].map((v) => [v, Object.keys(pal[v] ?? {}).length ? pal[v] : Object.fromEntries(Object.entries(css[v]).map(([k, x]) => [k, hexNorm(x)]).filter(([, x]) => x))]));
  for (const s of steps) console.log(`${s.sev.toUpperCase().padEnd(5)} ${s.name}${s.lines.length ? "\n        " + s.lines.join("\n        ") : ""}`);

  const results = [];
  const say = (R, extra = "") => console.log(`${worst(R.findings).toUpperCase().padEnd(5)} ${R.name}${extra}`);
  if (!args["no-browser"]) {
    if (!(await fetch(cfg.url).then((r) => r.status < 500, () => false))) {
      steps.push({ name: "app", sev: "fail", lines: [`nothing answers at ${cfg.url}; start it: (cd apps/home && npm run dev)`] });
      console.log(`FAIL  app\n        ${steps.at(-1).lines[0]}`);
    } else {
      const { chromium } = await import("playwright-core").catch(() => { throw new Error("playwright-core is not installed: npm i -D playwright-core @axe-core/playwright"); });
      const channel = process.env.PW_CHANNEL === undefined ? "chrome" : process.env.PW_CHANNEL;
      const browser = await chromium.launch({ headless: true, ...(channel ? { channel } : {}) });
      const AxeBuilder = await import("@axe-core/playwright").then((m) => m.AxeBuilder ?? m.default?.AxeBuilder ?? m.default);
      cfg.pub = await ownerPub(cfg.url, cfg.policy);
      if (!cfg.pub) steps.push({ name: "owner-mode", sev: "warn", lines: [`could not read ${cfg.policy}'s owner key through ${cfg.url}: every account page is rendered as a guest (read-only)`] });
      const c2 = { ...cfg, palette };
      const runnable = plan.filter((s) => !s.skip);
      if (plan.length > runnable.length) results.push({ name: "policy", findings: [{ sev: cfg.strict ? "fail" : "warn", rule: "no-policy", sel: "", text: "", detail: `POLICY is not set: ${plan.length - runnable.length} account scenarios were not run` }] });
      for (const sc of runnable) {
        const R = await scenario(browser, AxeBuilder, sc, c2);
        results.push(R);
        const f = R.findings.filter((x) => x.sev === "fail").length, w = R.findings.filter((x) => x.sev === "warn").length;
        say(R, `${R.mocked ? " (mocked reply)" : ""}${f || w ? `  ${f} fail ${w} warn` : ""}`);
      }
      if (!args["no-extras"]) {
        for (const sc of runnable.filter((s) => s.state === "rest" && ["account", "new", "guest"].includes(s.route))) { const R = await focusWalk(browser, sc, c2); results.push(R); say(R, `  ${R.stops} stops`); }
        for (const sc of runnable.filter((s) => s.route === "account" && s.state === "rest" && s.view === "owner"))
          for (const R of await motionAndShell(browser, sc, c2)) { results.push(R); say(R, R.crossfade ? "  " + R.crossfade : ""); }
        for (const sc of runnable.filter((s) => s.route === "account" && s.state === "detail")) { const R = await sheetCheck(browser, sc, c2); results.push(R); say(R); }
        if (runnable.some((s) => s.route === "new")) { const R = await typingV(browser, c2); results.push(R); say(R); }
        for (const route of ["new", "add"]) if (runnable.some((s) => s.route === route)) { const R = await keyboardFit(browser, route, c2); results.push(R); say(R); }
      }
      await browser.close();
    }
  }

  // the report: what failed, with the element; the same finding in many scenarios is one line
  const flat = results.flatMap((R) => R.findings.map((f) => ({ ...f, name: R.name })));
  const fails = flat.filter((f) => f.sev === "fail"), warns = flat.filter((f) => f.sev === "warn"), notes = flat.filter((f) => f.sev === "note" && f.rule !== "mocked");
  const lines = [`== ${clock()}  ${treeInfo(cfg.url)}  seat H6  ${cfg.url}`];
  for (const s of steps) lines.push(`${s.sev.toUpperCase().padEnd(5)} ${s.name}${s.lines.length ? "  " + s.lines[0] : ""}${s.lines.slice(1, 8).map((l) => "\n        " + l).join("")}`);
  if (results.length) lines.push(`browser: ${results.length} checks, ${fails.length} FAIL, ${warns.length} WARN (${group(fails).length} + ${group(warns).length} distinct), ${notes.length} note`);
  lines.push(...group(fails).map(showGroup), ...group(warns).slice(0, 30).map(showGroup));
  if (group(warns).length > 30) lines.push(`  ... ${group(warns).length - 30} more WARN in preflight-last-<port>.json`);
  lines.push(...group(notes).slice(0, 8).map(showGroup));
  const mocked = results.filter((R) => R.mocked).map((R) => R.name);
  if (mocked.length) lines.push(`  mocked replies (a fixture, not a real account): ${mocked.join(", ")}`);
  const measured = results.filter((R) => R.metrics && /\/rest\//.test(R.name)).map((R) => `  ${R.name}: h1 ${R.metrics.h1 ?? "none"}; main ${R.metrics.main ?? "none"}${R.metrics.gutter ? `; ${R.metrics.gutter}` : ""}${R.metrics.reds ? `; ${R.metrics.reds} element(s) in --red` : ""}`);
  if (measured.length) lines.push("measured:", ...measured);
  const keys = [...fails, ...warns].map((f) => keyOf(f.name, f));
  const lastFile = join(cfg.proof, `preflight-last-${new URL(cfg.url).port || "80"}.json`);
  if (args.log || args.shots) {
    mkdirSync(cfg.proof, { recursive: true });
    const prev = existsSync(lastFile) ? JSON.parse(readFileSync(lastFile, "utf8")).keys ?? [] : null;
    if (prev) { const d = diffRuns(prev, keys); lines.push(`since the last run: ${d.fixed.length} fixed, ${d.added.length} new`); }
    writeFileSync(lastFile, JSON.stringify({ at: clock(), keys, findings: flat }, null, 1));
    appendFileSync(join(cfg.proof, "preflight.txt"), lines.join("\n") + "\n\n");
  }
  console.log("\n" + lines.join("\n"));
  const bad = fails.length + steps.filter((s) => s.sev === "fail").length;
  console.log(bad ? `\nci: ${bad} FAIL` : "\nci: pass");
  return bad ? 1 : 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main().then((c) => process.exit(c), (e) => { console.error(e); process.exit(2); });
