import { asciiBytes } from "@native-sdk/core";

export type RateLimitState = "idle" | "loading" | "ready" | "error";

export interface RateWindowSnapshot {
  readonly usedPercent: number;
  readonly resetAt: number;
  readonly windowSeconds: number;
}

export interface RateLimitSnapshot {
  readonly plan: Uint8Array;
  readonly primary: RateWindowSnapshot | null;
  readonly secondary: RateWindowSnapshot | null;
  readonly fetchedAt: number;
}

export interface PaceSnapshot {
  readonly expectedUsedPercent: number;
  readonly deltaPercent: number;
  readonly remainingPercent: number;
  readonly status: "on_track" | "ahead" | "behind";
}

const EMPTY = asciiBytes("");

function lineValue(body: Uint8Array, key: string): Uint8Array {
  const prefix = asciiBytes(`${key}=`);
  for (let start = 0; start < body.length;) {
    let end = start;
    while (end < body.length && body[end] !== 10) end += 1;
    const line = body.subarray(start, end);
    let matches = line.length >= prefix.length;
    for (let i = 0; matches && i < prefix.length; i += 1) matches = line[i] === prefix[i];
    if (matches) return line.subarray(prefix.length);
    start = end + 1;
  }
  return EMPTY;
}

function parseAsciiNumber(value: Uint8Array): number | null {
  if (value.length === 0) return null;
  let n = 0;
  for (let i = 0; i < value.length; i += 1) {
    const c = value[i];
    if (c < 48 || c > 57) return null;
    n = n * 10 + (c - 48);
    if (!Number.isSafeInteger(n)) return null;
  }
  return n;
}

function parseWindow(body: Uint8Array, prefix: "primary" | "secondary"): RateWindowSnapshot | null {
  const usedPercent = parseAsciiNumber(lineValue(body, `${prefix}_used`));
  const resetAt = parseAsciiNumber(lineValue(body, `${prefix}_reset`));
  const windowSeconds = parseAsciiNumber(lineValue(body, `${prefix}_seconds`));
  if (usedPercent === null || resetAt === null || windowSeconds === null || windowSeconds <= 0) return null;
  return {
    usedPercent: Math.max(0, Math.min(100, usedPercent)),
    resetAt,
    windowSeconds,
  };
}

export function parseRateLimitProbe(body: Uint8Array, fetchedAt = Math.floor(Date.now() / 1000)): RateLimitSnapshot | null {
  const primary = parseWindow(body, "primary");
  const secondary = parseWindow(body, "secondary");
  if (primary === null && secondary === null) return null;
  const plan = lineValue(body, "plan");
  return { plan: plan.length > 0 ? plan : asciiBytes("ChatGPT"), primary, secondary, fetchedAt };
}

export function paceFor(window: RateWindowSnapshot | null, nowSeconds = Math.floor(Date.now() / 1000)): PaceSnapshot | null {
  if (window === null) return null;
  const timeRemaining = Math.max(0, Math.min(window.windowSeconds, window.resetAt - nowSeconds));
  const elapsed = window.windowSeconds - timeRemaining;
  const expected = Math.max(0, Math.min(100, (elapsed / window.windowSeconds) * 100));
  const delta = window.usedPercent - expected;
  return {
    expectedUsedPercent: expected,
    deltaPercent: delta,
    remainingPercent: Math.max(0, 100 - window.usedPercent),
    status: Math.abs(delta) <= 6 ? "on_track" : delta > 0 ? "ahead" : "behind",
  };
}

export function isRateSnapshotFresh(snapshot: RateLimitSnapshot | null, nowSeconds = Math.floor(Date.now() / 1000), ttlSeconds = 120): boolean {
  return snapshot !== null && nowSeconds >= snapshot.fetchedAt && nowSeconds - snapshot.fetchedAt < ttlSeconds;
}
