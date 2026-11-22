# Hermes Agent Data Source (Experimental)

> Hermes Agent support is experimental. Expect changes while both ccusage and [Hermes Agent](https://github.com/NousResearch/hermes-agent) continue to evolve.

ccusage can read Hermes Agent session usage from its local SQLite state database. The adapter uses the same focused and unified report shape as the other local coding CLI data sources.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage hermes --help
```

```bash [npx]
npx ccusage@latest hermes --help
```

```bash [pnpm]
pnpm dlx ccusage hermes --help
```

:::

## Data Source

The CLI reads Hermes Agent session rows from `$HERMES_HOME/state.db`. When `HERMES_HOME` is not set, ccusage checks `~/.hermes/state.db`.

```bash
HERMES_HOME="$HOME/.hermes" ccusage hermes daily
```

```text
~/.hermes/
└── state.db
```

## Report Views

| Focused view             | Description                      | See also                                |
| ------------------------ | -------------------------------- | --------------------------------------- |
| `ccusage hermes daily`   | Aggregate usage by date          | [Daily Usage](/guide/daily-reports)     |
| `ccusage hermes weekly`  | Aggregate usage by week          | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage hermes monthly` | Aggregate usage by month         | [Monthly Usage](/guide/monthly-reports) |
| `ccusage hermes session` | Group usage by Hermes session ID | [Session Usage](/guide/session-reports) |

These views support `--json`, `--compact`, `--offline`, `--since`, `--until`, and `--timezone`.

## What Gets Calculated

- **Token usage** - Reads input, output, cache read, cache write, and reasoning token totals from Hermes session rows.
- **Reasoning tokens** - Hermes records reasoning as part of output tokens, so reasoning is neither added to the total token count nor billed again.
- **Costs** - Selects recorded actual cost when present, otherwise estimated cost. Auto uses that value when positive and otherwise prices tokens. Calculate always prices tokens. Display uses only the selected recorded cost. See [Cost Modes](/guide/cost-modes).
- **Message count** - Preserves Hermes `message_count` in JSON metadata.

## Environment Variables

| Variable      | Description                                                                                                   |
| ------------- | ------------------------------------------------------------------------------------------------------------- |
| `HERMES_HOME` | Override the directory containing `state.db`; comma-separated roots are supported                             |
| `LOG_LEVEL`   | `0` hides aggregated warnings, progress, and table titles ([details](/guide/environment-variables#log-level)) |

## Troubleshooting

::: details No Hermes Agent usage data found
Ensure the database exists at `$HERMES_HOME/state.db` or `~/.hermes/state.db`. If your database lives elsewhere, set `HERMES_HOME` to the directory that contains `state.db`.
:::

::: details Costs showing as $0.00
If Hermes has no recorded cost and a model is not in LiteLLM or ccusage's bundled fallback pricing, the calculated cost will be $0.00.
:::
