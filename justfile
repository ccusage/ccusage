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

# Install the JS dependencies needed to work in this repository
install:
    #!/usr/bin/env bash
    set -euo pipefail
    pnpm install --frozen-lockfile
    # Tool directories under nix/tools are outside the pnpm workspace. Nix builds
    # their dependencies itself, so only the ones carrying a tsconfig need a local
    # node_modules — `just typecheck` resolves their types out of it.
    for tsconfig in nix/tools/*/tsconfig.json; do
        toolDir="$(dirname "$tsconfig")"
        (cd "$toolDir" && bun install --frozen-lockfile)
    done

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
    TZ=UTC node --test apps/ccusage/src/cli.test.ts nix/tools/models-dev-gen/compact.test.ts

# Generate a large benchmark fixture for PR performance comparisons
generate-large-fixture output_dir codex_output_dir size_mib="1024":
    apps/ccusage/scripts/generate-large-fixture.ts --output-dir "{{output_dir}}" --codex-output-dir "{{codex_output_dir}}" --size-mib {{size_mib}}

# Format the whole tree (Nix, Rust, JS/TS, workflows, typos) via treefmt
fmt:
    nix fmt

# Run every flake check (treefmt, oxlint, clippy, schema drift, gitleaks)
check:
    nix flake check

# Report `pub` items in the Rust workspace that no other crate needs (needs cargo-hawk)
hawk:
    cargo hawk check --manifest-path rust/Cargo.toml

# Regenerate apps/ccusage/config-schema.json from the Rust source
schema:
    nix run .#generate-schema

# Re-resolve the Nix-built JS tools under nix/tools and regenerate their bun.nix
gen-bun-nix:
    #!/usr/bin/env bash
    set -euo pipefail
    for lockfile in nix/tools/*/bun.lock; do
        toolDir="$(dirname "$lockfile")"
        echo "Regenerating $toolDir"
        (cd "$toolDir" && bun install && bun2nix -o bun.nix)
    done

# Update the locked LiteLLM pricing snapshot and validate the result
update-litellm-pricing:
    nix flake update litellm
    just check

# Regenerate committed models.dev snapshots from the pinned input
gen-models-dev-pricing:
    snapshots="$(nix build .#models-dev-pricing --no-link --print-out-paths)" && cp "$snapshots/models-dev-pricing.json" rust/crates/ccusage-core/src/models-dev-pricing.json && cp "$snapshots/codex-auto-review-fallbacks.json" rust/adapters/codex/src/codex-auto-review-fallbacks.json
    chmod u+w rust/crates/ccusage-core/src/models-dev-pricing.json
    chmod u+w rust/adapters/codex/src/codex-auto-review-fallbacks.json
    nix fmt rust/crates/ccusage-core/src/models-dev-pricing.json
    nix fmt rust/adapters/codex/src/codex-auto-review-fallbacks.json

# Update the pinned models.dev input, regenerate its pricing snapshot, and validate
update-models-dev-pricing:
    nix flake update models-dev
    just gen-models-dev-pricing
    just check
