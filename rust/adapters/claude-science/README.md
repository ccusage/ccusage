# Claude Science adapter

Reads token usage from the [Claude Science](https://claude.com/science)
desktop app's local SQLite metadata database.

## Data location

Claude Science stores one metadata database per machine. The adapter looks for
it under the user's home directory in the following roots:

- `.claude-science`
- `.config/claude-science` / `.config/Claude Science`
- `.local/share/claude-science` / `.local/share/Claude Science`
- `Library/Application Support/Claude Science` (macOS)

Set `CLAUDE_SCIENCE_DB=/path/to/metadata.db` to point the adapter at an
explicit database file (multiple paths may be given, comma-separated).

A database is treated as a Claude Science database when it contains a `frames`
table with an `input_tokens` column; other SQLite files found during discovery
are ignored.

## Record shape and semantics

- One row in `frames` = one conversation frame (a root conversation or a
  delegated sub-agent turn). Token counts are aggregates for the whole frame,
  not per-message records.
- `input_tokens` / `output_tokens` / `cache_read_tokens` / `cache_write_tokens`
  map directly to ccusage token columns.
- `root_frame_id` (falling back to `id`) maps to the ccusage session id, so
  sub-agent frames roll up into their parent session.
- `projects.name` (when a `projects` table exists) is used as the project
  label; otherwise entries are labeled `claude-science`.
- Model names may carry a routing prefix such as `cs-switch-direct:`; the
  adapter strips everything before the first colon before pricing lookups.
- `frames.total_cost` (the platform's own cost estimate) is preferred under
  `--cost auto` whenever it is non-NULL; the pricing catalog is consulted only
  for frames with a NULL recorded cost. `--cost display` always reports the
  recorded cost.
- Timestamps are stored as epoch milliseconds; `updated_at` is the record
  timestamp.

## Caveats

- The Claude Science database is an private internal format that may change in
  any release. The adapter reads it with `SELECT` queries only, in read-only
  mode, and fails soft (skips non-matching databases) when the schema drifts.
- Frames whose token counts are `NULL` (for example upload frames) are
  skipped.
