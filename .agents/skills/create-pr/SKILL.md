---
name: create-pr
description: Runs the full PR lifecycle. Use when creating a branch, committing, pushing, opening a PR, requesting AI review, replying to review threads, and driving CI to green.
---

# Create PR

This skill owns PR work in this repository: branch setup, opening the PR, AI review requests, reviewer thread replies, follow-up pushes, CI inspection, and merge.

A PR is one reviewable responsibility and may hold several atomic commits. A branch is not a PR boundary — split independent work into separate PRs and stack only dependent work.

## Workflow

1. Branch. Pushing to `main` needs explicit permission from the user; everything else goes through a feature branch named after the change (existing branches read `<type>/<description>`). Commits come from the `commit` skill, so they stay atomic and independently revertable. Leave unrelated formatter churn, generated files, and user edits outside the task unstaged.
2. Push and open the PR — `references/open-pr.md`. A squash merge writes the PR title as the commit subject on `main`, so CI holds the title to the same Conventional Commit scope rules the `commit` skill describes.
3. Request and handle AI review — `references/ai-review.md`, with the `gh` reply and thread-state calls in `references/gh-review.md`.
4. Watch CI after opening and after every push with `gh pr checks`. Read the failing step logs and annotations rather than the summary, then use the `fix-ci` skill for the fix, and ask the bots for another pass if they do not rerun on their own.
5. Merge only when the user explicitly asks and the conditions below hold: `gh pr merge <pr> --squash --delete-branch`. Squash is the repository's normal flow.

## Ready means

- The branch is pushed, the PR exists, and its body describes the change and the validation that ran.
- CodeRabbit — and Cubic when it is available on the PR — reviewed the latest pushed commit, with no unresolved actionable feedback.
- Every required check passes. Queued, cancelled, failing, or missing required checks all mean not ready.
- The user has the PR URL plus any residual risk or pending external state.

When a bot or CI system stays silent past a reasonable polling window, state exactly what is pending instead of claiming completion, and leave the visible PR comment or CI state for follow-up.

## Context

- Reviewer bots act on a comment only when their handle is mentioned — in the initial request and in every reply asking them to do something.
- Amending or force-pushing after reviewers have read the PR needs an explicit request from the user.
