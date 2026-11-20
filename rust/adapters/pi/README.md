# ccusage-adapter-pi

The Pi adapter: it turns Pi agent stores, including additional named stores from the config file
into the usage entries the reports render.

## Data source

- `${PI_AGENT_DIR:-~/.pi/agent/sessions}`, or `--pi-path` / `--pi-agent-dir`, plus any `pi.stores[]` entries in `ccusage.json`

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

## Build layer

Built in the `adapters` Crane artifact layer.
