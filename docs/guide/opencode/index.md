# OpenCode Data Source (Beta)

> OpenCode support is experimental. Expect breaking changes while both ccusage and [OpenCode](https://github.com/opencode-ai/opencode) continue to evolve.

ccusage can read [OpenCode](https://github.com/opencode-ai/opencode) session logs as one of its supported local data sources. OpenCode is a terminal-based AI coding assistant that supports multiple AI providers.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage opencode --help
```

```bash [npx]
npx ccusage@latest opencode --help
```

```bash [pnpm]
pnpm dlx ccusage opencode --help
```

:::

## Data Source

The CLI reads OpenCode message and session JSON files from `OPENCODE_DATA_DIR` when it is set. Otherwise, it uses `${XDG_DATA_HOME:-$HOME/.local/share}/opencode`. `OPENCODE_DATA_DIR` can be one directory or a comma-separated list of directories.

```bash
OPENCODE_DATA_DIR="$HOME/.local/share/opencode,/backup/opencode" ccusage opencode daily
```

```
${XDG_DATA_HOME:-$HOME/.local/share}/opencode/
└── storage/
    ├── message/{sessionID}/msg_{messageID}.json
    └── session/{projectHash}/{sessionID}.json
```

## Report Views

| Focused view               | Description                                     | See also                                |
| -------------------------- | ----------------------------------------------- | --------------------------------------- |
| `ccusage opencode daily`   | Aggregate usage by date (YYYY-MM-DD)            | [Daily Usage](/guide/daily-reports)     |
| `ccusage opencode weekly`  | Aggregate usage by week start date (YYYY-MM-DD) | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage opencode monthly` | Aggregate usage by month (YYYY-MM)              | [Monthly Usage](/guide/monthly-reports) |
| `ccusage opencode session` | Usage grouped by session                        | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output and `--compact` for narrow terminals. See the linked ccusage documentation for detailed flag descriptions.

## Forked Sessions

When OpenCode v2 forks a session, it copies the parent's history into the fork. ccusage counts only the usage the fork generates itself, so copied turns are not counted twice in session, daily, weekly, or monthly reports. If the fork's parent session or boundary message is no longer in the database, the fork keeps all of its rows.

## Compaction Usage

OpenCode v2 records each context compaction as its own message, with the tokens and cost of the compaction request itself. ccusage counts completed compactions alongside assistant turns in every report, using the compaction's recorded model, timestamp, and cost. Running compactions carry no usage yet, and failed compactions record no model to attribute their usage to, so neither is counted.

A session report without a date range uses OpenCode's cumulative session totals for a session whose only remaining usage rows are compactions, such as after a revert removed its assistant turns. Those totals already include the compaction usage, so the compaction rows are not added on top.

## Claude Agent Provider

The `claude-agent` provider runs Claude Code under the hood, so each turn is also saved in Claude Code's own logs. In [unified reports](/guide/all-reports) such as `ccusage daily`, ccusage skips the OpenCode copy of turns the Claude source already counts, so they are priced from the Claude Code logs. OpenCode stores only a model alias such as `opus`, while Claude Code records the exact model, so its costs are accurate.

Matching uses the native Claude Code session ID, including session aliases retained when repeated calls are deduplicated. An OpenCode turn is excluded when Claude main-thread usage claims that session, even when the turn and its model calls fall on different dates.

With date bounds, claim discovery keeps `--since` unchanged and includes one extra calendar day after `--until` to cover model calls recorded after their turn crossed midnight. The report still contains only Claude usage inside the requested window. OpenCode rows remain when there is no main-thread Claude claim, for example when the native logs are missing, contain only subagent usage, or are outside that discovery margin.

Unified reports check for mirrors across all Claude projects, even when a project filter limits the Claude rows.

Focused OpenCode reports also check for mirrored sessions. Pass `--include-claude-agent` to include every `claude-agent` row:

```bash
ccusage opencode daily --include-claude-agent
```

Those rows are priced from the `opus` alias, so their cost can differ from the Claude report.

## Environment Variables

| Variable            | Description                                                                                                   |
| ------------------- | ------------------------------------------------------------------------------------------------------------- |
| `OPENCODE_DATA_DIR` | Override the root directory, or comma-separated root directories, containing OpenCode data                    |
| `XDG_DATA_HOME`     | Base directory for OpenCode data when `OPENCODE_DATA_DIR` is not set                                          |
| `LOG_LEVEL`         | `0` hides aggregated warnings, progress, and table titles ([details](/guide/environment-variables#log-level)) |

## Cost Calculation

`--mode auto` uses a positive recorded cost when available and otherwise prices tokens. `--mode calculate` always prices tokens, ignoring recorded cost. `--mode display` shows recorded cost only; tokens without a recorded cost contribute $0.

OpenCode uses `cost: 0` as an unknown-cost sentinel, so Auto still calculates those entries. Model aliases (for example, `gemini-3-pro-high` → `gemini-3-pro-preview`) are resolved automatically. See [Cost Modes](/guide/cost-modes).

## Troubleshooting

::: details No OpenCode usage data found
Ensure the data directory exists at `${XDG_DATA_HOME:-$HOME/.local/share}/opencode/storage/message/`. Set `OPENCODE_DATA_DIR` for custom paths or comma-separated archive roots.
:::

::: details Costs showing as $0.00
If ccusage cannot resolve a model price, the calculated cost is $0.00. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
