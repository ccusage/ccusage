import { Cmd, asciiBytes } from "@native-sdk/core";

export type Page = "overview" | "agents" | "models" | "sessions";

export interface DayRow {
  readonly day: Uint8Array;
  readonly date: Uint8Array;
  readonly tokens: Uint8Array;
  readonly cost: Uint8Array;
  readonly change: Uint8Array;
  readonly changeUp: boolean;
}

export interface AgentRow {
  readonly name: Uint8Array;
  readonly detail: Uint8Array;
  readonly tokens: Uint8Array;
  readonly cost: Uint8Array;
  readonly share: Uint8Array;
}

export interface Model {
  readonly page: Page;
  readonly range: Uint8Array;
  readonly refreshed: boolean;
}

export type Msg =
  | { readonly kind: "overview" }
  | { readonly kind: "agents" }
  | { readonly kind: "models" }
  | { readonly kind: "sessions" }
  | { readonly kind: "refresh" };

export function initialModel(): [Model, Cmd<Msg>] {
  return [{ page: "overview", range: asciiBytes("Last 30 days"), refreshed: false }, Cmd.none];
}

export function update(model: Model, msg: Msg): [Model, Cmd<Msg>] {
  switch (msg.kind) {
    case "overview": return [{ ...model, page: "overview" }, Cmd.none];
    case "agents": return [{ ...model, page: "agents" }, Cmd.none];
    case "models": return [{ ...model, page: "models" }, Cmd.none];
    case "sessions": return [{ ...model, page: "sessions" }, Cmd.none];
    case "refresh": return [{ ...model, refreshed: true }, Cmd.none];
  }
}

export function title(_model: Model): Uint8Array { return asciiBytes("Usage overview"); }
export function subtitle(model: Model): Uint8Array {
  return model.refreshed ? asciiBytes("Refreshed just now / local ccusage data") : asciiBytes("Local coding-agent usage / updated 2 min ago");
}
export function totalCost(_model: Model): Uint8Array { return asciiBytes("$184.72"); }
export function totalTokens(_model: Model): Uint8Array { return asciiBytes("48.6M"); }
export function activeDays(_model: Model): Uint8Array { return asciiBytes("24"); }
export function avgDaily(_model: Model): Uint8Array { return asciiBytes("$7.70"); }
export function costDelta(_model: Model): Uint8Array { return asciiBytes("+12.4%"); }
export function tokenDelta(_model: Model): Uint8Array { return asciiBytes("+8.1%"); }
export function dayDelta(_model: Model): Uint8Array { return asciiBytes("+3 days"); }
export function avgDelta(_model: Model): Uint8Array { return asciiBytes("+$0.82"); }

export function costHistory(_model: Model): readonly number[] {
  return [0.19,0.27,0.22,0.34,0.31,0.42,0.37,0.48,0.43,0.39,0.52,0.58,0.47,0.62,0.55,0.68,0.64,0.73,0.69,0.78,0.71,0.82,0.76,0.88,0.81,0.92,0.84,0.96,0.89,1.0];
}
export function tokenHistory(_model: Model): readonly number[] {
  return [0.22,0.18,0.29,0.25,0.33,0.38,0.31,0.44,0.40,0.49,0.46,0.54,0.50,0.61,0.56,0.66,0.60,0.72,0.67,0.75,0.70,0.80,0.73,0.85,0.79,0.90,0.83,0.94,0.87,0.97];
}

export function recentDays(_model: Model): readonly DayRow[] {
  return [
    { day: asciiBytes("Today"), date: asciiBytes("Aug 16"), tokens: asciiBytes("2.84M"), cost: asciiBytes("$11.92"), change: asciiBytes("+18%"), changeUp: true },
    { day: asciiBytes("Saturday"), date: asciiBytes("Aug 15"), tokens: asciiBytes("1.96M"), cost: asciiBytes("$8.44"), change: asciiBytes("-6%"), changeUp: false },
    { day: asciiBytes("Friday"), date: asciiBytes("Aug 14"), tokens: asciiBytes("2.31M"), cost: asciiBytes("$9.67"), change: asciiBytes("+9%"), changeUp: true },
    { day: asciiBytes("Thursday"), date: asciiBytes("Aug 13"), tokens: asciiBytes("1.72M"), cost: asciiBytes("$7.21"), change: asciiBytes("-12%"), changeUp: false },
    { day: asciiBytes("Wednesday"), date: asciiBytes("Aug 12"), tokens: asciiBytes("2.48M"), cost: asciiBytes("$10.36"), change: asciiBytes("+21%"), changeUp: true },
  ];
}

export function topAgents(_model: Model): readonly AgentRow[] {
  return [
    { name: asciiBytes("Codex"), detail: asciiBytes("GPT-5.6 Sol / 42 sessions"), tokens: asciiBytes("21.4M"), cost: asciiBytes("$82.16"), share: asciiBytes("44.5%") },
    { name: asciiBytes("Claude Code"), detail: asciiBytes("Opus 4.1 / 31 sessions"), tokens: asciiBytes("15.8M"), cost: asciiBytes("$64.09"), share: asciiBytes("34.7%") },
    { name: asciiBytes("OpenCode"), detail: asciiBytes("mixed models / 18 sessions"), tokens: asciiBytes("7.6M"), cost: asciiBytes("$27.31"), share: asciiBytes("14.8%") },
    { name: asciiBytes("Gemini CLI"), detail: asciiBytes("Gemini 2.5 Pro / 9 sessions"), tokens: asciiBytes("3.8M"), cost: asciiBytes("$11.16"), share: asciiBytes("6.0%") },
  ];
}

export function pageIsOverview(model: Model): boolean { return model.page === "overview"; }
export function pageIsAgents(model: Model): boolean { return model.page === "agents"; }
export function pageIsModels(model: Model): boolean { return model.page === "models"; }
export function pageIsSessions(model: Model): boolean { return model.page === "sessions"; }
