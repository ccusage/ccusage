# ccusage-adapter-kilo

The Kilo Code adapter: it turns the Kilo SQLite database
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${KILO_DATA_DIR:-~/.local/share/kilo}/kilo.db`

Reads SQLite with the bundled `sqlite` crate, which is why this crate declares it and
most adapters do not.

Message creation times are epoch milliseconds. Usage records without a valid
creation time are skipped with an invalid-timestamp diagnostic.

## Public surface

- `SOURCE`
- `summarize_entries`
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
