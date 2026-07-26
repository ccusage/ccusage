# ccusage-test-support

Test-only helpers: filesystem fixtures and environment-variable guards.

## Owns

- `Fixture` and the `fs_fixture!` macro — build a temporary directory tree from a
  literal file map, so loader tests read real files instead of mocks.
- `EnvVarGuard` and `EnvVarsGuard` — set the data-directory variables an adapter
  reads and restore them when the test ends.

Every crate that has tests uses this as a dev-dependency; nothing depends on it
at runtime.

## Public surface

- `EnvVarGuard`
- `EnvVarsGuard`
- `Fixture`

## Depends on

- `assert_fs`

## Build layer

Built in the `foundation` Crane artifact layer, so a change here recompiles every adapter., so test fixtures compile once for the whole workspace.
