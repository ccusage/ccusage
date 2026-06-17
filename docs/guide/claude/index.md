# Claude Code Data Source

ccusage can read Claude Code usage data as one of its supported local data sources. Claude Code is no longer treated as the only ccusage target; it uses the same unified and focused report model as Codex, OpenCode, Amp, Droid, Codebuff, Hermes Agent, pi-agent, Goose, OpenClaw, Kilo, Kimi, Qwen, GitHub Copilot CLI, and Gemini CLI.

## Focused Views

```bash
# Daily Claude Code usage
ccusage claude daily

# Weekly Claude Code usage
ccusage claude weekly

# Monthly Claude Code usage
ccusage claude monthly

# Claude Code sessions
ccusage claude session
```

Most users can start with unified reports such as `ccusage daily`. Add the `claude` namespace only when you want to focus the same report shape on Claude Code usage or pass Claude-specific options.

## Data Source

ccusage reads Claude Code project logs from the standard Claude data directories:

| Source      | Default paths                                       |
| ----------- | --------------------------------------------------- |
| Claude Code | `~/.config/claude/projects/`, `~/.claude/projects/` |

The tool checks both locations and combines valid data. When `CLAUDE_CONFIG_DIR` is set, that value replaces the default lookup and can contain one directory or a comma-separated list of directories.

::: warning Retention
Claude Code can retain logs for only 30 days by default, deleting older session files on startup. To keep the underlying logs longer, change `cleanupPeriodDays` in your Claude Code settings.

[Claude Code settings - Claude Docs](https://docs.claude.com/en/docs/claude-code/settings#settings-files)
:::

### Historical cache

So that historical totals do not shrink when Claude Code prunes old logs, the `daily`, `weekly`, and `monthly` reports persist each day's computed totals to a small cache file (`<claude-dir>/ccusage/daily-cache.json`, outside the pruned `projects/` directory). On later runs, days whose logs were deleted are served from the cache, and live data always wins for days that still have logs — the cache only ever adds history back, never overrides or inflates current numbers.

- Pass `--no-cache` to ignore the cache and report strictly from the logs currently on disk.
- Set `CCUSAGE_CACHE_DIR` to store the cache somewhere other than the Claude data directory.
- `session` and `blocks` reports always read live data and are not cached.

## Report Views

| Focused view             | Description                   | See also                                |
| ------------------------ | ----------------------------- | --------------------------------------- |
| `ccusage claude daily`   | Aggregate usage by date       | [Daily Usage](/guide/daily-reports)     |
| `ccusage claude weekly`  | Aggregate usage by week       | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage claude monthly` | Aggregate usage by month      | [Monthly Usage](/guide/monthly-reports) |
| `ccusage claude session` | Group usage by Claude session | [Session Usage](/guide/session-reports) |

## Claude Code Features

Claude Code exposes additional local data that enables features beyond the shared report views:

- [Blocks](/guide/blocks-reports) - Claude Code 5-hour billing window analysis
- [Statusline](/guide/statusline) - Compact real-time usage display for Claude Code status bar hooks

## Environment Variables

| Variable            | Description                                          |
| ------------------- | --------------------------------------------------- |
| `CLAUDE_CONFIG_DIR` | Override the root Claude Code data directory         |
| `CCUSAGE_CACHE_DIR` | Override where the historical usage cache is stored  |
| `LOG_LEVEL`         | Adjust verbosity (0 silent ... 5 trace)             |

### Custom Claude Code Paths

Set `CLAUDE_CONFIG_DIR` when Claude Code logs live outside the default locations:

```bash
export CLAUDE_CONFIG_DIR="/path/to/your/claude/data"
ccusage claude daily
```

Use comma-separated directories to combine current and archived Claude Code data:

```bash
export CLAUDE_CONFIG_DIR="~/.config/claude,/backup/claude-archive"
ccusage claude monthly
```

For Codex, OpenCode, Amp, Droid, Codebuff, Hermes Agent, pi-agent, Goose, OpenClaw, Kilo, Kimi, Qwen, GitHub Copilot CLI, and Gemini CLI data locations, use the source-specific environment variables listed in [Environment Variables](/guide/environment-variables).

### Directory Detection

When `CLAUDE_CONFIG_DIR` is not set, ccusage searches in this order:

1. `~/.config/claude/projects/`
2. `~/.claude/projects/`

Data from all valid directories is combined automatically.

## Troubleshooting

::: details No Claude Code usage data found
Check whether your logs live under `~/.config/claude/projects/` or `~/.claude/projects/`. If your data lives elsewhere, set `CLAUDE_CONFIG_DIR` or use the relevant path option.
:::
