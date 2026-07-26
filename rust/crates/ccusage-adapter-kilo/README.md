# ccusage-adapter-kilo

The Kilo Code adapter: it turns the Kilo SQLite database
into the usage entries the reports render.

## Owns

- `paths.rs` — the environment variables and default directories below.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `loader.rs` — discovery, dedupe, and date filtering.
- `report.rs` — the JSON and table shapes when they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${KILO_DATA_DIR:-~/.local/share/kilo}/kilo.db`

Reads SQLite with the bundled `sqlite` crate, which is why this crate declares it and
most adapters do not.

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
