# ccusage-adapter-kimi

The Kimi CLI adapter: it turns per-session `wire.jsonl` files
into the usage entries the reports render.

## Data source

- `${KIMI_DATA_DIR:-~/.kimi}/sessions/**/wire.jsonl`, and the same layout under `~/.kimi-code`

Of the `usage.record` entries, only turn-scoped records count. Legacy
`token_usage` records also count. For duplicate entries
with the same session, message ID (when present), timestamp, model, and token
counts, ccusage keeps the first copy. Usage records without a valid timestamp
are skipped with a diagnostic.

Duplicate records with matching report counts keep the copy with the most
recorded output tokens. A total-only copy contributes its usage once and
supplies no billable output of its own.

## Build layer

Built in the `adapters` Crane artifact layer.
