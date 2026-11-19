# ccusage-report

Report helpers for the `ccusage` binary, unified reports and Claude adapter: the `--last` window math,
the JSON shapes of the daily, weekly, monthly, and session reports, the
since/until row filter, and the date and time formatters the blocks and
statusline reports print.

## Owns

- `last_window.rs` — `last_periods_since`.
- `output.rs` — `summary_json`, `session_summary_json`, and `group_project_output`.
- `date_utils.rs` — `format_date`, `zoned_parts`, `hour_12`, and `am_pm`.

These sit above `ccusage-core` so editing them recompiles the Claude adapter, unified adapter
and binary without rebuilding every source adapter. Helpers shared across
adapters belong in core or `ccusage-adapter-common`.

## Public surface

- `am_pm`
- `format_date`
- `group_project_output`
- `hour_12`
- `last_periods_since`
- `zoned_parts`
- `session_cost_ordering`
- `session_summary_json`
- `summary_json`

## Depends on

- `ccusage-core`
- `jiff`
- `serde_json`

## Build layer

Outside the Crane artifact layers: it is compiled with the final binary, so editing it leaves the cached layers untouched.
