# ccusage-adapter-claude

The Claude Code adapter: it turns the JSONL transcripts Claude Code writes per project and session
into the usage entries the reports render.

## Owns

- `daily.rs` — the daily report path, which reads the same files with a narrower parser.
- `paths.rs` — environment variables, default directories, and file discovery.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `~/.claude/projects/**/*.jsonl` and `~/.config/claude/projects/**/*.jsonl`

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

Reads plain files through `ccusage-adapter-common`, which handles walking, size-balanced
chunking, and ordered parallel reads.

Read failures and rejected usage records produce sanitized diagnostics while readable usage is
retained. Focused loaders print the warnings after loading; unified reports return them with the
source rows. Optional missing sources and deliberately suppressed records stay quiet. Existing
lenient field conversions are unchanged.

## Public surface

- `paths::timestamp_from_line`
- `paths::claude_paths`
- `paths::extract_project`
- `paths::extract_session_parts`
- `paths::usage_files`
- `load_entries`
- `load_daily_summaries`
- `usage_limit_reset_time_from_line`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `memchr`
- `rustc-hash`
- `serde`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
