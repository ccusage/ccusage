# Claude Science Data Source

ccusage reads per-frame token usage from the [Claude Science](https://claude.com/science)
local metadata database and exposes the same focused and unified reports as the other
supported sources.

## Focused Views

```bash
ccusage claude-science daily
ccusage claude-science monthly
ccusage claude-science session
```

Use `ccusage daily`, `ccusage monthly`, or `ccusage session` when Claude Science should
be included with every other detected source.

## Data Source

Claude Science stores conversation metadata — including aggregate token usage per
conversation frame — in a local SQLite database. The adapter looks for it in the
well-known roots below and in the daemon's org layout:

```text
~/.claude-science/cs-switch-proxy/orgs/<org>/operon-cli.db
```

Set `CLAUDE_SCIENCE_DB` to one database path or a comma-separated list of paths when
the database lives elsewhere. The override is exclusive: when it is set, the
well-known locations are not scanned.

A database is read only when it exposes a `frames` table with the columns the loader
needs; every other SQLite file found during discovery is skipped.

## Record Shape

- One `frames` row = one conversation frame (a root conversation or a delegated
  sub-agent turn). Token counts are aggregates for the whole frame.
- `root_frame_id` maps to the ccusage session id, so sub-agent frames roll up into
  their parent session in session reports.
- Model names may carry a routing prefix such as `cs-switch-direct:`; the prefix is
  stripped before pricing lookups.
- Under `--cost auto` the platform's own recorded cost (`frames.total_cost`) is used
  whenever it is non-NULL; the pricing catalog is consulted only for frames without a
  recorded cost. `--cost display` always reports the recorded cost.

## Caveats

Claude Science's database is an internal format that may change without notice. The
adapter reads it with `SELECT`-only, read-only connections and skips databases whose
schema does not match, so a format change degrades to empty reports rather than
errors.
