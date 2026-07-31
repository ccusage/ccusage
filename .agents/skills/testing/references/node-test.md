# Node Tests

`*.test.ts` files sit beside the code or owning tooling area. Today that is the
package launcher and the Nix-side JS tooling — `apps/ccusage/src/cli.test.ts` and
`nix/tools/models-dev-gen/compact.test.ts`. Production CLI runtime behavior is tested
in Rust, see `rust.md`.

- `just test-node` lists its test files explicitly and runs them with `TZ=UTC`, and
  the `node-test` pre-push hook in `nix/git-hooks.nix` repeats that list. A new test
  file has to be added to both or it never runs.
- Import from `node:test` and `node:assert/strict` instead of relying on globals, and
  keep imports static.
- Small tooling fixtures use built-in Node filesystem and temporary directory APIs,
  with explicit setup per test.
- Save and restore `process.env` around the test body when a test mutates it.
- Table-driven loops belong outside the test body when several cases share the same
  assertion shape.
