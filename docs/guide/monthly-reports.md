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

`ccusage monthly` prints an `All` row per month followed by one row per source. This excerpt from the fixture report keeps 2026-04 and the Total row; the other months are omitted:

```text
┌──────────┬──────────────────────┬───────────────────────────┬────────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┐
│ Month    │ Agent                │ Models                    │      Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │
├──────────┼──────────────────────┼───────────────────────────┼────────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-04  │ All                  │                           │      1,000 │       500 │             0 │       2,000 │         3,500 │       $0.00 │
├──────────┼──────────────────────┼───────────────────────────┼────────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│          │ - Kimi               │ - kimi-for-coding         │      1,000 │       500 │             0 │       2,000 │         3,500 │       $0.00 │
├──────────┼──────────────────────┼───────────────────────────┼────────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Total    │                      │                           │  1,347,024 │    58,670 │        17,765 │   1,309,867 │     2,736,743 │      $28.15 │
└──────────┴──────────────────────┴───────────────────────────┴────────────┴───────────┴───────────────┴─────────────┴───────────────┴─────────────┘
```

## Understanding Monthly Data

### Month Format

Months are displayed in YYYY-MM format:

- `2026-05` = May 2026
- `2026-04` = April 2026

### Aggregation Logic

All usage within a calendar month is aggregated:

- Input/output tokens summed across all days
- Costs summed across all entries, each priced under the selected [cost mode](/guide/cost-modes)
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
Even though you specify full dates (YYYYMMDD), monthly reports group by month. The filters select individual entries before monthly aggregation, so a boundary month can contain only part of that month.
:::

### Recent Months

`--last` counts whole months, so the shell date arithmetic above is not needed:

```bash
# This month so far
ccusage monthly --last 1

# This month and the five before it
ccusage monthly --last 6
```

`--last` cannot be combined with `--since` or `--until` on the command line. A `since` or `until` set in a [configuration file](/guide/config-files) is not a conflict: `--last` replaces that window.

### Sort Order

```bash
# Oldest months first (default)
ccusage monthly --order asc

# Newest months first
ccusage monthly --order desc
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

Each source row is followed by one `└─` row per model. This excerpt from a 120-column terminal keeps the 2025-12 row, its Amp row, and Amp's model rows; the other sources and months are omitted:

```text
┌──────────┬──────────────┬──────────────┬────────────┬───────────┬────────────┬─────────────┬─────────────┬───────────┐
│ Month    │ Agent        │ Models       │      Input │    Output │      Cache │  Cache Read │       Total │      Cost │
│          │              │              │            │           │     Create │             │      Tokens │     (USD) │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│ 2025-12  │ All          │              │    114,260 │    29,921 │     13,225 │     295,459 │     453,935 │    $21.17 │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │ - Amp        │ - gpt-5      │      3,400 │       950 │        500 │      16,000 │      20,850 │     $0.02 │
│          │              │ - haiku-4-5  │            │           │            │             │             │           │
│          │              │ - sonnet-4   │            │           │            │             │             │           │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │              │ └─ sonnet-4  │      1,100 │       250 │        300 │       4,000 │       5,650 │     $0.01 │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │              │   └─ gpt-5   │      1,400 │       300 │          0 │       5,000 │       6,700 │     $0.01 │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │              │ └─ haiku-4-5 │        900 │       400 │        200 │       7,000 │       8,500 │     $0.00 │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│ Total    │              │              │  1,347,024 │    58,670 │     17,765 │   1,309,867 │   2,736,743 │    $28.15 │
└──────────┴──────────────┴──────────────┴────────────┴───────────┴────────────┴─────────────┴─────────────┴───────────┘
```

### JSON Output

```bash
ccusage monthly --json
```

Rows use the same fields as [daily JSON](/guide/daily-reports#json-output), with the month in `period`. This excerpt keeps one row; `totals` covers the complete report:

```json
{
	"monthly": [
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
			"period": "2026-04",
			"totalCost": 0.00327,
			"totalTokens": 3500
		}
	],
	"totals": {
		"cacheCreationTokens": 17765,
		"cacheReadTokens": 1309867,
		"inputTokens": 1347024,
		"outputTokens": 58670,
		"totalCost": 28.146135924999996,
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

```bash
ccusage monthly --offline
```

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
