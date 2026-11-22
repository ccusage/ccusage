# Session Usage

Session usage shows usage grouped by individual conversations, threads, or sessions. `ccusage session` combines all detected supported sources; use `ccusage <source> session` to inspect one source's session format.

## Basic Usage

```bash
ccusage session
ccusage codex session
ccusage opencode session
ccusage amp session
ccusage pi session
ccusage qwen session
```

Use `ccusage claude session --project <name>` to list sessions from one Claude project. The same flag on `ccusage session` filters the Claude Code rows in the unified report.

## Specific Session Lookup

Query individual session details by providing a session ID:

```bash
ccusage claude session --id <session-id>
ccusage codex session --id <session-id>
```

Claude session lookups with `--id` search across projects even when a project filter is set.

This is particularly useful for:

- **Custom statuslines**: Integrate specific session data into your development environment
- **Programmatic usage**: Extract session metrics for scripts and automation
- **Detailed analysis**: Get comprehensive data about a single conversation

### Examples

```bash
# Get session data in table format
ccusage claude session --id session-abc123-def456

# Add per-model rows to the session table
ccusage claude session --id session-abc123-def456 --breakdown

# Get session data as JSON for scripting
ccusage codex session --id codex://threads/01a0bb8c-c7c0-7630-9d82-860b875930f0 --json

# Extract just the cost using jq
ccusage codex session --id 01a0bb8c-c7c0-7630-9d82-860b875930f0 --json | jq '.totalCost'

# Use in a custom statusline script
COST=$(ccusage claude session --id "$SESSION_ID" --json | jq '.totalCost')
echo "Current session: \$${COST}"
```

The table shows the same columns as `ccusage claude session`, restricted to that one session, and `--breakdown` adds a row per model. Claude's JSON output keeps `sessionId`, `totalCost`, `totalTokens`, and the per-request `entries`, and also carries the session's `modelsUsed` and per-model `modelBreakdowns`.

### Session ID Format

For Claude Code, session IDs are the actual filenames (without `.jsonl` extension) stored in Claude data directories. They typically look like:

- `session-20260516-abc123-def456`
- `project-conversation-xyz789`

You can find Claude session IDs by running `ccusage claude session` and looking for the files in your Claude data directory.

For Codex, use the `sessionId` from `ccusage codex session --json`. The lookup accepts that full ID, its JSONL filename with or without the `.jsonl` or `.jsonl.zst` extension, the trailing UUID, or a `codex://threads/<uuid>` URI. A shortened UUID is not accepted, and a UUID that matches more than one session is reported as ambiguous.

## Example Output

`ccusage session` lists the most expensive session first. This excerpt from the fixture report keeps that first row and the Total row; the other sessions are omitted:

```text
┌──────────┬────────────────────┬───────────────────────────┬────────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┐
│ Session  │ Agent              │ Models                    │      Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │
├──────────┼────────────────────┼───────────────────────────┼────────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ sess-be… │ pi-agent           │ - [pi] claude-haiku-4-5   │      5,055 │     2,126 │           400 │      18,000 │        25,831 │       $4.02 │
│          │                    │ - [pi] claude-opus-4      │            │           │               │             │               │             │
│          │                    │ - [pi] claude-sonnet-4    │            │           │               │             │               │             │
│          │                    │ - [pi] gpt-5              │            │           │               │             │               │             │
├──────────┼────────────────────┼───────────────────────────┼────────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Total    │                    │                           │  1,348,649 │    58,942 │        17,865 │   1,311,467 │     2,740,360 │      $28.93 │
└──────────┴────────────────────┴───────────────────────────┴────────────┴───────────┴───────────────┴─────────────┴───────────────┴─────────────┘
```

The unified table has an Agent column and no Last Activity column; its JSON keeps the timestamp in `metadata.lastActivity`. Focused Claude tables include Last Activity.

## Understanding Session Data

### Session Metrics

- **Session**: The session ID. When the table is wider than the terminal, long IDs are shortened with `…`, as `sess-be…` is above; the unified table is often shortened because some sources use long IDs. The full ID is `period` in `ccusage session --json`, or `sessionId` in a focused report's JSON such as `ccusage claude session --json`
- **Input/Output Tokens**: Total tokens exchanged in the conversation
- **Cache Tokens**: Cache creation and read tokens for context efficiency
- **Cost**: Estimated USD cost for the usage shown; without a date filter, the entire conversation
- **Last Activity**: Date of the most recent message included in the report, shown in the report timezone (`--timezone`); JSON output keeps the full RFC 3339 [`lastActivity`](/guide/json-output#session-reports) timestamp

### Sorting

Claude session reports also default to highest cost first. Use `ccusage claude session --order asc` for lowest cost first; configured order is used unless a CLI flag overrides it.

Unified session reports are sorted by cost (highest first) by default. Use `ccusage session --order asc` for lowest cost first or `--order desc` for highest cost first. Equal costs sort by session ID, then agent, in ascending order.

The `order` configuration option also controls this direction; an explicit CLI flag takes precedence. With mixed `--sections` reports, the selected order applies to session costs and chronological periods. Without an order option, sessions use descending cost while daily, weekly, and monthly sections stay chronological (ascending).

## Command Options

### Session ID Lookup

Get detailed information about a specific session:

```bash
# Query a specific session by ID
ccusage claude session --id <session-id>
ccusage codex session --id <session-id>

# Get JSON output for a specific session
ccusage codex session --id <session-id> --json

# Short form using -i flag
ccusage codex session -i <session-id>
```

**Use cases:**

- Building custom statuslines that show current session costs
- Creating scripts that monitor specific conversation expenses
- Debugging or analyzing individual conversation patterns
- Integrating session data into development workflows

### Date Filtering

For Claude Code, filter session usage by the local date of each entry:

```bash
# Show sessions active since May 10th
ccusage claude session --since 20260510

# Show sessions active in a specific date range
ccusage claude session --since 20260501 --until 20260516

# Show only recent sessions (last week)
ccusage claude session --since $(date -d '7 days ago' +%Y%m%d)
```

For `ccusage claude session` and the Claude rows of `ccusage session`, date filters are applied before session totals are calculated, so token and cost totals represent only the entries inside the selected window. A session that continues past `--until` still shows its usage from inside the window. Both bounds are inclusive, and `--timezone` determines the local date used for filtering.

### Cost Calculation Modes

```bash
# Use pre-calculated costs when available (default)
ccusage session --mode auto

# Always calculate costs from tokens
ccusage session --mode calculate

# Only show pre-calculated costs
ccusage session --mode display
```

### Model Breakdown

See per-model cost breakdown within each session:

```bash
ccusage claude session --breakdown
```

Each session row is followed by one `└─` row per model. This excerpt from a 120-column terminal keeps the first session, `sess-beta-1`, with its model rows; the other sessions are omitted:

```text
┌──────────┬───────────────┬───────────┬───────────┬─────────────┬────────────┬────────────┬────────────┬──────────────┐
│ Session  │ Models        │     Input │    Output │       Cache │ Cache Read │      Total │ Cost (USD) │ Last         │
│          │               │           │           │      Create │            │     Tokens │            │ Activity     │
├──────────┼───────────────┼───────────┼───────────┼─────────────┼────────────┼────────────┼────────────┼──────────────┤
│ sess-be… │ - opus-4      │     2,353 │       854 │         250 │     16,000 │     19,457 │      $0.57 │ 2026-01-02   │
│          │ - sonnet-4    │           │           │             │            │            │            │              │
├──────────┼───────────────┼───────────┼───────────┼─────────────┼────────────┼────────────┼────────────┼──────────────┤
│ └─       │               │     1,453 │       454 │          50 │      9,000 │     10,957 │      $0.51 │              │
│ sonnet-4 │               │           │           │             │            │            │            │              │
├──────────┼───────────────┼───────────┼───────────┼─────────────┼────────────┼────────────┼────────────┼──────────────┤
│ └─       │               │       900 │       400 │         200 │      7,000 │      8,500 │      $0.06 │              │
│ opus-4   │               │           │           │             │            │            │            │              │
├──────────┼───────────────┼───────────┼───────────┼─────────────┼────────────┼────────────┼────────────┼──────────────┤
│ Total    │               │     8,207 │     3,659 │       2,500 │     44,350 │     58,716 │      $0.85 │              │
└──────────┴───────────────┴───────────┴───────────┴─────────────┴────────────┴────────────┴────────────┴──────────────┘
```

### JSON Output

Export session data as JSON for further analysis:

```bash
ccusage session --json
```

Each unified session row carries its `agent`, and `period` holds the session ID. This excerpt keeps the Claude session `sess-beta-1`; `totals` covers the complete report:

```json
{
	"session": [
		{
			"agent": "claude",
			"cacheCreationTokens": 250,
			"cacheReadTokens": 16000,
			"inputTokens": 2353,
			"metadata": {
				"lastActivity": "2026-01-02T10:00:00.000Z"
			},
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 50,
					"cacheReadTokens": 9000,
					"cost": 0.5098565,
					"inputTokens": 1453,
					"modelName": "claude-sonnet-4-20250514",
					"outputTokens": 454,
					"storedCost": 0.5
				},
				{
					"cacheCreationTokens": 200,
					"cacheReadTokens": 7000,
					"cost": 0.05775,
					"inputTokens": 900,
					"modelName": "claude-opus-4-20250514",
					"outputTokens": 400
				}
			],
			"modelsUsed": ["claude-opus-4-20250514", "claude-sonnet-4-20250514"],
			"outputTokens": 854,
			"period": "sess-beta-1",
			"totalCost": 0.5676065,
			"totalTokens": 19457
		}
	],
	"totals": {
		"cacheCreationTokens": 17865,
		"cacheReadTokens": 1311467,
		"inputTokens": 1348649,
		"outputTokens": 58942,
		"totalCost": 28.93028592500002,
		"totalTokens": 2740360,
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

`ccusage claude session --json` uses the Claude shape instead: a `sessions` array whose rows carry `sessionId`, `projectPath`, `firstActivity`, and `lastActivity` at the top level. See [JSON Output](/guide/json-output#session-reports).

### Offline Mode

Use cached pricing data without network access:

```bash
ccusage session --offline
# or short form:
ccusage session -O
```

## Analysis Use Cases

### Identify Expensive Conversations

Session reports help you understand which conversations are most costly:

```bash
ccusage session
```

Look at the top sessions to understand:

- Which types of conversations cost the most
- Whether long coding sessions or research tasks are more expensive
- How model choice (Opus 4.1 vs Sonnet 4.5) affects costs

### Track Conversation Patterns

```bash
# See recent conversation activity
ccusage session --since 20260510

# Compare different time periods
ccusage session --since 20260501 --until 20260515  # First half of month
ccusage session --since 20260516 --until 20260531  # Second half of month
```

### Model Usage Analysis

```bash
# See which models you use in different conversations
ccusage session --breakdown
```

This helps understand:

- Whether you prefer Opus 4.1 for complex tasks
- If Sonnet 4.5 is sufficient for routine work
- How model mixing affects total costs

### Budget Optimization

```bash
# Export data for spreadsheet analysis
ccusage session --json > sessions.json

# Find sessions above a certain cost threshold
ccusage session --json | jq '.session[] | select(.totalCost > 50)'
```

## Tips for Session Analysis

### 1. Cost Context Understanding

Session costs help you understand:

- **Conversation Value**: High-cost sessions should provide proportional value
- **Efficiency Patterns**: Some conversation styles may be more token-efficient
- **Model Selection**: Whether your model choices align with task complexity

### 2. Usage Optimization

Use session data to:

- **Identify expensive patterns**: What makes some conversations cost more?
- **Optimize conversation flow**: Break long sessions into smaller focused chats
- **Choose appropriate models**: Use Sonnet 4.5 for simpler tasks, Opus 4.1 for complex ones

### 3. Budget Planning

Session analysis helps with:

- **Conversation budgeting**: Understanding typical session costs
- **Usage forecasting**: Predicting monthly costs based on session patterns
- **Value assessment**: Ensuring expensive sessions provide good value

### 4. Comparative Analysis

Compare sessions to understand:

- **Task types**: Coding vs writing vs research costs
- **Model effectiveness**: Whether Opus 4.1 provides value over Sonnet 4.5
- **Time patterns**: Whether longer sessions are more or less efficient

## Responsive Display

Session reports adapt to your terminal width:

- **Wide terminals**: Show the full table when it fits
- **Narrow terminals**: Use a compact layout; `--compact` also selects it

When in compact mode, ccusage displays a message explaining how to see the full data.

## Related Commands

- [All Sources (Default)](/guide/all-reports) - How unified views work
- [Daily Usage](/guide/daily-reports) - Usage aggregated by date
- [Monthly Usage](/guide/monthly-reports) - Monthly summaries
- [Claude Code](/guide/claude/) - Claude Code-specific setup and features

## Next Steps

After analyzing session patterns, consider:

1. [Daily Usage](/guide/daily-reports) to see how session patterns vary by day
2. [Monthly Usage](/guide/monthly-reports) to compare longer-term patterns
3. [Claude Code](/guide/claude/) for Claude Code-specific setup and features
