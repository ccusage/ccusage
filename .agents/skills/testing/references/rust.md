# Rust Tests

Fixture-backed tests cover parsing, path discovery, SQLite loading, dedupe,
aggregation, pricing, and CLI output parity.

## Fixtures

Tests needing temporary files or environment variables use the internal
`ccusage-test-support` crate rather than hand-rolled `env::temp_dir()` paths:

- `fs_fixture!` builds an inline tree; `Fixture::new()` exposes `create_dir_all`,
  `write_file`, and `path` for incremental setup.
- `EnvVarGuard::set` and `EnvVarsGuard::set_many` take a process-wide lock and
  restore the previous value on drop, which is what stops env-dependent tests from
  racing each other under `cargo test`.

A fixture owns an `assert_fs::TempDir` and deletes it when the fixture variable
drops, so keep that variable alive as long as paths under it are used — returning
only a `PathBuf` out of an inner scope loses the directory.

Working examples: `rust/adapters/codex/src/loader.rs`,
`rust/adapters/gemini/src/paths.rs`.

## Snapshots

`insta` (with the `json` feature) covers CLI help, parse shapes, report JSON, and
table rendering — see `rust/crates/ccusage-cli-parser/src/tests.rs` and
`rust/adapters/codex/src/lib.rs`. Committed `.snap` files live in a `snapshots/`
directory beside the module. `cargo-insta` is in the dev shell, so review pending
snapshots with `cargo insta review` and read the diff before accepting, rather than
hand-editing `.snap` files.

Prefer JSON assertions for structured behavior, and snapshots or explicit layout
assertions for human-readable tables, so table layout and responsive behavior stay
reviewable for each affected agent/report combination.

## Readability

For expected failures, use `Result` tests, `matches!`, or explicit error assertions.
The workspace has no test-case crate, so cover table-driven cases with explicit case
structs or a small local macro for repeated assertions.

Doc tests run under `cargo test`. Use them for public helpers where the example
doubles as documentation, not for broad CLI behavior.

## Pricing And Model Names

Pricing resolves against two embedded snapshots: the LiteLLM one generated into
`OUT_DIR` at build time, and the committed `models-dev-pricing.json`.
`PricingMap::load_embedded()` reads only those and touches no network, so tests use it;
`load_with_overrides(offline: false, ..)` is the path that fetches. Lookup is not plain
equality — alias and separator handling means the tests in
`rust/crates/ccusage-core/src/pricing.rs` are the reference for what a new model name
should resolve to. A pricing failure usually means the upstream snapshot moved or the
name is genuinely unsupported.

Use current Claude 4 names, `claude-{model-type}-{generation}-{date}`:

```text
claude-sonnet-4-20250514
claude-opus-4-20250514
```

Include both Sonnet and Opus when model coverage matters. Alias forms such as
`claude-4-sonnet-*` and Claude 3 names belong in tests that specifically cover
pricing lookup, alias handling, or legacy compatibility.
