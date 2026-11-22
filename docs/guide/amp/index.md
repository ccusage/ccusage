# Amp Data Source (Beta)

> Amp support is experimental. Expect breaking changes while both ccusage and [Amp](https://ampcode.com/) continue to evolve.

ccusage can read Amp thread files as one of its supported local data sources, using the same reporting experience as the rest of ccusage: responsive tables, JSON output, LiteLLM-based pricing, cache token accounting, and credit totals where Amp records them.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage amp --help
```

```bash [npx]
npx ccusage@latest amp --help
```

```bash [pnpm]
pnpm dlx ccusage amp --help
```

:::

## Data Source

The CLI reads Amp thread JSON files from `AMP_DATA_DIR` (defaults to `~/.local/share/amp`). `AMP_DATA_DIR` can be one directory or a comma-separated list of directories.

```bash
AMP_DATA_DIR="$HOME/.local/share/amp,/backup/amp" ccusage amp session
```

```text
~/.local/share/amp/
└── threads/
    └── **/*.json
```

## Report Views

| Focused view          | Description               | See also                                |
| --------------------- | ------------------------- | --------------------------------------- |
| `ccusage amp daily`   | Aggregate usage by date   | [Daily Usage](/guide/daily-reports)     |
| `ccusage amp weekly`  | Aggregate usage by week   | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage amp monthly` | Aggregate usage by month  | [Monthly Usage](/guide/monthly-reports) |
| `ccusage amp session` | Group usage by Amp thread | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output, `--compact` for narrow terminals, and `--offline` for cached pricing data.

## What Gets Calculated

- **Token usage** - Amp usage ledger events provide input and output token counts.
- **Cache tokens** - Assistant message usage fields provide cache creation and cache read tokens when available.
- **Credits** - Amp credit values are summed alongside token and cost totals.
- **Pricing** - Costs are calculated from LiteLLM pricing data for Claude and Anthropic model names, including provider-prefixed variants.

## Environment Variables

| Variable       | Description                                                                                                   |
| -------------- | ------------------------------------------------------------------------------------------------------------- |
| `AMP_DATA_DIR` | Override the root directory, or comma-separated root directories, containing Amp data                         |
| `LOG_LEVEL`    | `0` hides aggregated warnings, progress, and table titles ([details](/guide/environment-variables#log-level)) |

## Troubleshooting

::: details No Amp usage data found
Ensure the data directory exists at `~/.local/share/amp/threads/`. Set `AMP_DATA_DIR` if your Amp data lives elsewhere or in multiple archive roots.
:::

::: details Costs showing as $0.00
If a model is not in LiteLLM's database, the cost will be $0.00. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
