---
name: blacksmith-testbox
description: Runs local changes on a Blacksmith testbox. Use for Linux builds, Linux static binary size, Linux-only tests, or CI-like hyperfine and callgrind comparisons that a local machine cannot give.
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

- `dev` job of `testbox.yaml`: Linux builds, `nix build .#ccusage-static` size
  checks, Linux-only tests.
- `bench` job: CI-like timing and instruction counts. Generated fixtures are
  under `$CCUSAGE_FIXTURES`; `source ~/ccusage-fixtures.env` first.
- The owner's MacBook: timing that should reflect the owner's machine.
- Local `just fmt`, `just typecheck`, and `just test` stay fine; a testbox is
  not required for them.

## Session

- Run every `blacksmith` command from the repository root.
- Start with `blacksmith testbox warmup testbox.yaml --job <dev|bench>`; wait
  with `blacksmith testbox status --id <id> --wait`, never a sleep loop.
- Run commands as `blacksmith testbox run --id <id> "nix develop --command <cmd>"`;
  fetch results with `blacksmith testbox download`.
- Always `blacksmith testbox stop --id <id>` when done, including after failures.
- Warmup works only once `testbox.yaml` is on the default branch; `--ref`
  selects the checkout the setup steps run on.

## Data

- `run` syncs tracked and untracked non-ignored files. Before a run, make sure
  no real agent logs or other private data sit in the worktree.
- Never sync, upload, or generate real agent logs on a testbox; use the
  generated fixtures only.
