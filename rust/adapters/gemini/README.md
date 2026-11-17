# ccusage-adapter-gemini

The Gemini CLI adapter: it turns chat JSON and JSONL files
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${GEMINI_DATA_DIR:-~/.gemini}/tmp/**/chats/*.{json,jsonl}`

JSONL usage records without a valid timestamp are skipped with a diagnostic.
Whole-file JSON documents can use their session timestamp or file modification time.

Reads plain files through `ccusage-adapter-common`, which handles walking,
size-balanced chunking, and ordered parallel reads. Antigravity SQLite data is
owned by the standalone `ccusage-adapter-antigravity` source.

## Public surface

- `SOURCE`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
