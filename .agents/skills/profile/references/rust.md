# Rust Profiling

## Before optimizing

https://nnethercote.github.io/perf-book/

## Measuring

Time release builds only, on CI or a testbox (see the skill's "Where to
measure"); the commands below are what runs there:

```sh
direnv exec . cargo build --manifest-path rust/Cargo.toml --release --bin ccusage
```

Build the baseline and candidate the same way in separate worktrees. Use main
as the baseline for a branch comparison and run each worktree's own
`rust/target/release/ccusage`.

Use the same environment for both measured commands:

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

## Comparing Refs And Reproducing The CI Comment

`just bench-history <ref|binary>...` builds each ref in release mode, runs every
report command over the generated realistic fixture in interleaved rounds, and
judges each ref against the previous one the same way the CI comment judges a
PR against its merge base. Run it on a `testbox-bench.yaml` box, or
`testbox-bench-macos.yaml` for macOS timing. For the final local real-log
confirmation, do not use `--real`: it also builds refs and runs the synthetic
section. Build both macOS binaries on a `testbox-bench-macos.yaml` box and fetch
them with `blacksmith testbox download`, then run them over APFS clones with
`hyperfine --export-json` (wall `mean`, and `user` + `system` for CPU) and
`/usr/bin/time -l` (peak RSS), and report the wall, CPU and peak-RSS ratios
only.

When a regression slips past the comparison, fix the fixture generator
(`apps/ccusage/scripts/generate-large-fixture.ts`) so its lines carry the shape
that triggered it; do not add a special-case benchmark.

When real histories outgrow the fixture, refresh its per-agent size profiles
from `apps/ccusage/scripts/profile-log-sizes.nu`. Commit only the file counts,
totals, and quantiles it prints, never real log content.
