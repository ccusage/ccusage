# ccusage-adapter-goose

The Goose adapter: it turns the Goose SQLite session database
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${GOOSE_PATH_ROOT}/data/sessions/sessions.db`, or
- `~/.local/share/goose/sessions/sessions.db` and
- `~/.local/share/Block/goose/sessions/sessions.db`

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
