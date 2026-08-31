# ccusage-adapter-copilot

The GitHub Copilot CLI adapter: it turns Copilot session-state and OpenTelemetry JSONL files
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${COPILOT_HOME:-~/.copilot}/session-state/*/events.jsonl`
- `${COPILOT_HOME:-~/.copilot}/otel/**/*.jsonl`
- `COPILOT_HOME` (single relocated Copilot data root)
- `COPILOT_OTEL_FILE_EXPORTER_PATH` (one explicit JSONL file)

Session-state shutdown records are preferred for an exact `(session, model)` pair when both
sources contain it. Other OpenTelemetry records remain available, while duplicate shutdown
records are collapsed by their event identity. Session-state `inputTokens` includes cache reads
and writes, so the adapter reports the uncached remainder as input and keeps the cache buckets
separate. Reasoning tokens are already included in output tokens and are not added to totals or
costs. Internal model suffixes such as `-1m-internal` are removed before pricing and source
deduplication.

Reads plain files through `ccusage-adapter-common`, which handles walking, size-balanced
chunking, and ordered parallel reads.

## Public surface

- `loader::load_entries`
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
