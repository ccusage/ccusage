use ./core.nu [gh-api-json repository required-env]

def implementation-pull-requests [repo: string, marker: string]: nothing -> any {
    gh-api-json [
        '--paginate'
        '--slurp'
        $"repos/($repo)/pulls?state=open&sort=created&direction=desc&per_page=100"
    ]
    | flatten
    | where {|pull_request|
        let body = $pull_request | get --optional body | default ''
        if ($body | describe) != 'string' {
            false
        } else {
            $body | str contains $marker
        }
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

def commit-authors [repo: string, sha: string]: nothing -> list<int> {
    let parts = $repo | split row '/'
    let owner = $parts | get 0
    let name = $parts | get 1
    let query = 'query($owner: String!, $repo: String!, $expression: String!) { repository(owner: $owner, name: $repo) { object(expression: $expression) { ... on Commit { authors(first: 100) { nodes { user { databaseId } } } } } } }'
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
    | each {|author|
        let user = $author | get --optional user
        match $user {
            null => null
            $value => ($value | get --optional databaseId)
        }
    }
    | compact
}

def commit-trailers [message: string]: nothing -> list<string> {
    $message
    | lines
    | where {|line| $line | str starts-with 'Co-authored-by:'}
}

def validate-commit [
    repo: string
    commit: record
    expected_trailer: string
    issue_author_id: int
]: nothing -> record {
    let trailers = commit-trailers $commit.commit.message
    let expected_count = $trailers | where {|trailer| $trailer == $expected_trailer } | length
    let unexpected_count = $trailers | where {|trailer| $trailer != $expected_trailer } | length
    # GitHub resolves the authors connection from both the commit author and trailer emails, so this checks attribution rather than text alone.
    let authors = commit-authors $repo $commit.sha
    {
        sha: $commit.sha
        trailer_ok: (($expected_count == 1) and ($unexpected_count == 0))
        author_ok: ($issue_author_id in $authors)
    }
}

export def verify-coauthor []: nothing -> nothing {
    let repo = repository
    let marker = required-env IMPLEMENTATION_MARKER
    let expected_trailer = required-env COAUTHOR_TRAILER
    let issue_author_id = required-env ISSUE_AUTHOR_ID | into int
    let pull_requests = implementation-pull-requests $repo $marker
    let pull_request = match ($pull_requests | length) {
        0 => (
            error make {msg: 'Could not find the implementation pull request for this issue-gate run'}
        )
        1 => ($pull_requests | get 0)
        _ => (
            error make {msg: 'Found multiple implementation pull requests for this issue-gate run'}
        )
    }
    let commits = pull-request-commits $repo $pull_request.number
    if ($commits | is-empty) {
        error make {msg: $"Implementation PR #($pull_request.number) has no commits to verify"}
    }
    let validation = $commits | each {|commit|
        validate-commit $repo $commit $expected_trailer $issue_author_id
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
