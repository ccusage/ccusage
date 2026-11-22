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

ccusage checks every location in this table on every operating system and skips the ones that do not exist. On Windows, if `%APPDATA%` is unset, ccusage uses `%USERPROFILE%\AppData\Roaming`.

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

Session IDs carry a prefix for the format they came from: `task:<task-id>` for extension tasks and legacy CLI tasks, and `cli:<session-id>` for CLI session transcripts. The prefix keeps a task and a CLI session that share an ID as separate rows.

## Token and Cost Data

Reports include complete usage records with a timestamp, a non-blank model ID
other than `unknown` (case-insensitive), and non-negative integer input, output,
cache-read, and cache-write counters. Individual zero counters are valid.
Reports exclude requests with all four counters at zero, partial records,
deleted-request rollups, and subagent aggregates. Missing or omitted records
can make the reported totals lower than actual historical usage.

CLI input includes cache tokens. ccusage subtracts their sum from input and
reports them separately. It skips the record if that sum exceeds the input
count or overflows. The extension already reports cache tokens separately. See the [Cline adapter README](https://github.com/ccusage/ccusage/blob/main/rust/adapters/cline/README.md)
for record formats and timestamp requirements.

All cost modes follow the same eligibility rules; a recorded cost cannot rescue a record missing required counters, model, or timestamp. `auto` uses Cline's recorded cost when present and calculates only when that cost is missing and the model has known pricing. `display` uses recorded costs, while `calculate` estimates cost from token counts using known pricing for the request's timestamp. A recorded `$0.00` is distinct from a missing cost. A valid model ID without known pricing remains visible; when calculation is needed, cost is unavailable and ccusage shows a missing-pricing warning.

## Environment Variables

| Variable         | Description                                                                                                   |
| ---------------- | ------------------------------------------------------------------------------------------------------------- |
| `CLINE_DATA_DIR` | Comma-separated roots containing `tasks/`, `data/tasks/`, `data/sessions/`, or `sessions/`; replaces defaults |

## Troubleshooting

::: details No Cline usage data found
Check that the editor `globalStorage` root contains `tasks/<task-id>/ui_messages.json`, or that the CLI root contains `data/tasks/<task-id>/ui_messages.json`, `data/sessions/<session-id>/<session-id>.messages.json`, or `sessions/<session-id>/<session-id>.messages.json`. If your data lives elsewhere, set `CLINE_DATA_DIR` to the parent root that contains one of those directories.
:::
