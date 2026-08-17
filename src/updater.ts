import { asciiBytes } from "@native-sdk/core";

export interface ReleaseFeed {
  readonly version: Uint8Array;
  readonly url: Uint8Array;
  readonly sha256: Uint8Array;
}

function parseDecimal(bytes: Uint8Array): number {
  if (bytes.length === 0) return -1;
  let value = 0;
  for (let i = 0; i < bytes.length; i += 1) {
    const digit = bytes[i];
    if (digit < 48 || digit > 57) return -1;
    value = value * 10 + (digit - 48);
  }
  return value;
}

function validSha256(bytes: Uint8Array): boolean {
  if (bytes.length !== 64) return false;
  for (let i = 0; i < bytes.length; i += 1) {
    const c = bytes[i];
    const number = c >= 48 && c <= 57;
    const lower = c >= 97 && c <= 102;
    if (!number && !lower) return false;
  }
  return true;
}

export function parseReleaseFeed(body: Uint8Array): ReleaseFeed | null {
  const lines = body.trim().split(asciiBytes("\n"));
  if (lines.length < 3) return null;

  const version = lines[0].trim();
  const url = lines[1].trim();
  const sha256 = lines[2].trim();

  if (version.length === 0) return null;
  if (!url.startsWith(asciiBytes("https://github.com/vedanth-jadhav/ccusage-wrap/releases/download/macos-v"))) return null;
  if (!validSha256(sha256)) return null;

  return { version, url, sha256 };
}

export function isNewerRelease(candidate: Uint8Array, current: Uint8Array): boolean {
  const next = candidate.split(asciiBytes("."));
  const now = current.split(asciiBytes("."));
  if (next.length !== 3 || now.length !== 3) return false;

  for (let i = 0; i < 3; i += 1) {
    const nextPart = parseDecimal(next[i]);
    const nowPart = parseDecimal(now[i]);
    if (nextPart < 0 || nowPart < 0) return false;
    if (nextPart > nowPart) return true;
    if (nextPart < nowPart) return false;
  }
  return false;
}
