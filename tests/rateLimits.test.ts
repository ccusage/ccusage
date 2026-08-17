import test from "node:test";
import assert from "node:assert/strict";
import { parseRateLimitProbe, paceFor } from "../src/rateLimits.ts";

const bytes = (value: string) => new TextEncoder().encode(value);

test("parses Codex primary and weekly windows", () => {
  const snapshot = parseRateLimitProbe(bytes("plan=plus\nprimary_used=42\nprimary_reset_in=1000\nprimary_seconds=18000\nprimary_expected=35\nsecondary_used=17\nsecondary_reset_in=8000\nsecondary_seconds=604800\nsecondary_expected=12\n"));
  assert.ok(snapshot);
  assert.equal(new TextDecoder().decode(snapshot.plan), "plus");
  assert.deepEqual(snapshot.primary, { usedPercent: 42, resetInSeconds: 1000, windowSeconds: 18000, expectedUsedPercent: 35 });
  assert.equal(snapshot.secondary?.usedPercent, 17);
});

test("rejects a response with no valid windows", () => {
  assert.equal(parseRateLimitProbe(bytes("plan=plus\nprimary_used=nope\n")), null);
});

test("clamps provider percentages", () => {
  const snapshot = parseRateLimitProbe(bytes("primary_used=150\nprimary_reset_in=1000\nprimary_seconds=18000\nprimary_expected=160\n"));
  assert.equal(snapshot?.primary?.usedPercent, 100);
  assert.equal(snapshot?.primary?.expectedUsedPercent, 100);
});

test("pace compares consumed capacity with elapsed window reported by probe", () => {
  const p = paceFor({ usedPercent: 60, resetInSeconds: 500, windowSeconds: 1000, expectedUsedPercent: 50 });
  assert.ok(p);
  assert.equal(p.expectedUsedPercent, 50);
  assert.equal(p.remainingPercent, 40);
  assert.equal(p.status, "ahead");
});
