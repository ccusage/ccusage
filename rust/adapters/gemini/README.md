# ccusage-adapter-gemini

The Gemini CLI adapter: it turns chat JSON and JSONL files
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${GEMINI_DATA_DIR:-~/.gemini}/tmp/**/chats/*.{json,jsonl}`
- `${GEMINI_DATA_DIR:-~/.gemini}/antigravity/conversations/*.db`

Reads plain files and SQLite databases through `ccusage-adapter-common` and `sqlite`, which handles walking, size-balanced
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
- `sqlite`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
