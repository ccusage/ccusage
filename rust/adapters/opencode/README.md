# ccusage-adapter-opencode

The OpenCode adapter: it turns the OpenCode SQLite database
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `OPENCODE_DATA_DIR`, or `${XDG_DATA_HOME:-$HOME/.local/share}/opencode` when unset

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

Reads SQLite with the bundled `sqlite` crate, which is why this crate declares it and
most adapters do not.

Read failures and rejected usage records produce sanitized diagnostics while readable usage is
retained. Focused loaders print the warnings after loading; unified reports return them with the
source rows. Optional missing sources and deliberately suppressed records stay quiet. Existing
lenient field conversions are unchanged.

## Public surface

- `loader::load_entries`
- `report::report_json`
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
