---
name: profile
description: Profiles ccusage performance. Use when a CLI command is slow, when checking that an optimization actually helped, when comparing a branch against main, or when reading a captured CPU profile.
---

# ccusage Profile

Nearly all runtime cost lives in the Rust binary. `references/rust.md` covers
release builds, the branch-vs-main worktree setup, deterministic measurement,
JSON parity, and reproducing the CI performance comment.

Two habits carry across every profiling session here:

- Measure end-to-end on real logs before trusting a profile. The wins in this
  repository came from timing the real CLI over real Claude data; a
  microbenchmark answers an isolated question without predicting a CLI win.
- Re-verify `daily`, `weekly`, `monthly`, `session`, and `blocks` JSON after
  touching aggregation order, and record in the commit message any hotspot that
  was measured and deliberately left alone.

## Node Paths

`apps/ccusage/src/cli.js` resolves and spawns the platform binary, so the
JavaScript side is a startup-latency question rather than a throughput one — a
profile of it covers launcher startup, not the work the binary does:

```sh
LOG_LEVEL=0 node --cpu-prof --cpu-prof-dir /tmp/ccusage-profiles apps/ccusage/src/cli.js daily --offline --json
```

Nothing else here runs on `node`. The tools under `nix/tools/` are built with
`bunCli`/`bunNodeModules` and run under Bun; scripts in `apps/ccusage/scripts/`
carry `nix shell` shebangs that pick nushell, babashka, or bun. Time those
scripts directly and profile with their own runtime's tooling rather than
`NODE_OPTIONS`.

Point `--cpu-prof-dir` outside the worktree — nothing here ignores a `profiles/`
directory.
