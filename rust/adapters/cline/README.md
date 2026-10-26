# ccusage-adapter-cline

Reads completed usage records from Cline extension tasks and Cline CLI sessions
and produces the standard daily, weekly, monthly, and session reports.

## Data sources

By default, the adapter searches `~/.cline` and Cline extension `globalStorage`
directories for VS Code, VS Code Insiders, Cursor, Windsurf, and VSCodium. The
extension storage locations follow each platform's normal config directory:
`~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` (or `~/.config`)
on Linux, and `%APPDATA%` on Windows.

It recognizes these layouts under each data root:

- Extension task: `tasks/<task-id>/ui_messages.json`
- Legacy CLI task: `data/tasks/<task-id>/ui_messages.json`
- CLI transcript: `data/sessions/<session-id>/<session-id>.messages.json`
- CLI transcript: `sessions/<session-id>/<session-id>.messages.json`

The `data/tasks` layout is retained for compatibility with older Cline CLI
versions; current CLI documentation describes session transcripts instead.

Set `CLINE_DATA_DIR` to comma-separated roots to replace default discovery, for
example `CLINE_DATA_DIR="$HOME/.cline,/path/to/editor/globalStorage/saoudrizwan.claude-dev"`.

## Usage data

Extension usage comes only from completed `say` events named
`api_req_started`. The event must have its own valid integer millisecond `ts`,
`modelInfo.modelId`, and a JSON string in `text` with all four unsigned integer
counters: `tokensIn`, `tokensOut`, `cacheReads`, and `cacheWrites`. An explicit
`partial: true` flag, a missing/blank/`unknown` model ID, an incomplete or
malformed counter set, an all-zero usage record, or a counter total that
overflows is skipped. Individual zero counters are valid. `deleted_api_reqs`
and `subagent_usage` are aggregate records and are ignored. `task_metadata.json`
and `api_conversation_history.json` never supply model attribution, and a cost
field cannot make an otherwise incomplete record eligible.

CLI usage comes only from assistant messages with a valid timestamp, a
nonblank/non-`unknown` `modelInfo.id`, and all four unsigned integer metrics:
`inputTokens`, `outputTokens`, `cacheReadTokens`, and `cacheWriteTokens`. The
timestamp must be the message's integer-millisecond `ts` or, when `ts` is
absent, a timezone-qualified RFC 3339 `timestamp` with whole seconds or exactly
three fractional digits. An invalid present `ts` is not replaced by
`timestamp`. CLI `inputTokens` includes cached tokens, so the
adapter subtracts both cache counters using checked arithmetic; it skips
inconsistent or overflowing totals instead of clamping. All-zero usage is
skipped even when a cost is present. Extension `tokensIn` is already separate
from its cache counters. Task-format records use `task:<id>` session IDs and
CLI transcript records use `cli:<id>` so matching IDs from different formats
do not collide.

The shared cost mode controls cost output: `auto` prefers a reported cost and
calculates from historical pricing when no cost was recorded; `display` uses a
reported cost without calculating one; `calculate` ignores recorded cost and
uses pricing. A reported zero is preserved. Valid custom model IDs remain
labeled with their actual ID; reports flag them when pricing is unavailable
rather than assigning a guessed model.

## Adapter surface

- `load_entries(&SharedArgs, &PricingMap)` loads normalized usage records.
- `summarize_entries(&[LoadedEntry], AgentReportKind)` aggregates a report.
- `run(AgentCommandArgs)` runs the Cline report command.

The crate is built in the adapters Crane artifact layer with the other source
adapters.
