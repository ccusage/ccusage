---
name: profile
description: Profiles ccusage performance. Use when a CLI command is slow, when checking that an optimization actually helped, when comparing a branch against main, or when reading a captured CPU profile.
---

# ccusage Profile

Read [Rust profiling](references/rust.md) for release builds, ref comparisons,
output parity, and the CI measurements.

Measure end-to-end on real logs or equivalent fixtures. Verify `daily`,
`weekly`, `monthly`, `session`, and `blocks` JSON after changing aggregation
order. Record measured hotspots left alone in the commit message.

## Node paths

Profile launcher startup with `node --cpu-prof`, using a `--cpu-prof-dir`
outside the worktree. Run other tools with their pinned interpreter and use
its profiler.
