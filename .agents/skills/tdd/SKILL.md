---
name: tdd
description: Guides t-wada Red-Green-Refactor TDD for ccusage logic changes. Use when implementing a feature, writing a regression test for a bug, or refactoring Rust or TypeScript behavior test-first.
---

# TDD

Logic changes here — bug fixes, features, refactors — follow t-wada style
Red-Green-Refactor. For unfamiliar APIs, prototypes, and data exploration,
execute-inspect-adjust first; not every probe needs a test.

The `testing` skill owns the ccusage-specific side: fixtures, adapter coverage,
pricing, model names, snapshots, and CLI output.

## Cycle

1. Sketch the behaviors as placeholders — `it.todo(...)` in Node test, `#[ignore]` in
   Rust — then take them one at a time, simplest first. Bug fixes start with a
   regression test that reproduces the bug.
2. **Red** — write the failing test and confirm it fails for the expected reason.
3. **Green** — write the minimum production code that passes. Fake it, make it real,
   then triangulate with more tests.
4. **Refactor** — clean up test and production code while everything stays green.
   Restructure only while green; when a test is red, fix the production code first.
5. Run the affected tests after each green and each refactor step. Full-suite runs are
   for final verification and CI.

Keep each test on one observable behavior, named after that behavior and asserted
through a public interface; never weaken a valid test to get a green build. A test
that asserts a document contains certain wording freezes text rather than proving a
contract.

## Focused runs

Rust tests live in the `rust/Cargo.toml` workspace. Prefix with `direnv exec .` when
`cargo` is not already on `PATH`.

```sh
direnv exec . cargo test --manifest-path rust/Cargo.toml --workspace <name-filter>
direnv exec . cargo test --manifest-path rust/Cargo.toml --workspace -- --ignored

node --test --test-name-pattern '<name-filter>' apps/ccusage/src/cli.test.ts
```

`just test` runs both suites, `just rust::test` the Rust one, `just test-node` the
Node one.
