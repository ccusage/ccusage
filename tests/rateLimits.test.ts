import test from "node:test";
import assert from "node:assert/strict";
import { parseRateLimitProbe, paceFor, isRateSnapshotFresh } from "../src/rateLimits.ts";

const bytes = (value: string) => new TextEncoder().encode(value);

test("parses Codex primary and weekly windows", () => {
  const snapshot = parseRateLimitProbe(bytes("plan=plus\nprimary_used=42\nprimary_reset=2000\nprimary_seconds=18000\nsecondary_used=17\nsecondary_reset=9000\nsecondary_seconds=604800\n"), 1000);
  assert.ok(snapshot);
  assert.equal(new TextDecoder().decode(snapshot.plan), "plus");
  assert.deepEqual(snapshot.primary, { usedPercent: 42, resetAt: 2000, windowSeconds: 18000 });
  assert.equal(snapshot.secondary?.usedPercent, 17);
});

test("rejects a response with no valid windows", () => {
  assert.equal(parseRateLimitProbe(bytes("plan=plus\nprimary_used=nope\n"), 1000), null);
});

test("clamps provider percentages", () => {
  const snapshot = parseRateLimitProbe(bytes("primary_used=150\nprimary_reset=2000\nprimary_seconds=18000\n"), 1000);
  assert.equal(snapshot?.primary?.usedPercent, 100);
});

test("pace compares consumed capacity with elapsed window", () => {
  const p = paceFor({ usedPercent: 60, resetAt: 1500, windowSeconds: 1000 }, 1000);
  assert.ok(p);
  assert.equal(p.expectedUsedPercent, 50);
  assert.equal(p.remainingPercent, 40);
  assert.equal(p.status, "ahead");
});

test("freshness prevents repeated polling inside TTL", () => {
  const snapshot = parseRateLimitProbe(bytes("primary_used=20\nprimary_reset=5000\nprimary_seconds=18000\n"), 1000);
  assert.equal(isRateSnapshotFresh(snapshot, 1119, 120), true);
  assert.equal(isRateSnapshotFresh(snapshot, 1120, 120), false);
});
