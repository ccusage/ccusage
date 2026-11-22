# ccusage-adapter-kimi

The Kimi CLI adapter: it turns per-session `wire.jsonl` files
into the usage entries the reports render.

## Data source

- `${KIMI_DATA_DIR:-~/.kimi}/sessions/**/wire.jsonl`, and the same layout under `~/.kimi-code`

Of the `usage.record` entries, only turn-scoped records count. Legacy
`token_usage` records also count. Usage records without a valid timestamp are
skipped with a diagnostic.

Duplicates with matching session, message ID (when present), timestamp, model,
and token counts keep the copy with the most recorded output tokens. If a copy
lacks output tokens, the remainder can match the output count on a complete
copy. The remainder contributes only to the usage total.

## Build layer

Built in the `adapters` Crane artifact layer.
