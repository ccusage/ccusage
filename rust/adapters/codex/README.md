# ccusage-adapter-codex

The Codex adapter: it turns Codex session JSONL, including forked and replayed sessions
into the usage entries the reports render.

## Owns

- `aggregate.rs` — reading the source, date filtering, and grouping events into report
  periods, including cross-session dedupe.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `reader.rs` — opening plain and Zstandard-compressed rollouts as one line stream.
- `replay.rs` — detecting history that a forked or subagent session replays.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.
- `speed.rs` — resolving the service tier that priced each request.
- `statusline.rs` — usage summaries and the statusline command wiring.
- `statusline/mcp.rs` — the native Codex stdio MCP statusline provider.
- `statusline/native.rs` — background session and Today aggregation and cached native output.
- `types.rs` — types that stay inside this adapter.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `~/.codex/sessions/**/*.jsonl`
- `~/.codex/sessions/**/*.jsonl.zst`
- `~/.codex/archived_sessions/**/*.jsonl`
- `~/.codex/archived_sessions/**/*.jsonl.zst`

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

Uses `ccusage-adapter-common` for walking and size-balanced chunking. `aggregate.rs` performs
the ordered parallel reads, and `reader.rs` streams `.jsonl.zst` rollouts through `zstd`
without unpacking them to disk.

Read failures and rejected usage records produce sanitized diagnostics while readable usage is
retained. Focused loaders print the warnings after loading; unified reports return them with the
source rows. Optional missing sources and deliberately suppressed records stay quiet. Existing
lenient field conversions are unchanged.

## Public surface

- `run_statusline`
- `report::calculate_group_cost`
- `speed::CodexSpeedPolicy`
- `types::CodexGroup`
- `types::CodexModelUsage`
- `types::CodexServiceTier`
- `types::CodexUsageBucket`
- `types::TierBuckets`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `compact_str`
- `jiff`
- `memchr`
- `rustc-hash`
- `serde`
- `serde_json`
- `toml`
- `zstd`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
