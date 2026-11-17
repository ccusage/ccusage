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
[script('bash')]
install mode="always":
    set -euo pipefail
    toolDirs=()
    lockfiles=(pnpm-lock.yaml)
    for tsconfig in nix/tools/*/tsconfig.json; do
        toolDir="$(dirname "$tsconfig")"
        toolDirs+=("$toolDir")
        lockfiles+=("$toolDir/bun.lock")
    done
    case "{{mode}}" in
        always) ;;
        if-stale)
            if [ -e node_modules/.install-stamp ]; then
                needsInstall=false
                for lockfile in "${lockfiles[@]}"; do
                    if [ "$lockfile" -nt node_modules/.install-stamp ]; then
                        needsInstall=true
                        break
                    fi
                done
                if [ "$needsInstall" = false ]; then
                    exit 0
                fi
            fi
            ;;
        *) echo "Unknown install mode: {{mode}}" >&2; exit 1 ;;
    esac
    pnpm install --frozen-lockfile
    for toolDir in "${toolDirs[@]}"; do
        (cd "$toolDir" && bun install --frozen-lockfile)
    done
    # The dev shell re-installs when a lockfile is newer than this stamp. It
    # cannot compare against node_modules itself, because an install that
    # changes nothing leaves those directory mtimes untouched.
    touch node_modules/.install-stamp

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
    TZ=UTC node --test 'apps/ccusage/**/*.test.ts' 'nix/tools/**/*.test.ts'

# Generate a large benchmark fixture for PR performance comparisons
generate-large-fixture output_dir codex_output_dir size_mib="1024":
    apps/ccusage/scripts/generate-large-fixture.ts --output-dir "{{output_dir}}" --codex-output-dir "{{codex_output_dir}}" --size-mib {{size_mib}}

# Build git refs (or take binary paths) and compare their report performance; `--real` adds local logs
[positional-arguments]
bench-history *args:
    apps/ccusage/scripts/bench-history.bb "$@"

# Build two git refs (or take binary paths) and prove they print the same reports over the golden seeds; `--real` reads local logs
[positional-arguments]
diff-refs *args:
    apps/ccusage/scripts/diff-refs.bb "$@"

# Format the whole tree (Nix, Rust, JS/TS, workflows, typos) via treefmt
fmt:
    nix fmt

# Run every flake check (treefmt, oxlint, clippy, schema drift, gitleaks)
check:
    nix flake check

# Regenerate apps/ccusage/config-schema.json from the Rust source
schema:
    nix run .#generate-schema

# Re-resolve the Nix-built JS tools under nix/tools and regenerate their bun.nix
[script('bash')]
gen-bun-nix:
    set -euo pipefail
    for lockfile in nix/tools/*/bun.lock; do
        toolDir="$(dirname "$lockfile")"
        echo "Regenerating $toolDir"
        (cd "$toolDir" && bun install)
    done
    nix run .#generate-bun-nix

# Update the locked LiteLLM pricing snapshot and validate the result
update-litellm-pricing:
    .github/scripts/update-litellm-lock.nu
    just check

# Regenerate committed models.dev snapshots from the pinned input
gen-models-dev-pricing:
    snapshots="$(nix build .#models-dev-pricing --no-link --print-out-paths)" && cp "$snapshots/models-dev-pricing.json" rust/crates/ccusage-pricing-snapshots/src/models-dev-pricing.json && cp "$snapshots/models-dev-catalog-rules.json" rust/crates/ccusage-pricing-snapshots/src/models-dev-catalog-rules.json
    chmod u+w rust/crates/ccusage-pricing-snapshots/src/models-dev-pricing.json
    chmod u+w rust/crates/ccusage-pricing-snapshots/src/models-dev-catalog-rules.json
    nix fmt rust/crates/ccusage-pricing-snapshots/src/models-dev-pricing.json
    nix fmt rust/crates/ccusage-pricing-snapshots/src/models-dev-catalog-rules.json

# Update the pinned models.dev input, regenerate its pricing snapshot, and validate
update-models-dev-pricing:
    nix flake update models-dev
    just gen-models-dev-pricing
    just check
