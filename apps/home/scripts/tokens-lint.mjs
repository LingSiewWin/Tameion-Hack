// prebuild: no colour literal outside the tokens files, and no token used that tokens.css does not define
// (DESIGN Rules: "No hex outside the tokens file"). Lists uses of the first draft's legacy names. Exits 1 on a finding.
import { readdirSync, readFileSync } from "node:fs";
import { extname, join, relative } from "node:path";

const root = new URL("..", import.meta.url).pathname;
const src = join(root, "src");
const TOKENS = new Set(["src/tokens.ts", "src/tokens.css"]);
const CODE = new Set([".ts", ".tsx", ".js", ".jsx", ".mjs", ".css"]);
// set outside tokens.css: next/font's family variables and Tailwind's own
const FOREIGN = /^(font-plex-sans|font-plex-mono|tw-.*)$/;
const LEGACY = /--(?:text-2|red|accent|cta|content-owner)\b|\bstatement-line\b|(?<![-\w.])mono(?![-/.\w]|\s*[=:(])/g;

const COLOUR = [
  [/#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})\b/g, "hex colour"],
  [/\b(?:rgba?|hsla?|oklch|oklab|lab|lch|hwb)\(/g, "colour function"],
  [
    /\b(?:bg|text|border|ring|fill|stroke|outline|divide|decoration|caret|accent|shadow|from|via|to)-(?:white|black|(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d{2,3})\b/g,
    "Tailwind palette colour",
  ],
];

const walk = (dir) =>
  readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(join(dir, e.name)) : CODE.has(extname(e.name)) ? [join(dir, e.name)] : []));

const defined = new Set([...readFileSync(join(src, "tokens.css"), "utf8").matchAll(/(--[\w-]+)\s*:/g)].map((m) => m[1].slice(2)));
const findings = [];
const legacy = [];
for (const file of walk(src)) {
  const rel = relative(root, file);
  if (TOKENS.has(rel)) continue;
  readFileSync(file, "utf8")
    .split("\n")
    .forEach((line, i) => {
      const at = `${rel}:${i + 1}`;
      for (const [re, what] of COLOUR) for (const m of line.matchAll(re)) findings.push(`${at} ${what} ${m[0]}: use a token from src/tokens.css`);
      for (const m of line.matchAll(/(?:var\(|\()--([\w-]+)/g)) {
        if (!defined.has(m[1]) && !FOREIGN.test(m[1])) findings.push(`${at} --${m[1]} is not a token in src/tokens.css`);
      }
      for (const m of line.matchAll(LEGACY)) legacy.push(`${at} ${m[0]}`);
    });
}

if (legacy.length) console.log(`legacy names still in use (${legacy.length}; rename, then delete the legacy block in tokens.css):\n  ${legacy.join("\n  ")}`);
if (findings.length) {
  console.error(`\ntokens-lint: ${findings.length} finding(s)\n${findings.join("\n")}`);
  process.exit(1);
}
console.log("tokens-lint: no colour literal outside the tokens files; every token used is defined");
