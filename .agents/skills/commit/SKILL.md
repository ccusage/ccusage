---
name: commit
description: Creates atomic Conventional Commits. Use when committing code changes, splitting a diff into independently revertable hunks, staging precise patches non-interactively, or writing commit messages.
---

<!--
Example prompts:
  /commit
  /commit push=true
-->

# Commit

Arguments:

- `push`: push after committing (default: `false`).

## Workflow

1. Read the state and recent history — `git status --short`, `git diff HEAD`, `git log --oneline -10` — and match the granularity, scopes, and explanation style already in the log.
2. Split the diff into independently revertable units, hunk by hunk rather than file by file.
3. Stage each unit with `git apply --cached -v <patch>`. `git add -p` and `git add --interactive` hang in this environment, so a patch is the only way to stage part of a file. Read `references/git-apply.md` when one fails to apply.
4. Commit, then confirm with `git show HEAD`.

## Revertability

Every commit answers "if I revert this alone, does anything else break?". Tiny commits are expected: one review comment, one wording correction, one reference-file extraction.

Tiny is not partial. A move, rename, or extraction lands as a single commit holding both sides — old path removed, new path added, references updated, generated links synced.

Keep separate concerns in separate commits even when each change is correct, so reverting one concern does not revert unrelated work.

PR branches are squash-merged, so review fixes stack as follow-up commits. Amend only unpublished local mistakes, or when the user asks.

## Messages

The subject names the artifact or behavior changed and reads sensibly alone in a commit list; reviewer context goes in the body. Prefer `docs(skills): clarify reference routing` with a body citing the CodeRabbit feedback over `chore: address review feedback`. The body wraps at 72 columns and covers problem, rationale, decisions, and impact.

The `commit-msg` hook runs `scripts/validate-commit-scope.nu`: when staged paths live under `rust/adapters/<agent>/`, the scope must be that agent (`fix(kimi)`), one of the cross-cutting scopes, or — for a change spanning several agents — a workspace scope. `rust/adapters/common/` derives `adapter`, not `common`. Read the script for the current lists; no other part of the tree derives a scope.

Formatter-only changes are `chore: format`, or `chore(<scope>): format` when the scope rule above applies. Messages are US English.

## Push (push=true)

Changes reach `main` through a PR, so commit on a feature branch; `references/push.md` has the branch and upstream checks.

Push once every commit is in place and let the hooks in `nix/git-hooks.nix` run — treefmt and gitleaks on commit; treefmt, gitleaks, oxlint, `clippy -D warnings`, node test, and cargo test on push. Their failures are part of normal validation, so fix them in a new small commit.
