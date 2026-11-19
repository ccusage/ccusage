# ccusage-adapter-goose

The Goose adapter: it turns the Goose SQLite session database
into the usage entries the reports render.

## Data source

- `${GOOSE_PATH_ROOT}/data/sessions/sessions.db`, or
- `~/.local/share/goose/sessions/sessions.db` and
- `~/.local/share/Block/goose/sessions/sessions.db`

## Build layer

Built in the `adapters` Crane artifact layer.
