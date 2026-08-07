# Hermes Agent Data Source (Experimental)

> Hermes Agent support is experimental. Expect changes while both ccusage and [Hermes Agent](https://github.com/NousResearch/hermes-agent) continue to evolve.

ccusage can read Hermes Agent session usage from its local SQLite state databases. The adapter uses the same focused and unified report shape as the other local coding CLI data sources.

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

The CLI reads Hermes Agent session rows from its local SQLite state databases. When `HERMES_HOME` is not set, ccusage checks both the default profile at `~/.hermes/state.db` and every named profile at `~/.hermes/profiles/*/state.db`.

```bash
HERMES_HOME="$HOME/.hermes" ccusage hermes daily
```

```text
~/.hermes/
├── state.db
└── profiles/
    ├── personal/state.db
    └── work/state.db
```

## Report Views

| Focused view             | Description                      | See also                                |
| ------------------------ | -------------------------------- | --------------------------------------- |
| `ccusage hermes daily`   | Aggregate usage by date          | [Daily Usage](/guide/daily-reports)     |
| `ccusage hermes monthly` | Aggregate usage by month         | [Monthly Usage](/guide/monthly-reports) |
| `ccusage hermes session` | Group usage by Hermes session ID | [Session Usage](/guide/session-reports) |

These views support `--json`, `--compact`, `--offline`, `--since`, `--until`, and `--timezone`.

## What Gets Calculated

- **Token usage** - Reads input, output, cache read, cache write, and reasoning token totals from Hermes session rows.
- **Total tokens** - Includes reasoning tokens in the total token count.
- **Costs** - Prefers recorded actual cost, then recorded estimated cost, then calculates from LiteLLM pricing when token data is available.
- **Message count** - Preserves Hermes `message_count` in JSON metadata.

## Environment Variables

| Variable      | Description                                                                                                             |
| ------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `HERMES_HOME` | Override the directories containing `state.db`; comma-separated roots are supported, and only the listed roots are read |
| `LOG_LEVEL`   | Adjust verbosity (0 silent ... 5 trace)                                                                                 |

## Troubleshooting

::: details No Hermes Agent usage data found
Ensure a database exists at `$HERMES_HOME/state.db`, `~/.hermes/state.db`, or `~/.hermes/profiles/<name>/state.db`. If your database lives elsewhere, set `HERMES_HOME` to the directory that contains `state.db`.
:::

::: details Costs showing as $0.00
If Hermes has no recorded cost and a model is not in LiteLLM or ccusage's bundled fallback pricing, the calculated cost will be $0.00.
:::
