# AGENTS.md

## Repository Shape

The production CLI is Rust-first. Put source-specific runtime behavior in
`rust/adapters/<agent>` and shared behavior in `ccusage-core` or `ccusage-adapter-common`.
Keep adapter logic out of the npm package in `apps/ccusage`.

Read the nearest `AGENTS.md` before editing:

- `apps/ccusage/AGENTS.md` - published npm package
- `rust/adapters/AGENTS.md` - adapter crates
- `docs/AGENTS.md` - VitePress documentation site
