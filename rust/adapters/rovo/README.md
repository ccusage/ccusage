# ccusage-adapter-rovo

The Atlassian Rovo Dev CLI adapter: it turns per-session `session_context.json`
files into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${ROVO_DATA_DIR:-~/.rovodev}/**/session_context.json` (sessions live under
  `sessions/<session-id>/`), plus the directory named by
  `sessions.persistenceDir` in `~/.rovodev/config.yml` when users relocate the
  sessions root.

Each session file is one JSON document. Per-turn usage lives in
`message_history[]` entries with `kind: "response"`; the top-level session
`usage` object is a cumulative duplicate of those turns and is ignored.

## Token semantics

- The top-level `input_tokens` (modern schema) and `request_tokens` (legacy
  CLI 0.6.x schema) prompt totals both INCLUDE cache write/read tokens. The
  `usage.details` object carries the raw provider values, so `details` is the
  source of truth for the uncached-input/cache-write/cache-read split.
- Legacy responses persist `model_name: null`; those entries report the
  `unknown` model and surface the missing-pricing warning instead of a cost.
- Forked sessions copy the parent's `message_history` prefix verbatim into a
  new session file. Responses are deduped across files by
  `provider_response_id` so fork families do not double-count; legacy records
  without ids fall back to a composite token/timestamp key.
- No cost is persisted locally (Rovo Dev bills Atlassian credits, not
  dollars), so costs are LiteLLM estimates of equivalent API pricing and
  `--mode display` shows 0.

## Public surface

- `loader::load_entries`
- `report::report_from_rows`
- `report::summarize_entries`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
