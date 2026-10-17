// The console's hand-typed reply shapes (src/lib/types.ts) against the service's contract
// (apps/classic/openapi.json), both ways for names and required-ness. Zero dependencies.
//
//   node apps/console/scripts/check-types.mjs
//
// Fails when types.ts reads a field openapi.json does not describe (a renamed or removed field),
// or counts on a field (not optional) that openapi.json does not promise on every reply.
// Types only, not values: whether replies match openapi.json is scripts/acceptance.sh rule 17.
import { readFileSync } from "node:fs";

const read = (rel) => readFileSync(new URL(rel, import.meta.url), "utf8");
const doc = JSON.parse(read("../../classic/openapi.json"));
const SCHEMAS = doc.components.schemas;

// types.ts name -> openapi.json name, for every type checked. The inbox and what it holds at least.
const CHECKED = {
  Inbox: "Inbox", Paid: "Paid", NeedRow: "NeedRow", Invoice: "Invoice", Seller: "Seller", Purchase: "Purchase",
  Runway: "Runway", Momentum: "Momentum", Preview: "Preview", Party: "Party", Call: "Call",
  WatchJob: "Run", WatchReport: "Report", DeskBook: "DeskBook", DeskDeal: "DeskDeal", DeskMilestone: "DeskMilestone",
};
// Fields the page sets itself, never the service.
const PAGE_ONLY = { Inbox: ["readAt", "error"] };

// ---- types.ts: a small reader for `export type X = …;` (object literals, `&`, `|`, `[]`, Record<string, T>)
function splitTop(s, sep) {
  const out = [];
  let depth = 0, cur = "";
  for (const ch of s) {
    if ("{[(<".includes(ch)) depth++;
    if ("}])>".includes(ch)) depth--;
    if (ch === sep && depth === 0) { out.push(cur); cur = ""; } else cur += ch;
  }
  out.push(cur);
  return out.map((x) => x.trim()).filter(Boolean);
}
function aliases(src) {
  const code = src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
  const out = {};
  const re = /export type (\w+)\s*=/g;
  let m;
  while ((m = re.exec(code))) {
    let depth = 0, i = re.lastIndex;
    for (; i < code.length; i++) {
      const ch = code[i];
      if ("{[(<".includes(ch)) depth++;
      if ("}])>".includes(ch)) depth--;
      if (ch === ";" && depth === 0) break;
    }
    out[m[1]] = code.slice(re.lastIndex, i).trim();
  }
  return out;
}
function tsShape(text, types, seen = new Set()) {
  let t = text.trim();
  const alts = splitTop(t, "|").filter((x) => x !== "null" && x !== "undefined");
  if (alts.length > 1) {
    // A union of objects: a field is there if a branch has it, counted on only if every branch has it.
    const shapes = alts.map((a) => tsShape(a, types, seen));
    if (!shapes.every((x) => x.kind === "object")) return { kind: "leaf" }; // string-literal unions and the like
    const members = {};
    for (const x of shapes) {
      for (const [k, m] of Object.entries(x.members)) {
        const everywhere = shapes.every((y) => y.members[k] && !y.members[k].optional);
        members[k] = { optional: !everywhere, shape: members[k]?.shape ?? m.shape };
      }
    }
    return { kind: "object", members };
  }
  if (!alts.length) return { kind: "leaf" };
  t = alts[0];
  if (t.startsWith("(") && t.endsWith(")")) return tsShape(t.slice(1, -1), types, seen);
  if (t.endsWith("[]")) return { kind: "array", of: tsShape(t.slice(0, -2), types, seen) };
  const parts = splitTop(t, "&");
  if (parts.length > 1) {
    const members = {};
    for (const p of parts) Object.assign(members, tsShape(p, types, seen).members ?? {});
    return { kind: "object", members };
  }
  const rec = t.match(/^Record<\s*string\s*,([\s\S]*)>$/);
  if (rec) return { kind: "record", of: tsShape(rec[1], types, seen) };
  if (t.startsWith("{")) {
    const members = {};
    for (const m of splitTop(t.slice(1, -1), ";")) {
      if (m.startsWith("[")) continue; // an index signature: open by intent
      const g = m.match(/^["']?(\w+)["']?(\?)?\s*:([\s\S]*)$/);
      if (!g) throw new Error(`types.ts: cannot read member "${m}"`);
      members[g[1]] = { optional: !!g[2], shape: tsShape(g[3], types, seen) };
    }
    return { kind: "object", members };
  }
  if (types[t] && !seen.has(t)) return tsShape(types[t], types, new Set([...seen, t]));
  return { kind: "leaf" };
}

// ---- openapi.json: $ref, allOf, anyOf/oneOf (a field is promised only if every non-null branch promises it)
function scShape(s, seen = new Set()) {
  if (!s || typeof s !== "object") return { kind: "leaf" };
  if (s.$ref) {
    const name = s.$ref.split("/").pop();
    return seen.has(name) ? { kind: "leaf" } : scShape(SCHEMAS[name], new Set([...seen, name]));
  }
  const merge = (shapes, all) => {
    const objs = shapes.filter((x) => x.kind === "object");
    if (!objs.length) return shapes.find((x) => x.kind !== "leaf") ?? { kind: "leaf" };
    const props = {}, required = new Set();
    for (const o of objs) Object.assign(props, o.props);
    for (const k of Object.keys(props)) {
      if (all ? objs.some((o) => o.required.has(k)) : objs.every((o) => o.required.has(k))) required.add(k);
    }
    return { kind: "object", props, required };
  };
  if (s.allOf) return merge(s.allOf.map((x) => scShape(x, seen)), true);
  const alts = (s.anyOf ?? s.oneOf)?.filter((x) => x.type !== "null");
  if (alts) return alts.length === 1 ? scShape(alts[0], seen) : merge(alts.map((x) => scShape(x, seen)), false);
  if (s.items) return { kind: "array", of: scShape(s.items, seen) };
  if (s.properties) {
    const props = {};
    for (const [k, v] of Object.entries(s.properties)) props[k] = v;
    return { kind: "object", props, required: new Set(s.required ?? []) };
  }
  if (s.additionalProperties && typeof s.additionalProperties === "object") return { kind: "record", of: scShape(s.additionalProperties, seen) };
  const types = [s.type].flat();
  if (types.includes("object")) return { kind: "open" };
  return { kind: "leaf" };
}

function compare(ts, sc, path, errors, skip = []) {
  if (ts.kind === "leaf") return;
  if (ts.kind === "record" && ts.of.kind === "leaf" && sc.kind !== "leaf") return; // Record<string, unknown>: any object
  if (ts.kind === "array" || ts.kind === "record") {
    if (sc.kind !== ts.kind) return errors.push(`${path}: types.ts reads it as ${ts.kind === "array" ? "a list" : "a map"}; openapi.json does not`);
    return compare(ts.of, sc.of, `${path}[]`, errors);
  }
  if (sc.kind !== "object") return errors.push(`${path}: types.ts reads its fields; openapi.json does not describe them`);
  for (const [name, m] of Object.entries(ts.members)) {
    if (skip.includes(name)) continue;
    if (!(name in sc.props)) {
      errors.push(`${path}.${name}: in types.ts, not in openapi.json (renamed or removed?)`);
      continue;
    }
    if (!m.optional && !sc.required.has(name)) errors.push(`${path}.${name}: types.ts counts on it; openapi.json does not promise it`);
    compare(m.shape, scShape(sc.props[name]), `${path}.${name}`, errors);
  }
}

export function check(typesSrc) {
  const types = aliases(typesSrc);
  const errors = [];
  for (const [tsName, scName] of Object.entries(CHECKED)) {
    if (!types[tsName]) { errors.push(`${tsName}: not in types.ts`); continue; }
    if (!SCHEMAS[scName]) { errors.push(`${scName}: not in openapi.json components.schemas`); continue; }
    compare(tsShape(types[tsName], types), scShape({ $ref: `#/components/schemas/${scName}` }), tsName, errors, PAGE_ONLY[tsName]);
  }
  return errors;
}

const src = read("../src/lib/types.ts");
const errors = check(src);
// The check must bite: a field renamed in types.ts has to be caught.
const bites = check(src.replace("paidTotal: string", "paidTotl: string")).some((e) => e.startsWith("Inbox.paidTotl"));
if (!bites) errors.push("self-check: renaming Inbox.paidTotal in types.ts was not caught");
for (const e of errors) console.error(`  drift: ${e}`);
console.log(`  types.ts vs openapi.json: ${Object.keys(CHECKED).length} types, ${errors.length} drift${errors.length === 1 ? "" : "s"}`);
process.exit(errors.length ? 1 : 0);
