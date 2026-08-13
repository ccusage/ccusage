# Antigravity Data Source (Beta)

> Antigravity support covers Antigravity CLI (`agy`), IDE, and Desktop environments.

ccusage reads per-conversation SQLite databases written by Antigravity as one of its supported local data sources. Antigravity uses the same unified and focused report model as Claude Code, Codex, OpenCode, Amp, pi-agent, and Gemini CLI.

## Focused Views

```bash
# Daily Antigravity usage
ccusage antigravity daily

# Monthly Antigravity usage
ccusage antigravity monthly

# Antigravity sessions
ccusage antigravity session
```

Most users can start with unified reports such as `ccusage daily`. Add the `antigravity` namespace only when you want to focus the same report shape on Antigravity usage.

## Data Source

The CLI reads Antigravity conversation SQLite databases located under `ANTIGRAVITY_DATA_DIR` (defaults to `~/.gemini/antigravity`, `~/.gemini/antigravity-cli`, `~/.gemini/antigravity-ide`, `~/.gemini/Antigravity`, and `~/.config/Antigravity`). `ANTIGRAVITY_DATA_DIR` can be one directory or a comma-separated list of directories.

```bash
ANTIGRAVITY_DATA_DIR="$HOME/.gemini/antigravity,$HOME/.gemini/antigravity-cli" ccusage antigravity daily
```

```text
~/.gemini/antigravity/
├── conversations/
│   └── <conversation-id>.db
└── brain/
    └── <conversation-id>/
```

## Report Views

| Focused view                  | Description                             | See also                                |
| ----------------------------- | --------------------------------------- | --------------------------------------- |
| `ccusage antigravity daily`   | Aggregate usage by date                 | [Daily Usage](/guide/daily-reports)     |
| `ccusage antigravity monthly` | Aggregate usage by month                | [Monthly Usage](/guide/monthly-reports) |
| `ccusage antigravity session` | Group usage by conversation identifier  | [Session Usage](/guide/session-reports) |

These views support `--json`, `--compact`, `--last`, `--since`, `--until`, and `--offline`.

## What Gets Calculated

- **Token usage** - Decodes `steps.metadata` (`CortexStepMetadata`) and `gen_metadata.data` (`ChatModelMetadata`) protobuf payloads, tracking input, output, cache creation, cache read, and thinking/reasoning token counts.
- **Cache tokens** - Cache read tokens and input tokens are tracked and reported separately.
- **Deduplication** - Repeated retry attempts and shared subagent conversations are deduplicated by server response ID.
- **Pricing** - Costs are calculated from LiteLLM pricing data across Gemini, Anthropic, OpenAI, and DeepSeek provider models.

## Environment Variables

| Variable               | Description                                                                                  |
| ---------------------- | -------------------------------------------------------------------------------------------- |
| `ANTIGRAVITY_DATA_DIR` | Override the root directory, or comma-separated root directories, containing Antigravity data |
| `LOG_LEVEL`            | Adjust verbosity (0 silent ... 5 trace)                                                      |

## Troubleshooting

::: details No Antigravity usage data found
Ensure Antigravity has written conversation databases under `~/.gemini/antigravity*/conversations/*.db` or set `ANTIGRAVITY_DATA_DIR` to point to your data directory.
:::

::: details Costs showing as $0.00
If a model name is not yet recognized or is reported as an unnamed placeholder (`antigravity-model-<id>`), the cost will be $0.00 and reported as missing pricing. Use `--offline=false` or configure pricing overrides in `ccusage.json`.
:::
