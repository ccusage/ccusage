import test from "node:test";
import assert from "node:assert/strict";
import { initialModel, providers, totals, breakdown, codexSeries, update } from "../src/core.ts";

test("launch performs no network/process work", () => {
  const [model, cmd] = initialModel();
  assert.equal(model.updateState, "idle");
  assert.ok(cmd);
});

test("immutable usage datasets are reused instead of allocated per render", () => {
  const [model] = initialModel();
  assert.strictEqual(providers(model), providers(model));
  assert.strictEqual(totals(model), totals(model));
  assert.strictEqual(breakdown(model), breakdown(model));
  assert.strictEqual(codexSeries(model), codexSeries(model));
});

test("opening live limits is lazy and updates section immediately", () => {
  const [model] = initialModel();
  const [next] = update(model, { kind: "show_limits" });
  assert.equal(next.section, "limits");
  assert.equal(next.rateState, "loading");
});
