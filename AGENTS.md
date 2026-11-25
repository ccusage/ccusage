# AGENTS.md

## Repository Shape

The production CLI is Rust-first. Put source-specific runtime behavior in
`rust/adapters/<agent>` and shared behavior in `ccusage-core` or `ccusage-adapter-common`.
Keep adapter logic out of the npm package in `apps/ccusage`.

`telemetry/` collects daily usage pings from `rust/crates/ccusage/src/telemetry.rs`.
The payload is defined in `rust/crates/ccusage-telemetry`; regenerate the Worker schema
with `just telemetry::generate-schema` instead of editing it by hand.

Read the nearest `AGENTS.md` before editing:

- `apps/ccusage/AGENTS.md` - published npm package
- `rust/adapters/AGENTS.md` - adapter crates
- `docs/AGENTS.md` - VitePress documentation site

## Credit

When a change reuses someone else's commit, branch, fork, or PR, including one
linked from an issue, credit them with a `Co-authored-by:` trailer as the
`commit` skill describes.
