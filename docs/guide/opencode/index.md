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

```bash [opencode x]
BUN_BE_BUN=1 opencode x ccusage@latest opencode --help
```

:::

::: tip opencode x option
The `opencode x` option requires the native version of OpenCode. If you installed OpenCode via npm, use the `bunx` or `npx` options instead.
:::

## Data Source

The CLI reads OpenCode message and session JSON files from `OPENCODE_DATA_DIR` when it is set. Otherwise, it uses `${XDG_DATA_HOME:-$HOME/.local/share}/opencode`. `OPENCODE_DATA_DIR` can be one directory or a comma-separated list of directories.

```bash
OPENCODE_DATA_DIR="$HOME/.local/share/opencode,/backup/opencode" ccusage opencode daily
```

<!-- eslint-skip -->

```
${XDG_DATA_HOME:-$HOME/.local/share}/opencode/
└── storage/
    ├── message/{sessionID}/msg_{messageID}.json
    └── session/{projectHash}/{sessionID}.json
```

## Report Views

| Focused view               | Description                                          | See also                                |
| -------------------------- | ---------------------------------------------------- | --------------------------------------- |
| `ccusage opencode daily`   | Aggregate usage by date (YYYY-MM-DD)                 | [Daily Usage](/guide/daily-reports)     |
| `ccusage opencode weekly`  | Aggregate usage by ISO week (YYYY-Www)               | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage opencode monthly` | Aggregate usage by month (YYYY-MM)                   | [Monthly Usage](/guide/monthly-reports) |
| `ccusage opencode session` | Per-session breakdown with parent/subagent hierarchy | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output and `--compact` for narrow terminals. See the linked ccusage documentation for detailed flag descriptions.

## Session Hierarchy

OpenCode supports subagent sessions. The session report displays:

- **Bold titles** for parent sessions with subagents
- **Indented rows** (`↳`) for subagent sessions
- **Subtotal rows** combining parent + subagents

## Forked Sessions

When OpenCode v2 forks a session, it copies the parent's history into the fork. ccusage counts only the usage the fork generates itself, so copied turns are not counted twice in session, daily, weekly, or monthly reports. If the fork's parent session or boundary message is no longer in the database, the fork keeps all of its rows.

## Compaction Usage

OpenCode v2 records each context compaction as its own message, with the tokens and cost of the compaction request itself. ccusage counts completed compactions alongside assistant turns in every report, using the compaction's recorded model, timestamp, and cost. Running compactions carry no usage yet, and failed compactions record no model to attribute their usage to, so neither is counted.

A session report without a date range uses OpenCode's cumulative session totals for a session whose only remaining usage rows are compactions, such as after a revert removed its assistant turns. Those totals already include the compaction usage, so the compaction rows are not added on top.

## Claude Agent Provider

The `claude-agent` provider runs Claude Code under the hood, so each turn is also saved in Claude Code's own logs. In [unified reports](/guide/all-reports) such as `ccusage daily`, ccusage skips the OpenCode copy of turns the Claude source already counts, so they are priced from the Claude Code logs. OpenCode stores only a model alias such as `opus`, while Claude Code records the exact model, so its costs are accurate.

ccusage matches OpenCode rows to Claude usage by Claude Code session, not by individual turn:

- **Daily, weekly, and monthly reports** skip an OpenCode `claude-agent` row when the Claude source counted usage from the same Claude Code session on the row's date, in the Claude source's timezone. Rows still count when the Claude Code log is gone, holds no usage for that day, holds only subagent usage, or falls outside the Claude source's date range.
- **Session reports** count only the usage of each Claude Code session inside the Claude source's date range. They skip an OpenCode `claude-agent` row when the Claude source counted main-thread usage from the same session in that range and the row's date is in the range too. Resumed and forked transcripts repeat earlier turns, and ccusage keeps one copy of each repeated turn; the session of a dropped copy counts on the day of the kept one, as in daily reports.

Because OpenCode dates a turn by when it started and Claude Code dates each model call, a turn that runs across midnight can be off in the daily family. It is counted twice when Claude has no usage from that session on the day the turn started. With `--until` on that day, the calls Claude logs after midnight are left out, even though OpenCode dates the whole turn on the earlier day.

`ccusage opencode` skips the rows that `ccusage claude` counts. Every `ccusage opencode` report, session reports included, matches them by Claude Code session and date, because `ccusage claude` counts only usage dated inside its date range. This way the totals of `ccusage claude` and `ccusage opencode` for the same range add up to the matching unified report, such as `ccusage daily`, and tools that read each source separately count these turns once. These turns appear under Claude. Pass `--include-claude-agent` to show every `claude-agent` row in the OpenCode report:

```bash
ccusage opencode daily --include-claude-agent
```

Those rows are priced from the `opus` alias, so their cost can differ from the Claude report.

## Environment Variables

| Variable            | Description                                                                                |
| ------------------- | ------------------------------------------------------------------------------------------ |
| `OPENCODE_DATA_DIR` | Override the root directory, or comma-separated root directories, containing OpenCode data |
| `XDG_DATA_HOME`     | Base directory for OpenCode data when `OPENCODE_DATA_DIR` is not set                       |
| `LOG_LEVEL`         | Adjust verbosity (0 silent ... 5 trace)                                                    |

## Cost Calculation

OpenCode stores `cost: 0` in message files. Costs are calculated from token counts using LiteLLM pricing. Model aliases (e.g., `gemini-3-pro-high` → `gemini-3-pro-preview`) are handled automatically.

## Troubleshooting

::: details No OpenCode usage data found
Ensure the data directory exists at `${XDG_DATA_HOME:-$HOME/.local/share}/opencode/storage/message/`. Set `OPENCODE_DATA_DIR` for custom paths or comma-separated archive roots.
:::

::: details Costs showing as $0.00
If a model is not in LiteLLM's database, the cost will be $0.00. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
