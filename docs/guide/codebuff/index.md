# Codebuff Data Source (Experimental)

> Codebuff support is experimental. Expect adjustments while the local data format continues to evolve.

ccusage can read Codebuff chat history files as one of its supported local data sources, using the same reporting experience as the rest of ccusage: responsive tables, JSON output, LiteLLM-based pricing, cache token accounting, and credit totals where Codebuff records them.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage codebuff --help
```

```bash [npx]
npx ccusage@latest codebuff --help
```

```bash [pnpm]
pnpm dlx ccusage codebuff --help
```

:::

## Data Source

The CLI reads Codebuff chat JSON files from `CODEBUFF_DATA_DIR`. Set `CODEBUFF_DATA_DIR` to one channel root or a comma-separated list of channel roots when your data lives outside the standard locations.

When `CODEBUFF_DATA_DIR` is not set, ccusage checks the stable, dev, and staging channel roots:

```text
~/.config/manicode/
~/.config/manicode-dev/
~/.config/manicode-staging/
```

Each root is expected to contain project chat histories:

```text
~/.config/manicode/
└── projects/
    └── <project>/
        └── chats/
            └── <chat-id>/
                └── chat-messages.json
```

Use a custom root when your Codebuff data lives elsewhere:

```bash
CODEBUFF_DATA_DIR="$HOME/.config/manicode,/backup/codebuff" ccusage codebuff session
```

## Report Views

| Focused view               | Description                          | See also                                |
| -------------------------- | ------------------------------------ | --------------------------------------- |
| `ccusage codebuff daily`   | Aggregate usage by date              | [Daily Usage](/guide/daily-reports)     |
| `ccusage codebuff weekly`  | Aggregate usage by week              | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage codebuff monthly` | Aggregate usage by month             | [Monthly Usage](/guide/monthly-reports) |
| `ccusage codebuff session` | Group usage by Codebuff chat session | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output, `--compact` for narrow terminals, and `--offline` for cached pricing data.

## What Gets Calculated

- **Token usage** - Assistant messages can provide input and output token counts from `metadata.usage`, `metadata.codebuff.usage`, or nested run-state provider usage. ccusage uses the first positive output count in that order. With no positive count, a recorded zero takes precedence over the total-based display fallback.
- **Cache tokens** - Cache creation and cache read tokens are included when the Codebuff message metadata records them.
- **Credits** - Codebuff credit values are summed in row metadata and included in per-agent JSON output.
- **Pricing** - Auto and Calculate estimate USD costs from model pricing. Codebuff records credits rather than USD costs, so Display reports $0; credit and token totals remain available. See [Cost Modes](/guide/cost-modes).

## Environment Variables

| Variable            | Description                                                                                                   |
| ------------------- | ------------------------------------------------------------------------------------------------------------- |
| `CODEBUFF_DATA_DIR` | Override the channel root, or comma-separated channel roots, containing Codebuff projects                     |
| `LOG_LEVEL`         | `0` hides aggregated warnings, progress, and table titles ([details](/guide/environment-variables#log-level)) |

## Troubleshooting

::: details No Codebuff usage data found
Ensure the data directory contains `projects/<project>/chats/<chat-id>/chat-messages.json`. Set `CODEBUFF_DATA_DIR` if your Codebuff data lives outside `~/.config/manicode`, `~/.config/manicode-dev`, or `~/.config/manicode-staging`.
:::

::: details Costs showing as $0.00
If a model is not in LiteLLM's database, the cost will be $0.00. Costs can also be $0.00 in offline mode when pricing data is unavailable. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
