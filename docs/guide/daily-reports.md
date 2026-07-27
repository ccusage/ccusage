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

- **Wide terminals (≥100 chars)**: Shows all columns
- **Narrow terminals (<100 chars)**: Compact mode with essential columns only

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
ccusage daily -l 7
```

`--last` cannot be combined with `--since` or `--until`. See [Command-Line Options](/guide/cli-options#recent-periods) for how it behaves on the other reports.

### Sort Order

Control the order of dates:

```bash
# Newest dates first (default)
ccusage daily --order desc

# Oldest dates first
ccusage daily --order asc
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

```
┌──────────────┬──────────────────┬────────┬─────────┬────────────┐
│ Date         │ Models           │ Input  │ Output  │ Cost (USD) │
├──────────────┼──────────────────┼────────┼─────────┼────────────┤
│ 2026-05-16   │ opus-4-1, sonnet-4-5 │    277 │  31,456 │     $17.58 │
├──────────────┼──────────────────┼────────┼─────────┼────────────┤
│   └─ opus-4-1  │                  │    100 │  15,000 │     $10.25 │
├──────────────┼──────────────────┼────────┼─────────┼────────────┤
│   └─ sonnet-4-5│                  │    177 │  16,456 │      $7.33 │
└──────────────┴──────────────────┴────────┴─────────┴────────────┘
```

### JSON Output

Export data as JSON for further analysis:

```bash
ccusage daily --json
```

```json
{
	"type": "daily",
	"data": [
		{
			"date": "2026-05-16",
			"models": ["claude-opus-4-1-20250805", "claude-sonnet-4-5-20250929"],
			"inputTokens": 277,
			"outputTokens": 31456,
			"cacheCreationTokens": 512,
			"cacheReadTokens": 1024,
			"totalTokens": 33269,
			"costUSD": 17.58
		}
	],
	"summary": {
		"totalInputTokens": 277,
		"totalOutputTokens": 31456,
		"totalCacheCreationTokens": 512,
		"totalCacheReadTokens": 1024,
		"totalTokens": 33269,
		"totalCostUSD": 17.58
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

Group usage by project instead of aggregating across all projects:

```bash
# Group daily usage by project
ccusage daily --instances
ccusage daily -i
```

When using `--instances`, the report shows usage for each project separately:

```
┌──────────────┬────────────────────────────────────────────────────────────────────────────────────────────┐
│ Project: my-project                                                                                     │
├──────────────┬──────────────────┬────────┬─────────┬────────────┬────────────┬─────────────┬──────────┤
│ Date         │ Models           │ Input  │ Output  │ Cache Create│ Cache Read │ Total Tokens│ Cost (USD)│
├──────────────┼──────────────────┼────────┼─────────┼────────────┼────────────┼─────────────┼──────────┤
│ 2026-05-16   │ • sonnet-4-5       │    277 │  31,456 │         512│      1,024 │      33,269 │     $7.33│
└──────────────┴──────────────────┴────────┴─────────┴────────────┴────────────┴─────────────┴──────────┘

┌──────────────┬────────────────────────────────────────────────────────────────────────────────────────────┐
│ Project: other-project                                                                                  │
├──────────────┬──────────────────┬────────┬─────────┬────────────┬────────────┬─────────────┬──────────┤
│ Date         │ Models           │ Input  │ Output  │ Cache Create│ Cache Read │ Total Tokens│ Cost (USD)│
├──────────────┼──────────────────┼────────┼─────────┼────────────┼────────────┼─────────────┼──────────┤
│ 2026-05-16   │ • opus-4-1         │    100 │  15,000 │         256│        512 │      15,868 │    $10.25│
└──────────────┴──────────────────┴────────┴─────────┴────────────┴────────────┴─────────────┴──────────┘
```

Filter to a specific project:

```bash
# Show only usage from "my-project"
ccusage daily --project my-project
ccusage daily -p my-project

# Combine with instances flag
ccusage daily --instances --project my-project
```

## Common Use Cases

### Track Monthly Spending

```bash
# See May 2026 usage
ccusage daily --since 20260501 --until 20260516
```

### Find Expensive Days

```bash
# Sort by cost (highest first)
ccusage daily --order desc
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
ccusage daily --instances

# Track specific project costs
ccusage daily --project my-important-project --since 20260501

# Compare project usage with JSON export
ccusage daily --instances --json > project-analysis.json
```

### Team Usage Analysis

Use project aliases to replace cryptic or long project directory names with readable labels:

```json
// .ccusage/ccusage.json - Set custom project names for better reporting
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
ccusage daily --instances --since 20260501
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
