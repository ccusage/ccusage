# ZCode Data Source

ccusage can read the local ZCode desktop app's usage database as a supported data source. ZCode uses the same unified and focused report model as other agents.

## Focused Views

```bash
# Daily ZCode usage
ccusage zcode daily

# Monthly ZCode usage
ccusage zcode monthly

# ZCode sessions
ccusage zcode session
```

Most users can start with unified reports such as `ccusage daily`. Add the `zcode` namespace only when you want to focus the same report shape on ZCode usage.

## Data Source

The CLI reads completed model requests from the `model_usage` table in ZCode's SQLite database, joining `session` for the project directory.

Root resolution (highest first):

1. A non-empty `ZCODE_HOME` (one root or a comma-separated list)
2. `~/.zcode`

```bash
ZCODE_HOME="$HOME/.zcode" ccusage zcode daily
```

```text
$ZCODE_HOME/   # or ~/.zcode
└── cli/
    └── db/
        └── db.sqlite  # model_usage + session tables
```

Only rows with `status = 'completed'` are counted. Error and cancelled attempts record zero tokens, so skipping them changes nothing. The database is opened read-only, which is safe while ZCode is running.

## Report Views

| Focused view            | Description                  | See also                                |
| ----------------------- | ---------------------------- | --------------------------------------- |
| `ccusage zcode daily`   | Aggregate usage by date      | [Daily Usage](/guide/daily-reports)     |
| `ccusage zcode monthly` | Aggregate usage by month     | [Monthly Usage](/guide/monthly-reports) |
| `ccusage zcode session` | Group usage by ZCode session | [Session Usage](/guide/session-reports) |

These views support [`--json`](/guide/json-output), `--compact`, `--mode`, and `--offline`.

## What Gets Calculated

- **Token usage** - ZCode records OpenAI-style usage where `input_tokens` includes cache-read and cache-creation tokens. ccusage carves both cache slices out of input so the reported buckets remain additive and use their own pricing rates.
- **Reasoning tokens** - ZCode's schema has a `reasoning_tokens` column, but every observed row records zero; reasoning is assumed to sit inside output and is never added on top. If a future ZCode version moves it outside, the recorded total grows past the counted buckets and ccusage routes the difference into its extra-tokens bucket instead of dropping it.
- **Costs** - ZCode records no per-request cost. The default `auto` mode and `calculate` estimate costs from the pricing tables; `display` shows `$0.00` because there is no recorded cost to display. Pricing lookup honors an exact raw-model override first, then tries provider-qualified Z.ai and normalized aliases.
- **Session metadata** - Session reports carry the project directory and first/last activity.

## Environment Variables

| Variable     | Description                                      |
| ------------ | ------------------------------------------------ |
| `ZCODE_HOME` | ZCode data home (single root or comma-separated) |
| `LOG_LEVEL`  | Adjust verbosity (0 silent ... 5 trace)          |

## Configuration

```json
{
	"zcode": {
		"defaults": {
			"offline": true
		},
		"commands": {
			"session": {
				"json": true
			}
		}
	}
}
```

The `zcode` namespace supports the same shared report options as other focused sources. Use `zcode.defaults` for all ZCode reports and a matching `zcode.commands.daily`, `zcode.commands.monthly`, or `zcode.commands.session` object for report-specific overrides. The data root is discovered from `ZCODE_HOME` or `~/.zcode`, not from ccusage configuration.

## Troubleshooting

::: details No ZCode usage data found
Ensure `~/.zcode/cli/db/db.sqlite` exists and has completed rows in `model_usage`. Set `ZCODE_HOME` if your data lives elsewhere. A database ZCode has reformatted to an unexpected schema degrades to no entries; run with `--debug` to see the failure.
:::

::: details Costs showing as $0.00
ZCode records no per-request cost, so `--mode display` shows `$0.00`. Use the default `auto` mode or `--mode calculate` to estimate costs from the pricing tables. If a model is missing from pricing, the estimate stays at zero and a missing-pricing warning may appear.
:::
