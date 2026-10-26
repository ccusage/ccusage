# Cline Data Source (Experimental)

> Cline support is experimental. Expect changes while ccusage and [Cline](https://github.com/cline/cline) continue to evolve.

ccusage reads local usage data from the Cline editor extension and Cline CLI. It supports the standard daily, weekly, monthly, and session reports.

## Usage

```bash
# Daily, weekly, and monthly usage
ccusage cline daily
ccusage cline weekly
ccusage cline monthly

# Usage grouped by Cline conversation
ccusage cline session
```

Use `ccusage daily`, `ccusage weekly`, `ccusage monthly`, or `ccusage session` to include detected Cline data in a unified report.

## Data Locations

For the Cline editor extension, ccusage searches the standard `globalStorage` locations for VS Code, VS Code Insiders, Cursor, Windsurf, and VSCodium. The editor folder name in these paths is `Code`, `Code - Insiders`, `Cursor`, `Windsurf`, or `VSCodium`.

| Operating system | Cline extension storage root                                                           |
| ---------------- | -------------------------------------------------------------------------------------- |
| macOS            | `~/Library/Application Support/<editor>/User/globalStorage/saoudrizwan.claude-dev`     |
| Linux            | `${XDG_CONFIG_HOME:-$HOME/.config}/<editor>/User/globalStorage/saoudrizwan.claude-dev` |
| Windows          | `%APPDATA%\<editor>\User\globalStorage\saoudrizwan.claude-dev`                         |

On Windows, if `%APPDATA%` is unset, ccusage uses `%USERPROFILE%\AppData\Roaming`.

Each extension task stores its messages at:

```text
tasks/<task-id>/ui_messages.json
```

Older Cline CLI versions, including v2.8, may store task transcripts at:

```text
~/.cline/data/tasks/<task-id>/ui_messages.json
```

Cline CLI sessions are read from:

```text
~/.cline/data/sessions/<session-id>/<session-id>.messages.json
```

## Custom Data Roots

Set `CLINE_DATA_DIR` to one or more comma-separated roots. Setting it replaces the default search locations. Each root can contain an extension `tasks/` directory, a legacy CLI `data/tasks/` directory, a CLI `data/sessions/` directory, or a CLI `sessions/` directory.

```bash
CLINE_DATA_DIR="$HOME/.cline,/path/to/Code/User/globalStorage/saoudrizwan.claude-dev" ccusage cline daily
```

To read both extension and CLI data from custom locations, include both roots in `CLINE_DATA_DIR`.

## Supported Reports

| Command                 | Description                  | Related Report                          |
| ----------------------- | ---------------------------- | --------------------------------------- |
| `ccusage cline daily`   | Group usage by day           | [Daily Usage](/guide/daily-reports)     |
| `ccusage cline weekly`  | Group usage by week          | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage cline monthly` | Group usage by month         | [Monthly Usage](/guide/monthly-reports) |
| `ccusage cline session` | Group usage by Cline session | [Session Usage](/guide/session-reports) |

## Token and Cost Data

The extension parser counts only `say` records whose `say` value is `api_req_started`. Each needs its own `ts` timestamp, `modelInfo.modelId`, and flat JSON in `text` with `tokensIn`, `tokensOut`, `cacheReads`, and `cacheWrites`. The CLI parser counts assistant messages with their own timestamp, `modelInfo.id`, and `metrics` containing `inputTokens`, `outputTokens`, `cacheReadTokens`, and `cacheWriteTokens`. All four counters must be present as non-negative integer JSON numbers; an explicit `0` is valid, and missing fields are not filled with zero. Records with a missing, blank, or case-insensitive `unknown` model are skipped. A model is not inferred from `task_metadata` or conversation history.

Extension `ts` and CLI `ts` values must be non-negative integer epoch milliseconds within the supported date range. If CLI `ts` is absent, `timestamp` may use a timezone-qualified ISO string with whole seconds or exactly three fractional digits (`Z` or `±HH:MM`); an invalid present `ts` does not fall back to `timestamp`. Fractional or string-valued `ts`, and negative or out-of-range timestamps, are skipped. Requests with all four counters equal to zero are also skipped, even if they have a recorded cost; individual zero-valued counters are valid. Extension records marked `partial: true` are skipped even when their counters are complete, and counter-total overflow is skipped.

In CLI records, `inputTokens` includes cache reads and writes. ccusage subtracts those counts only when their sum is valid and no greater than `inputTokens`; otherwise, it skips the record. Extension `tokensIn` is already separate from cache counters. The `deleted_api_reqs` and `subagent_usage` aggregate rollups are excluded. Daily, weekly, monthly, and session reports therefore cover eligible requests only, not complete historical totals where records are missing or omitted. This also applies to legacy task files: incomplete records are skipped.

All cost modes follow the same eligibility rules; a recorded cost cannot rescue a record missing required counters, model, or timestamp. `auto` uses Cline's recorded cost when present and calculates only when that cost is missing and the model has known pricing. `display` uses recorded costs, while `calculate` estimates cost from token counts using known pricing for the request's timestamp. A recorded `$0.00` is distinct from a missing cost. A valid model ID without known pricing remains visible; when calculation is needed, cost is unavailable and ccusage shows a missing-pricing warning.

## Environment Variables

| Variable         | Description                                                                                                   |
| ---------------- | ------------------------------------------------------------------------------------------------------------- |
| `CLINE_DATA_DIR` | Comma-separated roots containing `tasks/`, `data/tasks/`, `data/sessions/`, or `sessions/`; replaces defaults |

## Troubleshooting

::: details No Cline usage data found
Check that the editor `globalStorage` root contains `tasks/<task-id>/ui_messages.json`, or that the CLI root contains `data/tasks/<task-id>/ui_messages.json`, `data/sessions/<session-id>/<session-id>.messages.json`, or `sessions/<session-id>/<session-id>.messages.json`. If your data lives elsewhere, set `CLINE_DATA_DIR` to the parent root that contains one of those directories.
:::
