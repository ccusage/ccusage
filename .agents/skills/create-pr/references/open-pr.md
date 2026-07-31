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

Match the body to the change: 2-4 sentences on what changed and why for focused
work. For a complex change add only the sections that carry information —
Summary, What Changed, Why, Testing, Related Issues. "Testing" lists validation
that actually ran, so documentation-only PRs usually omit it.

## Passing the body

Pass multi-line bodies through stdin with `gh pr create --body-file -`. Shell
quoting keeps `\n` escapes literal inside a `--body` argument, so they show up
verbatim in the PR. The active shell may be fish, zsh, bash, or a
non-interactive runner; fish has no heredoc, so pipe `printf` instead:

```fish
printf "%s\n" \
	"Adds a repo-local create-pr skill and documents the PR review loop." \
	"" \
	"Testing:" \
	"- just fmt" \
	| gh pr create --title "docs(skills): add create-pr workflow" --body-file -
```
