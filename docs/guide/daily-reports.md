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

`--last` cannot be combined with `--since` or `--until`. See [Command-Line Options](/guide/cli-options#recent-periods) for how it behaves on the other reports.

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

This shows costs split by individual models:

Excerpt from the fixture report; additional rows are omitted.

```text
┌──────────┬──────────────┬──────────────┬────────────┬───────────┬────────────┬─────────────┬─────────────┬───────────┐
│ Date     │ Agent        │ Models       │      Input │    Output │      Cache │  Cache Read │       Total │      Cost │
│          │              │              │            │           │     Create │             │      Tokens │     (USD) │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
└──────────┴──────────────┴──────────────┴────────────┴───────────┴────────────┴─────────────┴─────────────┴───────────┘
```

### JSON Output

The following output excerpt omits other rows and some per-row fields; `totals` covers the complete report.

Export data as JSON for further analysis:

```bash
ccusage daily --json
```

Each row is one `period` (the date) with `agent: "all"`; `metadata.agents` lists the sources that contributed to it. See [JSON Output](/guide/json-output) for every field.

```json
{
	"daily": [
		{
			"agent": "all",
			"period": "2025-12-31",
			"inputTokens": 67294,
			"outputTokens": 19952,
			"cacheCreationTokens": 6111,
			"cacheReadTokens": 185916,
			"totalTokens": 279883,
			"totalCost": 11.418334255000001,
			"metadata": {
				"agents": [
					"amp",
					"antigravity",
					"claude",
					"cline",
					"codebuff",
					"codex",
					"copilot",
					"droid",
					"dsh",
					"gemini",
					"goose",
					"grok",
					"hermes",
					"kilo",
					"kimi",
					"openclaw",
					"opencode",
					"pi",
					"qwen",
					"zcode"
				]
			},
			"modelsUsed": [
				"GLM-5.3",
				"GLM-5.3-Flash",
				"[openclaw] claude-haiku-4-5-20251001",
				"[openclaw] gpt-5",
				"[openclaw] openclaw-mystery-1",
				"[pi] claude-haiku-4-5-20251001",
				"[pi] claude-sonnet-4-20250514",
				"anthropic/claude-sonnet-4",
				"claude-haiku-4-5-20251001",
				"claude-opus-4-20250514",
				"claude-sonnet-4-20250514",
				"copilot-mystery-1",
				"deepseek-v4-pro",
				"droid-mystery-1",
				"dsh-mystery-1",
				"gemini-2-5-pro",
				"gemini-2.5-pro",
				"gemini-3-flash-preview",
				"gemini-3-pro",
				"gemini-3.5-flash-high",
				"gemini-mystery-1",
				"gpt-5",
				"gpt-5-codex",
				"gpt-5.2-codex",
				"gpt-5.5",
				"grok-4.5-build",
				"grok-4.7-build",
				"grok-code-fast-1",
				"grok-mystery-1",
				"kimi-for-coding",
				"kimi-k2-thinking",
				"qwen-2.5-72b-instruct",
				"qwen-3-235b",
				"qwen-mystery-1",
				"qwen3-coder-plus",
				"unknown"
			]
		}
	],
	"totals": {
		"cacheCreationTokens": 9001,
		"cacheReadTokens": 671244,
		"inputTokens": 1172663,
		"outputTokens": 37582,
		"totalCost": 15.123121700000002,
		"totalTokens": 1892152,
		"unpricedModels": [
			"GLM-5.3",
			"[openclaw] openclaw-mystery-1",
			"[pi] pi-mystery-1",
			"amp-mystery-1",
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
│ 2025-12-31     │ - sonnet-4           │       820 │       490 │           100 │       7,000 │         8,410 │       $0.03 │
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
│ Total          │                      │     8,127 │     3,634 │         2,500 │      44,200 │        58,461 │       $0.84 │
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

`--instances` and `--project` belong to `ccusage claude daily`; the unified `ccusage daily` rejects them. To filter the Claude rows of a unified daily report, set `claude.commands.daily.project` in a [configuration file](/guide/config-files#source-specific-configuration).

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
