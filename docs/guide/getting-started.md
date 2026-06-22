# Getting Started

Welcome to ccusage! This guide will help you get up and running with analyzing your coding (agent) CLI usage data.

## Prerequisites

- At least one supported coding CLI installed and used
- Bun 1.3+ recommended for direct execution

## Quick Start

The fastest way to try ccusage is to run it directly without installation:

::: code-group

```bash [bunx (Recommended)]
bunx ccusage
```

```bash [Nix]
nix run github:ccusage/ccusage -- daily
```

```bash [npx]
npx ccusage@latest
```

```bash [pnpm]
pnpm dlx ccusage
```

```bash [pkg.pr.new preview]
bunx -p https://pkg.pr.new/ccusage/ccusage@<pr-number> ccusage --offline
```

:::

This will show your daily usage report for all detected supported coding CLIs by default.

::: tip Runtime
[bunx](https://bun.com/docs/pm/bunx) caches the downloaded package, so repeated runs are faster after the first launch.
:::

Use a data source namespace when you want the same report focused on one source:

```bash
ccusage claude daily
ccusage codex daily
ccusage opencode weekly
ccusage amp session
ccusage pi monthly
ccusage kilo daily
ccusage kimi daily
ccusage qwen daily
```

## Your First Report

When you run ccusage for the first time, you'll see a table showing detected coding CLI usage by date:

```text
╭──────────────────────────────────────────╮
│                                          │
│  Coding (Agent) CLI Usage Report - Daily │
│                                          │
╰──────────────────────────────────────────╯

┌────────────┬────────┬────────────────┬────────┬────────┬────────────┐
│ Date       │ Agent  │ Models         │ Input  │ Output │ Cost (USD) │
├────────────┼────────┼────────────────┼────────┼────────┼────────────┤
│ 2026-05-16 │ Claude │ • sonnet-4-5   │  1,234 │ 15,678 │     $12.34 │
│ 2026-05-16 │ Codex  │ • gpt-5.5      │    890 │ 12,345 │     $18.92 │
└────────────┴────────┴────────────────┴────────┴────────┴────────────┘
```

## Understanding the Output

### Columns Explained

- **Date**: The date when an agent was used
- **Agent**: The coding CLI that generated the usage
- **Models**: Which models were used
- **Input**: Number of input tokens sent to the agent/model
- **Output**: Number of output tokens received from the agent/model
- **Cost (USD)**: Estimated cost based on model pricing

### Cache Tokens

If you have a wide terminal, you'll also see cache token columns:

- **Cache Create**: Tokens used to create cache entries
- **Cache Read**: Tokens read from cache (typically cheaper)

## Next Steps

Now that you have your first unified view, explore these features:

1. **[All Sources (Default)](/guide/all-reports)** - Understand the default unified behavior
2. **[Weekly Usage](/guide/weekly-reports)** - Track usage patterns by week
3. **[Monthly Usage](/guide/monthly-reports)** - See usage aggregated by month
4. **[Session Usage](/guide/session-reports)** - Analyze individual conversations
5. **[Configuration](/guide/configuration)** - Customize ccusage behavior
6. **[Claude Code](/guide/claude/)** - Claude Code-specific setup and features

## Common Use Cases

### Monitor Daily Usage

```bash
ccusage daily --since 2026-05-01 --until 2026-05-16
```

### Focus on One Source

```bash
ccusage codex daily
ccusage claude monthly
```

### Use Source-Specific Options

```bash
ccusage claude daily --mode display
ccusage codex daily --speed fast
ccusage opencode weekly
```

### Analyze Sessions

```bash
ccusage session
```

### Export for Analysis

```bash
ccusage monthly --json > usage-data.json
```

### Claude Code Features

See [Claude Code](/guide/claude/) for Claude-specific features such as blocks and statusline integration.

## Colors

ccusage automatically colors the output based on the terminal's capabilities. If you want to disable colors, you can use the `--no-color` flag. Or you can use the `--color` flag to force colors on.

## Automatic Table Adjustment

ccusage automatically adjusts its table layout based on terminal width:

- **Wide terminals (≥100 characters)**: Full table with all columns including cache metrics, model names, and detailed breakdowns
- **Narrow terminals (<100 characters)**: Compact view with essential columns only (Date, Models, Input, Output, Cost)

The layout adjusts automatically based on your terminal width - no configuration needed. If you're in compact mode and want to see the full data, simply expand your terminal window.

## Troubleshooting

### No Data Found

If ccusage shows no data, check:

1. **A supported coding CLI is installed and used** - ccusage reads from local usage files
2. **Data directory exists** - Common locations:
   - Claude Code: `~/.config/claude/projects/` or `~/.claude/projects/`
   - Codex: `${CODEX_HOME:-~/.codex}`
   - OpenCode: `${OPENCODE_DATA_DIR:-~/.local/share/opencode}`
   - Amp: `${AMP_DATA_DIR:-~/.local/share/amp}`
   - Droid: `${DROID_SESSIONS_DIR:-~/.factory/sessions}`
   - Codebuff: `${CODEBUFF_DATA_DIR:-~/.config/manicode}`
   - Hermes Agent: `${HERMES_HOME:-~/.hermes}/state.db`
   - pi-agent: `${PI_AGENT_DIR:-~/.pi/agent/sessions}`
   - Goose: standard Goose data roots or `GOOSE_PATH_ROOT`
   - Kilo: `${KILO_DATA_DIR:-~/.local/share/kilo}`
   - Kimi: `${KIMI_DATA_DIR:-~/.kimi}` (also scans `~/.kimi-code`)
   - OpenClaw: `${OPENCLAW_DIR:-~/.openclaw}` (also scans `~/.clawdbot`, `~/.moltbot`, `~/.moldbot`)
   - Qwen: `${QWEN_DATA_DIR:-~/.qwen}`
   - GitHub Copilot CLI: `~/.copilot/otel/*.jsonl` or `COPILOT_OTEL_FILE_EXPORTER_PATH`

### Custom Data Directory

If your agent data is in a custom location, set the matching environment variable:

```bash
export CLAUDE_CONFIG_DIR="/path/to/your/claude/data"
export CODEX_HOME="/path/to/codex"
export OPENCODE_DATA_DIR="/path/to/opencode"
export AMP_DATA_DIR="/path/to/amp"
export DROID_SESSIONS_DIR="/path/to/factory/sessions"
export CODEBUFF_DATA_DIR="/path/to/manicode"
export HERMES_HOME="/path/to/hermes"
export PI_AGENT_DIR="/path/to/pi/sessions"
export GOOSE_PATH_ROOT="/path/to/goose"
export OPENCLAW_DIR="/path/to/openclaw"
export KILO_DATA_DIR="/path/to/kilo"
export KIMI_DATA_DIR="/path/to/kimi"
export QWEN_DATA_DIR="/path/to/qwen"
export COPILOT_OTEL_FILE_EXPORTER_PATH="/path/to/copilot-otel.jsonl"
```

Each source-specific path variable can also contain comma-separated directories:

```bash
export CODEX_HOME="/path/to/codex,/archive/codex,/path/to/codex-exec-jsonl"
export OPENCODE_DATA_DIR="/path/to/opencode,/archive/opencode"
export AMP_DATA_DIR="/path/to/amp,/archive/amp"
export DROID_SESSIONS_DIR="/path/to/factory/sessions,/archive/factory/sessions"
export CODEBUFF_DATA_DIR="/path/to/manicode,/archive/manicode"
export HERMES_HOME="/path/to/hermes,/archive/hermes"
export PI_AGENT_DIR="/path/to/pi/sessions,/archive/pi/sessions"
export GOOSE_PATH_ROOT="/path/to/goose,/archive/goose"
export OPENCLAW_DIR="/path/to/openclaw,/archive/openclaw"
export KILO_DATA_DIR="/path/to/kilo,/archive/kilo"
export KIMI_DATA_DIR="/path/to/kimi,/archive/kimi"
export QWEN_DATA_DIR="/path/to/qwen,/archive/qwen"
```

## Getting Help

- Use `ccusage --help` for command options
- Visit our [GitHub repository](https://github.com/ccusage/ccusage) for issues
- Use [JSON Output](/guide/json-output) for programmatic usage
