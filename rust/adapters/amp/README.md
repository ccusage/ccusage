# ccusage-adapter-amp

The Amp adapter: it turns thread JSON files with a usage ledger
into the usage entries the reports render.

## Data source

- `${AMP_DATA_DIR:-~/.local/share/amp}/threads/T-{uuid}.json`

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

## Build layer

Built in the `adapters` Crane artifact layer.
