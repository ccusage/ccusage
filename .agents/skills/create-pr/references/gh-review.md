# GitHub Review Commands

`gh pr comment`, `gh pr view --json comments,reviews,statusCheckRollup`, and
`gh pr checks` cover requesting review and polling. The calls below are the ones
`gh` has no porcelain for. Reviewer handles are examples; use the ones current
on the PR.

## Reply inside an inline thread

`gh pr comment` only posts top-level comments, so replying in the thread a bot
opened needs the REST replies endpoint with that comment's id (from
`gh api repos/:owner/:repo/pulls/<pr-number>/comments`):

```sh
gh api -X POST repos/:owner/:repo/pulls/<pr-number>/comments/<comment-id>/replies \
  -f body='@coderabbitai Fixed in <commit-sha>. Validation: just typecheck, just test.'
```

## Thread resolution state

Resolution state is GraphQL-only. This returns the first 100 threads; add
`pageInfo` and `after` pagination for large PRs.

```sh
gh api graphql \
  -F owner='OWNER' \
  -F repo='REPO' \
  -F number=<pr-number> \
  -f query='
query($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        nodes {
          id
          isResolved
          comments(first: 20) {
            nodes {
              id
              databaseId
              author { login }
              path
              body
            }
          }
        }
      }
    }
  }
}'
```
