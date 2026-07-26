# ccusage-adapter-pi

The Pi adapter: it turns Pi agent stores, including additional named stores from the config file
into the usage entries the reports render.

## Owns

- `paths.rs` — the environment variables and default directories below.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `loader.rs` — discovery, dedupe, and date filtering.
- `report.rs` — the JSON and table shapes when they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${PI_AGENT_DIR:-~/.pi/agent}`, plus any `pi.stores[]` entries in `ccusage.json`

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

Reads plain files through `ccusage-adapter-common`, which handles walking, size-balanced
chunking, and ordered parallel reads.

## Public surface

- `loader::load_entries_for_store_path`
- `loader::load_entries`
- `loader::load_entries_for_store_paths`
- `paths::named_store_paths`
- `paths::paths as default_paths`
- `report::report_from_rows`
- `report::summarize_entries`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
