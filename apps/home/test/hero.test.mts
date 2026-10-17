// The hero's pure parts: keyframes whole at their p, the land/sea threshold, the replay shows only recorded fixes and
// fields, the projection, and the book's rows carry names and never a hash.
import "../scripts/tsx.mjs";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { BBOX, cellAt, fixAt, labelOf, otsu, pack, phases, placeholderScene, toPixel, typeWord, type Report } from "../src/features/hero/model.ts";

const { rowsOf, day } = await import("../src/features/hero/hero.tsx");

test("every keyframe is whole at its p", () => {
  assert.equal(phases(0).quant, 0);
  assert.equal(phases(0.2).quant, 1);
  assert.equal(cellAt(phases(0.2).quant), 40, "the grid reaches the row module at p = .2");
  assert.equal(phases(0.4).drain, 1);
  assert.equal(phases(0.6).track, 1);
  assert.equal(phases(0.8).hand, 1);
  assert.equal(phases(1).book, 1);
  assert.equal(phases(0.39).track, 0, "no label before the drain is done");
});

test("Otsu splits dark sea from bright land", () => {
  const lum = [...Array(500).fill(20), ...Array(500).fill(200)];
  const t = otsu(lum);
  assert.ok(t >= 20 && t < 200, `threshold ${t}`);
});

test("the replay keeps only recorded fixes inside the bbox, and a label only recorded fields", () => {
  const reports: Report[] = [
    { mmsi: 422000001, lat: 26.5, lon: 56.4, sog: 11.8, cog: 290, type: null, ts: "2026-10-18T00:00:00Z" },
    { mmsi: 422000001, lat: 26.52, lon: 56.35, sog: 11.6, cog: 291, type: 80, ts: "2026-10-18T00:10:00Z" },
    { mmsi: 422000002, lat: 30, lon: 50, sog: 5, cog: 0, type: 70, ts: "2026-10-18T00:05:00Z" }, // outside: dropped
    { mmsi: 422000003, lat: 25.1, lon: 56.4, sog: null, cog: null, type: null, ts: "2026-10-18T00:20:00Z" },
  ];
  const r = pack(reports, { synthetic: false, source: "aisstream.io" });
  assert.equal(r.vessels.length, 2);
  const v = r.vessels.find((x) => x.mmsi === "422000001")!;
  assert.equal(v.type, 80, "the vessel's own static type applies to its track");
  assert.equal(fixAt(v, Date.parse("2026-10-17T23:59:00Z")), null, "nothing before the first fix");
  assert.equal(fixAt(v, Date.parse("2026-10-18T00:05:00Z"))![2], 26.5, "the last fix, never an interpolated one");
  assert.equal(labelOf(v, v.fixes[1]), "MMSI 422000001  tanker  11.6 kn  00:10");
  const bare = r.vessels.find((x) => x.mmsi === "422000003")!;
  assert.equal(labelOf(bare, bare.fixes[0]), "MMSI 422000003  00:20", "no type, no speed: not shown");
  assert.equal(typeWord(52), "tug");
  assert.equal(r.from, Date.parse("2026-10-18T00:00:00Z"));
  assert.equal(r.to, Date.parse("2026-10-18T00:20:00Z"));
});

test("the synthetic replay is 200 lines with MMSIs no country owns", () => {
  const lines = readFileSync(new URL("./fixtures/ais.synthetic.jsonl", import.meta.url), "utf8").trim().split("\n");
  assert.equal(lines.length, 200);
  for (const l of lines) assert.match(String(JSON.parse(l).mmsi), /^999/);
});

test("the placeholder frame maps the bbox corners to the image corners", () => {
  const s = placeholderScene(1024);
  const [x0, y0] = toPixel(s, BBOX[0], BBOX[3]);
  const [x1, y1] = toPixel(s, BBOX[2], BBOX[1]);
  assert.ok(Math.abs(x0) < 1e-6 && Math.abs(y0) < 1e-6, `${x0},${y0}`);
  assert.ok(Math.abs(x1 - 1024) < 1e-6 && Math.abs(y1 - s.height) < 1, `${x1},${y1}`);
  assert.ok(Math.abs(1024 / s.height - 1.049) < 0.01, "the export's aspect ratio in Web Mercator");
});

test("the book's rows name payees and never carry a hash", () => {
  const payee = "0xE5E2d8dCC2038fd0A010A6e7845378b646e33854";
  const rows = rowsOf({
    paid: [
      { counterparty: payee, amount: "0.002000 USDC", at: 1792277792, reason: "Demo Vendor bill", tx: "0x" + "ab".repeat(32) },
      { counterparty: "0x" + "12".repeat(20), amount: "1.500000 USDC", at: 1792277000, reason: "0x" + "cd".repeat(32) },
    ],
    refused: [{ counterparty: payee, amount: "50 USDC", why: "over the daily cap" }],
    parties: { [payee.toLowerCase()]: { label: "Demo Vendor" } },
  } as never);
  assert.equal(day(1792277792), new Date(1792277792e3).toLocaleDateString("en-GB", { day: "numeric", month: "short" }));
  assert.deepEqual(rows[0], { date: day(1792277792), who: "Demo Vendor", amount: "0.002 USDC", why: "Demo Vendor bill", state: "paid" });
  assert.equal(rows[1].who, "a payee you approved");
  assert.equal(rows[1].why, "", "a why that is a hash is dropped");
  assert.equal(rows[1].amount, "1.5 USDC");
  assert.equal(rows[2].state, "refused");
  assert.doesNotMatch(JSON.stringify(rows), /0x[0-9a-fA-F]{8,}/);
});
