# Gemini CLI Data Source (Beta)

> Gemini CLI support is experimental while the Gemini CLI log format continues to evolve.

ccusage can read Gemini CLI chat logs as one of its supported local data sources. Gemini uses the same unified and focused report model as Claude Code, Codex, OpenCode, Amp, pi-agent, and GitHub Copilot CLI.

## Focused Views

```bash
# Daily Gemini CLI usage
ccusage gemini daily

# Monthly Gemini CLI usage
ccusage gemini monthly

# Gemini CLI sessions
ccusage gemini session
```

Most users can start with unified reports such as `ccusage daily`. Add the `gemini` namespace only when you want to focus the same report shape on Gemini CLI usage.

## Data Source

The CLI reads Gemini CLI JSON and JSONL files located under `GEMINI_DATA_DIR` (defaults to `~/.gemini/tmp`). `GEMINI_DATA_DIR` can be one directory or a comma-separated list of directories. Antigravity SQLite conversations are a separate source; see [Antigravity](/guide/antigravity/).

```bash
GEMINI_DATA_DIR="$HOME/.gemini/tmp,/backup/gemini/tmp" ccusage gemini daily
```

```text
~/.gemini/tmp/
└── */
    └── chats/
        ├── *.json
        └── *.jsonl
```

## Report Views

| Focused view             | Description                       | See also                                |
| ------------------------ | --------------------------------- | --------------------------------------- |
| `ccusage gemini daily`   | Aggregate usage by date           | [Daily Usage](/guide/daily-reports)     |
| `ccusage gemini monthly` | Aggregate usage by month          | [Monthly Usage](/guide/monthly-reports) |
| `ccusage gemini session` | Group usage by Gemini CLI session | [Session Usage](/guide/session-reports) |

These views support `--json`, `--compact`, and `--offline`.

## What Gets Calculated

- **Token usage** - Gemini token records provide input, output, cached, thought, tool, and total token counts when available.
- **Cache tokens** - Gemini `input` values can include cached prompt tokens. ccusage separates those cached tokens into cache read so input and cache read are not double-counted.
- **Reasoning tokens** - Gemini `thoughts` tokens are included in total tokens and exposed as `reasoningTokens` in JSON output. They are priced with output tokens.
- **Pricing** - Costs are calculated from LiteLLM pricing data for Gemini, Google, Vertex AI, and OpenRouter Gemini model names.

## Environment Variables

| Variable          | Description                                                                              |
| ----------------- | ---------------------------------------------------------------------------------------- |
| `GEMINI_DATA_DIR` | Override the root directory, or comma-separated root directories, containing Gemini data |
| `LOG_LEVEL`       | Adjust verbosity (0 silent ... 5 trace)                                                  |

## Troubleshooting

::: details No Gemini CLI usage data found
Ensure Gemini CLI has written chat logs under `~/.gemini/tmp/*/chats/`. Set `GEMINI_DATA_DIR` if your Gemini data lives elsewhere or in multiple archive roots.
:::

::: details Costs showing as $0.00
If a model is not in LiteLLM's database, the cost will be $0.00. Use `--offline=false` to fetch the latest pricing data, or [open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
