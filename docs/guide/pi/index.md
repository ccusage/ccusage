# Pi-Agent and Oh My Pi Data Sources

ccusage can read [pi-agent](https://github.com/badlogic/pi-mono) usage data as one of its supported local data sources. pi-agent is an alternative Claude coding (agent) CLI from [shittycodingagent.ai](https://shittycodingagent.ai).

ccusage also discovers Oh My Pi sessions automatically for unified reports. They appear as agent `omp`, with raw model names prefixed by `[omp] ` (for example, `[omp] claude-opus-4-1`). There is no separate `ccusage omp` command; `ccusage pi ...` remains focused on the default pi-agent store.

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

The focused `ccusage pi ...` command reads pi-agent data. Unified reports also discover Oh My Pi sessions:

| Source                     | Default path             | Override                                 |
| -------------------------- | ------------------------ | ---------------------------------------- |
| pi-agent                   | `~/.pi/agent/sessions/`  | `PI_AGENT_DIR` or `--pi-path`            |
| Oh My Pi (unified reports) | `~/.omp/agent/sessions/` | `PI_CONFIG_DIR` or `PI_CODING_AGENT_DIR` |

Both `PI_AGENT_DIR` and `--pi-path` can be one sessions directory or a comma-separated list of sessions directories.

Oh My Pi discovery uses the default config root `~/.omp`. `PI_CONFIG_DIR` defaults to `.omp` and is joined under `$HOME`; a leading slash in its value does not make it absolute. Unified reports check the existing `agent/sessions/` directory under that config root. `PI_CODING_AGENT_DIR` adds another root, with `sessions/` appended; relative values resolve from the current working directory.

Unified reports also check existing profile session roots under `<config-root>/profiles/<name>/agent/sessions/`. On Linux and macOS, if `$XDG_DATA_HOME/omp` exists, ccusage also checks its `sessions/` directory and existing `profiles/<name>/sessions/` directories as migrated OMP data. These default and XDG roots are still checked when `PI_CODING_AGENT_DIR` is set. Profile names found under either root are included whether or not that profile is currently active.

Oh My Pi is included automatically when ccusage finds its session directories. You do not need to configure a named store for the default location. Existing `pi.stores[]` entries remain supported; an explicit `omp` entry is useful for compatibility with existing setups or to select an alternate OMP root:

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

An explicit `pi.stores[]` entry named `omp` suppresses automatic OMP discovery and uses the configured path instead. Other explicit stores and the default Pi source take precedence when their paths overlap the automatically discovered OMP path, preventing duplicate usage. Named store paths can be comma-separated lists of sessions directories; missing paths are treated as empty, while paths configured to overlap the default `pi` store or another explicit named store — including nested paths — are rejected. Explicit stores are loaded in unified reports (`ccusage daily`, `monthly`, `weekly`, or `session`) alongside the default `pi` agent.

### OMP Session Format and Scope

ccusage reads Oh My Pi's local JSONL session files. Supported format details include the fixed title slot that can precede the session header, billable top-level `model_usage` entries, the `activeLeafId` branch pointer, and `parentSession` references stored as opaque IDs or paths. When a session records both provider and model, ccusage uses the pair for provider-specific pricing when a matching price is available. See the [Oh My Pi session format reference](https://github.com/can1357/oh-my-pi/blob/e0996aad2cf487c82a7ba00f46172600fde48528/docs/session.md).

The `tokensBefore` value in a `compaction` entry describes context before compaction; it is metadata, not billable usage, and is not added to report totals. ccusage reads local `.jsonl` files only. SQLite or Redis-backed session storage and compressed `.jsonl.gz` archives are outside the supported input format.

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
