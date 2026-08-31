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

Both `OPENCLAW_DIR` and `--open-claw-path` can be one root directory or a comma-separated list of root directories.

## Report Views

```bash
# Show daily OpenClaw usage
ccusage openclaw daily

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

OpenClaw session messages embed a `cost.total` value per assistant message. In the default mode, ccusage uses these embedded costs directly, so reports work offline without `--offline`. With `--mode calculate`, ccusage resolves the raw model identity through the merged LiteLLM and models.dev pricing data; this includes timestamp-aware DeepSeek V4 rates when applicable.

## Model Attribution

OpenClaw sessions emit `model_change` and `custom`/`model-snapshot` events that set the active provider and model for subsequent assistant messages. ccusage tracks this state per file and tags every model name with an `[openclaw]` prefix in the `modelsUsed` column to keep it distinguishable in the unified `ccusage daily` view.

## Environment Variables

| Variable       | Description                                                         |
| -------------- | ------------------------------------------------------------------- |
| `OPENCLAW_DIR` | Custom path, or comma-separated paths, to OpenClaw root directories |
| `LOG_LEVEL`    | Adjust logging verbosity (0 silent … 5 trace)                       |

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

```text
┌────────────┬──────────────────────┬───────────┬───────────┬──────────────┬────────────┬──────────────┬──────────────┐
│ Date       │ Models               │     Input │    Output │ Cache Create │ Cache Read │ Total Tokens │   Cost (USD) │
├────────────┼──────────────────────┼───────────┼───────────┼──────────────┼────────────┼──────────────┼──────────────┤
│ 2026-05-16 │ - [openclaw] gpt-5.2 │     1,860 │        95 │          500 │    109,928 │      112,383 │        $0.03 │
├────────────┼──────────────────────┼───────────┼───────────┼──────────────┼────────────┼──────────────┼──────────────┤
│ Total      │                      │     1,860 │        95 │          500 │    109,928 │      112,383 │        $0.03 │
└────────────┴──────────────────────┴───────────┴───────────┴──────────────┴────────────┴──────────────┴──────────────┘
```

### JSON Output

Use `--json` for automation and scripting:

```bash
ccusage openclaw daily --json
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
			"modelsUsed": ["[openclaw] gpt-5.2"]
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

```text
┌────────────┬──────────────────────┬───────────┬───────────┬──────────────┬────────────┬──────────────┬──────────────┐
│ Session    │ Models               │     Input │    Output │ Cache Create │ Cache Read │ Total Tokens │   Cost (USD) │
├────────────┼──────────────────────┼───────────┼───────────┼──────────────┼────────────┼──────────────┼──────────────┤
│ abc        │ - [openclaw] gpt-5.2 │     1,860 │        95 │          500 │    109,928 │      112,383 │        $0.03 │
├────────────┼──────────────────────┼───────────┼───────────┼──────────────┼────────────┼──────────────┼──────────────┤
│ Total      │                      │     1,860 │        95 │          500 │    109,928 │      112,383 │        $0.03 │
└────────────┴──────────────────────┴───────────┴───────────┴──────────────┴────────────┴──────────────┴──────────────┘
```

## Related

- [ccusage](https://github.com/ccusage/ccusage) - Main usage analysis tool for coding (agent) CLIs
