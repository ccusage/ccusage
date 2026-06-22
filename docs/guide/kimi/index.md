# Kimi Data Source (Experimental)

> Kimi support is experimental. Expect breaking changes while both ccusage and [Kimi CLI](https://github.com/MoonshotAI/kimi-cli) continue to evolve.

ccusage can read Kimi CLI wire logs as one of its supported local data sources. Kimi uses the same unified and focused report model as Claude Code, Codex, OpenCode, Amp, pi-agent, GitHub Copilot CLI, and Gemini CLI.

## Usage

```sh
# Daily Kimi usage
ccusage kimi daily

# Monthly Kimi usage
ccusage kimi monthly

# Kimi sessions
ccusage kimi session

# Include Kimi in the default all-source report
ccusage daily
```

## Data Location

The CLI reads Kimi wire JSONL files from `KIMI_DATA_DIR` (defaults to `~/.kimi` and `~/.kimi-code`). `KIMI_DATA_DIR` can be one directory or a comma-separated list of directories.

```sh
KIMI_DATA_DIR="$HOME/.kimi,/backup/kimi" ccusage kimi daily
```

Expected files are discovered under:

```text
~/.kimi/sessions/<group-id>/<session-id>/wire.jsonl
~/.kimi-code/sessions/<workspace-id>/<session-id>/agents/<agent-id>/wire.jsonl
```

## Supported Reports

| Command                | Description                 | Related Report                          |
| ---------------------- | --------------------------- | --------------------------------------- |
| `ccusage kimi daily`   | Group usage by day          | [Daily Usage](/guide/daily-reports)     |
| `ccusage kimi monthly` | Group usage by month        | [Monthly Usage](/guide/monthly-reports) |
| `ccusage kimi session` | Group usage by Kimi session | [Session Usage](/guide/session-reports) |

## Token Mapping

- **Input tokens** - `token_usage.input_other` (or `usage.inputOther` in Kimi Code logs)
- **Output tokens** - `token_usage.output` (or `usage.output`)
- **Cache read tokens** - `token_usage.input_cache_read` (or `usage.inputCacheRead`)
- **Cache creation tokens** - `token_usage.input_cache_creation` (or `usage.inputCacheCreation`)

Only `StatusUpdate` messages with non-zero token usage are included. Kimi Code logs instead emit `usage.record` lines; only turn-scoped records are counted, since session-scoped records are cumulative totals.

## Cost Calculation

Kimi rows do not store recorded USD cost, so ccusage estimates cost from token counts and LiteLLM pricing. The default `kimi-for-coding` model is kept as the displayed model name, but pricing resolves to Moonshot K2.5 before `2026-04-20T15:28:10.072Z` and Moonshot K2.6 at or after that timestamp.

## Environment Variables

| Variable        | Description                                                                            |
| --------------- | -------------------------------------------------------------------------------------- |
| `KIMI_DATA_DIR` | Override the root directory, or comma-separated root directories, containing Kimi data |

## Troubleshooting

::: details No Kimi usage data found
Ensure the data directory exists at `~/.kimi/sessions/` or `~/.kimi-code/sessions/`. Set `KIMI_DATA_DIR` if your Kimi data lives elsewhere or in multiple archive roots.
:::
