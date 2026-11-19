# ccusage-adapter-common

The file-reading and table-rendering behavior the adapters share, kept out of
`ccusage-core` so that changing it does not touch the report, pricing, or config
layers.

## Owns

- file discovery by extension, including the JSONL helper most adapters use.
- `chunk_file_indexes_by_size` — size-balanced work splitting, so one large file
  does not leave workers idle.
- `map_parallel` — ordered parallel reads: results
  come back in input order, so output does not depend on worker scheduling, a
  worker's panic reaches the caller, and `--single-thread` collapses to a plain
  sequential read. `worker_count` is the one rule for how many workers run.
- `timestamps` — a file's mtime, and an epoch number whose unit the source
  does not fix.
- `units::read_units` and `read_units_flat` — ordered parallel reads with fault
  isolation: a unit whose read fails is recorded in the load's `Diagnostics`
  and the rest of the report goes on, so no call site writes its own skip.
- `units::read_typed_units` and `read_typed_units_flat` preserve discovered
  metadata and input order, including a slot for every failed unit.
- `sqlite::sqlite_units` opens databases read-only with a five-second busy
  timeout, keeps successfully decoded rows and records row failures.
- `dedupe` keeps the first or last occurrence of a typed key, preserving the
  input order of the surviving records.
- `source` — the contract between a usage source and the reports: `UsageSource`,
  the object-safe trait the unified report holds, and
  `EntrySource`, the hooks of a source whose loader yields `Vec<LoadedEntry>`,
  with its declared window, detection, session identity and table policies. Standard aggregation, JSON and table hooks have shared defaults.
- `pipeline` — the one order an `EntrySource` report runs in, with focused
  and unified entry points sharing date-window and pricing policies. Detection
  applies to the unified entry point.
- `rows` and `claims` — the row a source hands the unified report, and the
  record of what the Claude source counted that OpenCode must not count again.
  `claims::channel` connects those two adapters directly; a producer that exits
  without publishing returns an error to its consumer.
- `source::LoadRequest` and `LoadedRows` carry source-neutral loads at day or
  session granularity. `ProgressOwner::run` controls their progress output.
- `source::LoadContext` gives entry loaders typed `ReportPricing`.
  `ReportPricing` uses a supplied or locally loaded table. Display costs skip pricing. `UsageLoadContext` shares lazy pricing and report
  windows between format-specific sources.
- `filter_loaded_entries_by_date` — the shared `--since`/`--until` filter.
- `jsonl` — the streaming JSONL parsing helpers, including diagnostics for
  selected records that fail to decode without discarding later records.

## Public surface

- `roots::{resolve, RootsEnv}` for normalized list or single directory roots
- `source::{EntryDescriptor, SummaryKind, SessionScope, SessionShape}`
- `source::{LoadContext, ReportPricing, UsageLoadContext}`
- `collect_files_with_extension`, `collect_files_matching`,
  `visit_usage_files` and `dir_entries`, which record directories, entries
  and file types they cannot read in a `Diagnostics`
- `read_discovered_files`
- `filter_loaded_entries_by_date`
- `chunk_file_indexes_by_size`
- `map_parallel`, `Split` and `worker_count`
- `timestamps::file_mtime` and `timestamps::epoch_secs_or_millis`
- `units::read_units`
- `units::read_units_flat`
- `units::UnitError`
- `units::read_json` and `units::decode_json`
- `jsonl::numbered_records_with_usage_diagnostics`
- `source::UsageSource`, `source::EntrySource` and the request, row and policy
  types they exchange
- `rows::{SourceRow, RowUsage}`
- `rows::summary_rows`
- `claims::ClaudeCounted`

## Depends on

- `ccusage-core`
- `serde`
- `serde_json`

## Build layer

Built in the `foundation` Crane artifact layer, so a change here recompiles every adapter.
