# ccusage-adapter-codex

The Codex adapter: it turns Codex session JSONL, including forked and replayed sessions
into the usage entries the reports render.

## Owns

- `paths.rs` — the environment variables and default directories below.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `loader.rs` — discovery, dedupe, and date filtering.
- `report.rs` — the JSON and table shapes when they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `~/.codex/sessions/**/*.jsonl`

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

Reads plain files through `ccusage-adapter-common`, which handles walking, size-balanced
chunking, and ordered parallel reads.

## Public surface

- `aggregate::aggregate_events`
- `aggregate::filter_events_by_date`
- `aggregate::load_groups`
- `loader::load_codex_events`
- `loader::load_codex_events_from_directory`
- `report::calculate_codex_model_cost`
- `report::calculate_group_cost`
- `report::codex_model_missing_pricing`
- `report::non_cached_input_tokens`
- `speed::CodexSpeedPolicy`
- `speed::resolve_codex_speed`
- `types::CodexGroup`
- `types::CodexModelUsage`
- `types::CodexRawUsage`
- `types::CodexServiceTier`
- `types::CodexTokenUsageEvent`
- `types::CodexUsageBucket`
- `types::merge_codex_service_tiers`
- `run`
- `report_json`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `compact_str`
- `jiff`
- `memchr`
- `rustc-hash`
- `serde`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
