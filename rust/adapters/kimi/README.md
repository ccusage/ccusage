# ccusage-adapter-kimi

The Kimi CLI adapter: it turns per-session `wire.jsonl` files
into the usage entries the reports render.

## Data source

- `${KIMI_DATA_DIR:-~/.kimi}/sessions/**/wire.jsonl`, and the same layout under `~/.kimi-code`

Usage records without a valid timestamp are skipped with a diagnostic.

## Build layer

Built in the `adapters` Crane artifact layer.
