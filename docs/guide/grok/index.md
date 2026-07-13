# Grok Build CLI Data Source

ccusage can read Grok Build CLI session usage as one of its supported local data sources. Grok writes per-turn token accounting on `turn_completed` events in session `updates.jsonl` files under `~/.grok/sessions`.

## What is Grok Build CLI?

Grok Build is xAI's coding agent CLI. Sessions live under `~/.grok` and record usage on each completed turn, including input, output, cache-read, and reasoning tokens per model.

## Focused Views

```bash
# Recommended
bunx ccusage grok --help

# Alternative package runners
npx ccusage@latest grok --help
pnpm dlx ccusage grok --help
pnpx ccusage grok --help
```

## Data Source

The CLI scans these directories for Grok session files:

| Source | Default paths | Override |
| ------ | ------------- | -------- |
| Grok Build CLI | `~/.grok/sessions/**/updates.jsonl` | `GROK_HOME` or `--grok-path` |

ccusage walks each Grok home root for `sessions/**/updates.jsonl` and optionally reads a sibling `summary.json` for project path metadata (`info.cwd` / `git_root_dir`).

Both `GROK_HOME` and `--grok-path` can be one root directory or a comma-separated list of root directories.

## Report Views

```bash
# Show daily Grok usage
ccusage grok daily

# Show monthly Grok usage
ccusage grok monthly

# Show session-based Grok usage
ccusage grok session

# JSON output for automation
ccusage grok daily --json

# Custom Grok home
ccusage grok daily --grok-path /path/to/.grok

# Multiple Grok homes
ccusage grok daily --grok-path /path/to/.grok,/archive/.grok

# Filter by date range
ccusage grok daily --since 2026-07-01 --until 2026-07-13
```

## Cost Calculation

Grok does not embed USD costs in session files. ccusage calculates cost from token counts and model pricing:

- Displayed **output** tokens stay as Grok's `outputTokens`.
- **Reasoning** tokens (`reasoningTokens`) are included in total token accounting via `extra_total_tokens` and are billed at the **output** unit price (same pattern as Goose).
- Embedded fixed rates exist for active models such as `grok-4.5` and `grok-composer-2.5-fast`. Unknown models warn as missing pricing rather than inventing rates.

## Model Attribution

Models come from each turn's `usage.modelUsage` map. Multi-model turns emit one row per model. Display names use a `[grok]` prefix so Grok rows stay distinct in unified `ccusage daily` reports.

## Environment Variables

| Variable | Description |
| -------- | ----------- |
| `GROK_HOME` | Custom path, or comma-separated paths, to Grok home directories (default: `~/.grok`) |
| `LOG_LEVEL` | Adjust logging verbosity (0 silent … 5 trace) |

## Daily View

```bash
# Recommended (fastest)
bunx ccusage grok daily

# Using npx
npx ccusage@latest grok daily
```

### Options

| Flag | Short | Description |
| ---- | ----- | ----------- |
| `--since` | | Start date filter (YYYY-MM-DD or YYYYMMDD) |
| `--until` | | End date filter (YYYY-MM-DD or YYYYMMDD) |
| `--timezone` | `-z` | Override timezone for date grouping |
| `--json` | `-j` | Emit structured JSON instead of a table |
| `--compact` | | Force compact table layout for narrow terminals |
| `--grok-path` | | Custom path, or comma-separated paths, to Grok home directories |
| `--breakdown` | `-b` | Show per-model breakdown |

### Example Output

```text
┌────────────┬──────────────────────────┬───────────┬───────────┬──────────────┬────────────┬──────────────┬──────────────┐
│ Date       │ Models                   │     Input │    Output │ Cache Create │ Cache Read │ Total Tokens │   Cost (USD) │
├────────────┼──────────────────────────┼───────────┼───────────┼──────────────┼────────────┼──────────────┼──────────────┤
│ 2026-07-13 │ - [grok] grok-4.5        │    70,378 │     8,294 │            0 │     38,272 │      124,015 │       $0.24 │
├────────────┼──────────────────────────┼───────────┼───────────┼──────────────┼────────────┼──────────────┼──────────────┤
│ Total      │                          │    70,378 │     8,294 │            0 │     38,272 │      124,015 │       $0.24 │
└────────────┴──────────────────────────┴───────────┴───────────┴──────────────┴────────────┴──────────────┴──────────────┘
```

### JSON Output

```bash
ccusage grok daily --json
```

Returns structured data:

<!-- eslint-skip -->

```json
{
	"daily": [
		{
			"date": "2026-07-13",
			"inputTokens": 70378,
			"outputTokens": 8294,
			"cacheCreationTokens": 0,
			"cacheReadTokens": 38272,
			"totalTokens": 124015,
			"totalCost": 0.24,
			"modelsUsed": ["[grok] grok-4.5"]
		}
	],
	"totals": {
		"inputTokens": 70378,
		"outputTokens": 8294,
		"cacheCreationTokens": 0,
		"cacheReadTokens": 38272,
		"totalTokens": 124015,
		"totalCost": 0.24
	}
}
```

## Monthly View

```bash
ccusage grok monthly
ccusage grok monthly --json
ccusage grok monthly --since 2026-01-01 --until 2026-07-31
```

## Session View

Session IDs are the Grok session directory UUID (parent of `updates.jsonl`).

```bash
ccusage grok session
ccusage grok session --json
ccusage grok session --since 2026-07-09
```

## Limitations

- Local files only — ccusage does not scrape xAI cloud billing.
- Only `turn_completed` events with billable token fields are loaded. Tool updates and meta-only `totalTokens` lines are ignored.
- Reasoning tokens are billed at the output rate for cost estimates; the output column still shows raw output tokens.
- Nested `subagents/` directories currently hold metadata pointers only; child usage lives in sibling session directories and is loaded separately (see adapter README).

## Related

- [ccusage](https://github.com/ccusage/ccusage) - Main usage analysis tool for coding (agent) CLIs
- [Source Support Q&A](/guide/source-support-qa) - Support criteria for other CLIs
