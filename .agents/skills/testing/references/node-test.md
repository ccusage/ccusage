# Node Tests

`*.test.ts` files sit beside the code or owning tooling area. Production CLI
runtime behavior is tested in Rust, see `rust.md`.

- `just test-node` owns test discovery and runs with `TZ=UTC`; the pre-push hook
  calls that recipe.
- Import from `node:test` and `node:assert/strict` instead of relying on globals, and
  keep imports static.
- Small tooling fixtures use built-in Node filesystem and temporary directory APIs,
  with explicit setup per test.
- Save and restore `process.env` around the test body when a test mutates it.
- Table-driven loops belong outside the test body when several cases share the same
  assertion shape.
