# ZCode Data Source

ccusage reads completed ZCode model-usage records from the local SQLite
database and exposes the same focused and unified reports as the other
supported sources.

## Focused Views

```bash
ccusage zcode daily
ccusage zcode monthly
ccusage zcode session
```

Use `ccusage daily`, `ccusage monthly`, or `ccusage session` when ZCode should
be included with every other detected source.

## Data Source

ZCode stores its usage ledger at `cli/db/db.sqlite` beneath its data root:

```text
$ZCODE_HOME/   # or ~/.zcode
└── cli/
    └── db/
        └── db.sqlite
```

Set `ZCODE_HOME` to one root or a comma-separated list of roots when usage data
is stored elsewhere. Roots and database paths are canonicalized and
deduplicated. When `ZCODE_HOME` is non-empty, invalid configured roots are
skipped and discovery does not fall back to `~/.zcode`.

The adapter validates the `model_usage` and `session` tables before reading
them. The required columns are `id`, `session_id`, `started_at`, `model_id`,
`status`, `input_tokens`, and `output_tokens` on `model_usage`, plus `id` and
`directory` on `session`. Optional cache, provider, computed-total, and session
version columns are detected individually. The presence of `session.version`
is reported as the session-versioned layout; older databases without it use
the legacy layout. Compatible additive schema changes are accepted, while
incomplete or unrelated databases are ignored.

Only rows whose status is exactly `completed` are counted. `started_at` is an
epoch-millisecond timestamp, so report dates retain sub-second precision in
the source entry. The read query selects only identifiers, timestamps, status,
token counters, model/provider metadata, and the session directory. Prompt,
content, and raw usage fields are not materialized.

Connections are opened read-only, use a busy timeout while ZCode may be
writing, and enable connection-local SQLite query-only mode. The adapter does
not change journal mode, so an existing ZCode WAL remains available for
concurrent reads.

## Token and Cost Handling

ZCode's `input_tokens` includes both cache-read and cache-creation tokens. The
adapter subtracts both buckets from fresh input and reports them separately.
Cache buckets are bounded by the inclusive input count, and any positive
remainder in `computed_total_tokens` is preserved by the shared extra-token
fallback.

ZCode does not record a per-request cost. `--mode display` therefore reports
zero cost. `--mode calculate` and `auto` use the shared model pricing map when
the model can be resolved. Built-in Z.ai pricing is selected for an explicit
Z.ai provider, or for legacy rows with no provider and a GLM model. A custom
provider UUID is not treated as a billable provider even when its model name
matches a generic pricing entry; use an explicit model pricing override when
that provider should be estimated. Unpriced models remain at zero cost and use
the standard missing-pricing warning. Z.ai cache-creation tokens remain in the
displayed cache-creation bucket but are estimated at the model's standard input
rate because they represent new content.

## Configuration

The `zcode` namespace supports the shared report options:

```json
{
	"zcode": {
		"defaults": {
			"offline": true
		},
		"commands": {
			"daily": {
				"json": true
			}
		}
	}
}
```

The data root is controlled by `ZCODE_HOME`; it is not a billing or provider
configuration setting.

## Report Views

| Focused view            | Description                  | See also                                |
| ----------------------- | ---------------------------- | --------------------------------------- |
| `ccusage zcode daily`   | Aggregate usage by date      | [Daily Usage](/guide/daily-reports)     |
| `ccusage zcode monthly` | Aggregate usage by month     | [Monthly Usage](/guide/monthly-reports) |
| `ccusage zcode session` | Group usage by ZCode session | [Session Usage](/guide/session-reports) |

These views support the standard `--json`, `--compact`, `--mode`, `--offline`,
`--since`, `--until`, and `--timezone` options.

## Troubleshooting

::: details No ZCode usage data found
Ensure `$ZCODE_HOME/cli/db/db.sqlite` or `~/.zcode/cli/db/db.sqlite` exists and contains the `model_usage` and `session` tables. Use `--debug` to see rejected roots, schema diagnostics, and database read errors.
:::

::: details Costs showing as $0.00
ZCode has no per-request cost field. Use `--mode calculate` to estimate rows whose model is in the pricing map. Custom provider UUIDs do not identify a billable provider, so unpriced custom models remain at zero.
:::

::: details Totals lower than expected while ZCode is open
Only completed model-usage rows are included. Wait for the turn to complete and rerun the report.
:::
