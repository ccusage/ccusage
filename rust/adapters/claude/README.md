# ccusage-adapter-claude

The Claude Code adapter: it turns the JSONL transcripts Claude Code writes per project and session
into the usage entries the reports render.

## Owns

- `commands/report.rs` — `ccusage daily`, `monthly`, `weekly`, `session` and `blocks`.
- `commands/statusline.rs` — `ccusage statusline` and its output cache.
- `blocks.rs` — billing-block grouping, burn rate and the blocks table.
- `daily.rs` — the shared record parser and deduplication, plus daily aggregation.
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
source rows. Optional missing sources, nonusage records and unsupported envelopes stay quiet.
Malformed confirmed usage is diagnosed even when the existing parser deliberately rejects it.
Existing lenient field conversions are unchanged.

Statusline reads share caller-owned diagnostics and emit warnings once after rendering.

## Public surface

- `SOURCE`
- `load_entries`
- `run_daily`, `run_bucket`, `run_session`, `run_blocks`, `run_statusline`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `ccusage-report`
- `jiff`
- `memchr`
- `rustc-hash`
- `serde`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
