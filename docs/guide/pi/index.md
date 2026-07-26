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

| Variable       | Description                                                             |
| -------------- | ----------------------------------------------------------------------- |
| `PI_AGENT_DIR` | Custom path, or comma-separated paths, to pi-agent sessions directories |
| `LOG_LEVEL`    | Adjust logging verbosity (0 silent … 5 trace)                           |

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
| `--order`     |       | Sort order: `asc` or `desc` (default: `desc`)                           |

### Example Output

```
┌────────────┬────────────┬─────────────┬───────────┬───────────┬────────┬─────────┐
│ Date       │ Input      │ Output      │ Cache Cr. │ Cache Rd. │ Cost   │ Models  │
├────────────┼────────────┼─────────────┼───────────┼───────────┼────────┼─────────┤
│ 2026-05-16 │ 567,890    │ 123,456     │ 5,678     │ 45,678    │ $0.89  │ opus-4-1  │
├────────────┼────────────┼─────────────┼───────────┼───────────┼────────┼─────────┤
│ Total      │ 567,890    │ 123,456     │ 5,678     │ 45,678    │ $0.89  │         │
└────────────┴────────────┴─────────────┴───────────┴───────────┴────────┴─────────┘
```

### JSON Output

Use `--json` for automation and scripting:

```bash
ccusage pi daily --json
```

Returns structured data:

<!-- eslint-skip -->

```json
{
  "daily": [
    {
      "date": "2026-05-16",
      "source": "pi-agent",
      "inputTokens": 567890,
      "outputTokens": 123456,
      "cacheCreationTokens": 5678,
      "cacheReadTokens": 45678,
      "totalCost": 0.89,
      "modelsUsed": ["claude-opus-4-1-20250805"],
      "modelBreakdowns": [...]
    }
  ],
  "totals": {
    "inputTokens": 567890,
    "outputTokens": 123456,
    "cacheCreationTokens": 5678,
    "cacheReadTokens": 45678,
    "totalCost": 0.89
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
| `--order`     |       | Sort order: `asc` or `desc` (default: `desc`)                           |

### Example Output

```
┌─────────┬────────────┬─────────────┬───────────┬───────────┬─────────┬─────────┐
│ Month   │ Input      │ Output      │ Cache Cr. │ Cache Rd. │ Cost    │ Models  │
├─────────┼────────────┼─────────────┼───────────┼───────────┼─────────┼─────────┤
│ 2026-05 │ 12,345,678 │ 2,345,678   │ 123,456   │ 987,654   │ $12.34  │ opus-4-1  │
├─────────┼────────────┼─────────────┼───────────┼───────────┼─────────┼─────────┤
│ Total   │ 12,345,678 │ 2,345,678   │ 123,456   │ 987,654   │ $12.34  │         │
└─────────┴────────────┴─────────────┴───────────┴───────────┴─────────┴─────────┘
```

### JSON Output

Use `--json` for automation and scripting:

```bash
ccusage pi monthly --json
```

Returns structured data:

<!-- eslint-skip -->

```json
{
  "monthly": [
    {
      "month": "2026-05",
      "source": "pi-agent",
      "inputTokens": 12345678,
      "outputTokens": 2345678,
      "cacheCreationTokens": 123456,
      "cacheReadTokens": 987654,
      "totalCost": 12.34,
      "modelsUsed": ["claude-opus-4-1-20250805"],
      "modelBreakdowns": [...]
    }
  ],
  "totals": {
    "inputTokens": 12345678,
    "outputTokens": 2345678,
    "cacheCreationTokens": 123456,
    "cacheReadTokens": 987654,
    "totalCost": 12.34
  }
}
```

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
| `--order`     |       | Sort order: `asc` or `desc` (default: `desc`)                           |

### Example Output

Sessions are sorted by last activity:

```
┌──────────────────────────────┬────────────┬───────────┬───────────┬───────────┬────────┬─────────┐
│ Session                      │ Input      │ Output    │ Cache Cr. │ Cache Rd. │ Cost   │ Models  │
├──────────────────────────────┼────────────┼───────────┼───────────┼───────────┼────────┼─────────┤
│ my-project                   │ 123,456    │ 23,456    │ 1,234     │ 9,876     │ $0.12  │ opus-4-1  │
│ another-repo                 │ 345,678    │ 67,890    │ 3,456     │ 29,876    │ $0.34  │ sonnet-4-5│
├──────────────────────────────┼────────────┼───────────┼───────────┼───────────┼────────┼─────────┤
│ Total                        │ 469,134    │ 91,346    │ 4,690     │ 39,752    │ $0.46  │         │
└──────────────────────────────┴────────────┴───────────┴───────────┴───────────┴────────┴─────────┘
```

### Session Identification

Sessions are identified by the project folder name from `~/.pi/agent/sessions/{project}/`.

Long project names are truncated to 25 characters with `...` suffix for readability.

### JSON Output

Use `--json` for detailed session data:

```bash
ccusage pi session --json
```

Returns structured data including full paths:

<!-- eslint-skip -->

```json
{
  "sessions": [
    {
      "sessionId": "abc123-def456",
      "projectPath": "my-project",
      "source": "pi-agent",
      "inputTokens": 123456,
      "outputTokens": 23456,
      "cacheCreationTokens": 1234,
      "cacheReadTokens": 9876,
      "totalCost": 0.12,
      "firstActivity": "2026-05-15T09:30:00.000Z",
      "lastActivity": "2026-05-16T17:45:30.000Z",
      "modelsUsed": ["claude-opus-4-1-20250805"],
      "modelBreakdowns": [...]
    }
  ],
  "totals": {
    "inputTokens": 123456,
    "outputTokens": 23456,
    "cacheCreationTokens": 1234,
    "cacheReadTokens": 9876,
    "totalCost": 0.12
  }
}
```

### Filtering Sessions

Filter sessions by their last activity date:

```bash
# Sessions active today
ccusage pi session --since 2026-05-16 --until 2026-05-16

# Sessions from the past week
ccusage pi session --since 2026-05-09
```

## Related

- [ccusage](https://github.com/ccusage/ccusage) - Main usage analysis tool for coding (agent) CLIs
- [pi-agent](https://github.com/badlogic/pi-mono) - Alternative Claude coding (agent) CLI
