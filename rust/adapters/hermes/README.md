# ccusage-adapter-hermes

The Hermes adapter: it turns the Hermes SQLite state database
into the usage entries the reports render.

## Data source

- `${HERMES_HOME:-~/.hermes}/state.db`

## Build layer

Built in the `adapters` Crane artifact layer.
