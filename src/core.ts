import { Cmd, asciiBytes } from "@native-sdk/core";
import { isNewerRelease, parseReleaseFeed } from "./updater.ts";
import { paceFor, parseRateLimitProbe, type RateLimitSnapshot, type RateLimitState } from "./rateLimits.ts";

export type Range = "seven" | "thirty" | "ninety";
export type Metric = "cost" | "tokens";
export type Section = "usage" | "limits";
export type UpdateState = "idle" | "checking" | "current" | "available" | "staging" | "restarting" | "failed";

export interface ProviderRow { readonly name: Uint8Array; readonly amount: Uint8Array; readonly detail: Uint8Array; readonly share: Uint8Array; }
export interface TotalRow { readonly label: Uint8Array; readonly value: Uint8Array; readonly detail: Uint8Array; }
export interface BreakdownRow { readonly id: Uint8Array; readonly model: Uint8Array; readonly provider: Uint8Array; readonly cost: Uint8Array; readonly share: Uint8Array; readonly tokens: Uint8Array; }

export interface Model {
  readonly range: Range;
  readonly metric: Metric;
  readonly section: Section;
  readonly refreshed: boolean;
  readonly rateState: RateLimitState;
  readonly rateSnapshot: RateLimitSnapshot | null;
  readonly rateDetailText: Uint8Array;
  readonly updateState: UpdateState;
  readonly updateVersion: Uint8Array;
  readonly updateUrl: Uint8Array;
  readonly updateSha256: Uint8Array;
  readonly updateDetail: Uint8Array;
}

export type Msg =
  | { readonly kind: "range7" } | { readonly kind: "range30" } | { readonly kind: "range90" }
  | { readonly kind: "cost" } | { readonly kind: "tokens" }
  | { readonly kind: "show_usage" } | { readonly kind: "show_limits" }
  | { readonly kind: "refresh" } | { readonly kind: "rate_refresh" }
  | { readonly kind: "rate_probe_done"; readonly code: number; readonly output: Uint8Array }
  | { readonly kind: "rate_probe_failed"; readonly reason: Uint8Array }
  | { readonly kind: "update_action" }
  | { readonly kind: "update_check_done"; readonly code: number; readonly output: Uint8Array }
  | { readonly kind: "update_check_failed"; readonly reason: Uint8Array }
  | { readonly kind: "update_stage_done"; readonly code: number; readonly output: Uint8Array }
  | { readonly kind: "update_stage_failed"; readonly reason: Uint8Array };

export const viewUnbound = ["range","metric","section","refreshed","rateState","rateSnapshot","rateDetailText","updateState","updateVersion","updateUrl","updateSha256","rate_probe_done","rate_probe_failed","update_check_done","update_check_failed","update_stage_done","update_stage_failed"] as const;

const EMPTY = asciiBytes("");
const LOCAL_DATA = asciiBytes("Local usage data");
const UPDATED_NOW = asciiBytes("Updated just now");
const CURRENT_VERSION = asciiBytes("0.1.0");
const CHECK_UPDATES = asciiBytes("Check for updates");
const INSTALL_UPDATE = asciiBytes("Install update");
const INSTALLING = asciiBytes("Installing...");
const RESTARTING = asciiBytes("Restarting...");
const RATE_IDLE = asciiBytes("Open Rate Limits to read your Codex subscription limits.");
const RATE_LOADING = asciiBytes("Reading current subscription limits...");
const RATE_ERROR = asciiBytes("Could not read Codex limits. Make sure codex login is active, then retry.");

const CODEX_COST: readonly number[] = [0.01,0.01,0.05,0.02,0,0,0,0,0.01,0.04,0.03,0.05,0.04,0.01,0.04,0.03,0.03,0.02,0.16,0.23,0.68,0.01,0.02,0.06,0.03,0.21,0.76,0.95,0.27,0.09];
const CODEX_TOKENS: readonly number[] = [0.01,0.01,0.03,0.01,0,0,0,0,0.01,0.02,0.02,0.03,0.02,0.01,0.03,0.02,0.02,0.02,0.14,0.22,0.66,0.02,0.01,0.06,0.03,0.16,0.64,0.91,0.17,0.07];
const CLAUDE_COST: readonly number[] = [0.01,0.01,0.03,0.02,0.06,0.01,0,0,0.02,0.06,0.04,0.06,0.05,0.01,0.05,0.04,0.03,0.02,0.04,0.04,0.03,0.01,0.02,0.03,0.03,0.10,0.09,0.23,0.12,0.08];
const CLAUDE_TOKENS: readonly number[] = [0.01,0.01,0.08,0.03,0.07,0.01,0,0,0.03,0.11,0.06,0.11,0.09,0.02,0.07,0.06,0.05,0.03,0.06,0.05,0.04,0.01,0.03,0.05,0.05,0.12,0.08,0.20,0.14,0.11];
const EMPTY_SERIES: readonly number[] = [0];

const PROVIDERS: readonly ProviderRow[] = [
  { name: asciiBytes("Codex"), amount: asciiBytes("$35,304.93"), detail: asciiBytes("64.3% of cost / 48.8B tokens"), share: asciiBytes("64.3%") },
  { name: asciiBytes("Claude Code"), amount: asciiBytes("$19,585.54"), detail: asciiBytes("35.7% of cost / 17.2B tokens"), share: asciiBytes("35.7%") },
];
const TOTALS: readonly TotalRow[] = [
  { label: asciiBytes("Processed tokens"), value: asciiBytes("65.9B"), detail: asciiBytes("2.27B per active day") },
  { label: asciiBytes("Cached input"), value: asciiBytes("63.3B"), detail: asciiBytes("97.5% of observed input") },
  { label: asciiBytes("Uncached input"), value: asciiBytes("1.61B"), detail: asciiBytes("582M cache writes") },
  { label: asciiBytes("Output"), value: asciiBytes("193M"), detail: asciiBytes("includes reasoning") },
  { label: asciiBytes("Cache savings"), value: asciiBytes("$320,610.13"), detail: asciiBytes("5.8x the raw cost") },
];
const BREAKDOWN: readonly BreakdownRow[] = [
  { id: asciiBytes("codex-sol"), model: asciiBytes("gpt-5.6-sol"), provider: asciiBytes("Codex"), cost: asciiBytes("$35,098.97"), share: asciiBytes("63.9%"), tokens: asciiBytes("48.6B") },
  { id: asciiBytes("fable-5"), model: asciiBytes("claude-fable-5"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$12,769.21"), share: asciiBytes("23.3%"), tokens: asciiBytes("8.14B") },
  { id: asciiBytes("opus-5"), model: asciiBytes("claude-opus-5"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$6,267.20"), share: asciiBytes("11.4%"), tokens: asciiBytes("8.27B") },
  { id: asciiBytes("claude-sol"), model: asciiBytes("gpt-5.6-sol"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$205.18"), share: asciiBytes("0.4%"), tokens: asciiBytes("207M") },
  { id: asciiBytes("opus-4-8"), model: asciiBytes("claude-opus-4-8"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$127.77"), share: asciiBytes("0.2%"), tokens: asciiBytes("131M") },
  { id: asciiBytes("sonnet-5"), model: asciiBytes("claude-sonnet-5"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$16.98"), share: asciiBytes("0.0%"), tokens: asciiBytes("33.7M") },
  { id: asciiBytes("haiku-4-5"), model: asciiBytes("claude-haiku-4-5"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$2.89"), share: asciiBytes("0.0%"), tokens: asciiBytes("16.2M") },
  { id: asciiBytes("terra"), model: asciiBytes("gpt-5.6-terra"), provider: asciiBytes("Codex"), cost: asciiBytes("$0.78"), share: asciiBytes("0.0%"), tokens: asciiBytes("1.35M") },
];

const RATE_PROBE_SCRIPT = asciiBytes(`set -eu
A="\${CODEX_HOME:-$HOME/.codex}/auth.json"
[ -r "$A" ] || { echo auth_missing; exit 41; }
T=$(/usr/bin/plutil -extract tokens.access_token raw -o - "$A" 2>/dev/null || /usr/bin/plutil -extract tokens.accessToken raw -o - "$A" 2>/dev/null || true)
[ -n "$T" ] || { echo token_missing; exit 42; }
I=$(/usr/bin/plutil -extract tokens.account_id raw -o - "$A" 2>/dev/null || /usr/bin/plutil -extract tokens.accountId raw -o - "$A" 2>/dev/null || true)
R=$(mktemp /tmp/ccusage-limits.XXXXXX); trap '/bin/rm -f "$R"' EXIT
if [ -n "$I" ]; then /usr/bin/curl -fsSL --max-time 12 -H "Authorization: Bearer $T" -H "Accept: application/json" -H "ChatGPT-Account-Id: $I" https://chatgpt.com/backend-api/wham/usage -o "$R"; else /usr/bin/curl -fsSL --max-time 12 -H "Authorization: Bearer $T" -H "Accept: application/json" https://chatgpt.com/backend-api/wham/usage -o "$R"; fi
v(){ /usr/bin/plutil -extract "$1" raw -o - "$R" 2>/dev/null || true; }
N=$(/bin/date +%s)
PU=$(v rate_limit.primary_window.used_percent); PR=$(v rate_limit.primary_window.reset_at); PS=$(v rate_limit.primary_window.limit_window_seconds)
SU=$(v rate_limit.secondary_window.used_percent); SR=$(v rate_limit.secondary_window.reset_at); SS=$(v rate_limit.secondary_window.limit_window_seconds)
PRI=0; PE=0; SRI=0; SE=0
if [ -n "$PR" ] && [ -n "$PS" ] && [ "$PS" -gt 0 ] 2>/dev/null; then PRI=$((PR>N ? PR-N : 0)); PE=$(((PS-PRI)*100/PS)); fi
if [ -n "$SR" ] && [ -n "$SS" ] && [ "$SS" -gt 0 ] 2>/dev/null; then SRI=$((SR>N ? SR-N : 0)); SE=$(((SS-SRI)*100/SS)); fi
printf 'plan=%s\nprimary_used=%s\nprimary_reset_in=%s\nprimary_seconds=%s\nprimary_expected=%s\nsecondary_used=%s\nsecondary_reset_in=%s\nsecondary_seconds=%s\nsecondary_expected=%s\n' "$(v plan_type)" "$PU" "$PRI" "$PS" "$PE" "$SU" "$SRI" "$SS" "$SE"
`);

export function initialModel(): [Model, Cmd<Msg>] {
  return [{ range: "thirty", metric: "cost", section: "usage", refreshed: false, rateState: "idle", rateSnapshot: null, rateDetailText: RATE_IDLE, updateState: "idle", updateVersion: EMPTY, updateUrl: EMPTY, updateSha256: EMPTY, updateDetail: asciiBytes("Updates are checked only when you ask.") }, Cmd.none];
}

export function update(model: Model, msg: Msg): [Model, Cmd<Msg>] {
  switch (msg.kind) {
    case "range7": return [{ ...model, range: "seven" }, Cmd.none];
    case "range30": return [{ ...model, range: "thirty" }, Cmd.none];
    case "range90": return [{ ...model, range: "ninety" }, Cmd.none];
    case "cost": return [{ ...model, metric: "cost" }, Cmd.none];
    case "tokens": return [{ ...model, metric: "tokens" }, Cmd.none];
    case "show_usage": return [{ ...model, section: "usage" }, Cmd.none];
    case "show_limits":
      if (model.rateSnapshot !== null) return [{ ...model, section: "limits", rateState: "ready" }, Cmd.none];
      return [{ ...model, section: "limits", rateState: "loading", rateDetailText: RATE_LOADING }, Cmd.spawn([asciiBytes("/bin/sh"), asciiBytes("-c"), RATE_PROBE_SCRIPT], { key: "rate-limit-probe", collect: true, exit: "rate_probe_done", err: "rate_probe_failed" })];
    case "refresh": return [{ ...model, refreshed: true }, Cmd.none];
    case "rate_refresh": return [{ ...model, rateState: "loading", rateDetailText: RATE_LOADING }, Cmd.spawn([asciiBytes("/bin/sh"), asciiBytes("-c"), RATE_PROBE_SCRIPT], { key: "rate-limit-probe", collect: true, exit: "rate_probe_done", err: "rate_probe_failed" })];
    case "rate_probe_done": {
      if (msg.code !== 0) return [{ ...model, rateState: "error", rateDetailText: RATE_ERROR }, Cmd.none];
      const snapshot = parseRateLimitProbe(msg.output);
      if (snapshot === null) return [{ ...model, rateState: "error", rateDetailText: asciiBytes("The usage response did not contain supported rate-limit windows.") }, Cmd.none];
      return [{ ...model, rateState: "ready", rateSnapshot: snapshot, rateDetailText: asciiBytes("Live Codex subscription data | refreshes only on request") }, Cmd.none];
    }
    case "rate_probe_failed": return [{ ...model, rateState: "error", rateDetailText: RATE_ERROR }, Cmd.none];
    case "update_action":
      if (model.updateState === "available") {
        return [{ ...model, updateState: "staging", updateDetail: asciiBytes("Downloading and verifying the update...") }, Cmd.spawn([asciiBytes("/bin/sh"),asciiBytes("-c"),asciiBytes("set -eu; U=\"$1\"; H=\"$2\"; V=\"$3\"; T=\"$(mktemp -d /tmp/ccusage-update.XXXXXX)\"; Z=\"$T/update.zip\"; /usr/bin/curl -fL --retry 3 --connect-timeout 15 \"$U\" -o \"$Z\"; A=\"$(/usr/bin/shasum -a 256 \"$Z\" | /usr/bin/awk '{print $1}')\"; [ \"$A\" = \"$H\" ] || { echo checksum_mismatch; exit 21; }; mkdir \"$T/unpack\"; /usr/bin/ditto -x -k \"$Z\" \"$T/unpack\"; N=\"$(/usr/bin/find \"$T/unpack\" -maxdepth 1 -name '*.app' -print -quit)\"; [ -n \"$N\" ] || exit 22; /usr/bin/codesign --verify --deep --strict \"$N\" || exit 23; NV=\"$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \"$N/Contents/Info.plist\")\"; [ \"$NV\" = \"$V\" ] || exit 24; P=\"$PPID\"; C=\"$(/bin/ps -p \"$P\" -o command=)\"; AP=\"$(printf '%s' \"$C\" | /usr/bin/sed -E 's#^(.+\\.app)/Contents/MacOS/.*#\\1#')\"; [ -d \"$AP\" ] || exit 25; /usr/bin/nohup /bin/sh -c 'P=\"$1\"; A=\"$2\"; N=\"$3\"; T=\"$4\"; while /bin/kill -0 \"$P\" 2>/dev/null; do /bin/sleep 0.2; done; B=\"${A}.previous\"; /bin/rm -rf \"$B\"; if /bin/mv \"$A\" \"$B\" && /bin/mv \"$N\" \"$A\"; then /bin/rm -rf \"$B\"; /usr/bin/open \"$A\"; else [ -d \"$B\" ] && /bin/mv \"$B\" \"$A\"; /usr/bin/open \"$A\"; exit 1; fi; /bin/rm -rf \"$T\"' ccusage-install \"$P\" \"$AP\" \"$N\" \"$T\" >/tmp/ccusage-updater.log 2>&1 & echo staged"),asciiBytes("ccusage-updater"),model.updateUrl,model.updateSha256,model.updateVersion], { key: "update-stage", collect: true, exit: "update_stage_done", err: "update_stage_failed" })];
      }
      if (model.updateState === "staging" || model.updateState === "restarting") return [model, Cmd.none];
      return [{ ...model, updateState: "checking", updateDetail: asciiBytes("Checking the latest GitHub release...") }, Cmd.spawn([asciiBytes("/usr/bin/curl"),asciiBytes("-fsSL"),asciiBytes("--max-time"),asciiBytes("12"),asciiBytes("https://github.com/vedanth-jadhav/ccusage-wrap/releases/latest/download/macos-update-feed.txt")], { key: "update-feed", collect: true, exit: "update_check_done", err: "update_check_failed" })];
    case "update_check_done": {
      if (msg.code !== 0) return [{ ...model, updateState: "failed", updateDetail: asciiBytes("Could not read the release feed. Try again.") }, Cmd.none];
      const feed = parseReleaseFeed(msg.output);
      if (feed === null) return [{ ...model, updateState: "failed", updateDetail: asciiBytes("The release feed was invalid. Try again later.") }, Cmd.none];
      if (isNewerRelease(feed.version, CURRENT_VERSION)) return [{ ...model, updateState: "available", updateVersion: feed.version, updateUrl: feed.url, updateSha256: feed.sha256, updateDetail: asciiBytes("A newer GitHub release is ready.") }, Cmd.none];
      return [{ ...model, updateState: "current", updateVersion: feed.version, updateUrl: EMPTY, updateSha256: EMPTY, updateDetail: asciiBytes("You are on the latest release.") }, Cmd.none];
    }
    case "update_check_failed": return [{ ...model, updateState: "failed", updateDetail: asciiBytes("Update check failed. Check your connection and retry.") }, Cmd.none];
    case "update_stage_done":
      if (msg.code === 0) return [{ ...model, updateState: "restarting", updateDetail: asciiBytes("Update verified. Restarting into the new version...") }, Cmd.quitApp()];
      return [{ ...model, updateState: "failed", updateDetail: msg.output.length > 0 ? msg.output : asciiBytes("The update could not be installed.") }, Cmd.none];
    case "update_stage_failed": return [{ ...model, updateState: "failed", updateDetail: msg.reason }, Cmd.none];
  }
}

function percentBytes(value: number): Uint8Array { return asciiBytes(`${Math.round(value)}%`); }
function windowFor(model: Model, kind: "primary" | "secondary") { return model.rateSnapshot === null ? null : model.rateSnapshot[kind]; }
function resetBytes(seconds: number | null): Uint8Array {
  if (seconds === null) return asciiBytes("Unavailable");
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return asciiBytes(`${minutes}m`);
  const hours = Math.floor(minutes / 60);
  const mins = minutes % 60;
  return asciiBytes(mins === 0 ? `${hours}h` : `${hours}h ${mins}m`);
}
function paceLabel(model: Model, kind: "primary" | "secondary"): Uint8Array {
  const pace = paceFor(windowFor(model, kind));
  if (pace === null) return asciiBytes("Waiting for live data");
  if (pace.status === "on_track") return asciiBytes("On pace with this window");
  if (pace.status === "ahead") return asciiBytes(`${Math.round(Math.abs(pace.deltaPercent))} pts faster than elapsed time`);
  return asciiBytes(`${Math.round(Math.abs(pace.deltaPercent))} pts below elapsed time`);
}

export function usageSelected(model: Model): boolean { return model.section === "usage"; }
export function limitsSelected(model: Model): boolean { return model.section === "limits"; }
export function usageVisible(model: Model): boolean { return model.section === "usage"; }
export function limitsVisible(model: Model): boolean { return model.section === "limits"; }
export function rateRefreshDisabled(model: Model): boolean { return model.rateState === "loading"; }
export function ratePlan(model: Model): Uint8Array { return model.rateSnapshot === null ? asciiBytes("Codex") : model.rateSnapshot.plan; }
export function rateDetail(model: Model): Uint8Array { return model.rateDetailText; }
export function fiveHourUsed(model: Model): Uint8Array { const w=windowFor(model,"primary"); return w === null ? asciiBytes("--") : percentBytes(w.usedPercent); }
export function fiveHourRemaining(model: Model): Uint8Array { const p=paceFor(windowFor(model,"primary")); return p === null ? asciiBytes("--") : percentBytes(p.remainingPercent); }
export function fiveHourReset(model: Model): Uint8Array { const w=windowFor(model,"primary"); return resetBytes(w === null ? null : w.resetInSeconds); }
export function fiveHourPace(model: Model): Uint8Array { return paceLabel(model,"primary"); }
export function weeklyUsed(model: Model): Uint8Array { const w=windowFor(model,"secondary"); return w === null ? asciiBytes("--") : percentBytes(w.usedPercent); }
export function weeklyRemaining(model: Model): Uint8Array { const p=paceFor(windowFor(model,"secondary")); return p === null ? asciiBytes("--") : percentBytes(p.remainingPercent); }
export function weeklyReset(model: Model): Uint8Array { const w=windowFor(model,"secondary"); return resetBytes(w === null ? null : w.resetInSeconds); }
export function weeklyPace(model: Model): Uint8Array { return paceLabel(model,"secondary"); }
export function fiveHourUsageSeries(model: Model): readonly number[] { const w=windowFor(model,"primary"); return w === null ? EMPTY_SERIES : [w.usedPercent/100]; }
export function fiveHourTimeSeries(model: Model): readonly number[] { const p=paceFor(windowFor(model,"primary")); return p === null ? EMPTY_SERIES : [p.expectedUsedPercent/100]; }
export function weeklyUsageSeries(model: Model): readonly number[] { const w=windowFor(model,"secondary"); return w === null ? EMPTY_SERIES : [w.usedPercent/100]; }
export function weeklyTimeSeries(model: Model): readonly number[] { const p=paceFor(windowFor(model,"secondary")); return p === null ? EMPTY_SERIES : [p.expectedUsedPercent/100]; }

export function updateActionLabel(model: Model): Uint8Array { if (model.updateState === "available") return INSTALL_UPDATE; if (model.updateState === "staging") return INSTALLING; if (model.updateState === "restarting") return RESTARTING; return CHECK_UPDATES; }
export function updateVersionText(model: Model): Uint8Array { return model.updateState === "available" ? model.updateVersion : CURRENT_VERSION; }
export function dateRange(model: Model): Uint8Array { if (model.range === "seven") return asciiBytes("Aug 3 to Aug 9"); if (model.range === "ninety") return asciiBytes("May 12 to Aug 9"); return asciiBytes("Jul 11 to Aug 9"); }
export function rawTokenCost(model: Model): Uint8Array { if (model.range === "seven") return asciiBytes("$21,744.10*"); if (model.range === "ninety") return asciiBytes("$128,230.61*"); return asciiBytes("$54,890.48*"); }
export function refreshStatus(model: Model): Uint8Array { return model.refreshed ? UPDATED_NOW : LOCAL_DATA; }
export function range7Selected(model: Model): boolean { return model.range === "seven"; }
export function range30Selected(model: Model): boolean { return model.range === "thirty"; }
export function range90Selected(model: Model): boolean { return model.range === "ninety"; }
export function costSelected(model: Model): boolean { return model.metric === "cost"; }
export function tokensSelected(model: Model): boolean { return model.metric === "tokens"; }
export function codexSeries(model: Model): readonly number[] { return model.metric === "tokens" ? CODEX_TOKENS : CODEX_COST; }
export function claudeSeries(model: Model): readonly number[] { return model.metric === "tokens" ? CLAUDE_TOKENS : CLAUDE_COST; }
export function providers(_model: Model): readonly ProviderRow[] { return PROVIDERS; }
export function totals(_model: Model): readonly TotalRow[] { return TOTALS; }
export function breakdown(_model: Model): readonly BreakdownRow[] { return BREAKDOWN; }
