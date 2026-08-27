use ./core.nu [gh-api-json repository required-env]

def implementation-pull-requests [repo: string, branch: string, marker: string]: nothing -> any {
    gh-api-json [
        '--paginate'
        '--slurp'
        $"repos/($repo)/pulls?state=open&sort=created&direction=desc&per_page=100"
    ]
    | flatten
    | where {|pull_request|
        let body = $pull_request | get --optional body | default ''
        [
            (($pull_request | get --optional user.login) == 'github-actions[bot]')
            (($pull_request | get --optional head.repo.full_name) == $repo)
            (($pull_request | get --optional head.ref) == $branch)
            (($body | describe) == 'string' and ($body | str contains $marker))
        ]
        | all {|valid| $valid}
    }
}

def pull-request-commits [repo: string, number: int]: nothing -> any {
    gh-api-json [
        '--paginate'
        '--slurp'
        $"repos/($repo)/pulls/($number)/commits?per_page=100"
    ]
    | flatten
}

def find-implementation-pull-request [repo: string, branch: string, marker: string]: nothing -> any {
    let max_attempts = 12
    for attempt in 0..($max_attempts - 1) {
        let pull_requests = implementation-pull-requests $repo $branch $marker
        if ($pull_requests | length) > 1 {
            error make {msg: 'Found multiple implementation pull requests for this issue-gate run'}
        }
        if ($pull_requests | length) == 1 {
            return ($pull_requests | get 0)
        }
        if $attempt < ($max_attempts - 1) {
            sleep 5sec
        }
    }
    error make {msg: 'Could not find the implementation pull request for this issue-gate run after retrying the GitHub API'}
}

def commit-attributions [repo: string, sha: string]: nothing -> list<record> {
    let parts = $repo | split row '/'
    let owner = $parts | get 0
    let name = $parts | get 1
    let query = 'query($owner: String!, $repo: String!, $expression: String!) { repository(owner: $owner, name: $repo) { object(expression: $expression) { ... on Commit { authors(first: 100) { nodes { email user { databaseId } } } } } } }'
    let response = gh-api-json [
        graphql
        --field
        $"query=($query)"
        --field
        $"owner=($owner)"
        --field
        $"repo=($name)"
        --field
        $"expression=($sha)"
    ]
    $response.data.repository.object.authors.nodes
    | each {|attribution|
        let email = $attribution | get --optional email | default ''
        let user_id = $attribution | get --optional user.databaseId | default 0
        if (($email | describe) == 'string' and ($user_id | describe) == 'int' and $user_id > 0 and not ($email | is-empty)) {
            {email: $email, user_id: $user_id}
        } else {
            null
        }
    }
    | compact
}

def commit-trailers [message: string]: nothing -> list<string> {
    $message
    | lines
    | where {|line| $line | str starts-with 'Co-authored-by:'}
}

export def coauthor-validation [
    message: string
    attributions: list<record>
    expected_trailer: string
    expected_email: string
    issue_author_id: int
]: nothing -> record {
    let trailers = commit-trailers $message
    let expected_count = $trailers | where {|trailer| $trailer == $expected_trailer } | length
    let unexpected_count = $trailers | where {|trailer| $trailer != $expected_trailer } | length
    let author_ok = $attributions | any {|attribution|
        $attribution.email == $expected_email and $attribution.user_id == $issue_author_id
    }
    {
        trailer_ok: (($expected_count == 1) and ($unexpected_count == 0))
        author_ok: $author_ok
    }
}

def validate-commit [
    repo: string
    commit: record
    expected_trailer: string
    expected_email: string
    issue_author_id: int
]: nothing -> record {

    # Match the expected trailer email to GitHub's resolved author identity so a primary author cannot satisfy a co-author check.
    let attributions = commit-attributions $repo $commit.sha
    let validation = (coauthor-validation
        $commit.commit.message
        $attributions
        $expected_trailer
        $expected_email
        $issue_author_id
    )
    {sha: $commit.sha, trailer_ok: $validation.trailer_ok, author_ok: $validation.author_ok}
}

export def verify-coauthor []: nothing -> nothing {
    let repo = repository
    let branch = required-env IMPLEMENTATION_BRANCH
    let marker = required-env IMPLEMENTATION_MARKER
    let expected_trailer = required-env COAUTHOR_TRAILER
    let expected_email = required-env COAUTHOR_EMAIL
    let issue_author_id = required-env ISSUE_AUTHOR_ID | into int
    let pull_request = find-implementation-pull-request $repo $branch $marker
    let commits = pull-request-commits $repo $pull_request.number
    if ($commits | is-empty) {
        error make {msg: $"Implementation PR #($pull_request.number) has no commits to verify"}
    }
    let validation = $commits | each {|commit|
        validate-commit $repo $commit $expected_trailer $expected_email $issue_author_id
    }
    let invalid = $validation | where {|result| (not $result.trailer_ok) or (not $result.author_ok) }
    if ($invalid | is-not-empty) {
        let summary = $invalid
        | each {|result| $"($result.sha): trailer_ok=($result.trailer_ok), author_ok=($result.author_ok)"}
        | str join "\n"
        error make {msg: $"Issue-author co-author verification failed for PR #($pull_request.number):\n($summary)"}
    }
    print $"Verified issue-author attribution on PR #($pull_request.number) across ($commits | length) commit(s)."
}
