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

The extension's `ui_messages.json` records `api_req_started`, `deleted_api_reqs`, and `subagent_usage` events. Cline CLI message files provide assistant usage through their `metrics` fields. Both formats can include model information, token counts, and a recorded request cost. Rows without an attributed model appear as `unknown`.

Extension usage in `deleted_api_reqs` and `subagent_usage` rollups is dated using the rollup message timestamp because the original request timestamps are unavailable. Daily reports can therefore place some rolled-up usage on a different day from the requests that produced it.

Cache read and cache write counts are reported separately. In Cline CLI data, `inputTokens` includes cache reads and writes, so ccusage subtracts those counts from input tokens before reporting them. Extension `tokensIn` values are kept as recorded.

`auto` uses Cline's recorded cost when present. When it is missing, ccusage calculates cost only if the model has known pricing. `display` uses recorded costs, while `calculate` estimates cost from token counts using known model pricing for the request's historical timestamp. A recorded `$0.00` is valid and remains distinct from a missing cost. Unknown or unpriced models remain visible; when pricing is unavailable, ccusage surfaces a missing-pricing warning and leaves calculated cost unavailable.

## Environment Variables

| Variable         | Description                                                                                                   |
| ---------------- | ------------------------------------------------------------------------------------------------------------- |
| `CLINE_DATA_DIR` | Comma-separated roots containing `tasks/`, `data/tasks/`, `data/sessions/`, or `sessions/`; replaces defaults |

## Troubleshooting

::: details No Cline usage data found
Check that the editor `globalStorage` root contains `tasks/<task-id>/ui_messages.json`, or that the CLI root contains `data/tasks/<task-id>/ui_messages.json`, `data/sessions/<session-id>/<session-id>.messages.json`, or `sessions/<session-id>/<session-id>.messages.json`. If your data lives elsewhere, set `CLINE_DATA_DIR` to the parent root that contains one of those directories.
:::
