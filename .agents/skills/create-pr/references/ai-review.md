# AI Review

Request CodeRabbit (`@coderabbitai`) on every PR, and Cubic (`@cubic-dev-ai`,
the GitHub user `cubic.dev`) when it is usable on the repository. If the PR or
recent repository comments show a different Cubic handle, use the one shown
there.

Add a top-level comment mentioning the bots after opening the PR, and mention
the relevant bot again after every meaningful push; repeat the request when a
bot does not rerun on its own.

Poll comments, reviews, and inline threads before calling the PR ready — see
`gh-review.md`, including the GraphQL query for thread resolution state.
Classify each item as actionable, a question, a false positive, or
informational, and for every actionable one apply the smallest fix that keeps
repo conventions, run the relevant checks, commit and push through the `commit`
skill, then reply in that thread — opening with the bot's mention — stating what
changed and which validation passed.
