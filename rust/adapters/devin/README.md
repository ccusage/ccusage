# ccusage-adapter-devin

The Devin adapter: it turns Devin CLI session transcripts
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${DEVIN_TRANSCRIPTS_DIR:-${XDG_DATA_HOME:-~/.local/share}/devin/cli/transcripts}/*.json`
  (`%APPDATA%\devin\cli\transcripts` on Windows)
- `${XDG_DATA_HOME:-~/.local/share}/cognition/cli/transcripts/*.json` (pre-migration
  installs keep a compatibility symlink here, so it usually resolves to the same
  directory; discovered paths are canonicalized and deduplicated)

Transcripts are ATIF JSON documents, one per session. Only `source: "agent"`
steps carry `metrics` (`prompt_tokens`, `completion_tokens`, `cached_tokens`,
`extra.cache_creation_input_tokens`); other steps and transcripts without
metrics (schema versions before ATIF-v1.7) contribute no usage.

`prompt_tokens` counts the whole request context including the cached and
cache-creation portions, so `input_tokens` is the remainder after both are
subtracted — the same normalization the shared cost code expects.

Reads plain files through `ccusage-adapter-common`, which handles walking,
size-balanced chunking, and ordered parallel reads.

## Public surface

- `loader::load_entries`
- `report::report_from_rows`
- `report::summarize_entries`
- `has_data`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
