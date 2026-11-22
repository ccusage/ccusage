# Weekly Usage

`ccusage weekly` groups usage by week across every detected source. Use `ccusage <agent> weekly` to focus on one source. For Codex, use `ccusage weekly --by-agent` to see its weekly rows in the unified report.

## Basic Usage

Show all weekly usage:

```bash
ccusage weekly
ccusage claude weekly
ccusage opencode weekly
```

## Example Output

`ccusage weekly` prints an `All` row per week followed by one row per source. This excerpt from the fixture report keeps the week of 2026-04-20 and the Total row; the other weeks are omitted:

```text
┌────────────┬──────────────────────┬───────────────────────────┬────────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┐
│ Week       │ Agent                │ Models                    │      Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │
├────────────┼──────────────────────┼───────────────────────────┼────────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-04-20 │ All                  │                           │      1,000 │       500 │             0 │       2,000 │         3,500 │       $0.00 │
├────────────┼──────────────────────┼───────────────────────────┼────────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│            │ - Kimi               │ - kimi-for-coding         │      1,000 │       500 │             0 │       2,000 │         3,500 │       $0.00 │
├────────────┼──────────────────────┼───────────────────────────┼────────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Total      │                      │                           │  1,347,024 │    58,670 │        17,765 │   1,309,867 │     2,736,743 │      $28.15 │
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

Weekly commands accept `-w` and `--start-of-week`, as well as the [`startOfWeek` configuration setting](/guide/config-files#weekly-command). Weeks start on Monday by default, with Sunday as the default for `ccusage claude weekly`. Unified weekly reports apply their resolved week start to every source.

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

The window starts on the same weekday the report buckets by, so `ccusage claude weekly --last 1 -w monday` starts on Monday. `--last` cannot be combined with `--since` or `--until` on the command line. A `since` or `until` set in a [configuration file](/guide/config-files) is not a conflict: `--last` replaces that window.

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

Each source row is followed by one `└─` row per model. This excerpt from a 120-column terminal keeps the week of 2025-12-29, its Amp row, and Amp's model rows; the other sources and weeks are omitted:

```text
┌──────────┬──────────────┬──────────────┬────────────┬───────────┬────────────┬─────────────┬─────────────┬───────────┐
│ Week     │ Agent        │ Models       │      Input │    Output │      Cache │  Cache Read │       Total │      Cost │
│          │              │              │            │           │     Create │             │      Tokens │     (USD) │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│ 2025     │ All          │              │      8,483 │     1,760 │      1,264 │       1,093 │      12,630 │     $3.83 │
│ 12-29    │              │              │            │           │            │             │             │           │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │ - Amp        │ - amp-myste… │      5,000 │     2,215 │        550 │      25,000 │      32,765 │     $0.03 │
│          │              │ - gpt-5      │            │           │            │             │             │           │
│          │              │ - haiku-4-5  │            │           │            │             │             │           │
│          │              │ - sonnet-4   │            │           │            │             │             │           │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │              │ └─ sonnet-4  │      1,600 │       800 │        300 │       7,000 │       9,700 │     $0.02 │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │              │   └─ gpt-5   │      1,400 │       645 │          0 │       5,000 │       7,045 │     $0.01 │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │              │ └─ haiku-4-5 │        900 │       400 │        200 │       7,000 │       8,500 │     $0.00 │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│          │              │ └─           │      1,100 │       370 │         50 │       6,000 │       7,520 │     $0.00 │
│          │              │ amp-mystery… │            │           │            │             │             │           │
├──────────┼──────────────┼──────────────┼────────────┼───────────┼────────────┼─────────────┼─────────────┼───────────┤
│ Total    │              │              │  1,347,024 │    58,670 │     17,765 │   1,309,867 │   2,736,743 │    $28.15 │
└──────────┴──────────────┴──────────────┴────────────┴───────────┴────────────┴─────────────┴─────────────┴───────────┘
```

### JSON Output

Export weekly data as JSON:

```bash
ccusage weekly --json
```

Rows use the same fields as [daily JSON](/guide/daily-reports#json-output), with the first day of the week in `period`. This excerpt keeps one row; `totals` covers the complete report:

```json
{
	"weekly": [
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
			"period": "2026-04-20",
			"totalCost": 0.00327,
			"totalTokens": 3500
		}
	],
	"totals": {
		"cacheCreationTokens": 17765,
		"cacheReadTokens": 1309867,
		"inputTokens": 1347024,
		"outputTokens": 58670,
		"totalCost": 28.146135925000003,
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
