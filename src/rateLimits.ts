import { asciiBytes } from "@native-sdk/core";

export type RateLimitState = "idle" | "loading" | "ready" | "error";

export interface RateWindowSnapshot {
  readonly usedPercent: number;
  readonly resetInSeconds: number;
  readonly windowSeconds: number;
  readonly expectedUsedPercent: number;
}

export interface RateLimitSnapshot {
  readonly plan: Uint8Array;
  readonly primary: RateWindowSnapshot | null;
  readonly secondary: RateWindowSnapshot | null;
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
  const resetInSeconds = parseAsciiNumber(lineValue(body, `${prefix}_reset_in`));
  const windowSeconds = parseAsciiNumber(lineValue(body, `${prefix}_seconds`));
  const expectedUsedPercent = parseAsciiNumber(lineValue(body, `${prefix}_expected`));
  if (usedPercent === null || resetInSeconds === null || windowSeconds === null || expectedUsedPercent === null || windowSeconds <= 0) return null;
  return {
    usedPercent: Math.max(0, Math.min(100, usedPercent)),
    resetInSeconds: Math.max(0, resetInSeconds),
    windowSeconds,
    expectedUsedPercent: Math.max(0, Math.min(100, expectedUsedPercent)),
  };
}

export function parseRateLimitProbe(body: Uint8Array): RateLimitSnapshot | null {
  const primary = parseWindow(body, "primary");
  const secondary = parseWindow(body, "secondary");
  if (primary === null && secondary === null) return null;
  const plan = lineValue(body, "plan");
  return { plan: plan.length > 0 ? plan : asciiBytes("ChatGPT"), primary, secondary };
}

export function paceFor(window: RateWindowSnapshot | null): PaceSnapshot | null {
  if (window === null) return null;
  const delta = window.usedPercent - window.expectedUsedPercent;
  return {
    expectedUsedPercent: window.expectedUsedPercent,
    deltaPercent: delta,
    remainingPercent: Math.max(0, 100 - window.usedPercent),
    status: Math.abs(delta) <= 6 ? "on_track" : delta > 0 ? "ahead" : "behind",
  };
}
