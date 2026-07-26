# Rust Profiling

Use this reference for native CLI performance work.

## Preparation

Read the relevant local Rust Performance Book pages before non-trivial
optimization. Locate the clone instead of assuming a machine-specific path:

```fish
set perf_book_dir (ghq list --full-path nnethercote/perf-book | head -n 1)
sed -n '1,220p' "$perf_book_dir/src/profiling.md"
sed -n '1,220p' "$perf_book_dir/src/io.md"
sed -n '1,220p' "$perf_book_dir/src/heap-allocations.md"
sed -n '1,220p' "$perf_book_dir/src/parallelism.md"
sed -n '1,220p' "$perf_book_dir/src/type-sizes.md"
```

If there is no local clone, use the hosted Rust Performance Book as fallback:
`https://nnethercote.github.io/perf-book/`.

Build release binaries before timing:

```sh
direnv exec . cargo build --manifest-path rust/Cargo.toml --release --bin ccusage
```

## Branch Comparison

Create a separate main worktree for branch-vs-main comparisons:

```sh
command git fetch origin main
command git worktree add /tmp/ccusage-main origin/main
direnv exec . cargo build --manifest-path rust/Cargo.toml --release --bin ccusage
cd /tmp/ccusage-main
direnv exec . cargo build --manifest-path rust/Cargo.toml --release --bin ccusage
```

Measure real commands with deterministic output settings:

```sh
hyperfine --warmup 4 --runs 10 --shell none \
	"env LOG_LEVEL=0 COLUMNS=200 NO_COLOR=1 TZ=UTC rust/target/release/ccusage daily --offline --json" \
	"env LOG_LEVEL=0 COLUMNS=200 NO_COLOR=1 TZ=UTC /tmp/ccusage-main/rust/target/release/ccusage daily --offline --json" \
	--export-json /tmp/rust-hyperfine.json
```

For JSON parity, write both outputs and validate with `jq`:

```sh
env LOG_LEVEL=0 COLUMNS=200 NO_COLOR=1 TZ=UTC rust/target/release/ccusage daily --offline --json >/tmp/head.json
env LOG_LEVEL=0 COLUMNS=200 NO_COLOR=1 TZ=UTC /tmp/ccusage-main/rust/target/release/ccusage daily --offline --json >/tmp/main.json
jq -e . /tmp/head.json >/dev/null
jq -e . /tmp/main.json >/dev/null
```

## What To Check

- I/O count and buffering before CPU-only tweaks.
- Avoid unnecessary `String` allocation and cloning on hot paths; prefer borrowed
  `&str`, `Arc<str>`, or typed summaries where ownership is needed.
- Avoid returning large intermediate object vectors when aggregation can happen
  earlier without changing output.
- Use parallelism only when it improves end-to-end command time on real fixture
  shapes.
- Keep binary size visible when adding dependencies or enabling features.

## PR Performance Workflow

When CI performance comments are relevant, inspect options with `--help`, but do
not treat help output as a profiling workload:

```sh
apps/ccusage/scripts/compare-pr-performance.bb --head-runtime rust --help
```

To reproduce the workflow shape locally, pass real fixture and worktree inputs:

```sh
apps/ccusage/scripts/compare-pr-performance.bb \
	--base-dir /tmp/ccusage-main \
	--head-dir "$PWD" \
	--head-runtime rust \
	--fixture-dir apps/ccusage/test/fixtures/claude \
	--codex-fixture-dir apps/ccusage/test/fixtures/codex \
	--runs 1 \
	--warmup 0 \
	--output /tmp/rust-perf-comment.md
```

Profile before committing an optimization. Validate with end-to-end `hyperfine`
and JSON/table parity, not only microbenchmarks.
