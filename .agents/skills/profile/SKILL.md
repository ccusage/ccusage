---
name: profile
description: Profiles ccusage performance. Use when a CLI command is slow, when checking that an optimization actually helped, when comparing a branch against main, or when reading a captured CPU profile.
---

# ccusage Profile

Read [Rust profiling](references/rust.md) for release builds, ref comparisons,
output parity, and the CI measurements.

## Where to measure

In this order:

1. CI: open a PR to get the `ccusage perf` comment (it runs only on pull
   requests from this repository, not forks), and dispatch `binary-size.yaml` or `profile-instructions.yaml`
   with `gh workflow run ... -f refs="<base> <branch>"` for sizes and
   instruction counts on any pushed branch.
2. A Blacksmith testbox (`blacksmith-testbox` skill): `testbox-bench.yaml`,
   `testbox-bench-macos.yaml`, `testbox-dev.yaml`, one box per lever or PR.
3. Locally only for quick, focused runs.

Never queue long benchmarks or builds on one local machine. Real-log
measurement is the one exception that stays local: run it once, at the end,
on APFS clones, and report ratios only, because the owner's logs never leave
their machine. A real-log ratio that disagrees with the fixture ratio means the
fixture is missing a shape; add it to the generator.

## Checks

Measure end-to-end on generated fixtures. Verify `daily`,
`weekly`, `monthly`, `session`, and `blocks` JSON after changing aggregation
order. Record measured hotspots left alone in the commit message.

## Node paths

Profile launcher startup with `node --cpu-prof`, using a `--cpu-prof-dir`
outside the worktree. Run other tools with their pinned interpreter and use
its profiler.
