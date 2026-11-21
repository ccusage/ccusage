# Kilo Data Source (Beta)

ccusage can read Kilo CLI usage data as one of its supported local data sources. Kilo stores local session data in a SQLite database with model, token, cache, and cost fields.

## Focused Views

```bash
# Recommended
bunx ccusage kilo --help

# Alternative package runners
npx ccusage@latest kilo --help
pnpm dlx ccusage kilo --help
pnpx ccusage kilo --help
```

## Data Source

The CLI reads Kilo messages from `KILO_DATA_DIR` (defaults to `~/.local/share/kilo`). `KILO_DATA_DIR` can be one directory or a comma-separated list of directories.

```bash
KILO_DATA_DIR="$HOME/.local/share/kilo,/backup/kilo" ccusage kilo daily
```

```text
~/.local/share/kilo/
└── kilo.db
```

## Report Views

| Focused view           | Description                          | See also                                |
| ---------------------- | ------------------------------------ | --------------------------------------- |
| `ccusage kilo daily`   | Aggregate usage by date (YYYY-MM-DD) | [Daily Usage](/guide/daily-reports)     |
| `ccusage kilo monthly` | Aggregate usage by month (YYYY-MM)   | [Monthly Usage](/guide/monthly-reports) |
| `ccusage kilo session` | Per-session breakdown                | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output and `--compact` for narrow terminals. See the linked ccusage documentation for detailed flag descriptions.

## Environment Variables

| Variable        | Description                                                                                                   |
| --------------- | ------------------------------------------------------------------------------------------------------------- |
| `KILO_DATA_DIR` | Override the root directory, or comma-separated root directories, containing Kilo data                        |
| `LOG_LEVEL`     | `0` hides aggregated warnings, progress, and table titles ([details](/guide/environment-variables#log-level)) |

## Cost Calculation

In `auto` mode, ccusage uses a positive recorded cost when present; otherwise it calculates from tokens and model pricing. Kilo records `0` both for free usage and when the provider returns no cost, so a recorded `0` is calculated from tokens too, which stays $0.00 for a model whose price is zero. `calculate` always prices tokens, while `display` uses recorded costs only.

## Troubleshooting

::: details No Kilo usage data found
Ensure the data directory exists at `~/.local/share/kilo/` and contains `kilo.db`. Set `KILO_DATA_DIR` for custom paths or comma-separated archive roots.
:::

::: details Costs showing as $0.00
If Kilo recorded no positive cost and ccusage cannot resolve a model price, the calculated cost is $0.00. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
