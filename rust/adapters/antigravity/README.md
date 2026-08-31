# ccusage-adapter-antigravity

The Antigravity adapter turns local Antigravity SQLite conversation databases
into the usage entries the reports render. It is a separate source boundary
from the Gemini CLI adapter so unified reports preserve source attribution.

## Owns

- `loader.rs` — database discovery, ordered parallel reads, and response-ID deduplication.
- `parser.rs` — SQLite row handling, GeneratorMetadata protobuf decoding, token buckets, and model naming.
- `paths.rs` — environment variables, default roots, and `.db` discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

## Data source

The adapter reads `.db` files below these default roots:

- `~/.gemini/antigravity/conversations/`
- `~/.gemini/antigravity-cli/conversations/`
- `~/.gemini/antigravity-ide/conversations/`
- `~/.gemini/antigravity-backup/conversations/`
- `~/.config/antigravity/conversations/`

`ANTIGRAVITY_DATA_DIR` accepts one or more comma-separated data roots. Each
root may contain a `conversations/` child or be the conversation directory
itself. Databases are opened read-only and must provide the `gen_metadata`
table.

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
- `sqlite`

## Build layer

Built in the `adapters` Crane artifact layer with the other per-source
adapters.
