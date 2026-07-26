# Contributing to ccusage

This guide exists to save maintainers and contributors time.

## The One Rule

**You must understand your change.** If you cannot explain what your code does and how it interacts with the rest of the project, the PR may be closed.

Using AI tools is fine. Submitting generated output that you have not reviewed and cannot explain is not.

If you use an agent, run it from the repository root so it picks up `CLAUDE.md` and the repo-local skills.

## Contribution Gate

Issues and PRs from new contributors are auto-closed by default.

This gate is based on the contributor approval workflow used by [earendil-works/pi](https://github.com/earendil-works/pi).

Start with an issue before opening a PR. Keep it short, concrete, and written in your own voice.

Maintainers may approve contributors by replying on an issue:

- `lgtmi`: future issues will not be auto-closed
- `lgtm`: future issues and PRs will not be auto-closed

`lgtmi` does not grant rights to submit PRs. Only `lgtm` grants rights to submit PRs.

## Quality Bar For Issues

Use one of the GitHub issue templates.

- Keep it concise.
- Write in your own voice.
- State the bug or request clearly.
- Explain why it matters.
- If you want to implement the change yourself, say so.

Maintainers may reopen clear, useful issues and approve the author for future issues or PRs.

## Before Submitting a PR

Do not open a PR unless you have already been approved with `lgtm`.

Before submitting a PR, run:

```bash
just install
just fmt
just typecheck
just test
```

`just install` is only needed once per checkout (and after a lockfile change);
`git wt` runs it for you when it creates a worktree.

Use the canonical `ccusage` command in docs and tests. Standalone wrapper packages such as `ccusage-codex`, `ccusage-opencode`, `ccusage-amp`, and `ccusage-pi` have been removed and should not be reintroduced.

Do not proactively create documentation files unless the change requires user-facing documentation.

## FAQ

### Why are new issues and PRs auto-closed?

ccusage receives agent-assisted reports and changes. Auto-closing gives maintainers a buffer to review issues on their own schedule and reopen the ones that are concrete, reproducible, and worth investigating.

### Why might an issue get no reply?

Low-signal issues, unclear reports, duplicates, and issues that do not follow this guide may be closed without discussion. A reply is maintenance work too.

### Is AI-generated code banned?

No. AI assistance is allowed. The requirement is that the contributor understands the change, tests it, and can explain it in their own words.
