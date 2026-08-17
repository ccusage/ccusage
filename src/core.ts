import { Cmd, asciiBytes } from "@native-sdk/core";

export type Range = "seven" | "thirty" | "ninety";
export type Metric = "cost" | "tokens";

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
}

export type Msg =
  | { readonly kind: "range7" }
  | { readonly kind: "range30" }
  | { readonly kind: "range90" }
  | { readonly kind: "cost" }
  | { readonly kind: "tokens" }
  | { readonly kind: "refresh" };

export function initialModel(): [Model, Cmd<Msg>] {
  return [{ range: "thirty", metric: "cost", refreshed: false }, Cmd.none];
}

export function update(model: Model, msg: Msg): [Model, Cmd<Msg>] {
  switch (msg.kind) {
    case "range7": return [{ ...model, range: "seven" }, Cmd.none];
    case "range30": return [{ ...model, range: "thirty" }, Cmd.none];
    case "range90": return [{ ...model, range: "ninety" }, Cmd.none];
    case "cost": return [{ ...model, metric: "cost" }, Cmd.none];
    case "tokens": return [{ ...model, metric: "tokens" }, Cmd.none];
    case "refresh": return [{ ...model, refreshed: true }, Cmd.none];
  }
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

export function modeLabel(model: Model): Uint8Array {
  return model.metric === "cost" ? asciiBytes("Daily cost") : asciiBytes("Daily tokens");
}

export function range7Selected(model: Model): boolean { return model.range === "seven"; }
export function range30Selected(model: Model): boolean { return model.range === "thirty"; }
export function range90Selected(model: Model): boolean { return model.range === "ninety"; }
export function costSelected(model: Model): boolean { return model.metric === "cost"; }
export function tokensSelected(model: Model): boolean { return model.metric === "tokens"; }

export function dailySeries(model: Model): readonly number[] {
  if (model.metric === "tokens") {
    return [0.02,0.02,0.11,0.04,0.07,0.01,0.00,0.00,0.04,0.13,0.08,0.14,0.11,0.03,0.10,0.08,0.07,0.05,0.20,0.27,0.62,0.03,0.04,0.11,0.08,0.28,0.72,0.91,0.31,0.18];
  }
  return [0.01,0.01,0.08,0.03,0.06,0.01,0.00,0.00,0.03,0.10,0.07,0.11,0.09,0.02,0.08,0.06,0.05,0.04,0.16,0.23,0.68,0.02,0.03,0.09,0.06,0.31,0.76,0.95,0.39,0.17];
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
