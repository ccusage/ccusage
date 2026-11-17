# ccusage-report

Report helpers for the `ccusage` binary and Claude adapter: the `--last` window math,
the JSON shapes of the daily, weekly, monthly, and session reports, the
since/until row filter, and the date and time formatters the blocks and
statusline reports print.

## Owns

- `last_window.rs` — `PeriodUnit` and `last_periods_since`.
- `output.rs` — `summary_json`, `session_summary_json`, and `group_project_output`.
- `summary.rs` — `filter_and_sort_summaries`.
- `date_utils.rs` — `format_date`, `format_utc_second`,
  `local_parts`, `hour_12`, `am_pm`, and `is_valid_timezone`.

These sit above `ccusage-core` so editing them recompiles the Claude adapter
and binary without rebuilding every source adapter. Helpers shared across
adapters belong in core or `ccusage-adapter-common`.

## Public surface

- `PeriodUnit`
- `am_pm`
- `filter_and_sort_summaries`
- `format_date`
- `format_utc_second`
- `group_project_output`
- `hour_12`
- `is_valid_timezone`
- `last_periods_since`
- `local_parts`
- `session_summary_json`
- `summary_json`

## Depends on

- `ccusage-core`
- `jiff`
- `serde_json`

## Build layer

Outside the Crane artifact layers: it is compiled with the final binary, so editing it leaves the cached layers untouched.
