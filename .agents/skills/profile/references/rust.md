# Rust Profiling

## Before Optimizing

For non-trivial work, read the Rust Performance Book chapters that apply
(`profiling`, `io`, `heap-allocations`, `parallelism`, `type-sizes`). A local
clone is faster to grep than the site; locate it instead of assuming a path:

```fish
set perf_book_dir (ghq list --full-path nnethercote/perf-book | head -n 1)
rg --files "$perf_book_dir/src"
```

https://nnethercote.github.io/perf-book/

## Measuring

Time release builds only:

```sh
direnv exec . cargo build --manifest-path rust/Cargo.toml --release --bin ccusage
```

Build main in a separate worktree so the checkout you are editing stays put
(`git wt`, or `git worktree add /tmp/ccusage-main origin/main`) and build it the
same way; the binaries land at `<worktree>/rust/target/release/ccusage`.

Every measured command carries the same prefix. Without it, progress output,
terminal width, color, and timezone all move the numbers:

```sh
env LOG_LEVEL=0 COLUMNS=200 NO_COLOR=1 TZ=UTC rust/target/release/ccusage daily --offline --json
```

Feed both binaries to one `hyperfine --warmup 4 --runs 10 --shell none` run, and
confirm the two JSON outputs match — plus table output when the change can
affect rendering — before believing a speedup.

## What Moves The Needle Here

- I/O count and buffering, before any CPU-only tweak.
- Allocation and cloning on hot paths: borrowed `&str`, `Arc<str>`, or typed
  summaries instead of owned `String`, and aggregating earlier so large
  intermediate vectors never exist.
- Parallelism only when it improves end-to-end time on real fixture shapes.
- Binary size when adding a dependency or feature (`rust-binary-size` skill).

## Reproducing The CI Performance Comment

`apps/ccusage/scripts/compare-pr-performance.bb --help` lists the options; the
`compare-pr-performance.bb` step in `.github/workflows/ci.yaml` is the reference
for a realistic argument set. Locally, point `--base-dir` at the main worktree
and `--head-dir` at `$PWD`, and cut `--runs`/`--warmup` down to keep iteration
fast.
