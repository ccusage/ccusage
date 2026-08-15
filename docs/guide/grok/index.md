# Grok Build CLI Data Source

ccusage can read local Grok Build CLI session logs as a supported data source. Grok uses the same unified and focused report model as other agents.

## Focused Views

```bash
# Daily Grok usage
ccusage grok daily

# Monthly Grok usage
ccusage grok monthly

# Grok sessions
ccusage grok session
```

Most users can start with unified reports such as `ccusage daily`. Add the `grok` namespace only when you want to focus the same report shape on Grok usage.

## Data Source

The CLI reads completed turns from `updates.jsonl` under the Grok home directory.

Root resolution (highest first):

1. A non-empty `GROK_HOME` (official Grok environment variable)
2. `~/.grok`

```bash
GROK_HOME="$HOME/.grok" ccusage grok daily
```

```text
$GROK_HOME/   # or ~/.grok
└── sessions/
    └── <url-encoded-cwd>/
        └── <session-uuid>/
            ├── updates.jsonl  # PRIMARY (turn_completed + usage)
            └── summary.json   # optional metadata
```

Only rows with `sessionUpdate == "turn_completed"` and a usable usage breakdown are counted. In-progress turns are not included until they complete, and a session killed mid-turn never records one, so its usage cannot be reported. `logs/unified.jsonl` is not used as a source: it has no per-request model id, so its tokens cannot be priced or attributed to a model.

## Report Views

| Focused view           | Description                 | See also                                |
| ---------------------- | --------------------------- | --------------------------------------- |
| `ccusage grok daily`   | Aggregate usage by date     | [Daily Usage](/guide/daily-reports)     |
| `ccusage grok monthly` | Aggregate usage by month    | [Monthly Usage](/guide/monthly-reports) |
| `ccusage grok session` | Group usage by Grok session | [Session Usage](/guide/session-reports) |

These views support `--json`, `--compact`, `--mode`, and `--offline`.

Focused terminal tables omit the `Cache Create` column when the selected Grok
rows report no cache-creation tokens. The column reappears when selected rows
contain a non-zero value; JSON output keeps `cacheCreationTokens` in the stable
report schema either way.

## What Gets Calculated

- **Token usage** - Grok records OpenAI-style usage where `inputTokens` includes cache. ccusage splits it into uncached input, cache read (`cachedReadTokens`) and cache write (`cacheCreationTokens`), and stores the full `outputTokens` as output.
- **Reasoning tokens** - `reasoningTokens` are a subset of `outputTokens`, so they are already counted in the total. They are **not** added on top of output for either tokens or cost.
- **Precomputed cost** - Grok records `costUsdTicks` on each completed turn, in units of 1e-10 USD. ccusage uses it as the invoice cost, so `display` and the default `auto` report exactly what Grok billed.
- **Pricing** - `calculate`, and `auto` for turns that recorded no ticks, fall back to table estimates. Model ids such as `grok-4.5-build` try candidates with the trailing `-build` stripped and `xai/` / `x-ai/` prefixes, exact matches first. xAI's long-context rates apply when a turn's whole context — fresh input plus cache reads and writes — exceeds the model's boundary (200K for `grok-4.5` and `grok-4.6`). A `turn_completed` row aggregates several API requests, so the tier is chosen per turn rather than per request and the estimate only approximates the invoice.
- **Model labels** - Display form is the raw `modelUsage` key (e.g. `grok-4.5-build`). The Agent column identifies the Grok source in unified reports.

## Environment Variables

| Variable    | Description                                  |
| ----------- | -------------------------------------------- |
| `GROK_HOME` | Official Grok config/data home (single root) |
| `LOG_LEVEL` | Adjust verbosity (0 silent ... 5 trace)      |

## Configuration

```json
{
	"grok": {
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

The `grok` namespace supports the same shared report options as other focused
sources. Use `grok.defaults` for all Grok reports and a matching
`grok.commands.daily`, `grok.commands.monthly`, or `grok.commands.session`
object for report-specific overrides. The data root is discovered from
`GROK_HOME` or `~/.grok`, not from ccusage configuration.

## Troubleshooting

::: details No Grok usage data found
Ensure completed turns exist under `~/.grok/sessions/**/updates.jsonl`. In-progress turns do not appear until `turn_completed` is written. Set `GROK_HOME` if your data lives elsewhere.
:::

::: details Costs showing as $0.00
Turns written before Grok started recording `costUsdTicks` carry no cost, so `display` shows zero for them. Use `--mode calculate` to price those from the pricing tables instead. If a model is missing from pricing, the cost stays at zero and a missing-pricing warning may appear.
:::

::: details Totals lower than expected while a turn is open
v1 only counts completed turns. Finish the turn (or wait for `turn_completed`) and re-run the report.
:::
