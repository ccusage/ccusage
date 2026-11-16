# ccusage-adapter-junie

Reads JetBrains Junie session usage and produces the standard daily, weekly,
monthly, and session reports.

## Data sources

The adapter reads `${JUNIE_HOME:-~/.junie}/sessions/`. `JUNIE_HOME` is Junie's
own home override and takes a single root.

- `sessions/<session-id>/events.jsonl` — the event stream. Only direct child
  directories of `sessions/` are sessions.
- `sessions/index.jsonl` — per-session `sessionId`, `createdAt`, and
  `projectDir`. A missing or unreadable index does not stop events loading.

`stats-aggregate.json` and `state.json` are not read: they hold totals without
per-request timestamps.

## Usage data

Usage comes only from `event.agentEvent.kind == "LlmResponseMetadataEvent"`.
Each `modelUsage[]` entry is one record: `model`, `inputTokens`,
`outputTokens`, `cacheInputTokens` (cache read), `cacheCreateTokens` (cache
write), and `cost`. `inputTokens` is treated as excluding the cache counters,
as other Junie trackers do; JetBrains does not document it. Entries
without a model or with all four counters at zero are skipped.

The record time is the line's `timestampMs`. Builds before `timestampMs`
existed fall back to the session's index `createdAt`; with neither, the record
is skipped rather than dated by the directory name or file mtime.

`cost` is the cost Junie recorded. The shared cost mode decides how it competes
with token-derived cost; a negative or non-finite cost is treated as missing.

Session IDs are the session directory names. `project_path` is the index
`projectDir`, or `Junie` when the session has no index entry.

## Adapter surface

- `load_entries(&SharedArgs, &PricingMap)` loads normalized usage records.
- `summarize_entries(&[LoadedEntry], AgentReportKind)` aggregates a report.
- `run(AgentCommandArgs)` runs the Junie report command.

The crate is built in the adapters Crane artifact layer with the other source
adapters.
