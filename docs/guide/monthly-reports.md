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

Excerpt from the fixture report; additional rows are omitted.

```text
┌──────────┬──────────────────────┬───────────────────────────┬────────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┐
│ Month    │ Agent                │ Models                    │      Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │
├──────────┼──────────────────────┼───────────────────────────┼────────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 1970-01  │ All                  │                           │         33 │         9 │             0 │           0 │            42 │       $0.00 │
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

`--last` cannot be combined with `--since` or `--until`.

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

Example with breakdown:

Excerpt from the fixture report; additional rows are omitted.

```text
┌──────────┬──────────────┬──────────────┬────────────┬───────────┬────────────┬─────────────┬─────────────┬───────────┐
│ Month    │ Agent        │ Models       │      Input │    Output │      Cache │  Cache Read │       Total │      Cost │
│          │              │              │            │           │     Create │             │      Tokens │     (USD) │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
└──────────┴──────────────┴──────────────┴────────────┴───────────┴────────────┴─────────────┴─────────────┴───────────┘
```

### JSON Output

The following output excerpt omits other rows and some per-row fields; `totals` covers the complete report.

```bash
ccusage monthly --json
```

Rows use the same fields as [daily JSON](/guide/daily-reports#json-output), with the month in `period`:

```json
{
	"monthly": [
		{
			"agent": "all",
			"period": "2025-12",
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
