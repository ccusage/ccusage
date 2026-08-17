import { Cmd, asciiBytes } from "@native-sdk/core";
import { isNewerRelease, parseReleaseFeed } from "./updater.ts";

export type Range = "seven" | "thirty" | "ninety";
export type Metric = "cost" | "tokens";
export type UpdateState = "checking" | "current" | "available" | "staging" | "restarting" | "error";

export interface ProviderRow {
  readonly name: Uint8Array;
  readonly amount: Uint8Array;
  readonly detail: Uint8Array;
  readonly share: Uint8Array;
}

export interface TotalRow {
  readonly label: Uint8Array;
  readonly value: Uint8Array;
  readonly detail: Uint8Array;
}

export interface BreakdownRow {
  readonly id: Uint8Array;
  readonly model: Uint8Array;
  readonly provider: Uint8Array;
  readonly cost: Uint8Array;
  readonly share: Uint8Array;
  readonly tokens: Uint8Array;
}

export interface Model {
  readonly range: Range;
  readonly metric: Metric;
  readonly refreshed: boolean;
  readonly updateState: UpdateState;
  readonly updateVersion: Uint8Array;
  readonly updateUrl: Uint8Array;
  readonly updateSha256: Uint8Array;
  readonly updateDetail: Uint8Array;
}

export type Msg =
  | { readonly kind: "range7" }
  | { readonly kind: "range30" }
  | { readonly kind: "range90" }
  | { readonly kind: "cost" }
  | { readonly kind: "tokens" }
  | { readonly kind: "refresh" }
  | { readonly kind: "update_action" }
  | { readonly kind: "update_feed"; readonly status: number; readonly body: Uint8Array }
  | { readonly kind: "update_feed_failed"; readonly reason: Uint8Array }
  | { readonly kind: "update_stage_done"; readonly code: number; readonly output: Uint8Array }
  | { readonly kind: "update_stage_failed"; readonly reason: Uint8Array };

export const viewUnbound = [
  "updateUrl",
  "updateSha256",
  "update_feed",
  "update_feed_failed",
  "update_stage_done",
  "update_stage_failed",
] as const;

export function initialModel(): [Model, Cmd<Msg>] {
  return [
    {
      range: "thirty",
      metric: "cost",
      refreshed: false,
      updateState: "checking",
      updateVersion: asciiBytes(""),
      updateUrl: asciiBytes(""),
      updateSha256: asciiBytes(""),
      updateDetail: asciiBytes("Checking the latest GitHub release..."),
    },
    Cmd.fetch(
      { url: asciiBytes("https://github.com/vedanth-jadhav/ccusage-wrap/releases/latest/download/macos-update-feed.txt"), timeoutMs: 12000 },
      { key: "update-feed", ok: "update_feed", err: "update_feed_failed" },
    ),
  ];
}

export function update(model: Model, msg: Msg): [Model, Cmd<Msg>] {
  switch (msg.kind) {
    case "range7": return [{ ...model, range: "seven" }, Cmd.none];
    case "range30": return [{ ...model, range: "thirty" }, Cmd.none];
    case "range90": return [{ ...model, range: "ninety" }, Cmd.none];
    case "cost": return [{ ...model, metric: "cost" }, Cmd.none];
    case "tokens": return [{ ...model, metric: "tokens" }, Cmd.none];
    case "refresh": return [{ ...model, refreshed: true }, Cmd.none];
    case "update_action":
      if (model.updateState === "available") {
        return [
          { ...model, updateState: "staging", updateDetail: asciiBytes("Downloading and verifying the update...") },
          Cmd.spawn(
            [
              asciiBytes("/bin/sh"),
              asciiBytes("-c"),
              asciiBytes("set -eu; U=\"$1\"; H=\"$2\"; V=\"$3\"; T=\"$(mktemp -d /tmp/ccusage-update.XXXXXX)\"; Z=\"$T/update.zip\"; /usr/bin/curl -fL --retry 3 --connect-timeout 15 \"$U\" -o \"$Z\"; A=\"$(/usr/bin/shasum -a 256 \"$Z\" | /usr/bin/awk '{print $1}')\"; [ \"$A\" = \"$H\" ] || { echo checksum_mismatch; exit 21; }; mkdir \"$T/unpack\"; /usr/bin/ditto -x -k \"$Z\" \"$T/unpack\"; N=\"$(/usr/bin/find \"$T/unpack\" -maxdepth 1 -name '*.app' -print -quit)\"; [ -n \"$N\" ] || { echo app_missing; exit 22; }; /usr/bin/codesign --verify --deep --strict \"$N\" || { echo signature_invalid; exit 23; }; NV=\"$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \"$N/Contents/Info.plist\")\"; [ \"$NV\" = \"$V\" ] || { echo version_mismatch; exit 24; }; P=\"$PPID\"; C=\"$(/bin/ps -p \"$P\" -o command=)\"; AP=\"$(printf '%s' \"$C\" | /usr/bin/sed -E 's#^(.+\\.app)/Contents/MacOS/.*#\\1#')\"; [ -d \"$AP\" ] || { echo app_path_unknown; exit 25; }; /usr/bin/nohup /bin/sh -c 'P=\"$1\"; A=\"$2\"; N=\"$3\"; T=\"$4\"; while /bin/kill -0 \"$P\" 2>/dev/null; do /bin/sleep 0.2; done; B=\"${A}.previous\"; /bin/rm -rf \"$B\"; if /bin/mv \"$A\" \"$B\" && /bin/mv \"$N\" \"$A\"; then /bin/rm -rf \"$B\"; /usr/bin/open \"$A\"; else [ -d \"$B\" ] && /bin/mv \"$B\" \"$A\"; /usr/bin/open \"$A\"; exit 1; fi; /bin/rm -rf \"$T\"' ccusage-install \"$P\" \"$AP\" \"$N\" \"$T\" >/tmp/ccusage-updater.log 2>&1 & echo staged"),
              asciiBytes("ccusage-updater"),
              model.updateUrl,
              model.updateSha256,
              model.updateVersion,
            ],
            { key: "update-stage", collect: true, exit: "update_stage_done", err: "update_stage_failed" },
          ),
        ];
      }
      if (model.updateState === "staging" || model.updateState === "restarting") return [model, Cmd.none];
      return [
        { ...model, updateState: "checking", updateDetail: asciiBytes("Checking the latest GitHub release...") },
        Cmd.fetch(
          { url: asciiBytes("https://github.com/vedanth-jadhav/ccusage-wrap/releases/latest/download/macos-update-feed.txt"), timeoutMs: 12000 },
          { key: "update-feed", ok: "update_feed", err: "update_feed_failed" },
        ),
      ];
    case "update_feed": {
      if (msg.status !== 200) {
        return [
          { ...model, updateState: "error", updateDetail: asciiBytes("Could not read the release feed. Try again.") },
          Cmd.none,
        ];
      }
      const feed = parseReleaseFeed(msg.body);
      if (feed === null) {
        return [
          { ...model, updateState: "error", updateDetail: asciiBytes("The release feed was invalid. Try again later.") },
          Cmd.none,
        ];
      }
      if (isNewerRelease(feed.version, asciiBytes("0.1.0"))) {
        return [
          {
            ...model,
            updateState: "available",
            updateVersion: feed.version,
            updateUrl: feed.url,
            updateSha256: feed.sha256,
            updateDetail: asciiBytes("A newer GitHub release is ready."),
          },
          Cmd.none,
        ];
      }
      return [
        {
          ...model,
          updateState: "current",
          updateVersion: feed.version,
          updateUrl: asciiBytes(""),
          updateSha256: asciiBytes(""),
          updateDetail: asciiBytes("You are on the latest release."),
        },
        Cmd.none,
      ];
    }
    case "update_feed_failed":
      return [
        { ...model, updateState: "error", updateDetail: asciiBytes("Update check failed. Check your connection and retry.") },
        Cmd.none,
      ];
    case "update_stage_done":
      if (msg.code === 0) {
        return [
          { ...model, updateState: "restarting", updateDetail: asciiBytes("Update verified. Restarting into the new version...") },
          Cmd.quitApp(),
        ];
      }
      return [
        {
          ...model,
          updateState: "error",
          updateDetail: msg.output.length > 0 ? msg.output.trim() : asciiBytes("The update could not be installed."),
        },
        Cmd.none,
      ];
    case "update_stage_failed":
      return [
        { ...model, updateState: "error", updateDetail: msg.reason },
        Cmd.none,
      ];
  }
}

export function updateActionLabel(model: Model): Uint8Array {
  if (model.updateState === "available") return asciiBytes("Install update");
  if (model.updateState === "staging") return asciiBytes("Installing...");
  if (model.updateState === "restarting") return asciiBytes("Restarting...");
  return asciiBytes("Check for updates");
}

export function updateVersionText(model: Model): Uint8Array {
  if (model.updateState === "available") return model.updateVersion;
  return asciiBytes("0.1.0");
}

export function dateRange(model: Model): Uint8Array {
  if (model.range === "seven") return asciiBytes("Aug 3 to Aug 9");
  if (model.range === "ninety") return asciiBytes("May 12 to Aug 9");
  return asciiBytes("Jul 11 to Aug 9");
}

export function rawTokenCost(model: Model): Uint8Array {
  if (model.range === "seven") return asciiBytes("$21,744.10*");
  if (model.range === "ninety") return asciiBytes("$128,230.61*");
  return asciiBytes("$54,890.48*");
}

export function refreshStatus(model: Model): Uint8Array {
  return model.refreshed ? asciiBytes("Updated just now") : asciiBytes("Local usage data");
}

export function range7Selected(model: Model): boolean { return model.range === "seven"; }
export function range30Selected(model: Model): boolean { return model.range === "thirty"; }
export function range90Selected(model: Model): boolean { return model.range === "ninety"; }
export function costSelected(model: Model): boolean { return model.metric === "cost"; }
export function tokensSelected(model: Model): boolean { return model.metric === "tokens"; }

export function codexSeries(model: Model): readonly number[] {
  if (model.metric === "tokens") return [0.01,0.01,0.03,0.01,0.00,0.00,0.00,0.00,0.01,0.02,0.02,0.03,0.02,0.01,0.03,0.02,0.02,0.02,0.14,0.22,0.66,0.02,0.01,0.06,0.03,0.16,0.64,0.91,0.17,0.07];
  return [0.01,0.01,0.05,0.02,0.00,0.00,0.00,0.00,0.01,0.04,0.03,0.05,0.04,0.01,0.04,0.03,0.03,0.02,0.16,0.23,0.68,0.01,0.02,0.06,0.03,0.21,0.76,0.95,0.27,0.09];
}

export function claudeSeries(model: Model): readonly number[] {
  if (model.metric === "tokens") return [0.01,0.01,0.08,0.03,0.07,0.01,0.00,0.00,0.03,0.11,0.06,0.11,0.09,0.02,0.07,0.06,0.05,0.03,0.06,0.05,0.04,0.01,0.03,0.05,0.05,0.12,0.08,0.20,0.14,0.11];
  return [0.01,0.01,0.03,0.02,0.06,0.01,0.00,0.00,0.02,0.06,0.04,0.06,0.05,0.01,0.05,0.04,0.03,0.02,0.04,0.04,0.03,0.01,0.02,0.03,0.03,0.10,0.09,0.23,0.12,0.08];
}

export function providers(_model: Model): readonly ProviderRow[] {
  return [
    { name: asciiBytes("Codex"), amount: asciiBytes("$35,304.93"), detail: asciiBytes("64.3% of cost / 48.8B tokens"), share: asciiBytes("64.3%") },
    { name: asciiBytes("Claude Code"), amount: asciiBytes("$19,585.54"), detail: asciiBytes("35.7% of cost / 17.2B tokens"), share: asciiBytes("35.7%") },
  ];
}

export function totals(_model: Model): readonly TotalRow[] {
  return [
    { label: asciiBytes("Processed tokens"), value: asciiBytes("65.9B"), detail: asciiBytes("2.27B per active day") },
    { label: asciiBytes("Cached input"), value: asciiBytes("63.3B"), detail: asciiBytes("97.5% of observed input") },
    { label: asciiBytes("Uncached input"), value: asciiBytes("1.61B"), detail: asciiBytes("582M cache writes") },
    { label: asciiBytes("Output"), value: asciiBytes("193M"), detail: asciiBytes("includes reasoning") },
    { label: asciiBytes("Cache savings"), value: asciiBytes("$320,610.13"), detail: asciiBytes("5.8x the raw cost") },
  ];
}

export function breakdown(_model: Model): readonly BreakdownRow[] {
  return [
    { id: asciiBytes("codex-sol"), model: asciiBytes("gpt-5.6-sol"), provider: asciiBytes("Codex"), cost: asciiBytes("$35,098.97"), share: asciiBytes("63.9%"), tokens: asciiBytes("48.6B") },
    { id: asciiBytes("fable-5"), model: asciiBytes("claude-fable-5"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$12,769.21"), share: asciiBytes("23.3%"), tokens: asciiBytes("8.14B") },
    { id: asciiBytes("opus-5"), model: asciiBytes("claude-opus-5"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$6,267.20"), share: asciiBytes("11.4%"), tokens: asciiBytes("8.27B") },
    { id: asciiBytes("claude-sol"), model: asciiBytes("gpt-5.6-sol"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$205.18"), share: asciiBytes("0.4%"), tokens: asciiBytes("207M") },
    { id: asciiBytes("opus-4-8"), model: asciiBytes("claude-opus-4-8"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$127.77"), share: asciiBytes("0.2%"), tokens: asciiBytes("131M") },
    { id: asciiBytes("sonnet-5"), model: asciiBytes("claude-sonnet-5"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$16.98"), share: asciiBytes("0.0%"), tokens: asciiBytes("33.7M") },
    { id: asciiBytes("haiku-4-5"), model: asciiBytes("claude-haiku-4-5"), provider: asciiBytes("Claude Code"), cost: asciiBytes("$2.89"), share: asciiBytes("0.0%"), tokens: asciiBytes("16.2M") },
    { id: asciiBytes("terra"), model: asciiBytes("gpt-5.6-terra"), provider: asciiBytes("Codex"), cost: asciiBytes("$0.78"), share: asciiBytes("0.0%"), tokens: asciiBytes("1.35M") },
  ];
}
