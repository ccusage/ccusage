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

Extension usage comes from `say` events named `api_req_started`,
`deleted_api_reqs`, and `subagent_usage`, with counters encoded in the event's
JSON `text`. Request events use their `modelInfo` when present; otherwise the
adapter uses the latest timestamped `task_metadata.json` `model_usage` entry at
or before the request. It does not assign a task-wide model from
`api_conversation_history.json`, since that can misattribute requests after a
model change. Aggregate deletion and subagent events are retained without
speculative model attribution and use the roll-up event timestamp for dating.

CLI usage comes from assistant messages with `modelInfo` and `metrics`. The CLI
`inputTokens` counter includes cached tokens, so the adapter subtracts
`cacheReadTokens` and `cacheWriteTokens` (clamped at zero) before reporting
input tokens. Extension `tokensIn` is already separate from its cache counters.
Task-format records use `task:<id>` session IDs and CLI transcript records use
`cli:<id>` so matching IDs from different formats do not collide.

The shared cost mode controls cost output: `auto` prefers a reported cost and
calculates from historical pricing when no cost was recorded; `display` uses a
reported cost without calculating one; `calculate` ignores recorded cost and
uses pricing. A reported zero is preserved. Unknown models remain labeled
`unknown`, and reports flag unpriced usage rather than assigning a guessed
model.

## Adapter surface

- `load_entries(&SharedArgs, &PricingMap)` loads normalized usage records.
- `summarize_entries(&[LoadedEntry], AgentReportKind)` aggregates a report.
- `run(AgentCommandArgs)` runs the Cline report command.

The crate is built in the adapters Crane artifact layer with the other source
adapters.
