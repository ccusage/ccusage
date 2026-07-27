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
- `filter_loaded_entries_by_date` — the shared `--since`/`--until` filter.
- `jsonl` — the streaming JSONL parsing helpers.
- `report::print_table_for_agent` — the usage table for adapters whose rows are
  plain `UsageSummary` values, parameterized by agent name.

## Public surface

- `report::print_table_for_agent`
- `collect_usage_files`
- `collect_files_with_extension`
- `filter_loaded_entries_by_date`
- `chunk_file_indexes_by_size`
- `read_files_parallel`

## Depends on

- `ccusage-core`
- `serde`
- `serde_json`

## Build layer

Built in the `foundation` Crane artifact layer, so a change here recompiles every adapter.
