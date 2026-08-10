---
name: testing
description: Guides ccusage Rust and Node tests. Use when adding or fixing cargo tests, Node test files, CLI snapshots, Claude model pricing, LiteLLM compatibility, or fixture-backed tests.
---

# ccusage Testing

The `tdd` skill owns the Red-Green-Refactor loop and the focused runner commands.
This skill owns what is specific to ccusage: fixtures, adapter coverage, pricing and
model behavior, snapshots, CLI output, and package tooling.

- Prefer behavior-focused tests over schema-shape tests, unless schema normalization
  itself is the behavior under test.
- Branching behavior belongs in separate tests or table-driven cases rather than an
  `if` inside a test body.
- Skipped local-data smoke tests are acceptable when real user log directories catch
  schema drift, as long as they pass on clean CI machines.

## Rust

Unit tests sit next to the module they exercise in `#[cfg(test)] mod tests`. When a
large module is split, its tests move with the code instead of staying in `main.rs`.

Read `references/rust.md` for fixtures, snapshots, pricing, and model names.

## Node

Read `references/node-test.md`. Node covers only the package launcher and the Nix-side
JS tooling; production CLI runtime behavior is tested in Rust.
