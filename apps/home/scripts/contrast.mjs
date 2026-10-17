// prebuild: every text/background pair in src/tokens.ts reaches WCAG 4.5:1 in both views, and src/tokens.css carries
// exactly the values src/tokens.ts declares (DESIGN: "a contrast script fails the build when any text pair drops under
// 4.5:1 in either view"). Exits 1 otherwise. Node >= 22.18 (type stripping).
import { readFileSync } from "node:fs";
import { palette, roleClass, shared, sheetShadow, textPairs, typeRoles, VIEWS } from "../src/tokens.ts";

const MIN = 4.5;
const channel = (c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const luminance = (hex) => {
  const [r, g, b] = [1, 3, 5].map((i) => channel(parseInt(hex.slice(i, i + 2), 16) / 255));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
export const contrast = (a, b) => {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

const failures = [];
for (const view of VIEWS) {
  for (const { text, on } of textPairs) {
    const ratio = contrast(palette[view][text], palette[view][on]);
    const line = `${view.padEnd(5)} --${text} on --${on}: ${ratio.toFixed(2)}:1`;
    console.log(`${ratio >= MIN ? "ok  " : "FAIL"} ${line}`);
    if (ratio < MIN) failures.push(line);
  }
}

// tokens.css against tokens.ts: the variables of each block, the type roles' font shorthands.
const css = readFileSync(new URL("../src/tokens.css", import.meta.url), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const vars = (selector) => {
  const out = {};
  for (let at = css.indexOf(`${selector} {`); at >= 0; at = css.indexOf(`${selector} {`, at + 1)) {
    const body = css.slice(css.indexOf("{", at) + 1, css.indexOf("}", at));
    for (const m of body.matchAll(/--([\w-]+):\s*([^;]+);/g)) out[m[1]] = m[2].trim();
  }
  return out;
};
const expect = (where, want, got) => {
  for (const [name, value] of Object.entries(want)) {
    if ((got[name] ?? "").toLowerCase() !== value.toLowerCase()) failures.push(`${where} --${name}: tokens.css ${got[name] ?? "(missing)"}, tokens.ts ${value}`);
  }
};
expect(":root", { ...shared, "sheet-shadow": sheetShadow }, vars(":root"));
for (const view of VIEWS) expect(`[data-view="${view}"]`, palette[view], vars(`:root[data-view="${view}"]`));
for (const [role, r] of Object.entries(typeRoles)) {
  const rule = new RegExp(`\\.${roleClass[role]} \\{ font: ${r.weight} ${r.size}px/${r.line}px `);
  if (!rule.test(css)) failures.push(`tokens.css .${roleClass[role]} is not ${r.weight} ${r.size}px/${r.line}px (tokens.ts ${role})`);
}
const narrow = typeRoles.heroStatement.narrow;
if (!css.includes(`.hero-statement { font-size: ${narrow.size}px; line-height: ${narrow.line}px; }`)) failures.push("tokens.css narrow hero statement differs from tokens.ts");

if (failures.length) {
  console.error(`\ncontrast/tokens: ${failures.length} failure(s)\n${failures.join("\n")}`);
  process.exit(1);
}
console.log(`\ncontrast/tokens: every pair >= ${MIN}:1 in both views; tokens.css matches tokens.ts`);
