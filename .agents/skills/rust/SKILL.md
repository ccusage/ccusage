---
name: rust
description: Guides ccusage Rust implementation work. Use when editing rust/crates, native packaging, parser/module layout, pricing embedding, or Rust/TypeScript parity.
paths:
  - 'rust/**/*.rs'
  - 'rust/**/*.toml'
  - 'rust/**/build.rs'
globs: 'rust/**/*.rs,rust/**/*.toml,rust/**/build.rs'
---

# ccusage Rust

Use this skill for the native Rust CLI: `rust/adapters/<agent>` holds one crate per
usage source, `rust/crates` everything that is not tied to a single source. Every crate has a
`README.md` stating what it owns and which Crane artifact layer it lands in; read
the one for the crate you are about to touch, because that layer determines how
much a change to it costs:

- `ccusage` - the binary; thin dispatch only.
- `ccusage-core` - pricing, cost, report shaping, config, dates, progress.
- `ccusage-cli` - the plain argument types; `ccusage-cli-parser` - the parser,
  help renderer, and embedded help JSON, which only the binary depends on.
- `ccusage-adapter-common` - shared file discovery, parallel reads, and the shared
  agent table; `adapters/<agent>` - one crate per source;
  `ccusage-adapter-all` - the unified report.
- `ccusage-terminal` - table and color primitives; `ccusage-test-support` -
  fixtures and environment guards.

## Source Parity

Rust is the production implementation. Preserve existing Rust behavior unless
the user explicitly scopes a behavior change. Before implementing or refactoring
an agent, inspect the current Rust adapter and the agent source reference docs:

```sh
fd . rust/adapters/<agent>
sed -n '1,220p' rust/adapters/<agent>/src/README.md
```

When porting behavior from the historical TypeScript implementation, first find
the relevant commit or tag that still contains `apps/ccusage/src/adapter`, then
compare against that source. Do not assume `origin/main` still contains the
TypeScript adapter.

Preserve report semantics, JSON fields, table columns, progress/spinner text, agent grouping, date filtering, `--offline`, `CLAUDE_CONFIG_DIR`, and source-specific environment variables.

## Module Layout

Do not keep growing `main.rs` or single large adapter files. Use these
responsibility boundaries where practical:

- `adapters/<agent>/src/lib.rs` - public adapter surface and command wiring.
- `adapters/<agent>/src/paths.rs` - environment variables, defaults, and path discovery.
- `adapters/<agent>/src/parser.rs` - raw record parsing and token/model mapping.
- `adapters/<agent>/src/loader.rs` - file walking, SQLite reads, dedupe, and date filtering entry points.
- `adapters/<agent>/src/report.rs` - JSON/table row shaping when agent-specific.
- shared modules stay in `ccusage-core` (`types.rs`, `summary.rs`, `output.rs`, `pricing.rs`, `progress.rs`, `date_utils.rs`) or in `ccusage-adapter-common` when they are about reading files or rendering the shared agent table.
- do not add a dependency from one adapter to another; move the shared part into `ccusage-adapter-common` instead.

Prefer moving tests with the code they exercise instead of leaving all Rust tests in `main.rs`.

When splitting large Rust modules or removing duplication, use the `reduce-similarities` skill, which runs `similarity-rs` for `.rs` files.

## Visibility

Inside this workspace `pub` is only for what another crate actually uses.
Everything else is `pub(crate)`, including items that other modules in the same
crate reach through a module chain.

`just hawk` runs [hawk](https://github.com/astral-sh/hawk) over the workspace and
reports `pub` items nothing outside their crate needs. Run it after adding a
crate and after widening a visibility. `rust/hawk.toml` lists the shipped entry
points; anything not reachable from them can be narrowed. Adding `--fix` to the
underlying `cargo hawk check` applies the narrowing.

hawk is not in nixpkgs, so the dev shell does not provide it. Install a prebuilt
release first:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/astral-sh/hawk/releases/latest/download/cargo-hawk-installer.sh | sh
```

It uses `rustc_private` and only runs on the toolchain it was built against,
which is why `rust-toolchain.toml` pins 1.97.1. It is experimental - its README
says it is "not intended for public consumption" - and no CI check runs it, so
read its findings as suggestions and confirm each one before narrowing.

## Pricing Embedding

TypeScript uses build/macro-time pricing snapshots. Rust should not rely on a manually edited `claude-pricing.json` as the only embedded source.

When changing pricing:

- Use the `litellm` flake input as the canonical pinned pricing revision for
  embedded pricing.
- For Nix builds, pass the locked LiteLLM `model_prices_and_context_window.json`
  to `build.rs` through `CCUSAGE_PRICING_JSON_PATH`.
- For non-Nix Cargo builds, have `build.rs` read the same `litellm` revision from
  `flake.lock` and fetch that pinned raw JSON at build time. That download lives
  behind the off-by-default `fetch-litellm-pricing` feature, because its rustls
  stack is the most expensive build-dependency in the workspace: the dev shell
  and every Nix package set `CCUSAGE_PRICING_JSON_PATH` instead, and only the
  Windows release build enables the feature.
- Do not check generated LiteLLM pricing snapshots into the repository.
- Keep pricing JSON filtering and compacting in `build.rs` so runtime code loads
  the generated build-time snapshot first, then built-in model overrides, then
  runtime fetch when not `--offline`.
- Add tests for embedded/offline pricing and context limits.

## Validation

Use the `testing` skill for Rust test commands. Use
`profile` for performance work and branch-vs-main comparisons. For
parity work, compare against the current main branch, a previous release, or a
pinned historical TypeScript commit for a stable fixture window before changing
behavior.
