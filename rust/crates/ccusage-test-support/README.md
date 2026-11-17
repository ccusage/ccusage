# ccusage-test-support

Test-only helpers: filesystem fixtures and environment-variable guards.

## Owns

- `Fixture` and the `fs_fixture!` macro — build a temporary directory tree from a
  literal file map, so loader tests read real files instead of mocks.
- `EnvVarGuard` and `EnvVarsGuard` — set the data-directory variables an adapter
  reads and restore them when the test ends.
- `mutation::assert_parser_survives_mutations` — run a parser over every damaged
  variant of a fixture (truncated, values nulled, number and string types
  swapped) and fail when it panics, since a panic aborts the release binary.
- `install_pricing_snapshots` — hand the embedded pricing snapshots to
  `ccusage-core`, as the binary does at startup; a test that loads pricing calls
  it first.
- `instant` and `Instant` — parse fixture dates as RFC 3339 and convert them
  explicitly to epoch seconds or milliseconds, including fractional wire values.
- `set_modified` — give a fixture file the mtime of a typed `Instant`.
- `zcode::create_fixture` — create the representative ZCode SQLite schema and
  usage rows shared by adapter and unified report tests.

Every crate that has tests uses this as a dev-dependency; nothing depends on it
at runtime.

## Public surface

- `EnvVarGuard`
- `EnvVarsGuard`
- `Fixture`
- `install_pricing_snapshots`
- `instant` and `Instant`
- `set_modified`
- `mutation::assert_parser_survives_mutations`
- `zcode::create_fixture`

## Depends on

- `assert_fs`
- `ccusage-core`
- `ccusage-pricing-snapshots`
- `jiff`
- `sqlite`

## Build layer

Outside the Crane artifact layers: they build only the shipped binary's dependency graph, and this crate is a dev-dependency only, so it never reaches that binary. The test and clippy derivations compile it themselves.
