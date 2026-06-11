# GitHub Copilot CLI Data Source (Beta)

> GitHub Copilot CLI support is experimental.

ccusage reads GitHub Copilot CLI usage from the per-session event stream the CLI writes by default (`~/.copilot/session-state/<sessionId>/events.jsonl`). No configuration is required. Reporting uses the same experience as the rest of ccusage: responsive tables, JSON output, LiteLLM-based pricing, cache token accounting, and all-source aggregation.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage copilot --help
```

```bash [npx]
npx ccusage@latest copilot --help
```

```bash [pnpm]
pnpm dlx ccusage copilot --help
```

:::

## Data Source

The Copilot CLI writes per-session events to `~/.copilot/session-state/<sessionId>/events.jsonl` automatically during each CLI session — no configuration is required.

```text
~/.copilot/
└── session-state/
    └── <sessionId>/
        └── events.jsonl
```

To override the base directory, set `COPILOT_CONFIG_DIR` to point elsewhere; ccusage will look for `session-state/` underneath it.

### session-state event schema

Each `events.jsonl` file contains a stream of JSON records. The adapter reads `session.shutdown` events only; their `data.modelMetrics` map carries per-model token totals (`usage.{inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, reasoningTokens}`) and request counts. A resumed Copilot session produces multiple shutdown events in the same file — each is a per-process snapshot, summed at report time.

## Report Views

| Focused view              | Description                        | See also                                |
| ------------------------- | ---------------------------------- | --------------------------------------- |
| `ccusage copilot daily`   | Aggregate usage by date            | [Daily Usage](/guide/daily-reports)     |
| `ccusage copilot monthly` | Aggregate usage by month           | [Monthly Usage](/guide/monthly-reports) |
| `ccusage copilot session` | Group usage by Copilot session IDs | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output, `--compact` for narrow terminals, and `--offline` for cached pricing data.

## Pricing modes

`ccusage copilot` supports the standard cost modes plus an `api` discoverability alias. For Copilot, the modes map to billing realities as follows:

| Mode | What it shows for Copilot |
|---|---|
| `auto` (default) | **True bill, billing-field-aware**: AI Credits when `totalNanoAiu` is present (typically post-cutover sessions, CLI ≥ 1.0.40), premium-request × $0.04 when only `requests.cost` is present (typically pre-cutover sessions), token-priced when neither billing field is recorded |
| `display` | Source-precomputed only: AI Credits → premium-requests → `$0.00`. No token-priced fallback |
| `calculate` / `api` | Always LiteLLM token pricing — what the same usage would have cost via the underlying provider's API |

Why billing-field-aware? GitHub Copilot switched billing models on June 1, 2026: pre-cutover sessions were billed as "premium requests" with per-model multipliers at $0.04 per overage request; post-cutover sessions bill as AI Credits at $0.01 per credit. The Copilot CLI records each session's own billing fields, so `auto` dispatches on which fields each session actually shipped (not on the recording date) — robust to backfills, future billing-channel additions, or sessions whose CLI version doesn't align with their date.

```bash
# Default: billing-field-aware true bill (AI Credits when totalNanoAiu is present, premium-requests otherwise).
ccusage copilot daily

# API-equivalent: what direct Anthropic/OpenAI/Google calls would have cost.
# Useful for justifying the Copilot subscription.
ccusage copilot daily --mode api

# Source-precomputed only: walks AIU → premium-requests → $0. No fallback.
ccusage copilot daily --mode display
```

ccusage reads `totalNanoAiu` from each `session.shutdown` event and converts directly to USD (`credits × $0.01`). When a session ships only `requests.cost` (no AIU field), the loader falls back to `requests.cost × $0.04`. No LiteLLM pricing lookup happens in `display` mode, so newer Copilot model variants that aren't yet in the LiteLLM dataset still report correct cost. The `credits` field is exposed in `--json` output regardless of the selected mode, so you can always see the raw credit count.

Credit-only `session.shutdown` rows (zero tokens, zero requests, non-null `totalNanoAiu`) are kept even when the default skip rule would otherwise discard them. Identical duplicate credit-only snapshots within the same session are collapsed via content-based deduplication. Free-tier models (sonnet, haiku with `requests.cost == 0`) genuinely cost $0 under the premium-request plan and are billed as such — not as their token-priced equivalent.

See [Cost Modes](/guide/cost-modes) for the full mode reference.

## What Gets Calculated

- **Token usage** — `inputTokens` in the session-state source schema already includes both cache reads and cache writes, so ccusage subtracts both to recover the "fresh" input bucket.
- **Cache tokens** — cache read and cache creation tokens are counted from the source's `cacheReadTokens` / `cacheWriteTokens` fields.
- **Reasoning tokens** — provider-dependent. For OpenAI (`gpt-*`) and Anthropic (`claude-*`) models reasoning is already inside `outputTokens` and is not double-counted. For Google (`gemini-*`) models reasoning is reported separately and is summed into the output cost bucket.
- **Credits** — when present, `totalNanoAiu` is divided by 10⁹ to get the AI Credit count (`1 credit = $0.01`). Always surfaced in JSON. Drives the cost figure under `auto` / `display` when present.
- **Pricing** — for `calculate` / `api` mode, costs are computed from LiteLLM pricing data using a normalized form of the model name. The raw name (e.g. `claude-opus-4.7-1m-internal`) is preserved in JSON, the table's `Models` column, and `modelsUsed`.

## Environment Variables

| Variable             | Description                                                                                  |
| -------------------- | -------------------------------------------------------------------------------------------- |
| `COPILOT_CONFIG_DIR` | Override the base Copilot directory (default: `~/.copilot`). `session-state/` is discovered underneath it. |
| `LOG_LEVEL`          | Adjust verbosity (0 silent ... 5 trace)                                                      |

## Troubleshooting

::: details No Copilot usage data found
The Copilot CLI writes per-session events to `~/.copilot/session-state/<sessionId>/events.jsonl` automatically. If `ccusage copilot daily` still reports no data, verify that the directory exists and that at least one of the per-session subdirectories contains an `events.jsonl` with a `session.shutdown` event (resumed sessions only produce shutdown rows on graceful exit).

If you have configured `COPILOT_CONFIG_DIR`, make sure it points at the directory that contains `session-state/`.
:::

::: details Costs showing as $0.00
There are two cases — most are genuine, only one is a problem.

**Genuine $0.00** (no fix needed):

- **Free-tier rows** under `--mode auto` / `--mode display` — sonnet/haiku entries shipped with `requests.cost == 0` legitimately cost $0 against your premium-request allotment. The source recorded zero; ccusage surfaces it faithfully.
- **Sessions without billing fields under `--mode display`** — rows that ship neither `totalNanoAiu` nor `requests.cost` resolve to $0 in display mode by design (display has no token-pricing fallback).

**Unexpected $0.00** (file an issue):

- A model is missing from LiteLLM's pricing dataset AND ccusage is on a token-priced path (`--mode calculate`, `--mode api`, or `--mode auto` falling back to token pricing because the session shipped neither `totalNanoAiu` nor `requests.cost`). In `--mode auto` / `--mode display`, rows with `totalNanoAiu` or `requests.cost` report cost directly from the source-precomputed values without consulting LiteLLM, so newer Copilot model variants that aren't yet in LiteLLM's database still bill correctly under those modes. If you hit this case on a token-priced path, [open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::

::: details "ccusage no longer reads OpenTelemetry exports" warning
If you ran an older ccusage version with `COPILOT_OTEL_FILE_EXPORTER_PATH`, `COPILOT_OTEL_DEDUP`, or `COPILOT_PREFER_OTEL` set, you will now see a one-time stderr warning that these variables are ignored. ccusage now reads `session-state/<sessionId>/events.jsonl` under `COPILOT_CONFIG_DIR` (or `~/.copilot` by default) — the OpenTelemetry export was a workaround for an older Copilot CLI that didn't expose its own usage data, and it is no longer needed. Unset the legacy variables (e.g. `unset COPILOT_OTEL_FILE_EXPORTER_PATH`) to silence the warning. Set `LOG_LEVEL=0` to silence it without unsetting the vars.
:::
