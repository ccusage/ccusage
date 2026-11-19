# ccusage-adapter-claude

The Claude Code adapter: it turns the JSONL transcripts Claude Code writes per project and session
into the usage entries the reports render.

## Data source

- `~/.claude/projects/**/*.jsonl` and `~/.config/claude/projects/**/*.jsonl`

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

Read failures and rejected usage records produce sanitized diagnostics while readable usage is
retained. Focused loaders print the warnings after loading; unified reports return them with the
source rows. Optional missing sources, nonusage records and unsupported envelopes stay quiet.
Malformed confirmed usage is diagnosed even when the existing parser deliberately rejects it.
Existing lenient field conversions are unchanged.

Statusline reads share caller-owned diagnostics and emit warnings once after rendering.

## Build layer

Built in the `adapters` Crane artifact layer.
