# ccusage-adapter-common

The file-reading and table-rendering behavior the adapters share, kept out of
`ccusage-core` so that changing it does not touch the report, pricing, or config
layers.

## Owns

- file discovery by extension, including the JSONL helper most adapters use.
- `chunk_file_indexes_by_size` — size-balanced work splitting, so one large file
  does not leave workers idle.
- `read_files_parallel` — ordered parallel reads: results come back in file order,
  so output does not depend on worker scheduling, and `--single-thread` collapses
  to a plain sequential read.
- `units::read_units` and `read_units_flat` — `read_files_parallel` with fault
  isolation: a unit whose read fails is recorded in the load's `Diagnostics`
  and the rest of the report goes on, so no call site writes its own skip.
- `source` — the contract between a usage source and the reports: `UsageSource`,
  the object-safe trait the unified report holds, and
  `EntrySource`, the hooks of a source whose loader yields `Vec<LoadedEntry>`,
  with its declared window, detection and pricing policies.
- `pipeline` — the one order an `EntrySource` report runs in, with focused
  and unified entry points sharing date-window and pricing policies. Detection
  applies to the unified entry point.
- `rows` and `claims` — the row a source hands the unified report, and the
  record of what the Claude source counted that OpenCode must not count again.
- `filter_loaded_entries_by_date` — the shared `--since`/`--until` filter.
- `jsonl` — the streaming JSONL parsing helpers, including diagnostics for
  selected records that fail to decode without discarding later records.
- `report::print_table_for_agent` — the usage table for adapters whose rows are
  plain `UsageSummary` values, parameterized by agent name.

## Public surface

- `report::print_table_for_agent`
- `collect_files_with_extension`, `collect_files_matching`,
  `visit_files_matching`, `visit_usage_files` and `dir_entries`, which record
  directories, entries and file types they cannot read in a `Diagnostics`
- `read_discovered_files`
- `filter_loaded_entries_by_date`
- `chunk_file_indexes_by_size`
- `units::read_units`
- `units::read_units_flat`
- `units::UnitError`
- `units::read_json` and `units::decode_json`
- `jsonl::records_with_usage_diagnostics`
- `jsonl::numbered_records_with_usage_diagnostics`
- `source::UsageSource`, `source::EntrySource` and the request, row and policy
  types they exchange
- `rows::UnifiedRow`
- `rows::summary_rows`
- `claims::ClaudeCounted`

## Depends on

- `ccusage-core`
- `serde`
- `serde_json`

## Build layer

Built in the `foundation` Crane artifact layer, so a change here recompiles every adapter.
