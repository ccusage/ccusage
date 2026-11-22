---
name: rust
description: Guides ccusage Rust implementation work. Use when editing rust/crates or rust/adapters, adapter module layout, crate visibility and Hawk findings, embedded pricing, or Rust behavior parity.
paths:
  - 'rust/**/*.rs'
  - 'rust/**/*.toml'
  - 'rust/**/build.rs'
globs: 'rust/**/*.rs,rust/**/*.toml,rust/**/build.rs'
---

# ccusage Rust

Read the crate README for the Crane build boundary. For adapter changes, follow
`rust/adapters/README.md` and `rust/adapters/AGENTS.md`. Put plain argument types
in `ccusage-cli` and parsing and help generation in `ccusage-cli-parser`.

Preserve reports, JSON, table layout, progress text, date windows, pricing
behavior, and source discovery unless the user requests a change. For parity
with a historical release, compare against the matching tag or commit.

## Visibility

Reserve `pub` for consumers in other crates. `just hawk` fails on every Hawk
finding, and `nix flake check` runs the same gate. Every binary the repository
ships or runs in a check has a `[[production]]` entry in `rust/hawk.toml`; add a
missing one first. Then fix `unnecessary_public` by narrowing the item, never by
editing `rust/hawk.toml` to silence it. Update the Hawk
archive pins in `nix/cargo-hawk.nix` with the compiler in `rust-toolchain.toml`.

## Pricing embedding

Keep snapshot bytes in `ccusage-pricing-snapshots` and filtering and compaction
at build time. Use the build and regeneration commands in that crate's README.
Keep `fetch-litellm-pricing` off by default. Maintain
`rust/crates/ccusage-core/src/fast-multiplier-overrides.json` by hand.

Use the `testing` skill for fixture and pricing checks, `profile` for performance
comparisons, and `development` for workspace validation.
