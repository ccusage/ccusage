# Open PR

```sh
git push -u origin <branch-name>
```

Let the pre-push hooks run; fix a failure in a new small commit and push again.

## Title

State the concrete outcome or benefit, rather than listing implementation
changes. For performance improvements, include measured before/after values or
the measured improvement; never invent numbers or infer gains from code alone.

- ❌ `perf(server): negotiate permessage-deflate on the websocket`
- ✅ `perf(server): cut websocket frame size by 70%+ with gzipping`

Squash-merge turns the title into the commit subject on `main`, so write it as a
commit subject. `.github/workflows/check-pr-title.yaml` checks the Conventional
Commit shape and then re-runs `scripts/validate-commit-scope.nu` against the PR
diff, so the scope rules from the `commit` skill apply to the title too.

## Body

Start agent-authored PR descriptions with
`🤖 **<model> opened this on behalf of <user>**`, using the actual model name
exposed by the harness and the user's name or GitHub login. Use the harness
name if it exposes no model name. Omit this only when the user supplies the
body verbatim.

List the checks actually run. For a bug fix, state the root cause and name the
regression test. If report output or snapshots must change, name the affected
rows and why.

Match the body to the change: 2-4 sentences on what changed and why for focused
work. For a complex change add only the sections that carry information —
Summary, What Changed, Why, Testing, Related Issues. "Testing" lists validation
that actually ran, so documentation-only PRs usually omit it.

## Passing the body

Write multiline bodies to a file and pass it to `gh pr create --body-file`.
