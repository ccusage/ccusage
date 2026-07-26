# ccusage-adapter-codebuff

The Codebuff adapter: it turns per-channel `chat-messages.json` files
into the usage entries the reports render.

## Owns

- `paths.rs` — the environment variables and default directories below.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `loader.rs` — discovery, dedupe, and date filtering.
- `report.rs` — the JSON and table shapes when they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${CODEBUFF_DATA_DIR:-~/.config/<channel>}/projects/**/chat-messages.json`
- Channels: `manicode`, `manicode-dev`, `manicode-staging`.

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
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
