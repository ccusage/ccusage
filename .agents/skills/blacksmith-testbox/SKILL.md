---
name: blacksmith-testbox
description: Runs local changes on a Blacksmith testbox. Use for Linux builds and static binary size, Linux-only tests, or Linux or macOS fixture benchmarks too long for a local machine.
---

# Blacksmith Testbox

## CLI

- Use the dev-shell `blacksmith` (`nix develop` or direnv); outside the shell,
  `nix run .#blacksmith -- <args>`. It is the pinned package in
  `nix/tools/blacksmith`.
- Never install it with `curl | sh`, a release channel, or a global install.
- Auth is the owner's browser login, `blacksmith auth login`, run by the owner.
  Agents never handle tokens or read `~/.blacksmith/credentials`.

## Choosing where to run

- `testbox-dev.yaml`: Linux builds, `nix build .#ccusage-static` size
  checks, Linux-only tests.
- `testbox-bench.yaml`: CI-like timing and instruction counts. Generated fixtures are
  under `$CCUSAGE_FIXTURES` on the box; start the remote command string with
  `source ~/ccusage-fixtures.env &&`, never source it locally.
- `testbox-bench-macos.yaml`: the same fixtures on a macOS runner, for macOS
  timing. Absolute times differ from any laptop; compare only ratios of two
  binaries on the same box, against a byte-copy control.
- Every fixture benchmark, build and size check runs on Blacksmith (testboxes,
  `binary-size.yaml`, `profile-instructions.yaml`), one box per lever or PR, so
  measurements run in parallel instead of queuing on a local machine.
- The owner's MacBook: only the final real-log confirmation, because real logs
  must never leave that machine.
- Local `just fmt`, `just typecheck`, and `just test` stay fine; a testbox is
  not required for them.

## Session

- Run every `blacksmith` command from the repository root.
- Start with `blacksmith testbox warmup testbox-<dev|bench|bench-macos>.yaml`; one workflow
  per box, because every job in a dispatched workflow starts a runner. Wait
  with `blacksmith testbox status --id <id> --wait`, never a sleep loop.
- Run commands as `blacksmith testbox run --id <id> 'nix develop --command <cmd>'`,
  single-quoted so `$CCUSAGE_FIXTURES` and other variables expand on the box,
  not locally; fetch results with `blacksmith testbox download`.
- Keep `--idle-timeout` at or below 60 minutes. Both workflows stop at their
  240-minute cap, warmup included, so for longer work stop the box and warm up a
  new one.
- Always `blacksmith testbox stop --id <id>` when done, including after failures.
- Warmup works only once the workflow is on the default branch; `--ref`
  selects the checkout the setup steps run on.

## Data

- `run` syncs tracked and untracked non-ignored files. Before a run, make sure
  no real agent logs or other private data sit in the worktree.
- Never sync, upload, or generate real agent logs on a testbox; use the
  generated fixtures only.
