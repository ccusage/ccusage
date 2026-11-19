# ccusage-adapter-openclaw

The OpenClaw adapter: it turns session JSONL files, including the tool's earlier names,
and per-agent SQLite transcript stores into the usage entries the reports render.

## Data source

- `${OPENCLAW_DIR:-~/.openclaw}/**/*.jsonl`, plus `~/.clawdbot`, `~/.moltbot`, and `~/.moldbot`
- `${OPENCLAW_DIR:-~/.openclaw}/agents/*/agent/openclaw-agent.sqlite` (2026.9.x database-first storage)

JSONL usage records without a valid timestamp are skipped with a diagnostic.
SQLite events can use the timestamp stored in their own `created_at` column.
The SQLite store is opened read-only. Its transcript events take precedence
over duplicates in migrated JSONL files.

## Build layer

Built in the `adapters` Crane artifact layer.
