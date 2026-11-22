# OpenClaw Data Source (Experimental)

ccusage can experimentally read OpenClaw session data as one of its supported local data sources. OpenClaw is a multi-provider coding agent that stores per-message token usage and embedded cost data in JSONL session files.

## What is OpenClaw?

OpenClaw is a third-party coding (agent) CLI (previously known as `clawdbot`, `moltbot`, and `moldbot`) that records assistant messages with token counts, cache metrics, and per-message cost. ccusage reads these JSONL files and aggregates them alongside its other supported sources.

## Focused Views

```bash
# Recommended
bunx ccusage openclaw --help

# Alternative package runners
npx ccusage@latest openclaw --help
pnpm dlx ccusage openclaw --help
pnpx ccusage openclaw --help
```

## Data Source

The CLI scans these directories for OpenClaw session files:

| Source   | Default paths                                                | Override                             |
| -------- | ------------------------------------------------------------ | ------------------------------------ |
| OpenClaw | `~/.openclaw/`, `~/.clawdbot/`, `~/.moltbot/`, `~/.moldbot/` | `OPENCLAW_DIR` or `--open-claw-path` |

ccusage walks each root recursively (typically `<root>/agents/<agentId>/sessions/<uuid>.jsonl`) and also picks up archived transcripts named `<uuid>.jsonl.deleted.<timestamp>` and `<uuid>.jsonl.reset.<timestamp>` so previously consumed tokens remain visible in totals.

Since OpenClaw 2026.9.x stores live sessions in per-agent SQLite databases (`<root>/agents/<agentId>/agent/openclaw-agent.sqlite`), ccusage also reads assistant usage from each database's `transcript_events` table. The SQLite rows reuse the same token mapping and embedded `usage.cost` contract as the JSONL path, and win over migrated JSONL duplicates of the same event.

Both `OPENCLAW_DIR` and `--open-claw-path` can be one root directory or a comma-separated list of root directories.

## Report Views

```bash
# Show daily OpenClaw usage
ccusage openclaw daily

# Show weekly OpenClaw usage
ccusage openclaw weekly

# Show monthly OpenClaw usage
ccusage openclaw monthly

# Show session-based OpenClaw usage
ccusage openclaw session

# JSON output for automation
ccusage openclaw daily --json

# Custom OpenClaw root
ccusage openclaw daily --open-claw-path /path/to/.openclaw

# Multiple OpenClaw roots
ccusage openclaw daily --open-claw-path /path/to/.openclaw,/archive/.clawdbot

# Filter by date range
ccusage openclaw daily --since 2026-05-01 --until 2026-05-16
```

## Cost Calculation

OpenClaw session messages embed a `cost.total` value per assistant message. In the default `auto` mode, ccusage uses an embedded cost when present and calculates from tokens otherwise. Use `--offline` to prevent pricing network requests. With `--mode calculate`, ccusage resolves the raw model identity through the merged LiteLLM and models.dev pricing data; this includes timestamp-aware DeepSeek V4 rates when applicable.

## Model Attribution

OpenClaw sessions emit `model_change` and `custom`/`model-snapshot` events that set the active provider and model for subsequent assistant messages. ccusage tracks this state per file and tags every model name with an `[openclaw]` prefix in the `modelsUsed` column to keep it distinguishable in the unified `ccusage daily` view.

## Environment Variables

| Variable       | Description                                                                                                   |
| -------------- | ------------------------------------------------------------------------------------------------------------- |
| `OPENCLAW_DIR` | Custom path, or comma-separated paths, to OpenClaw root directories                                           |
| `LOG_LEVEL`    | `0` hides aggregated warnings, progress, and table titles ([details](/guide/environment-variables#log-level)) |

## Daily View

This view shows daily usage from OpenClaw.

```bash
# Recommended (fastest)
bunx ccusage openclaw daily

# Using npx
npx ccusage@latest openclaw daily
```

### Options

| Flag               | Short | Description                                                         |
| ------------------ | ----- | ------------------------------------------------------------------- |
| `--since`          |       | Start date filter (YYYY-MM-DD or YYYYMMDD)                          |
| `--until`          |       | End date filter (YYYY-MM-DD or YYYYMMDD)                            |
| `--timezone`       | `-z`  | Override timezone for date grouping                                 |
| `--json`           | `-j`  | Emit structured JSON instead of a table                             |
| `--compact`        |       | Force compact table layout for narrow terminals                     |
| `--open-claw-path` |       | Custom path, or comma-separated paths, to OpenClaw root directories |

### Example Output

`ccusage openclaw daily` on the fixture data:

```text
┌────────────┬───────────────────────────┬───────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┐
│ Date       │ Models                    │     Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │
├────────────┼───────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12-27 │ - [openclaw]              │       111 │        57 │             0 │           0 │           168 │       $0.01 │
│            │ claude-opus-4             │           │           │               │             │               │             │
│            │ - [openclaw] unknown      │           │           │               │             │               │             │
├────────────┼───────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12-30 │ - [openclaw]              │     2,000 │       400 │           600 │       8,000 │        11,000 │       $1.10 │
│            │ claude-sonnet-4           │           │           │               │             │               │             │
├────────────┼───────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12-31 │ - [openclaw]              │     3,740 │     1,225 │         1,000 │      15,000 │        20,965 │       $0.37 │
│            │ claude-haiku-4-5          │           │           │               │             │               │             │
│            │ - [openclaw] gpt-5        │           │           │               │             │               │             │
│            │ - [openclaw]              │           │           │               │             │               │             │
│            │ openclaw-mystery-1        │           │           │               │             │               │             │
├────────────┼───────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-01-01 │ - [openclaw]              │     2,413 │       854 │           200 │      11,500 │        14,967 │       $0.01 │
│            │ claude-haiku-4-5          │           │           │               │             │               │             │
│            │ - [openclaw]              │           │           │               │             │               │             │
│            │ claude-sonnet-4           │           │           │               │             │               │             │
│            │ - [openclaw] gpt-5        │           │           │               │             │               │             │
├────────────┼───────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-01-02 │ - [openclaw]              │       860 │       340 │            50 │       6,000 │         7,600 │       $0.33 │
│            │ openclaw-mystery-1        │           │           │               │             │               │             │
│            │ - [openclaw] unknown      │           │           │               │             │               │             │
├────────────┼───────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Total      │                           │     9,124 │     2,876 │         1,850 │      40,500 │        54,700 │       $1.83 │
└────────────┴───────────────────────────┴───────────┴───────────┴───────────────┴─────────────┴───────────────┴─────────────┘
```

### JSON Output

Use `--json` for automation and scripting:

```bash
ccusage openclaw daily --json
```

This excerpt keeps one row; `totals` covers the complete report. `storedCost` is the part of `cost` that OpenClaw recorded itself, and `missingPricing` marks a model ccusage has no price for:

```json
{
	"daily": [
		{
			"cacheCreationTokens": 1000,
			"cacheReadTokens": 15000,
			"date": "2025-12-31",
			"inputTokens": 3740,
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 0,
					"cost": 0.2,
					"inputTokens": 340,
					"missingPricing": true,
					"modelName": "[openclaw] openclaw-mystery-1",
					"outputTokens": 80,
					"storedCost": 0.2
				},
				{
					"cacheCreationTokens": 1000,
					"cacheReadTokens": 10000,
					"cost": 0.11,
					"inputTokens": 2000,
					"modelName": "[openclaw] claude-haiku-4-5-20251001",
					"outputTokens": 500,
					"storedCost": 0.11
				},
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 5000,
					"cost": 0.06344999999999999,
					"inputTokens": 1400,
					"modelName": "[openclaw] gpt-5",
					"outputTokens": 645,
					"storedCost": 0.06
				}
			],
			"modelsUsed": [
				"[openclaw] claude-haiku-4-5-20251001",
				"[openclaw] gpt-5",
				"[openclaw] openclaw-mystery-1"
			],
			"outputTokens": 1225,
			"totalCost": 0.37345000000000006,
			"totalTokens": 20965
		}
	],
	"totals": {
		"cacheCreationTokens": 1850,
		"cacheReadTokens": 40500,
		"inputTokens": 9124,
		"outputTokens": 2876,
		"totalCost": 1.8257687500000002,
		"totalTokens": 54700,
		"unpricedModels": ["[openclaw] openclaw-mystery-1", "[openclaw] unknown"]
	}
}
```

## Monthly View

This view shows monthly usage from OpenClaw.

```bash
ccusage openclaw monthly
ccusage openclaw monthly --json
ccusage openclaw monthly --since 2026-01-01 --until 2026-03-31
```

## Session View

This view shows usage grouped by individual OpenClaw sessions. Session IDs come from the JSONL filename stem (the part before `.jsonl`, ignoring `.deleted.<ts>` or `.reset.<ts>` suffixes), and JSON output records activity timestamps and provider metadata for the most recent activity.

```bash
ccusage openclaw session
ccusage openclaw session --json
ccusage openclaw session --since 2026-05-09
```

### Example Output

This excerpt from the fixture report keeps the `gamma` session and the Total row; the other sessions are omitted:

```text
┌──────────┬───────────────────────────┬───────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┬───────────────┐
│ Session  │ Models                    │     Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │ Last Activity │
├──────────┼───────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┼───────────────┤
│ gamma    │ - [openclaw]              │     1,100 │       250 │           300 │       4,000 │         5,650 │       $0.51 │ 2025-12-30    │
│          │ claude-opus-4             │           │           │               │             │               │             │               │
│          │ - [openclaw]              │           │           │               │             │               │             │               │
│          │ claude-sonnet-4           │           │           │               │             │               │             │               │
├──────────┼───────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┼───────────────┤
│ Total    │                           │     9,124 │     2,876 │         1,850 │      40,500 │        54,700 │       $1.83 │               │
└──────────┴───────────────────────────┴───────────┴───────────┴───────────────┴─────────────┴───────────────┴─────────────┴───────────────┘
```

## Related

- [ccusage](https://github.com/ccusage/ccusage) - Main usage analysis tool for coding (agent) CLIs
