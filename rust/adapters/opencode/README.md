# ccusage-adapter-opencode

The OpenCode adapter: it turns the OpenCode SQLite database
into the usage entries the reports render.

## Data source

- `OPENCODE_DATA_DIR`, or `${XDG_DATA_HOME:-$HOME/.local/share}/opencode` when unset

Record shapes, token mapping, and cost rules are documented in [`src/README.md`](src/README.md).

Read failures and rejected usage records produce sanitized diagnostics while readable usage is
retained. Focused loaders print the warnings after loading; unified reports return them with the
source rows. Optional missing sources and deliberately suppressed records stay quiet. Existing
lenient field conversions are unchanged.

## Build layer

Built in the `adapters` Crane artifact layer.
