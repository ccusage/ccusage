---
name: testing
description: Guides ccusage Rust and Node tests. Use when adding or fixing cargo tests, Node test files, CLI snapshots, Claude model pricing, LiteLLM compatibility, or fixture-backed tests.
---

# ccusage Testing

The `tdd` skill owns the Red-Green-Refactor loop and the focused runner commands.

- Prefer behavior-focused tests over schema-shape tests, unless schema normalization
  itself is the behavior under test.
- Branching behavior belongs in separate tests or table-driven cases rather than an
  `if` inside a test body.
- Skipped local-data smoke tests are acceptable when real user log directories catch
  schema drift, as long as they pass on clean CI machines.

## Rust

Unit tests sit next to the module they exercise in `#[cfg(test)] mod tests`. When a
large module is split, its tests move with the code instead of staying in `main.rs`.

Read [Rust tests](references/rust.md) for fixtures, snapshots, pricing, and model names.

## Node

Read [Node tests](references/node-test.md) for launcher and JS tooling tests.
Test CLI runtime behavior in Rust.

## Telemetry Worker

`telemetry/` tests run with Vitest, never `node:test`. Write them in-source in an
`if (import.meta.vitest)` block of the module they cover, using the Vitest globals,
with no separate `*.test.ts` files. Keep tests out of `routes/` and `pages/`: put the
handler in `src/` and re-export it from the route file. Write test values inline
instead of building them with helper functions.
