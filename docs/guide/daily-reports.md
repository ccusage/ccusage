# Daily Usage

![Daily usage report showing token usage and costs by date with model breakdown](/screenshot.png)

Daily usage shows token usage and estimated costs aggregated by calendar date. By default, `ccusage daily` combines every detected supported data source; use `ccusage <source> daily` when you want one source.

## Basic Usage

Show all daily usage:

```bash
ccusage daily
# or simply:
ccusage

# Focus on one source:
ccusage codex daily
ccusage opencode daily
ccusage amp daily
ccusage pi daily
ccusage qwen daily
```

The daily command is the default, so you can omit it when running ccusage.

## Example Output

![Daily usage report showing token usage and costs by date with model breakdown](/screenshot.png)

## Understanding the Columns

### Basic Columns

- **Date**: Calendar date in YYYY-MM-DD format
- **Agent**: Source CLI for unified reports
- **Models**: Models used that day (shown as a bulleted list)
- **Input**: Total input tokens sent to the model
- **Output**: Total output tokens received from the model
- **Cost (USD)**: Estimated cost for that day

### Cache Columns

- **Cache Create**: Tokens used to create cache entries
- **Cache Read**: Tokens read from cache (typically cheaper)

### Responsive Display

ccusage automatically adapts to your terminal width:

- **Wide terminals**: Show the full table when it fits
- **Narrow terminals**: Use a compact layout; `--compact` also selects it

## Command Options

### Date Filtering

Filter reports by date range:

```bash
# Show usage from May 2026
ccusage daily --since 20260501 --until 20260516

# Show last week
ccusage daily --since 20260510 --until 20260516

# Show usage since a specific date
ccusage daily --since 20260501
```

### Recent Days

Skip the date arithmetic when you only want the days just gone:

```bash
# Today
ccusage daily --last 1

# The last seven days, including today
ccusage daily --last 7
```

`--last` cannot be combined with `--since` or `--until` on the command line. A `since` or `until` set in a [configuration file](/guide/config-files) is not a conflict: `--last` replaces that window. See [Command-Line Options](/guide/cli-options#recent-periods) for how it behaves on the other reports.

### Sort Order

Control the order of dates:

```bash
# Oldest dates first (default)
ccusage daily --order asc

# Newest dates first
ccusage daily --order desc
```

### Cost Calculation Modes

Control how costs are calculated:

```bash
# Use pre-calculated costs when available (default)
ccusage daily --mode auto

# Always calculate costs from tokens
ccusage daily --mode calculate

# Only show pre-calculated costs
ccusage daily --mode display
```

### Model Breakdown

See per-model cost breakdown:

```bash
ccusage daily --breakdown
```

Each source row is followed by one `└─` row per model with that model's tokens and cost. This excerpt from a 120-column terminal keeps the 2025-12-31 row, its Amp row, and Amp's model rows; the other sources and dates are omitted:

```text
┌──────────┬──────────────┬──────────────┬────────────┬───────────┬────────────┬─────────────┬─────────────┬───────────┐
│ Date     │ Agent        │ Models       │      Input │    Output │      Cache │  Cache Read │       Total │      Cost │
│          │              │              │            │           │     Create │             │      Tokens │     (USD) │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│ 2025     │ All          │              │     66,494 │    19,767 │      6,111 │     185,766 │     278,748 │    $11.67 │
│ 12-31    │              │              │            │           │            │             │             │           │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │ - Amp        │ - gpt-5      │      2,300 │       700 │        200 │      12,000 │      15,200 │     $0.01 │
│          │              │ - haiku-4-5  │            │           │            │             │             │           │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │              │   └─ gpt-5   │      1,400 │       300 │          0 │       5,000 │       6,700 │     $0.01 │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │              │ └─ haiku-4-5 │        900 │       400 │        200 │       7,000 │       8,500 │     $0.00 │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│ Total    │              │              │  1,347,024 │    58,670 │     17,765 │   1,309,867 │   2,736,743 │    $28.15 │
└──────────┴──────────────┴──────────────┴────────────┴───────────┴────────────┴─────────────┴─────────────┴───────────┘
```

### JSON Output

Export data as JSON for further analysis:

```bash
ccusage daily --json
```

Each row is one `period` (the date) with `agent: "all"`; `metadata.agents` lists the sources that contributed to it. See [JSON Output](/guide/json-output) for every field. This excerpt keeps one row; `totals` covers the complete report.

```json
{
	"daily": [
		{
			"agent": "all",
			"cacheCreationTokens": 0,
			"cacheReadTokens": 2000,
			"inputTokens": 1000,
			"metadata": {
				"agents": ["kimi"]
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 2000,
					"cost": 0.00327,
					"inputTokens": 1000,
					"modelName": "kimi-for-coding",
					"outputTokens": 500
				}
			],
			"modelsUsed": ["kimi-for-coding"],
			"outputTokens": 500,
			"period": "2026-04-21",
			"totalCost": 0.00327,
			"totalTokens": 3500
		}
	],
	"totals": {
		"cacheCreationTokens": 17765,
		"cacheReadTokens": 1309867,
		"inputTokens": 1347024,
		"outputTokens": 58670,
		"totalCost": 28.146135925000006,
		"totalTokens": 2736743,
		"unpricedModels": [
			"GLM-5.3",
			"[openclaw] openclaw-mystery-1",
			"[openclaw] unknown",
			"[pi] pi-mystery-1",
			"[pi] pi-mystery-2",
			"amp-mystery-1",
			"antigravity-model-77",
			"antigravity-mystery-1",
			"claude-sonnet-4-20250514",
			"cline-mystery-1",
			"codebuff-mystery-1",
			"codex-mystery-1",
			"copilot-mystery-1",
			"droid-mystery-1",
			"dsh-mystery-1",
			"gemini-mystery-1",
			"goose-mystery-1",
			"gpt-unknown",
			"grok-mystery-1",
			"grok-unknown",
			"hermes-mystery-1",
			"kilo-mystery-1",
			"kimi-mystery-1",
			"model_placeholder_m499",
			"mystery-model-9000",
			"opencode-mystery-1",
			"qwen-mystery-1",
			"unknown",
			"zcode-mystery-1"
		]
	}
}
```

### Offline Mode

Use cached pricing data without network access:

```bash
ccusage daily --offline
# or short form:
ccusage daily -O
```

### Project Analysis

Claude Code records the project of every request, so its daily report can group usage by project instead of aggregating across all projects:

```bash
# Group daily usage by project
ccusage claude daily --instances
ccusage claude daily -i
```

When using `--instances`, the report shows usage for each project separately:

```text
┌────────────────┬──────────────────────┬───────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┐
│ Date           │ Models               │     Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Project: gamma │                      │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12-27     │ - sonnet-4           │       100 │        50 │             0 │           0 │           150 │       $0.01 │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Project: alpha │                      │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12-30     │ - haiku-4-5          │     1,550 │     1,020 │           900 │      13,100 │        16,570 │       $0.15 │
│                │ - opus-4             │           │           │               │             │               │             │
│                │ - sonnet-4           │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12-31     │ - haiku-4-5          │       900 │       515 │           100 │       7,150 │         8,665 │       $0.03 │
│                │ - sonnet-4           │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Project: beta  │                      │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12-31     │ - opus-4             │       900 │       400 │           200 │       7,000 │         8,500 │       $0.06 │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Project: alpha │                      │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-01-01     │ - mystery-model-9000 │       950 │       220 │             0 │       2,300 │         3,470 │       $0.01 │
│                │ - opus-4             │           │           │               │             │               │             │
│                │ - sonnet-4           │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Project: beta  │                      │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-01-01     │ - opus-5             │     2,657 │       504 │         1,250 │       6,000 │        10,411 │       $0.53 │
│                │ - sonnet-4           │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Project: alpha │                      │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-01-02     │ - opus-5-fast        │       200 │        50 │             0 │         300 │           550 │       $0.00 │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Project: beta  │                      │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-01-02     │ - opus-4             │       950 │       900 │            50 │       8,500 │        10,400 │       $0.06 │
│                │ - sonnet-4           │           │           │               │             │               │             │
├────────────────┼──────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Total          │                      │     8,207 │     3,659 │         2,500 │      44,350 │        58,716 │       $0.85 │
└────────────────┴──────────────────────┴───────────┴───────────┴───────────────┴─────────────┴───────────────┴─────────────┘
```

Filter to a specific project:

```bash
# Show only usage from "my-project"
ccusage claude daily --project my-project
ccusage claude daily -p my-project

# Combine with instances flag
ccusage claude daily --instances --project my-project
```

Use these flags with `ccusage claude daily`. To filter the Claude rows of a unified daily report, set `claude.commands.daily.project` in a [configuration file](/guide/config-files#filter-claude-code-by-project).

## Common Use Cases

### Track Monthly Spending

```bash
# See May 2026 usage
ccusage daily --since 20260501 --until 20260516
```

### Find Expensive Days

`--order` sorts dates, not costs, so rank the days with `jq`:

```bash
# The five most expensive days, highest first
ccusage daily --json | jq -r '.daily | sort_by(-.totalCost) | .[:5][] | "\(.period) \(.totalCost)"'
```

### Export for Spreadsheet Analysis

```bash
ccusage daily --json > may-usage.json
```

### Compare Model Usage

```bash
# See which models you use most
ccusage daily --breakdown
```

### Check Recent Activity

```bash
# Last 7 days
ccusage daily --since $(date -d '7 days ago' +%Y%m%d)
```

### Analyze Project Usage

```bash
# See usage breakdown by project
ccusage claude daily --instances

# Track specific project costs
ccusage claude daily --project my-important-project --since 20260501

# Compare project usage with JSON export
ccusage claude daily --instances --json > project-analysis.json
```

### Team Usage Analysis

Use project aliases in `.ccusage/ccusage.json` to replace cryptic or long project directory names with readable labels:

```json
{
	"commands": {
		"daily": {
			"projectAliases": "uuid-project=Frontend App,long-name=Backend API"
		}
	}
}
```

The `projectAliases` setting uses a comma-separated format of `original-name=display-name` pairs. This is especially useful when:

- Your projects have UUID-based names (e.g., `a2cd99ed-a586=My App`)
- Directory names are long paths that get truncated
- You want consistent naming across team reports

```bash
# Generate team report with readable project names
ccusage claude daily --instances --since 20260501
# Now shows "Frontend App" instead of "uuid-project"
```

## Tips

1. **Compact Mode**: If your terminal is narrow, expand it to see all columns
2. **Date Format**: Use YYYYMMDD format for date filters (e.g., 20260516)
3. **Regular Monitoring**: Run daily reports regularly to track usage patterns
4. **JSON Export**: Use `--json` for creating charts or additional analysis

## Related Commands

- [All Sources (Default)](/guide/all-reports) - How unified views work
- [Monthly Usage](/guide/monthly-reports) - Aggregate by month
- [Session Usage](/guide/session-reports) - Per-conversation analysis
- [Claude Code](/guide/claude/) - Claude Code-specific setup and features
