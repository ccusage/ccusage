# ccusage-adapter-antigravity

The Antigravity adapter turns local Antigravity SQLite conversation databases
into the usage entries the reports render. It is a separate source boundary
from the Gemini CLI adapter so unified reports preserve source attribution.

## Owns

- `loader.rs` — database discovery, bounded ordered reads, and identity-based deduplication.
- `parser.rs` — SQLite row handling, GeneratorMetadata/CortexStepMetadata protobuf decoding, token buckets, retries, and model naming.
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
table. When present, the real `steps` schema contributes step and retry usage,
and `trajectory_metadata_blob` supplies the timestamp fallback. SQLite,
row-iteration, and protobuf failures are reported instead of becoming empty or
partial reports.

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
