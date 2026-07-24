# Antigravity Data Source (Beta)

ccusage can read [Google Antigravity](https://antigravity.google) usage data as one of its supported local data sources. Antigravity is Google's agentic IDE and CLI (Windsurf-derived), and it stores usage data in local SQLite conversation databases.

## What is Antigravity?

Antigravity is Google's agentic coding IDE and CLI. Each conversation is stored as a SQLite database with per-generation token counts (input, cache read, output, and thinking tokens), model identifiers, and timestamps. ccusage decodes these records offline — no credentials or live services are read.

## Focused Views

```bash
# Recommended
bunx ccusage antigravity --help

# Alternative package runners
npx ccusage@latest antigravity --help
pnpm dlx ccusage antigravity --help
pnpx ccusage antigravity --help
```

## Data Source

The CLI reads conversation databases from Antigravity:

| Source      | Default path                                                | Override               |
| ----------- | ----------------------------------------------------------- | ---------------------- |
| Antigravity | `~/.gemini/antigravity*/conversations/<conversation-id>.db` | `ANTIGRAVITY_DATA_DIR` |

The default roots are `~/.gemini/antigravity`, `~/.gemini/antigravity-cli`, `~/.gemini/antigravity-ide`, and `~/.gemini/antigravity-backup`; missing roots are skipped. `ANTIGRAVITY_DATA_DIR` accepts one root or a comma-separated list of roots.

Legacy `.pb` conversation files are compressed/encrypted and unsupported, and `*.db-wal`/`*.db-shm` sidecars are ignored.

## Report Views

```bash
# Show daily Antigravity usage
ccusage antigravity daily

# Show monthly Antigravity usage
ccusage antigravity monthly

# Show session-based Antigravity usage
ccusage antigravity session

# JSON output for automation
ccusage antigravity daily --json

# Filter by date range
ccusage antigravity daily --since 2026-05-01 --until 2026-05-16

# Show model breakdown
ccusage antigravity daily --breakdown
```

## Environment Variables

| Variable               | Description                                                      |
| ---------------------- | ---------------------------------------------------------------- |
| `ANTIGRAVITY_DATA_DIR` | Custom path, or comma-separated paths, to Antigravity data roots |
| `LOG_LEVEL`            | Adjust logging verbosity (0 silent … 5 trace)                    |

## Daily View

This view shows daily usage from Antigravity.

```bash
# Recommended (fastest)
bunx ccusage antigravity daily

# Using npx
npx ccusage@latest antigravity daily
```

### Options

| Flag          | Short | Description                                      |
| ------------- | ----- | ------------------------------------------------ |
| `--since`     |       | Start date filter (YYYY-MM-DD or YYYYMMDD)       |
| `--until`     |       | End date filter (YYYY-MM-DD or YYYYMMDD)         |
| `--timezone`  | `-z`  | Override timezone for date grouping              |
| `--json`      |       | Emit structured JSON instead of a table          |
| `--breakdown` | `-b`  | Show per-model token breakdown                   |
| `--order`     |       | Sort order: `asc` or `desc` (default: `desc`)    |

### JSON Output

Use `--json` for automation and scripting:

```bash
ccusage antigravity daily --json
```

Returns structured data:

<!-- eslint-skip -->

```json
{
  "daily": [
    {
      "date": "2026-05-16",
      "inputTokens": 567890,
      "outputTokens": 123456,
      "cacheCreationTokens": 0,
      "cacheReadTokens": 45678,
      "totalCost": 0.89,
      "modelsUsed": ["gemini-3.1-pro"],
      "modelBreakdowns": [...]
    }
  ],
  "totals": {
    "inputTokens": 567890,
    "outputTokens": 123456,
    "cacheCreationTokens": 0,
    "cacheReadTokens": 45678,
    "totalCost": 0.89
  }
}
```

## Monthly View

This view shows monthly usage from Antigravity.

```bash
# Recommended (fastest)
bunx ccusage antigravity monthly

# Using npx
npx ccusage@latest antigravity monthly
```

## Session View

This view shows usage grouped by individual Antigravity conversations.

```bash
# Recommended (fastest)
bunx ccusage antigravity session

# Using npx
npx ccusage@latest antigravity session
```

### Session Identification

Sessions are identified by the conversation database name (`<conversation-id>.db`). The project name comes from the workspace URI recorded in the conversation metadata.

## Token Semantics

- Input tokens include Antigravity's fixed system-prompt component plus fresh (non-cached) input.
- Cache-write tokens are always zero; cache-read tokens are reported separately.
- Thinking (reasoning) tokens are excluded from the displayed output column but included in total tokens and billed as output for cost calculation.
- Antigravity records no cost; costs are estimated from LiteLLM model pricing. Internal model ids (for example `MODEL_PLACEHOLDER_M16`) are mapped to priced names such as `gemini-3.1-pro`.

## Related

- [ccusage](https://github.com/ccusage/ccusage) - Main usage analysis tool for coding (agent) CLIs
- [Antigravity](https://antigravity.google) - Google's agentic IDE and CLI
