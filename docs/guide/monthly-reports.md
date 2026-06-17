# Monthly Usage

Monthly usage aggregates coding (agent) CLI usage by calendar month, providing a high-level view of usage patterns and estimated costs over longer periods. `ccusage monthly` combines all detected supported sources; use `ccusage <source> monthly` for a single source.

## Basic Usage

```bash
ccusage monthly
ccusage codex monthly
ccusage amp monthly
ccusage pi monthly
ccusage qwen monthly
```

## Example Output

```
╭─────────────────────────────────────────────╮
│                                             │
│  Claude Code Token Usage Report - Monthly  │
│                                             │
╰─────────────────────────────────────────────╯

┌─────────┬────────────────────┬─────────┬──────────┬──────────────┬────────────┬──────────────┬────────────┐
│ Month   │ Models             │ Input   │ Output   │ Cache Create │ Cache Read │ Total Tokens │ Cost (USD) │
├─────────┼────────────────────┼─────────┼──────────┼──────────────┼────────────┼──────────────┼────────────┤
│ 2026-05 │ • opus-4-1         │  45,231 │  892,456 │        2,048 │      4,096 │      943,831 │   $1,247.92│
│         │ • sonnet-4-5       │         │          │              │            │              │            │
│ 2026-04 │ • sonnet-4-5       │  38,917 │  756,234 │        1,536 │      3,072 │      799,759 │     $892.15│
│ 2026-03 │ • opus-4-1         │  22,458 │  534,789 │        1,024 │      2,048 │      560,319 │     $678.43│
├─────────┼────────────────────┼─────────┼──────────┼──────────────┼────────────┼──────────────┼────────────┤
│ Total   │                  │ 106,606 │2,183,479 │        4,608 │      9,216 │    2,303,909 │   $2,818.50│
└─────────┴────────────────────┴─────────┴──────────┴──────────────┴────────────┴──────────────┴────────────┘
```

## Understanding Monthly Data

### Month Format

Months are displayed in YYYY-MM format:

- `2026-05` = May 2026
- `2026-04` = April 2026

### Aggregation Logic

All usage within a calendar month is aggregated:

- Input/output tokens summed across all days
- Costs calculated from total token usage
- Models listed if used at any point in the month

## Command Options

### Date Filtering

Filter by month range:

```bash
# Show specific months
ccusage monthly --since 20260101 --until 20260531

# Show usage from Jan-May 2026
ccusage monthly --since 20260101 --until 20260531

# Show last 6 months
ccusage monthly --since $(date -d '6 months ago' +%Y%m%d)
```

::: tip Date Filtering
Even though you specify full dates (YYYYMMDD), monthly reports group by month. The filters determine which months to include.
:::

### Sort Order

```bash
# Newest months first (default)
ccusage monthly --order desc

# Oldest months first
ccusage monthly --order asc
```

### Cost Calculation Modes

```bash
# Use pre-calculated costs when available (default)
ccusage monthly --mode auto

# Always calculate costs from tokens
ccusage monthly --mode calculate

# Only show pre-calculated costs
ccusage monthly --mode display
```

### Model Breakdown

See costs broken down by model:

```bash
ccusage monthly --breakdown
```

Example with breakdown:

```
┌─────────┬──────────────────────┬─────────┬──────────┬────────────┐
│ Month   │ Models               │ Input   │ Output   │ Cost (USD) │
├─────────┼──────────────────────┼─────────┼──────────┼────────────┤
│ 2026-05 │ opus-4-1, sonnet-4-5 │  45,231 │  892,456 │  $1,247.92 │
├─────────┼──────────────────────┼─────────┼──────────┼────────────┤
│  └─ opus-4-1                   │  20,000 │  400,000 │    $750.50 │
├─────────┼──────────────────────┼─────────┼──────────┼────────────┤
│  └─ sonnet-4-5                 │  25,231 │  492,456 │    $497.42 │
└─────────┴──────────────────────┴─────────┴──────────┴────────────┘
```

### JSON Output

```bash
ccusage monthly --json
```

```json
[
	{
		"month": "2026-05",
		"models": ["opus-4-1", "sonnet-4-5"],
		"inputTokens": 45231,
		"outputTokens": 892456,
		"cacheCreationTokens": 2048,
		"cacheReadTokens": 4096,
		"totalTokens": 943831,
		"totalCost": 1247.92
	}
]
```

### Offline Mode

```bash
ccusage monthly --offline
```

### Historical Cache

Claude Code deletes session logs older than `cleanupPeriodDays` (30 by default), which would otherwise make past months shrink on every run. ccusage caches each day's totals so pruned months keep showing their original cost. Use `--no-cache` to report strictly from the logs currently on disk:

```bash
ccusage monthly --no-cache
```

See [Historical cache](/guide/claude/#historical-cache) for details.

## Analysis Use Cases

### Budget Planning

Monthly reports help with subscription planning:

```bash
# Check last year's usage
ccusage monthly --since 20250101 --until 20251231
```

Look at the total cost to understand what you'd pay on usage-based pricing.

### Usage Trends

Track how your usage changes over time:

```bash
# Compare year over year
ccusage monthly --since 20240101 --until 20241231  # 2024
ccusage monthly --since 20250101 --until 20251231  # 2025
```

### Model Migration Analysis

See how your model usage evolves:

```bash
ccusage monthly --breakdown
```

This helps track transitions between Opus 4.1, Sonnet 4.5, and other models.

### Seasonal Patterns

Identify busy/slow periods:

```bash
# Academic year analysis
ccusage monthly --since 20250901 --until 20260531
```

### Export for Business Analysis

```bash
# Create quarterly reports
ccusage monthly --since 20260101 --until 20260331 --json > q1-2026.json
```

## Tips for Monthly Analysis

### 1. Cost Context

Monthly totals show:

- **Subscription Value**: How much you'd pay with usage-based billing
- **Usage Intensity**: Months with heavy coding CLI usage
- **Model Preferences**: Which models you favor over time

### 2. Trend Analysis

Look for patterns:

- Increasing usage over time
- Seasonal variations
- Model adoption curves

### 3. Business Planning

Use monthly data for:

- Team budget planning
- Usage forecasting
- Subscription optimization

### 4. Comparative Analysis

Compare monthly reports with:

- Team productivity metrics
- Project timelines
- Business outcomes

## Related Commands

- [All Sources (Default)](/guide/all-reports) - How unified views work
- [Daily Usage](/guide/daily-reports) - Day-by-day breakdown
- [Session Usage](/guide/session-reports) - Individual conversations
- [Claude Code](/guide/claude/) - Claude Code-specific setup and features

## Next Steps

After analyzing monthly trends, consider:

1. [Session Usage](/guide/session-reports) to identify high-cost conversations
2. [Claude Code](/guide/claude/) for Claude Code-specific setup and features
3. [JSON Output](/guide/json-output) for programmatic analysis
