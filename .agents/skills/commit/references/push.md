# Push Reference

Confirm the current branch with `git branch --show-current`. On `main` or
`master`, stop and move the work to a feature branch before pushing.

Check for an upstream:

```bash
git rev-parse --abbrev-ref --symbolic-full-name @{u}
```

- Upstream exists: `git push`.
- No upstream: ask the user before running `git push -u origin HEAD`, and skip
  the push if they decline.
