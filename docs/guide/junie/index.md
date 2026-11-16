# Junie Data Source (Experimental)

> Junie support is experimental. Junie does not document its session file format, so expect changes while ccusage and [Junie](https://junie.jetbrains.com/) continue to evolve.

ccusage reads local usage data from JetBrains Junie sessions. It supports the standard daily, weekly, monthly, and session reports.

## Usage

```bash
# Daily, weekly, and monthly usage
ccusage junie daily
ccusage junie weekly
ccusage junie monthly

# Usage grouped by Junie session
ccusage junie session
```

Use `ccusage daily`, `ccusage weekly`, `ccusage monthly`, or `ccusage session` to include detected Junie data in a unified report.

## Data Locations

ccusage reads Junie sessions from `${JUNIE_HOME:-~/.junie}/sessions/`:

```text
sessions/
├── index.jsonl
└── <session-id>/
    └── events.jsonl
```

Only `events.jsonl` files in direct child directories of `sessions/` are read. `index.jsonl` supplies each session's project directory and creation time.

## Supported Reports

| Command                 | Description                  | Related Report                          |
| ----------------------- | ---------------------------- | --------------------------------------- |
| `ccusage junie daily`   | Group usage by day           | [Daily Usage](/guide/daily-reports)     |
| `ccusage junie weekly`  | Group usage by week          | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage junie monthly` | Group usage by month         | [Monthly Usage](/guide/monthly-reports) |
| `ccusage junie session` | Group usage by Junie session | [Session Usage](/guide/session-reports) |

Session IDs are the session directory names. Session rows show the project directory recorded in `index.jsonl`, or `Junie` when the session is missing from the index or its entry has no `projectDir`.

## Token and Cost Data

ccusage counts only `LlmResponseMetadataEvent` events. Each entry in an event's `modelUsage` array becomes one request, so an event that used several models is split per model:

| Junie field         | ccusage field         |
| ------------------- | --------------------- |
| `model`             | Model                 |
| `inputTokens`       | Input tokens          |
| `outputTokens`      | Output tokens         |
| `cacheInputTokens`  | Cache read tokens     |
| `cacheCreateTokens` | Cache creation tokens |
| `cost`              | Recorded cost         |

Requests are dated by the event's `timestampMs`. Older Junie builds wrote usage events without `timestampMs`; those requests are dated by the session's `createdAt` in `index.jsonl`, and skipped when the session has no index entry or its entry has no usable `createdAt`. Entries without a model, or with every token counter at zero, are skipped.

`auto` uses Junie's recorded cost when present and calculates from token counts when it is missing. `display` uses recorded costs only, while `calculate` estimates cost from token counts using known pricing. A model without known pricing remains visible; when calculation is needed, ccusage shows a missing-pricing warning. In JSON output, those models are listed in `totals.unpricedModels` and their `modelBreakdowns` entries carry `"missingPricing": true`; see [Unpriced Models](/guide/json-output#unpriced-models).

## Environment Variables

| Variable     | Description                                                      |
| ------------ | ---------------------------------------------------------------- |
| `JUNIE_HOME` | Junie home directory containing `sessions/`; replaces `~/.junie` |

## Troubleshooting

::: details No Junie usage data found
Check that `${JUNIE_HOME:-~/.junie}/sessions/<session-id>/events.jsonl` exists. If your Junie home lives elsewhere, set `JUNIE_HOME` to the directory that contains `sessions/`.
:::
