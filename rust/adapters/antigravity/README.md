# ccusage-adapter-antigravity

The Antigravity adapter turns local Antigravity SQLite conversation databases
into the usage entries the reports render. It is a separate source boundary
from the Gemini CLI adapter so unified reports preserve source attribution.

## Data source

The adapter reads `.db` files below these default roots:

- `~/.gemini/antigravity/conversations/`
- `~/.gemini/antigravity-cli/conversations/`
- `~/.gemini/antigravity-ide/conversations/`
- `~/.gemini/antigravity-backup/conversations/`
- `~/.gemini/antigravity-acp/conversations/`
- `~/.config/antigravity/conversations/`

`ANTIGRAVITY_DATA_DIR` accepts one or more comma-separated data roots. Each
root may contain a `conversations/` child or be the conversation directory
itself. Databases are opened read-only; a conversation database provides the
`gen_metadata` table. When present, the real `steps` schema contributes step
and retry usage, and `trajectory_metadata_blob` supplies the timestamp
fallback. Empty databases and databases with none of those tables (such as
`conversation_summaries.db`) are skipped silently (logged with `--debug`); a
database with `steps` or `trajectory_metadata_blob` but no `gen_metadata`, and
unreadable databases, are skipped with a filename warning. A database only contributes usage when its
full read succeeds. Other readable databases remain available to the report.

Usage rows use generation, step, linked-record, or trajectory timestamps. Rows
without any of these are skipped with an invalid-timestamp diagnostic; the
database's modification time does not date individual records.

## Build layer

Built in the `adapters` Crane artifact layer with the other per-source
adapters.
