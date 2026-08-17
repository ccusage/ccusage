import test from "node:test";
import assert from "node:assert/strict";
import { initialModel, providers, totals, breakdown, codexSeries, update } from "../src/core.ts";

test("launch performs no network/process work", () => {
  const [model] = initialModel();
  assert.equal(model.updateState, "idle");
  assert.equal(model.rateState, "idle");
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

test("reopening populated limits does not spawn another fetch", () => {
  const [model] = initialModel();
  const body = new TextEncoder().encode("plan=plus\nprimary_used=10\nprimary_reset_in=900\nprimary_seconds=18000\nprimary_expected=5\n");
  const [ready] = update(model, { kind: "rate_probe_done", code: 0, output: body });
  const [again] = update(ready, { kind: "show_limits" });
  assert.equal(again.rateState, "ready");
});
