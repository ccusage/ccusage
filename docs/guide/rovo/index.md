# Rovo Dev CLI Data Source (Experimental)

> Rovo Dev support is experimental. Expect breaking changes while both ccusage and [Rovo Dev CLI](https://www.atlassian.com/software/rovo-dev) continue to evolve.

ccusage can read Atlassian Rovo Dev CLI session files as one of its supported local data sources. Rovo Dev uses the same unified and focused report model as Claude Code, Codex, OpenCode, Amp, pi-agent, GitHub Copilot CLI, and Gemini CLI.

## Usage

```sh
# Daily Rovo Dev usage
ccusage rovo daily

# Monthly Rovo Dev usage
ccusage rovo monthly

# Rovo Dev sessions
ccusage rovo session

# Include Rovo Dev in the default all-source report
ccusage daily
```

## Data Location

The CLI reads Rovo Dev `session_context.json` files from `ROVO_DATA_DIR` (defaults to `~/.rovodev`). `ROVO_DATA_DIR` can be one directory or a comma-separated list of directories.

```sh
ROVO_DATA_DIR="$HOME/.rovodev,/backup/rovodev" ccusage rovo daily
```

Expected files are discovered under:

```text
~/.rovodev/sessions/<session-id>/session_context.json
```

When `~/.rovodev/config.yml` moves the sessions directory via `sessions.persistenceDir`, ccusage follows that setting too.

## Supported Reports

| Command                | Description                     | Related Report                          |
| ---------------------- | ------------------------------- | --------------------------------------- |
| `ccusage rovo daily`   | Group usage by day              | [Daily Usage](/guide/daily-reports)     |
| `ccusage rovo monthly` | Group usage by month            | [Monthly Usage](/guide/monthly-reports) |
| `ccusage rovo session` | Group usage by Rovo Dev session | [Session Usage](/guide/session-reports) |

## Token Mapping

Each `kind: "response"` entry in a session's `message_history` is one usage record:

- **Input tokens** - `usage.details.input_tokens` (the uncached share of the prompt)
- **Output tokens** - `usage.output_tokens`
- **Cache read tokens** - `usage.details.cache_read_input_tokens`
- **Cache creation tokens** - `usage.details.cache_creation_input_tokens`

The top-level `input_tokens` (and `request_tokens` in legacy sessions) already include cache tokens, so ccusage uses the `details` split to avoid double-counting. The cumulative session-level `usage` object is ignored for the same reason. Forked sessions copy the parent conversation's responses into a new session file; ccusage dedupes responses by provider response id — or, for legacy records without ids, by timestamp and token counts — so fork families are counted once.

## Cost Calculation

Rovo Dev bills Atlassian Rovo Dev credits and stores no cost in its local files, so ccusage estimates the equivalent API cost from token counts and LiteLLM pricing for the recorded model (for example `claude-sonnet-4-5-20250929`). The estimate is not what you pay Atlassian. Legacy sessions (CLI 0.6.x) store no model name; those rows display the `unknown` model and report a missing-pricing warning instead of a cost.

## Environment Variables

| Variable        | Description                                                                                |
| --------------- | ------------------------------------------------------------------------------------------ |
| `ROVO_DATA_DIR` | Override the root directory, or comma-separated root directories, containing Rovo Dev data |

## Troubleshooting

::: details No Rovo Dev usage data found
Ensure the data directory exists at `~/.rovodev/sessions/`. Set `ROVO_DATA_DIR` if your Rovo Dev data lives elsewhere or in multiple archive roots, and check `sessions.persistenceDir` in `~/.rovodev/config.yml` if sessions were relocated.
:::
