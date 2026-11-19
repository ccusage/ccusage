# Blocks

Blocks reports show your Claude Code usage grouped by 5-hour billing windows, helping you understand Claude's billing cycle and track active session progress. `ccusage blocks` and `ccusage claude blocks` are the same report; no other source has billing blocks.

## Basic Usage

```bash
ccusage blocks
```

## Example Output

This excerpt of `ccusage claude blocks --offline` on the test fixtures keeps the last completed block, the gap after it, and the active block with its REMAINING and PROJECTED rows; earlier blocks are omitted:

```text
┌────────────────────────────────────────────────────────────┬─────────────────┬──────────────────────┬───────────┬───────────┬───────────┐
│ Block Start                                                │ Duration/Status │ Models               │    Tokens │         % │      Cost │
├────────────────────────────────────────────────────────────┼─────────────────┼──────────────────────┼───────────┼───────────┼───────────┤
│ 1/1/2026, 12:00:00 PM (30m)                                │                 │ - mystery-model-9000 │     3,470 │     18.3% │     $0.01 │
│                                                            │                 │ - opus-4             │           │           │           │
│                                                            │                 │ - sonnet-4           │           │           │           │
├────────────────────────────────────────────────────────────┼─────────────────┼──────────────────────┼───────────┼───────────┼───────────┤
│ 1/1/2026, 5:30:00 PM - 1/2/2026, 8:00:00 AM (14h gap)      │ (inactive)      │ -                    │         - │         - │         - │
├────────────────────────────────────────────────────────────┼─────────────────┼──────────────────────┼───────────┼───────────┼───────────┤
│ 1/2/2026, 8:00:00 AM (4h 0m elapsed, 1h 0m remaining)      │ ACTIVE          │ - opus-4             │    10,950 │     57.9% │     $0.06 │
│                                                            │                 │ - opus-5-fast        │           │           │           │
│                                                            │                 │ - sonnet-4           │           │           │           │
├────────────────────────────────────────────────────────────┼─────────────────┼──────────────────────┼───────────┼───────────┼───────────┤
│                              (assuming 18,911 token limit) │ REMAINING       │                      │     7,961 │     42.1% │           │
├────────────────────────────────────────────────────────────┼─────────────────┼──────────────────────┼───────────┼───────────┼───────────┤
│                               (assuming current burn rate) │ PROJECTED       │                      │    14,600 │     77.2% │     $0.09 │
└────────────────────────────────────────────────────────────┴─────────────────┴──────────────────────┴───────────┴───────────┴───────────┘
```

## Understanding Blocks

### Session Block Concept

ccusage estimates Claude Code blocks from usage timestamps, using a **5-hour window** by default:

- **Block Start**: Triggered by your first message, rounded down to the hour
- **Block Duration**: Lasts 5 hours from the start time
- **Rolling Windows**: New blocks start with activity after the previous block expires

The Block Start column shows the start time in the report timezone, followed by how long the block was in use. Without `--timezone`, block times use the system timezone. JSON timestamps remain UTC.

### Rows

- **Usage blocks**: One row per block with its models, total tokens, and cost.
- **`ACTIVE`**: The block that is still running, with elapsed and remaining time.
- **`(inactive)`**: A gap with no activity between two blocks.
- **`REMAINING`**: Tokens left in the active block before it reaches the token limit.
- **`PROJECTED`**: Tokens and cost the active block reaches if the current burn rate continues.

The `%` column compares each block with the token limit. Without `--token-limit`, the limit is the largest completed block in the report, as with `--token-limit max`.

## Command Options

### Show Active Block Only

Focus on your current session with detailed projections. The detail view prints Block Started in the report timezone, using the system timezone when `--timezone` is omitted:

```bash
ccusage blocks --active
```

```
Block Started:   2026-01-02 08:00:00
Time Elapsed:    4h 0m
Time Remaining:  1h 0m

Current Usage:
  Input Tokens:     1,150
  Output Tokens:    950
  Total Cost:       $0.06

Burn Rate:
  Tokens/minute:    61
  Cost/hour:        $0.02

Projected Usage (if current rate continues):
  Total Tokens:     14,600
  Total Cost:       $0.09
```

When a token limit applies, the detail view adds a Token Limit Status section that reports the projected usage as `OK`, `WARNING`, or `EXCEEDS LIMIT`.

### Show Recent Blocks

Display blocks that started in the last 3 days, plus the active block:

```bash
ccusage blocks --recent
```

### Token Limit Tracking

Set the token limit the `%`, `REMAINING`, and `PROJECTED` rows compare against:

```bash
# Set explicit token limit
ccusage blocks --token-limit 500000

# Use highest previous block as limit
ccusage blocks --token-limit max
# or short form:
ccusage blocks -t max
```

The limit must be `max` or a positive integer; zero and invalid text are rejected. Percentages above 100% are highlighted in red.

### Live Monitoring

`blocks --live` is not supported. Use the [statusline command](/guide/statusline) for real-time monitoring.

### Custom Session Duration

Change the block duration (default is 5 hours):

Session lengths must be finite, positive, and representable as at least one millisecond. Fractional hours are stored in whole milliseconds. For example, a `4.35`-hour block starting at `10:00:00` ends at `14:21:00.000`.

```bash
# 3-hour blocks
ccusage blocks --session-length 3

# 8-hour blocks
ccusage blocks --session-length 8
```

### Date Filtering

Filter blocks by date range:

```bash
# Show blocks from specific date range
ccusage blocks --since 20260515 --until 20260516

# Show blocks from last week
ccusage blocks --since $(date -d '7 days ago' +%Y%m%d)
```

### Sort Order

```bash
# Show oldest blocks first (default)
ccusage blocks --order asc

# Show newest blocks first
ccusage blocks --order desc
```

### Cost Calculation Modes

```bash
# Use pre-calculated costs when available (default)
ccusage blocks --mode auto

# Always calculate costs from tokens
ccusage blocks --mode calculate

# Only show pre-calculated costs
ccusage blocks --mode display
```

### JSON Output

Export block data for analysis:

```bash
ccusage blocks --json
```

`blocks` holds every block, including gaps (`isGap: true`). `burnRate` and `projection` are set only on the active block, `tokenLimitStatus` appears on the active block when `--token-limit` is given, and `usageLimitResetTime` appears when Claude Code reported a usage-limit reset inside the block. There is no `totals` object. This excerpt of `ccusage claude blocks --json` on the test fixtures keeps the same three blocks as the table above:

```json
{
	"blocks": [
		{
			"actualEndTime": "2026-01-01T12:30:00.000Z",
			"burnRate": null,
			"costUSD": 0.011850000000000001,
			"endTime": "2026-01-01T17:00:00.000Z",
			"entries": 3,
			"id": "2026-01-01T12:00:00.000Z",
			"isActive": false,
			"isGap": false,
			"models": ["claude-sonnet-4-20250514", "claude-opus-4-20250514", "mystery-model-9000"],
			"projection": null,
			"startTime": "2026-01-01T12:00:00.000Z",
			"tokenCounts": {
				"cacheCreationInputTokens": 0,
				"cacheReadInputTokens": 2300,
				"inputTokens": 950,
				"outputTokens": 220
			},
			"totalTokens": 3470
		},
		{
			"actualEndTime": null,
			"burnRate": null,
			"costUSD": 0,
			"endTime": "2026-01-02T08:00:00.000Z",
			"entries": 0,
			"id": "gap-2026-01-01T17:30:00.000Z",
			"isActive": false,
			"isGap": true,
			"models": [],
			"projection": null,
			"startTime": "2026-01-01T17:30:00.000Z",
			"tokenCounts": {
				"cacheCreationInputTokens": 0,
				"cacheReadInputTokens": 0,
				"inputTokens": 0,
				"outputTokens": 0
			},
			"totalTokens": 0
		},
		{
			"actualEndTime": "2026-01-02T11:00:00.000Z",
			"burnRate": {
				"costPerHour": 0.021562500000000002,
				"tokensPerMinute": 60.833333333333336,
				"tokensPerMinuteForIndicator": 11.666666666666666
			},
			"costUSD": 0.06468750000000001,
			"endTime": "2026-01-02T13:00:00.000Z",
			"entries": 4,
			"id": "2026-01-02T08:00:00.000Z",
			"isActive": true,
			"isGap": false,
			"models": ["claude-opus-5-fast", "claude-sonnet-4-20250514", "claude-opus-4-20250514"],
			"projection": {
				"remainingMinutes": 60,
				"totalCost": 0.09,
				"totalTokens": 14600
			},
			"startTime": "2026-01-02T08:00:00.000Z",
			"tokenCounts": {
				"cacheCreationInputTokens": 50,
				"cacheReadInputTokens": 8800,
				"inputTokens": 1150,
				"outputTokens": 950
			},
			"totalTokens": 10950,
			"usageLimitResetTime": "2026-01-02T09:00:00.000Z"
		}
	]
}
```

### Offline Mode

Use embedded pricing data without network access:

```bash
ccusage blocks --offline
# or short form:
ccusage blocks -O
```

## Analysis Use Cases

### Session Planning

Understanding 5-hour windows helps with:

```bash
# Check current active block
ccusage blocks --active
```

- **Time Management**: Know how much time remains in current session
- **Usage Pacing**: Monitor if you're on track for reasonable usage
- **Break Planning**: Understand when current session will expire

### Usage Optimization

```bash
# Find your highest usage patterns
ccusage blocks -t max --recent
```

- **Peak Usage Identification**: Which blocks consumed the most tokens
- **Efficiency Patterns**: Compare block efficiency (tokens per hour)
- **Model Selection Impact**: How model choice affects block costs

### Real-time Session Tracking

```bash
# Monitor active sessions in real-time
ccusage statusline
```

Perfect for:

- **Long coding sessions**: Track progress against historical limits
- **Budget management**: Watch costs accumulate in real-time
- **Productivity tracking**: Understand work intensity patterns

### Historical Analysis

```bash
# Export data for detailed analysis
ccusage blocks --json > blocks-history.json

# Analyze patterns over time
ccusage blocks --since 20260501 --until 20260531
```

## Block Analysis Tips

### 1. Understanding Block Efficiency

Look for patterns in your block data:

- **High-efficiency blocks**: Lots of output tokens for minimal input
- **Exploratory blocks**: High input/output ratios (research, debugging)
- **Focused blocks**: Steady token burn rates with clear objectives

### 2. Time Management

Use blocks to optimize your Claude usage:

- **Session planning**: Start important work at the beginning of blocks
- **Break timing**: Use block boundaries for natural work breaks
- **Batch processing**: Group similar tasks within single blocks

### 3. Cost Optimization

Blocks help identify cost patterns:

- **Model switching**: When to use a larger model and when a smaller one is enough within blocks
- **Cache efficiency**: How cache usage affects block costs
- **Usage intensity**: Whether short focused sessions or long exploratory ones are more cost-effective

### 4. Quota Management

When working with token limits:

- **Rate monitoring**: Watch burn rates to avoid exceeding limits
- **Early warning**: Set limits below actual quotas for safety margin
- **Usage spreading**: Distribute heavy usage across multiple blocks

## Responsive Display

Blocks reports adapt to your terminal width: narrow terminals switch to a compact layout with shorter times. Use `--compact` to force it.

## Related Commands

- [Daily Usage](/guide/daily-reports) - Usage aggregated by calendar date
- [Monthly Usage](/guide/monthly-reports) - Monthly usage summaries
- [Session Usage](/guide/session-reports) - Individual conversation analysis
- [Statusline](/guide/statusline) - Real-time session tracking (replacement for live monitoring)

## Next Steps

After understanding block patterns, consider:

1. [Statusline](/guide/statusline) for real-time active session tracking
2. [Session Usage](/guide/session-reports) to analyze individual conversations within blocks
3. [Daily Usage](/guide/daily-reports) to see how blocks aggregate across days
