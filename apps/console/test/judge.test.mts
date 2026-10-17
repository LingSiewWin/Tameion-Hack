// The judge's five minutes (docs/115 §3): step order, the bill's next due time, step 5's raise and cut.
import assert from "node:assert/strict";
import { test } from "node:test";
import { clock, cutOf, nextDue, raiseOf, stepOf } from "../src/lib/judge.ts";

const none = { funded: false, billSaved: false, autopilot: false, paid: 0, cut: false };

test("a step needs the one before it; all five done is 5", () => {
  assert.equal(stepOf(none), 0);
  assert.equal(stepOf({ ...none, funded: true }), 1);
  // Autopilot on before the bill is saved still waits on the bill.
  assert.equal(stepOf({ ...none, funded: true, autopilot: true }), 1);
  assert.equal(stepOf({ funded: true, billSaved: true, autopilot: true, paid: 1, cut: false }), 4);
  assert.equal(stepOf({ funded: true, billSaved: true, autopilot: true, paid: 2, cut: true }), 5);
});

test("the next due time is the first one until it passes, then the cycle after now", () => {
  assert.equal(nextDue(1000, 120, 900), 1000);
  assert.equal(nextDue(1000, 120, 1000), 1000);
  assert.equal(nextDue(1000, 120, 1001), 1120);
  assert.equal(nextDue(1000, 120, 1240), 1240);
  assert.equal(nextDue(1000, 120, 1241), 1360);
});

test("clock reads m:ss and never goes negative", () => {
  assert.equal(clock(125), "2:05");
  assert.equal(clock(9.2), "0:10");
  assert.equal(clock(-3), "0:00");
});

test("the raise doubles one payment and lifts the day only past it", () => {
  assert.deepEqual(raiseOf({ perAction: "0.010000 USDC", daily: "0.050000 USDC" }), { perAction: "0.02", daily: "0.05" });
  assert.deepEqual(raiseOf({ perAction: "0.03", daily: "0.05" }), { perAction: "0.06", daily: "0.06" });
});

test("the cut brings one payment down to the bill, else halves the day, never below one payment", () => {
  assert.deepEqual(cutOf({ perAction: "0.02", daily: "0.05" }, "0.002"), { perAction: "0.002", daily: "0.05" });
  assert.deepEqual(cutOf({ perAction: "0.002", daily: "0.05" }, "0.002"), { perAction: "0.002", daily: "0.025" });
  assert.deepEqual(cutOf({ perAction: "0.002", daily: "0.003" }, "0.002"), { perAction: "0.002", daily: "0.002" });
  assert.equal(cutOf({ perAction: "0.002", daily: "0.002" }, "0.002"), null);
});
