# ccusage-adapter-droid

The Factory Droid adapter: it turns session JSON files
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${DROID_SESSIONS_DIR:-~/.factory/sessions}/**/*.json`

Reads plain files through `ccusage-adapter-common`, which handles walking, size-balanced
chunking, and ordered parallel reads.

A settings file uses its provider-lock timestamp, falling back to the file's
modification time. If neither is usable, it is skipped with an invalid-timestamp
diagnostic. A modification-time fallback does not select historical pricing.
Pricing keeps the model ID from settings or the sidecar as a candidate ahead
of the derived report label. An explicit provider lock also supplies strict
provider-qualified candidates.

## Public surface

- `SOURCE`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
