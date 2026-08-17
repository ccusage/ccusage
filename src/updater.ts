import { asciiBytes } from "@native-sdk/core";

export interface ReleaseFeed {
  readonly version: Uint8Array;
  readonly url: Uint8Array;
  readonly sha256: Uint8Array;
}

interface SemVer {
  readonly major: number;
  readonly minor: number;
  readonly patch: number;
}

function trimLineEnd(bytes: Uint8Array, start: number, end: number): Uint8Array {
  let last = end;
  while (last > start) {
    const c = bytes[last - 1];
    if (c === 13 || c === 32 || c === 9) last -= 1;
    else break;
  }
  return bytes.subarray(start, last);
}

function startsWithBytes(value: Uint8Array, prefix: Uint8Array): boolean {
  if (value.length < prefix.length) return false;
  for (let i = 0; i < prefix.length; i += 1) {
    if (value[i] !== prefix[i]) return false;
  }
  return true;
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

function parseSemVer(bytes: Uint8Array): SemVer | null {
  let major = 0;
  let minor = 0;
  let patch = 0;
  let part = 0;
  let value = 0;
  let digits = 0;

  for (let i = 0; i < bytes.length; i += 1) {
    const c = bytes[i];
    if (c >= 48 && c <= 57) {
      value = value * 10 + (c - 48);
      digits += 1;
      continue;
    }
    if (c !== 46 || digits === 0 || part >= 2) return null;
    if (part === 0) major = value;
    else minor = value;
    part += 1;
    value = 0;
    digits = 0;
  }

  if (part !== 2 || digits === 0) return null;
  patch = value;
  return { major, minor, patch };
}

export function parseReleaseFeed(body: Uint8Array): ReleaseFeed | null {
  let first = -1;
  let second = -1;
  let third = body.length;

  for (let i = 0; i < body.length; i += 1) {
    if (body[i] === 10) {
      if (first < 0) first = i;
      else if (second < 0) second = i;
      else {
        third = i;
        break;
      }
    }
  }

  if (first <= 0 || second <= first + 1) return null;
  const version = trimLineEnd(body, 0, first);
  const url = trimLineEnd(body, first + 1, second);
  const sha256 = trimLineEnd(body, second + 1, third);

  if (version.length === 0 || parseSemVer(version) === null) return null;
  if (!startsWithBytes(url, asciiBytes("https://github.com/vedanth-jadhav/ccusage-wrap/releases/download/macos-v"))) return null;
  if (!validSha256(sha256)) return null;

  return { version, url, sha256 };
}

export function isNewerRelease(candidate: Uint8Array, current: Uint8Array): boolean {
  const next = parseSemVer(candidate);
  const now = parseSemVer(current);
  if (next === null || now === null) return false;
  if (next.major !== now.major) return next.major > now.major;
  if (next.minor !== now.minor) return next.minor > now.minor;
  return next.patch > now.patch;
}
