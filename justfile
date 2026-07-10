# Task runner — the single entry point for development and release.
#
# Each workspace package has its own justfile, imported below as a module; root
# recipes aggregate them (e.g. `typecheck` runs every package's typecheck).
# Whole-repo jobs that the Nix flake owns (formatting, checks, schema) stay here.
#
# pnpm policy: repo-global tools and pinned package tools provided by the Nix
# dev shell are called directly; package-scoped tools that are not in Nix still
# go through pnpm. `build` is delegated with `pnpm run` because npm prepack
# invokes that script by name.
#
# Run `just --list` (or `just <module>::--list`) to see everything.

mod ccusage 'apps/ccusage'
mod docs
mod rust

[private]
default:
    @just --list

# Build every workspace package
build: ccusage::build docs::build

# Type-check and lint TypeScript with oxlint's type-aware checker
typecheck:
    oxlint .

# Run the full test suite (Rust workspace + Node test) in parallel
[parallel]
test: rust::test test-node

# Run Node's built-in test runner for TypeScript package and tooling tests
test-node:
    TZ=UTC node --test apps/ccusage/src/cli.test.ts nix/models-dev-compact.test.ts

# Generate a large benchmark fixture for PR performance comparisons
generate-large-fixture output_dir codex_output_dir size_mib="1024":
    apps/ccusage/scripts/generate-large-fixture.nu --output-dir "{{output_dir}}" --codex-output-dir "{{codex_output_dir}}" --size-mib {{size_mib}}

# Format the whole tree (Nix, Rust, JS/TS, workflows, typos) via treefmt
fmt:
    nix fmt

# Run every flake check (treefmt, oxlint, clippy, schema drift, gitleaks)
check:
    nix flake check

# Regenerate apps/ccusage/config-schema.json from the Rust source
schema:
    nix run .#generate-schema

# Update the locked LiteLLM pricing snapshot and validate the result
update-litellm-pricing:
    nix flake update litellm
    just check

# Regenerate committed models.dev snapshots from the pinned input
gen-models-dev-pricing:
    snapshots="$(nix build .#models-dev-pricing --no-link --print-out-paths)" && cp "$snapshots/models-dev-pricing.json" rust/crates/ccusage/src/models-dev-pricing.json && cp "$snapshots/codex-auto-review-fallbacks.json" rust/crates/ccusage/src/adapter/codex/codex-auto-review-fallbacks.json
    chmod u+w rust/crates/ccusage/src/models-dev-pricing.json
    chmod u+w rust/crates/ccusage/src/adapter/codex/codex-auto-review-fallbacks.json
    nix fmt rust/crates/ccusage/src/models-dev-pricing.json
    nix fmt rust/crates/ccusage/src/adapter/codex/codex-auto-review-fallbacks.json

# Update the pinned models.dev input, regenerate its pricing snapshot, and validate
update-models-dev-pricing:
    nix flake update models-dev
    just gen-models-dev-pricing
    just check
