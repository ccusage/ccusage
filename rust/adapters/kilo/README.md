# ccusage-adapter-kilo

The Kilo Code adapter: it turns the Kilo SQLite database
into the usage entries the reports render.

## Data source

- `${KILO_DATA_DIR:-~/.local/share/kilo}/kilo.db`

Message creation times are epoch milliseconds. Usage records without a valid
creation time are skipped with an invalid-timestamp diagnostic.

## Build layer

Built in the `adapters` Crane artifact layer.
