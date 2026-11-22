---
name: create-pr
description: Branches, commits, pushes, and opens a pull request or a stack of PRs. Use before opening any PR, whether the user asks for one or the task calls for it.
---

# Create PR

A PR is one reviewable responsibility and may hold several commits. Split
independent work into separate PRs; stack only work that depends on another PR.

## Workflow

1. Read `git status --short`, `git branch --show-current`, and `git diff --stat`,
   and check whether a PR for this branch already exists (`gh pr view`).
   Work on the user’s named branch, creating or switching to it as needed.
   Otherwise create a `feature/`, `fix/`, or `chore/` branch when on `main`.
2. Leave out unrelated changes, temporary files, secrets, and debug edits, then
   commit with the `commit` skill.
3. Title: follow the repository's convention (check recent merged PRs; default
   to a Conventional Commit type). State why the change matters, not the
   mechanism:
   - ❌ `perf(server): negotiate permessage-deflate on the websocket`
   - ✅ `perf(server): cut websocket frame size by 70%+ with gzipping`
4. Start the body with the attribution note from the `leaving-pr-comments`
   skill, worded `🤖 **<model> opened this on behalf of <user>**`; its
   exclusion of PR descriptions does not apply here. Omit it only when the
   user supplies the body verbatim.
   Then, in proportion to the change: open with the problem as the user saw it,
   then the root cause and why this fix; never lead with an inventory of edits. Add a Testing section only for tests actually run. Pass multi-line
   bodies with `--body-file -`.
5. For a visible change, attach real screenshots or short videos with
   `gh pr create --attach` (see `gh pr create --help`); never invent media.
6. Publish:
   - Ordinary PR: `git push -u origin <branch>` and `gh pr create --base <target-branch>`
     using the requested target, or the repository default when unspecified.
     Open it ready for review, not as a draft, so review bots run. If GraphQL is
     rate limited, use `gh api repos/<owner>/<repo>/pulls -f title=... -f head=... -f base=... -F body=@<file>`.
   - Stacked PRs (when asked, or the branch is already stacked): use the
     `gh-stack` skill, verifying parent-child ancestry, and report every PR URL
     from bottom to top.
7. Report the PR URL. Don't call it ready until checks and review feedback have
   been looked at; to watch them, use the `babysit-pr` skill.
