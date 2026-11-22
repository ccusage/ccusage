# Droid Data Source (Experimental)

> Droid support is experimental. Expect breaking changes while both ccusage and Droid continue to evolve.

ccusage can read local Droid session settings files as one of its supported data sources, using the same daily, weekly, monthly, and session report views as the rest of ccusage.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage droid --help
```

```bash [npx]
npx ccusage@latest droid --help
```

```bash [pnpm]
pnpm dlx ccusage droid --help
```

:::

## Data Source

The CLI reads Droid settings JSON files from `DROID_SESSIONS_DIR` (defaults to `~/.factory/sessions`). `DROID_SESSIONS_DIR` can be one directory or a comma-separated list of directories.

```bash
DROID_SESSIONS_DIR="$HOME/.factory/sessions,/backup/factory/sessions" ccusage droid session
```

```text
~/.factory/sessions/
└── **/*.settings.json
```

## Report Views

| Focused view            | Description                  | See also                                |
| ----------------------- | ---------------------------- | --------------------------------------- |
| `ccusage droid daily`   | Aggregate usage by date      | [Daily Usage](/guide/daily-reports)     |
| `ccusage droid weekly`  | Aggregate usage by week      | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage droid monthly` | Aggregate usage by month     | [Monthly Usage](/guide/monthly-reports) |
| `ccusage droid session` | Group usage by Droid session | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output, `--compact` for narrow terminals, and `--offline` for cached pricing data.

## What Gets Calculated

- **Token usage** - Droid settings files provide input, output, cache creation, cache read, and thinking token counts.
- **Reasoning tokens** - Thinking tokens are included in total tokens and output-side cost estimation.
- **Pricing** - Auto and Calculate price tokens for the recorded model and provider. The logs have no recorded USD cost, so Display reports $0 while retaining tokens. See [Cost Modes](/guide/cost-modes).

## Environment Variables

| Variable             | Description                                                                                                   |
| -------------------- | ------------------------------------------------------------------------------------------------------------- |
| `DROID_SESSIONS_DIR` | Override the sessions directory, or comma-separated sessions directories, containing Droid data               |
| `LOG_LEVEL`          | `0` hides aggregated warnings, progress, and table titles ([details](/guide/environment-variables#log-level)) |

## Troubleshooting

::: details No Droid usage data found
Ensure the data directory exists at `~/.factory/sessions/` and contains `*.settings.json` files. Set `DROID_SESSIONS_DIR` if your Droid data lives elsewhere or in multiple archive roots.
:::

::: details Costs showing as $0.00
If a model is not in LiteLLM's database, the cost will be $0.00. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
