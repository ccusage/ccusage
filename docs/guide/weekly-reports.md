# Weekly Usage

Weekly usage aggregates usage by week, providing a mid-range view between daily and monthly usage. `ccusage weekly` covers every detected source; `ccusage claude weekly`, `ccusage opencode weekly`, `ccusage cline weekly`, and `ccusage dsh weekly` focus the same view on one source.

## Basic Usage

Show all weekly usage:

```bash
ccusage weekly
ccusage claude weekly
ccusage opencode weekly
```

## Example Output

Excerpt from the fixture report; additional rows are omitted.

```text
┌────────────┬──────────────────────┬───────────────────────────┬────────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┐
│ Week       │ Agent                │ Models                    │      Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │
├────────────┼──────────────────────┼───────────────────────────┼────────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 1969-12-29 │ All                  │                           │         22 │         2 │             0 │           0 │            24 │       $0.00 │
└────────────┴──────────────────────┴───────────────────────────┴────────────┴───────────┴───────────────┴─────────────┴───────────────┴─────────────┘
```

## Understanding the Columns

The columns are identical to daily reports but aggregated by week:

- **Week**: Start date of the week (configurable)
- **Agent**: Source CLI, shown in unified all-source reports
- **Models**: All models used during the week
- **Input/Output**: Total tokens for the week
- **Cache Create/Read**: Cache token usage
- **Total Tokens**: Sum of all token types
- **Cost (USD)**: Estimated cost for the week

## Command Options

### Week Start Day

Configure which day starts the week:

```bash
# Start week on Monday (default for ccusage weekly)
ccusage weekly --start-of-week monday
ccusage weekly -w monday

# Start week on Sunday
ccusage weekly --start-of-week sunday

# ccusage claude weekly starts on Sunday by default
ccusage claude weekly

# Other options: tuesday, wednesday, thursday, friday, saturday
```

### Date Filtering

Filter by date range:

```bash
# Show specific period
ccusage weekly --since 20260501 --until 20260531

# Show last 4 weeks
ccusage weekly --since 20260420
```

### Recent Weeks

Count back in whole weeks instead of dates:

```bash
# This week so far
ccusage weekly --last 1

# This week and the three before it
ccusage weekly --last 4
```

The window starts on the same weekday the report buckets by, so `ccusage claude weekly --last 1 -w monday` starts on Monday. `--last` cannot be combined with `--since` or `--until`.

### Sort Order

Control the order of weeks:

```bash
# Oldest weeks first (default)
ccusage weekly --order asc

# Newest weeks first
ccusage weekly --order desc
```

### Model Breakdown

See per-model weekly costs:

```bash
ccusage weekly --breakdown
```

Excerpt from the fixture report; additional rows are omitted.

```text
┌──────────┬──────────────┬──────────────┬────────────┬───────────┬────────────┬─────────────┬─────────────┬───────────┐
│ Week     │ Agent        │ Models       │      Input │    Output │      Cache │  Cache Read │       Total │      Cost │
│          │              │              │            │           │     Create │             │      Tokens │     (USD) │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
└──────────┴──────────────┴──────────────┴────────────┴───────────┴────────────┴─────────────┴─────────────┴───────────┘
```

### JSON Output

The following output excerpt omits other rows and some per-row fields; `totals` covers the complete report.

Export weekly data as JSON:

```bash
ccusage weekly --json
```

Rows use the same fields as [daily JSON](/guide/daily-reports#json-output), with the first day of the week in `period`:

```json
{
	"weekly": [
		{
			"agent": "all",
			"period": "2025-12-29",
			"inputTokens": 1172663,
			"outputTokens": 37582,
			"cacheCreationTokens": 9001,
			"cacheReadTokens": 671244,
			"totalTokens": 1892152,
			"totalCost": 15.123121700000002,
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
				"[grok] grok-4.3-build",
				"[openclaw] claude-haiku-4-5-20251001",
				"[openclaw] claude-sonnet-4-20250514",
				"[openclaw] gpt-5",
				"[openclaw] openclaw-mystery-1",
				"[pi] claude-haiku-4-5-20251001",
				"[pi] claude-opus-4-20250514",
				"[pi] claude-sonnet-4-20250514",
				"[pi] gpt-5",
				"[pi] pi-mystery-1",
				"amp-mystery-1",
				"anthropic/claude-sonnet-4",
				"anthropic/claude-sonnet-4.5",
				"antigravity-mystery-1",
				"claude-haiku-4-5-20251001",
				"claude-opus-4-20250514",
				"claude-opus-5",
				"claude-sonnet-4-20250514",
				"cline-mystery-1",
				"codebuff-mystery-1",
				"codex-mystery-1",
				"copilot-mystery-1",
				"deepseek-flash",
				"deepseek-v4-pro",
				"droid-mystery-1",
				"dsh-mystery-1",
				"gemini-2-5-pro",
				"gemini-2.5-flash",
				"gemini-2.5-pro",
				"gemini-3-flash-preview",
				"gemini-3-pro",
				"gemini-3-pro-high",
				"gemini-3.5-flash-high",
				"gemini-3.6-flash-high",
				"gemini-mystery-1",
				"glm-5.3",
				"goose-mystery-1",
				"gpt-5",
				"gpt-5-codex",
				"gpt-5.2-codex",
				"gpt-5.5",
				"gpt-unknown",
				"grok-4.5-build",
				"grok-4.7-build",
				"grok-code-fast-1",
				"grok-mystery-1",
				"grok-unknown",
				"hermes-mystery-1",
				"kilo-mystery-1",
				"kimi-for-coding",
				"kimi-k2-thinking",
				"kimi-k2.5",
				"kimi-mystery-1",
				"mystery-model-9000",
				"opencode-mystery-1",
				"opus",
				"qwen-2.5-72b-instruct",
				"qwen-3-235b",
				"qwen-mystery-1",
				"qwen/qwen3-coder",
				"qwen3-coder-plus",
				"unknown",
				"zcode-mystery-1"
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

`ccusage claude weekly --json` uses the Claude shape, with the week start in `week` instead of `period`.

### Cost Calculation Modes

Control cost calculation:

```bash
# Auto mode (default)
ccusage weekly --mode auto

# Always calculate from tokens
ccusage weekly --mode calculate

# Only use pre-calculated costs
ccusage weekly --mode display
```

### Offline Mode

Use cached pricing data:

```bash
ccusage weekly --offline
```

## Common Use Cases

### Weekly Trends

```bash
# See usage trends over past months
ccusage weekly --since 20260401
```

### Sprint Analysis

```bash
# Track usage during 2-week sprints (Monday start)
ccusage weekly --start-of-week monday --since 20260501
```

### Budget Planning

```bash
# Export for weekly budget tracking
ccusage weekly --json > weekly-budget.json
```

### Compare Workweeks

```bash
# Monday-Friday work pattern analysis
ccusage weekly --start-of-week monday --breakdown
```

## Tips

1. **Week Start**: Choose a start day that aligns with your work schedule
2. **Breakdown View**: Use `--breakdown` to identify which models drive costs
3. **JSON Export**: Weekly JSON data is perfect for creating trend charts
4. **Project Tracking**: Weekly reports have no per-project grouping; use [`ccusage claude daily --instances`](/guide/daily-reports#project-analysis) for Claude Code projects

## Related Commands

- [All Sources (Default)](/guide/all-reports) - How unified views work
- [Daily Usage](/guide/daily-reports) - Day-by-day analysis
- [Monthly Usage](/guide/monthly-reports) - Monthly aggregates
- [Session Usage](/guide/session-reports) - Per-conversation analysis
