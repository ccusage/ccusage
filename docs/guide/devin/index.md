# Devin Data Source

ccusage can read Devin CLI ATIF trajectory transcripts as one of its supported local data sources. Devin records per-step token usage, model selection, and committed credit cost in transcript JSON files.

## What is Devin?

Devin is an AI software engineer CLI from Cognition. Devin writes ATIF (Agent Trajectory Interchange Format) trajectory files after every turn, which include per-step token usage and cost data. ccusage reads these ATIF transcripts and aggregates them alongside its other supported sources.

## Focused Views

```bash
# Recommended
bunx ccusage devin --help

# Alternative package runners
npx ccusage@latest devin --help
pnpm dlx ccusage devin --help
```

## Data Source

Devin writes an ATIF transcript after every turn to its CLI data directory. The CLI scans these directories for Devin transcript files:

| Source | Default paths                                                | Override                        |
| ------ | ------------------------------------------------------------ | -------------------------------- |
| Devin  | `~/.local/share/devin/cli/transcripts/` (Linux/macOS)         | `DEVIN_DATA_DIR` or `--devin-path` |
| Devin  | `%APPDATA%\devin\cli\transcripts\` (Windows)                 | `DEVIN_DATA_DIR` or `--devin-path` |

ccusage also reads `${DEVIN_DATA_DIR}/sessions.db` when it exists to enrich transcripts with the working directory, model fallback, and session timestamps. Transcript filenames are used as session IDs when a session ID is not present in the transcript itself.

Both `DEVIN_DATA_DIR` and `--devin-path` can point to one data directory or a comma-separated list of data directories.

## Report Views

```bash
# Show daily Devin usage
ccusage devin daily

# Show weekly Devin usage
ccusage devin weekly

# Show monthly Devin usage
ccusage devin monthly

# Show session-based Devin usage
ccusage devin session

# JSON output for automation
ccusage devin daily --json

# Custom Devin data directory
ccusage devin daily --devin-path /path/to/devin/cli

# Multiple Devin data directories
ccusage devin daily --devin-path /path/to/devin/cli,/archive/devin/cli

# Filter by date range
ccusage devin daily --since 2026-05-01 --until 2026-05-16
```

## Cost Calculation

Devin transcripts can include a per-step `committed_credit_cost` value (USD). When present, ccusage uses this embedded cost directly. When no embedded cost is available, ccusage falls back to calculating cost from the reported token counts and model name using the embedded LiteLLM pricing snapshot. Model names are resolved from the step metadata in this order: `step.metadata.generation_model`, `step.extra.generation_model`, `step.model_name`, `transcript.agent.model_name`, `sessions.model`.

## Environment Variables

| Variable         | Description                                                           |
| ---------------- | --------------------------------------------------------------------- |
| `DEVIN_DATA_DIR` | Custom path, or comma-separated paths, to Devin CLI data directories  |
| `LOG_LEVEL`      | Adjust logging verbosity (0 silent ... 5 trace)                      |

## Daily View

This view shows daily usage from Devin.

```bash
# Recommended (fastest)
bunx ccusage devin daily

# Using npx
npx ccusage@latest devin daily
```

### Options

| Flag            | Short | Description                                                       |
| --------------- | ----- | ----------------------------------------------------------------- |
| `--since`       |       | Start date filter (YYYY-MM-DD or YYYYMMDD)                        |
| `--until`       |       | End date filter (YYYY-MM-DD or YYYYMMDD)                          |
| `--timezone`    | `-z`  | Override timezone for date grouping                               |
| `--json`        | `-j`  | Emit structured JSON instead of a table                           |
| `--compact`     |       | Force compact table layout for narrow terminals                   |
| `--devin-path`  |       | Custom path, or comma-separated paths, to Devin data directories  |

### JSON Output

Use `--json` for automation and scripting:

```bash
ccusage devin daily --json
```

Returns structured data:

<!-- eslint-skip -->

```json
{
	"daily": [
		{
			"date": "2026-05-16",
			"inputTokens": 1860,
			"outputTokens": 95,
			"cacheCreationTokens": 500,
			"cacheReadTokens": 109928,
			"totalTokens": 112383,
			"totalCost": 0.03,
			"modelsUsed": ["claude-sonnet-4-20250514"]
		}
	],
	"totals": {
		"inputTokens": 1860,
		"outputTokens": 95,
		"cacheCreationTokens": 500,
		"cacheReadTokens": 109928,
		"totalTokens": 112383,
		"totalCost": 0.03
	}
}
```

## Weekly View

This view shows usage grouped by week from Devin.

```bash
ccusage devin weekly
ccusage devin weekly --json
ccusage devin weekly --since 2026-01-01 --until 2026-03-31
```

## Monthly View

This view shows monthly usage from Devin.

```bash
ccusage devin monthly
ccusage devin monthly --json
ccusage devin monthly --since 2026-01-01 --until 2026-03-31
```

## Session View

This view shows usage grouped by individual Devin sessions. Session IDs come from the transcript `session_id` field or from the transcript filename stem when `session_id` is absent.

```bash
ccusage devin session
ccusage devin session --json
ccusage devin session --since 2026-05-09
```

## Related

- [All sources report](/guide/all-reports) - Combine Devin with every other supported source in one view
- [Environment variables](/guide/environment-variables) - Full list of path overrides for all sources
- [Configuration files](/guide/config-files) - Persist `devinPath` and other options in `ccusage.json`
