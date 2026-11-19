# ccusage-adapter-codex

The Codex adapter: it turns Codex session JSONL, including forked and replayed sessions
into the usage entries the reports render.

## Data source

- `~/.codex/sessions/**/*.jsonl`
- `~/.codex/sessions/**/*.jsonl.zst`
- `~/.codex/archived_sessions/**/*.jsonl`
- `~/.codex/archived_sessions/**/*.jsonl.zst`

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

Read failures and rejected usage records produce sanitized diagnostics while readable usage is
retained. Focused loaders print the warnings after loading; unified reports return them with the
source rows. Optional missing sources and deliberately suppressed records stay quiet. Existing
lenient field conversions are unchanged.

## Build layer

Built in the `adapters` Crane artifact layer.
