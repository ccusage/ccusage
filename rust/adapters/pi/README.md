# ccusage-adapter-pi

The Pi adapter: it turns Pi agent stores, including additional named stores from the config file
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- Pi focused reports read `${PI_AGENT_DIR:-~/.pi/agent/sessions}`; unified reports also load configured `pi.stores[]` entries and discover Oh My Pi sessions automatically as synthetic named store `omp`.

The synthetic `omp` store is included in unified reports only. It prefixes raw model labels with `[omp] `; the focused `ccusage pi ...` command remains specific to pi-agent.

The default OMP session root is `~/.omp/agent/sessions`. `PI_CONFIG_DIR` defaults to `.omp` and is joined under the home directory. `PI_CODING_AGENT_DIR` adds another agent root with `sessions/` appended; relative values resolve from the working directory. Unified reports continue to discover the default and existing profile roots and, on Linux or macOS, existing migrated roots under `$XDG_DATA_HOME/omp` when this override is set.

The OMP reader consumes local `.jsonl` session files, including top-level `model_usage` entries. It ignores compaction `tokensBefore` metadata and does not read SQLite or Redis-backed session storage or compressed `.jsonl.gz` archives.

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

Reads plain files through `ccusage-adapter-common`, which handles walking, size-balanced
chunking, and ordered parallel reads.

## Public surface

- `loader::load_entries_for_store_path`
- `loader::load_entries`
- `loader::load_entries_for_store_paths`
- `loader::load_entries_for_store_paths_excluding`
- `paths::named_store_paths`
- `paths::omp_default_paths`
- `paths::paths as default_paths`
- `report::report_from_rows`
- `report::summarize_entries`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
