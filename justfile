mod ccusage 'apps/ccusage'
mod telemetry
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
    TZ=UTC node --test 'apps/ccusage/**/*.test.ts' 'apps/telemetry/**/*.test.ts' 'nix/tools/**/*.test.ts'

# Generate real-sized benchmark fixtures for PR performance comparisons; `--size-mib`, `--codex-size-mib`, `--grok-size-mib`, and `--opencode-size-mib` shrink each one
[positional-arguments]
generate-large-fixture output_dir codex_output_dir *args:
    output_dir="$1" codex_output_dir="$2"; shift 2; apps/ccusage/scripts/generate-large-fixture.ts --output-dir "$output_dir" --codex-output-dir "$codex_output_dir" "$@"

# Generate every fixture the CI perf job and the bench testbox measure, one directory per fixture under `root`
[positional-arguments]
[script('bash')]
generate-benchmark-fixtures root:
    set -euo pipefail
    root="$1"
    generator=apps/ccusage/scripts/generate-large-fixture.ts
    "$generator" --output-dir "$root/large" --codex-output-dir "$root/large-codex" \
        --grok-output-dir "$root/large-grok" --opencode-output-dir "$root/opencode" --opencode-size-mib 256
    "$generator" --output-dir "$root/many-files" --claude-shape many-small-files
    "$generator" --output-dir "$root/proxy" --claude-shape requestless-proxy --size-mib 1024
    "$generator" --output-dir "$root/small" --size-mib 4

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

# Check tracked code comments against the repository comment rule
comments:
    nu --no-config-file scripts/comments/check.nu

# Run every flake check (treefmt, oxlint, clippy, schema drift, gitleaks)
check: check-pricing-data check-clippy-config check-workspace-dependency-inheritance check-unsafe-code
    nix flake check

check-clippy-config:
    nu --no-config-file scripts/check-clippy-config.nu rust

check-workspace-dependency-inheritance:
    nu --no-config-file scripts/check-workspace-dependency-inheritance.nu rust

# Check every crate but ccusage-zstd with unsafe_code forbidden on the rustc command line
check-unsafe-code:
    nu --no-config-file scripts/check-unsafe-code.nu rust/Cargo.toml

[script('bash')]
check-pricing-data:
    set -euo pipefail
    names='deepseek|gemini|glm-|kimi|qwen|grok|claude|gpt-|chatgpt|codex|"o[134]"|anthropic|openai|bedrock|azure|vertex|fireworks|moonshot|zai|volcengine|openrouter'
    names="$names|google|xai|mistral|llama|minimax|doubao|alibaba|groq|copilot|droid|hermes|kilo|opencode|cline|goose|antigravity"
    removed='has_primary_price|has_base_rate_override_with_long_context|price_entry_with_zero_fallback|requires_full_vendor_id|direct_deepseek_id|matches_service_plan|literal_pricing_keys|ShorterKeys|ScheduleId|ProviderFamily'
    collect() {
        local pattern="$1"
        local scope="$2"
        if [[ "$scope" = '-' ]]; then
            rg -ni "$pattern" - | sed -E '/^[0-9]+:[[:space:]]*\/\//d' || [[ "$?" -eq 1 ]]
        else
            rg -n "$pattern" "$scope" || [[ "$?" -eq 1 ]]
        fi
    }
    failed=0
    for file in rust/crates/ccusage-core/src/pricing.rs rust/crates/ccusage-core/src/pricing/*.rs \
        rust/crates/ccusage-core/src/pricing/models_dev/*.rs \
        rust/crates/ccusage-core/src/cost.rs rust/crates/ccusage-pricing-snapshots/src/litellm_filter.rs; do
        case "$file" in */pricing/data.rs|*/pricing/builtin_pricing.rs|*/pricing/tests.rs) continue ;; esac
        matches="$(sed '/^mod tests/,$d' "$file" | collect "$names" -)"
        if [[ -n "$matches" ]]; then
            printf '%s\n' "$matches" | sed "s|^|$file:|"
            failed=1
        fi
    done
    data=rust/crates/ccusage-core/src/pricing/data.rs
    matches="$(collect '\b(fn|match|if|while|for|loop)\b' - < "$data")"
    if [[ -n "$matches" ]]; then
        printf '%s\n' "$matches" | sed "s|^|$data:|"
        failed=1
    fi
    matches="$(collect "$removed" rust)"
    if [[ -n "$matches" ]]; then
        printf '%s\n' "$matches"
        failed=1
    fi
    exit "$failed"

hawk:
    cargo hawk check --manifest-path rust/Cargo.toml -D warnings

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
    nu scripts/copy-models-dev-snapshots.nu

# Update the pinned models.dev input, regenerate its pricing snapshot, and validate
update-models-dev-pricing:
    nix flake update models-dev
    just gen-models-dev-pricing
    just check
