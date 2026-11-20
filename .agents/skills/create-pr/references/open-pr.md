# Open PR

```sh
git push -u origin <branch-name>
```

Let the pre-push hooks run; fix a failure in a new small commit and push again.

## Title

Squash-merge turns the title into the commit subject on `main`, so write it as a
commit subject. `.github/workflows/check-pr-title.yaml` checks the Conventional
Commit shape and then re-runs `scripts/validate-commit-scope.nu` against the PR
diff, so the scope rules from the `commit` skill apply to the title too.

## Body

List the checks actually run. For a bug fix, state the root cause and name the
regression test. If report output or snapshots must change, name the affected
rows and why.

Match the body to the change: 2-4 sentences on what changed and why for focused
work. For a complex change add only the sections that carry information —
Summary, What Changed, Why, Testing, Related Issues. "Testing" lists validation
that actually ran, so documentation-only PRs usually omit it.

## Passing the body

Write multiline bodies to a file and pass it to `gh pr create --body-file`.
