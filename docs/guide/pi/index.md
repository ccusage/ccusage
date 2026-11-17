# pi-agent Data Source (Beta)

ccusage can read [pi-agent](https://github.com/badlogic/pi-mono) usage data as one of its supported local data sources. pi-agent is an alternative Claude coding (agent) CLI from [shittycodingagent.ai](https://shittycodingagent.ai).

## What is Pi-Agent?

Pi-agent is a third-party Claude coding (agent) CLI that stores usage data in JSONL format. ccusage analyzes this data alongside its other supported sources.

## Focused Views

```bash
# Recommended
bunx ccusage pi --help

# Alternative package runners
npx ccusage@latest pi --help
pnpm dlx ccusage pi --help
pnpx ccusage pi --help
```

## Data Source

The CLI reads usage data from pi-agent:

| Source   | Default path            | Override                      |
| -------- | ----------------------- | ----------------------------- |
| pi-agent | `~/.pi/agent/sessions/` | `PI_AGENT_DIR` or `--pi-path` |

Both `PI_AGENT_DIR` and `--pi-path` can be one sessions directory or a comma-separated list of sessions directories.

Tools built on the pi session format can also be declared as named stores in the config file for unified reports:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"pi": {
		"stores": [
			{
				"name": "omp",
				"path": "~/.omp/agent/sessions"
			}
		]
	}
}
```

Named stores are loaded in addition to the default `pi` agent when running `ccusage daily`, `ccusage monthly`, `ccusage weekly`, or `ccusage session`. The example above appears as agent `omp` in unified report metadata and prefixes model labels with `[omp]` followed by a space. A named store path can also be a comma-separated list of sessions directories; missing paths are treated as empty, while paths that overlap the default `pi` store or another named store — including one path nested inside another — are rejected to avoid double-counting. It does not add a `ccusage omp` command; use `ccusage pi ...` for the default pi-agent store.

For custom Pi models, add a [`pricingOverrides` entry](/guide/config-files#pricing-overrides) keyed by the model label, such as `[pi] my-custom-model` or `[omp] my-custom-model` for a named store. In the default `auto` mode, an explicit override calculates cost from tokens even when Pi recorded a zero cost. Use `--mode display` to show Pi's recorded cost instead.

## Report Views

```bash
# Show daily pi-agent usage
ccusage pi daily

# Show monthly pi-agent usage
ccusage pi monthly

# Show session-based pi-agent usage
ccusage pi session

# JSON output for automation
ccusage pi daily --json

# Custom pi-agent path
ccusage pi daily --pi-path /path/to/sessions

# Multiple pi-agent paths
ccusage pi daily --pi-path /path/to/sessions,/archive/pi/sessions

# Filter by date range
ccusage pi daily --since 2026-05-01 --until 2026-05-16

# Show model breakdown
ccusage pi daily --breakdown
```

## Environment Variables

| Variable       | Description                                                                                                   |
| -------------- | ------------------------------------------------------------------------------------------------------------- |
| `PI_AGENT_DIR` | Custom path, or comma-separated paths, to pi-agent sessions directories                                       |
| `LOG_LEVEL`    | `0` hides aggregated warnings, progress, and table titles ([details](/guide/environment-variables#log_level)) |

## Daily View

This view shows daily usage from pi-agent.

```bash
# Recommended (fastest)
bunx ccusage pi daily

# Using npx
npx ccusage@latest pi daily
```

### Options

| Flag          | Short | Description                                                             |
| ------------- | ----- | ----------------------------------------------------------------------- |
| `--since`     |       | Start date filter (YYYY-MM-DD or YYYYMMDD)                              |
| `--until`     |       | End date filter (YYYY-MM-DD or YYYYMMDD)                                |
| `--timezone`  | `-z`  | Override timezone for date grouping                                     |
| `--json`      |       | Emit structured JSON instead of a table                                 |
| `--breakdown` | `-b`  | Show per-model token breakdown                                          |
| `--pi-path`   |       | Custom path, or comma-separated paths, to pi-agent sessions directories |
| `--order`     |       | Sort order: `asc` (default) or `desc`                                   |

### Example Output

`ccusage pi daily` on the fixture data:

```text
┌────────────┬─────────────────────────┬───────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┐
│ Date       │ Models                  │     Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │
├────────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12-27 │ - [pi] gpt-5            │     5,412 │       542 │         1,134 │         443 │         7,531 │       $3.38 │
│            │ - [pi] pi-mystery-1     │           │           │               │             │               │             │
│            │ - [pi] pi-mystery-2     │           │           │               │             │               │             │
├────────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12-30 │ - [pi] claude-opus-4    │     2,070 │       450 │           400 │       8,000 │        10,920 │       $1.17 │
│            │ - [pi] claude-sonnet-4  │           │           │               │             │               │             │
├────────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12-31 │ - [pi] claude-haiku-4-5 │     2,550 │       956 │           501 │      19,006 │        23,013 │       $4.02 │
│            │ - [pi] claude-sonnet-4  │           │           │               │             │               │             │
├────────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-01-01 │ - [pi] claude-haiku-4-5 │     1,915 │     1,185 │           200 │      11,000 │        14,550 │       $0.05 │
│            │ - [pi] claude-opus-4    │           │           │               │             │               │             │
│            │ - [pi] claude-sonnet-4  │           │           │               │             │               │             │
│            │ - [pi] gpt-5            │           │           │               │             │               │             │
│            │ - [pi] pi-mystery-1     │           │           │               │             │               │             │
├────────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-01-02 │ - [pi] claude-haiku-4-5 │     3,620 │       963 │             0 │       6,000 │        10,583 │       $0.50 │
│            │ - [pi] claude-opus-4    │           │           │               │             │               │             │
│            │ - [pi] claude-sonnet-4  │           │           │               │             │               │             │
│            │ - [pi] gpt-5            │           │           │               │             │               │             │
├────────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Total      │                         │    15,567 │     4,096 │         2,235 │      44,449 │        66,597 │       $9.12 │
└────────────┴─────────────────────────┴───────────┴───────────┴───────────────┴─────────────┴───────────────┴─────────────┘
```

### JSON Output

Use `--json` for automation and scripting:

```bash
ccusage pi daily --json
```

This excerpt keeps one row; `totals` covers the complete report. Model names carry the `[pi]` prefix, and `storedCost` is the part of `cost` that pi-agent recorded itself:

```json
{
	"daily": [
		{
			"cacheCreationTokens": 400,
			"cacheReadTokens": 8000,
			"date": "2025-12-30",
			"inputTokens": 2070,
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 0,
					"cost": 1.1,
					"inputTokens": 270,
					"modelName": "[pi] claude-opus-4-20250514",
					"outputTokens": 90,
					"storedCost": 1.1
				},
				{
					"cacheCreationTokens": 400,
					"cacheReadTokens": 8000,
					"cost": 0.07,
					"inputTokens": 1800,
					"modelName": "[pi] claude-sonnet-4-20250514",
					"outputTokens": 360,
					"storedCost": 0.07
				}
			],
			"modelsUsed": ["[pi] claude-sonnet-4-20250514", "[pi] claude-opus-4-20250514"],
			"outputTokens": 450,
			"totalCost": 1.17,
			"totalTokens": 10920
		}
	],
	"totals": {
		"cacheCreationTokens": 2235,
		"cacheReadTokens": 44449,
		"inputTokens": 15567,
		"outputTokens": 4096,
		"totalCost": 9.11883435,
		"totalTokens": 66597,
		"unpricedModels": ["[pi] pi-mystery-1", "[pi] pi-mystery-2"]
	}
}
```

### Date Filtering

Filter to a specific date range:

```bash
# Last week
ccusage pi daily --since 2026-05-09 --until 2026-05-16

# Single day
ccusage pi daily --since 2026-05-16 --until 2026-05-16
```

## Monthly View

This view shows monthly usage from pi-agent.

```bash
# Recommended (fastest)
bunx ccusage pi monthly

# Using npx
npx ccusage@latest pi monthly
```

### Options

| Flag          | Short | Description                                                             |
| ------------- | ----- | ----------------------------------------------------------------------- |
| `--since`     |       | Start date filter (YYYY-MM-DD or YYYYMMDD)                              |
| `--until`     |       | End date filter (YYYY-MM-DD or YYYYMMDD)                                |
| `--timezone`  | `-z`  | Override timezone for date grouping                                     |
| `--json`      |       | Emit structured JSON instead of a table                                 |
| `--breakdown` | `-b`  | Show per-model token breakdown                                          |
| `--pi-path`   |       | Custom path, or comma-separated paths, to pi-agent sessions directories |
| `--order`     |       | Sort order: `asc` (default) or `desc`                                   |

### Example Output

`ccusage pi monthly` on the fixture data:

```text
┌──────────┬─────────────────────────┬───────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┐
│ Month    │ Models                  │     Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │
├──────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2025-12  │ - [pi] claude-haiku-4-5 │    10,032 │     1,948 │         2,035 │      27,449 │        41,464 │       $8.57 │
│          │ - [pi] claude-opus-4    │           │           │               │             │               │             │
│          │ - [pi] claude-sonnet-4  │           │           │               │             │               │             │
│          │ - [pi] gpt-5            │           │           │               │             │               │             │
│          │ - [pi] pi-mystery-1     │           │           │               │             │               │             │
│          │ - [pi] pi-mystery-2     │           │           │               │             │               │             │
├──────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ 2026-01  │ - [pi] claude-haiku-4-5 │     5,535 │     2,148 │           200 │      17,000 │        25,133 │       $0.55 │
│          │ - [pi] claude-opus-4    │           │           │               │             │               │             │
│          │ - [pi] claude-sonnet-4  │           │           │               │             │               │             │
│          │ - [pi] gpt-5            │           │           │               │             │               │             │
│          │ - [pi] pi-mystery-1     │           │           │               │             │               │             │
├──────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┤
│ Total    │                         │    15,567 │     4,096 │         2,235 │      44,449 │        66,597 │       $9.12 │
└──────────┴─────────────────────────┴───────────┴───────────┴───────────────┴─────────────┴───────────────┴─────────────┘
```

### JSON Output

Use `--json` for automation and scripting:

```bash
ccusage pi monthly --json
```

Returns the same fields as the [daily JSON](#json-output), with `month` in place of `date`.

### Filtering by Date Range

You can filter the data to specific months:

```bash
# Current year only
ccusage pi monthly --since 2026-05-01

# Specific quarter
ccusage pi monthly --since 2026-01-01 --until 2026-03-31
```

## Session View

This view shows usage grouped by individual pi-agent sessions.

```bash
# Recommended (fastest)
bunx ccusage pi session

# Using npx
npx ccusage@latest pi session
```

### Options

| Flag          | Short | Description                                                             |
| ------------- | ----- | ----------------------------------------------------------------------- |
| `--since`     |       | Start date filter (YYYY-MM-DD or YYYYMMDD)                              |
| `--until`     |       | End date filter (YYYY-MM-DD or YYYYMMDD)                                |
| `--timezone`  | `-z`  | Override timezone for date grouping                                     |
| `--json`      |       | Emit structured JSON instead of a table                                 |
| `--breakdown` | `-b`  | Show per-model token breakdown                                          |
| `--pi-path`   |       | Custom path, or comma-separated paths, to pi-agent sessions directories |
| `--order`     |       | Sort order: `asc` or `desc`                                             |

### Example Output

This excerpt from the fixture report keeps one session and the Total row; the other sessions are omitted:

```text
┌────────────────────────┬─────────────────────────┬───────────┬───────────┬───────────────┬─────────────┬───────────────┬─────────────┬───────────────┐
│ Session                │ Models                  │     Input │    Output │  Cache Create │  Cache Read │  Total Tokens │  Cost (USD) │ Last Activity │
├────────────────────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┼───────────────┤
│ sess-alpha-fork        │ - [pi] claude-haiku-4-5 │     1,150 │       450 │           200 │       7,000 │         8,800 │       $0.03 │ 2026-01-02    │
│                        │ - [pi] claude-sonnet-4  │           │           │               │             │               │             │               │
├────────────────────────┼─────────────────────────┼───────────┼───────────┼───────────────┼─────────────┼───────────────┼─────────────┼───────────────┤
│ Total                  │                         │    15,567 │     4,096 │         2,235 │      44,449 │        66,597 │       $9.12 │               │
└────────────────────────┴─────────────────────────┴───────────┴───────────┴───────────────┴─────────────┴───────────────┴─────────────┴───────────────┘
```

### Session Identification

The Session column shows the session ID, shortened with `…` when the table is wider than the terminal; `sessionId` in the JSON output always holds the full ID. In JSON output, `projectPath` holds the project folder name from `~/.pi/agent/sessions/{project}/`.

### JSON Output

Use `--json` for detailed session data:

```bash
ccusage pi session --json
```

This excerpt keeps one session; `totals` covers the complete report. Each session row carries its `sessionId`, `projectPath`, `firstActivity`, and `lastActivity`:

```json
{
	"sessions": [
		{
			"cacheCreationTokens": 200,
			"cacheReadTokens": 7000,
			"firstActivity": "2026-01-01T13:10:00.000Z",
			"inputTokens": 1150,
			"lastActivity": "2026-01-02T10:30:00.000Z",
			"modelBreakdowns": [
				{
					"cacheCreationTokens": 200,
					"cacheReadTokens": 7000,
					"cost": 0.03,
					"inputTokens": 900,
					"modelName": "[pi] claude-haiku-4-5-20251001",
					"outputTokens": 400,
					"storedCost": 0.03
				},
				{
					"cacheCreationTokens": 0,
					"cacheReadTokens": 0,
					"cost": 0.0015,
					"inputTokens": 250,
					"modelName": "[pi] claude-sonnet-4-20250514",
					"outputTokens": 50
				}
			],
			"modelsUsed": ["[pi] claude-haiku-4-5-20251001", "[pi] claude-sonnet-4-20250514"],
			"outputTokens": 450,
			"projectPath": "--Users-dev-alpha--",
			"sessionId": "sess-alpha-fork",
			"totalCost": 0.0315,
			"totalTokens": 8800
		}
	],
	"totals": {
		"cacheCreationTokens": 2235,
		"cacheReadTokens": 44449,
		"inputTokens": 15567,
		"outputTokens": 4096,
		"totalCost": 9.11883435,
		"totalTokens": 66597,
		"unpricedModels": ["[pi] pi-mystery-1", "[pi] pi-mystery-2"]
	}
}
```

### Filtering Sessions

Filter entries by date before grouping them into sessions. Session totals include only usage inside the selected window:

```bash
# Session usage on one day
ccusage pi session --since 2026-05-16 --until 2026-05-16

# Session usage since a date
ccusage pi session --since 2026-05-09
```

## Related

- [ccusage](https://github.com/ccusage/ccusage) - Main usage analysis tool for coding (agent) CLIs
- [pi-agent](https://github.com/badlogic/pi-mono) - Alternative Claude coding (agent) CLI
