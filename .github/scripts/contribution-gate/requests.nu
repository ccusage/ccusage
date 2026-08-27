use ./core.nu [
    COMMENT_MARKER
    comment-body
    gh-api-body
    gh-api-json
    issue-number
    repository
    required-env
    write-output
]
use ./context.nu [require-open-issue]
use ./mutations.nu [upsert-comment]

def pullfrog-payload [
    prompt: string
    trigger: string
    number: int
    author_permission: string
    is_pr: bool
    silent: bool
]: nothing -> string {
    let event = {
        trigger: $trigger
        issue_number: $number
        authorPermission: $author_permission
        silent: $silent
    }
    let event = if $is_pr {
        $event | insert is_pr true
    } else { $event }

    {
        '~pullfrog': true
        version: '0.1.60'
        prompt: $prompt
        event: $event
    }
    | to json
}

export def render-prompt [filename: string, values: record]: nothing -> string {
    let path = ['.github' 'scripts' 'contribution-gate' 'prompts' $filename] | path join
    let template = open --raw $path
    $values | columns | reduce --fold $template {|key, output|
        let value = $values | get $key | into string
        $output | str replace --all $"{{($key)}}" $value
    }
}

export def existing-implementation-pull-request [pull_requests: list<record>, repo: string, number: int]: nothing -> any {
    if $number <= 0 {
        error make {msg: 'Issue number must be positive'}
    }
    let head_prefix = $"pullfrog/issue-($number)-run-"
    let matches = (
        $pull_requests
        | where {|pull_request|
            let head_ref = $pull_request | get --optional head.ref
            [
                (($pull_request | get --optional user.login) == 'github-actions[bot]')
                (($pull_request | get --optional head.repo.full_name) == $repo)
                (($head_ref | describe) == 'string')
                (($head_ref | describe) == 'string' and ($head_ref | str starts-with $head_prefix))
            ]
            | all {|valid| $valid}
        }
    )
    if ($matches | is-empty) {
        null
    } else {
        $matches | first
    }
}

export def open-closing-pull-request [pull_requests: list<record>]: nothing -> any {
    let matches = (
        $pull_requests
        | where {|pull_request|
            let number = $pull_request | get --optional number
            let number_is_valid = match $number {
                $value if ($value | describe) == 'int' => ($value > 0)
                _ => false
            }
            let url = $pull_request | get --optional url
            [
                (($pull_request | get --optional state) == 'OPEN')
                $number_is_valid
                (($url | describe) == 'string' and ($url | str starts-with 'https://github.com/') and ($url | str contains '/pull/'))
            ]
            | all {|valid| $valid}
        }
    )
    if ($matches | is-empty) {
        null
    } else {
        let pull_request = $matches | first
        {number: $pull_request.number, html_url: $pull_request.url}
    }
}

export def implementation-result [value: string]: nothing -> record {
    let result = try {
        $value | from json
    } catch {
        error make {msg: 'Pullfrog returned invalid implementation JSON'}
    }
    if ($result | describe) !~ '^record' {
        error make {msg: 'Pullfrog implementation result must be an object'}
    }
    let extra_fields = $result | columns | where {|field| $field not-in [implementation title body]}
    if ($extra_fields | is-not-empty) {
        error make {msg: 'Pullfrog implementation result contains unexpected fields'}
    }

    let implementation = $result | get --optional implementation
    let title = $result | get --optional title
    let body = $result | get --optional body
    let fields_are_strings = [
        (($implementation | describe) == 'string')
        (($title | describe) == 'string')
        (($body | describe) == 'string')
    ] | all {|valid| $valid}
    if not $fields_are_strings {
        error make {msg: 'Pullfrog implementation result fields must be strings'}
    }
    let prepared_metadata_valid = [
        ($title | str trim | is-not-empty)
        (($title | lines | length) == 1)
        (($title | str length) <= 240)
        ($body | str trim | is-not-empty)
        (($body | str length) <= 20000)
    ] | all {|valid| $valid}

    match $implementation {
        none if ($title | is-empty) and ($body | is-empty) => $result
        prepared if $prepared_metadata_valid => $result
        none => (
            error make {msg: 'A declined implementation must not include PR metadata'}
        )
        prepared => (
            error make {msg: 'A prepared implementation requires a single-line title and non-empty body'}
        )
        _ => (error make {msg: 'Unknown Pullfrog implementation result'})
    }
}

export def implementation-branch [number: int, run_id: int, run_attempt: int]: nothing -> string {
    if $number <= 0 or $run_id <= 0 or $run_attempt <= 0 {
        error make {msg: 'Issue and workflow run identifiers must be positive'}
    }
    $"pullfrog/issue-($number)-run-($run_id)-attempt-($run_attempt)"
}

export def neutralize-closing-references [body: string]: nothing -> string {
    let closing_reference = '(?i)\b(?:close(?:s|d)?|fix(?:es|ed)?|resolve(?:s|d)?)(?=\s*:?[\s]*(?:(?:[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)?#[1-9][0-9]*|https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/(?:issues|pull)/[1-9][0-9]*))'
    $body | str replace --all --regex $closing_reference References
}

export def implementation-pull-request-body [marker: string, body: string, number: int]: nothing -> string {
    if $number <= 0 or ($marker | str trim | is-empty) or ($body | str trim | is-empty) {
        error make {msg: 'Implementation PR metadata is incomplete'}
    }
    [
        $marker
        (neutralize-closing-references ($body | str trim))
        $"Closes #($number)"
    ] | str join "\n\n"
}

def open-pull-requests [repo: string]: nothing -> list<record> {
    gh-api-json [
        '--paginate'
        '--slurp'
        $"repos/($repo)/pulls?state=open&sort=created&direction=desc&per_page=100"
    ]
    | flatten
}

def closing-pull-requests [repo: string, number: int]: nothing -> list<record> {
    if $number <= 0 {
        error make {msg: 'Issue number must be positive'}
    }
    let parts = $repo | split row '/'
    if ($parts | length) != 2 or ($parts | any {|part| $part | is-empty}) {
        error make {msg: 'GITHUB_REPOSITORY must contain an owner and repository name'}
    }
    let owner = $parts | get 0
    let name = $parts | get 1
    let query = 'query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { issue(number: $number) { closedByPullRequestsReferences(first: 100, includeClosedPrs: false) { nodes { number state url } } } } }'
    let response = gh-api-json [
        graphql
        --field
        $"query=($query)"
        --field
        $"owner=($owner)"
        --field
        $"name=($name)"
        --field
        $"number=($number)"
    ]
    $response.data.repository.issue.closedByPullRequestsReferences.nodes
}

def existing-issue-pull-request [repo: string, number: int]: nothing -> any {
    let generated = (existing-implementation-pull-request
        (open-pull-requests $repo)
        $repo
        $number
    )
    if $generated != null {
        return $generated
    }
    open-closing-pull-request (closing-pull-requests $repo $number)
}

def git-complete [args: list<string>]: nothing -> record {
    run-external git ...$args | complete
}

def git-run [args: list<string>]: nothing -> nothing {
    let result = git-complete $args
    if $result.exit_code != 0 {
        error make {msg: $"git ($args | str join ' ') failed with exit code ($result.exit_code): ($result.stderr | str trim)"}
    }
}

def git-output [args: list<string>]: nothing -> string {
    let result = git-complete $args
    if $result.exit_code != 0 {
        error make {msg: $"git ($args | str join ' ') failed with exit code ($result.exit_code): ($result.stderr | str trim)"}
    }
    $result.stdout | str trim
}

def setup-git-auth []: nothing -> nothing {
    let result = run-external gh auth setup-git | complete
    if $result.exit_code != 0 {
        error make {msg: $"gh auth setup-git failed: ($result.stderr | str trim)"}
    }
}

export def issue-request []: nothing -> nothing {
    let number = issue-number
    let repo = repository
    let author_status = required-env AUTHOR_STATUS
    let close_allowed = (required-env CLOSE_ALLOWED) == 'true'
    let prompt = render-prompt 'issue-judge.md' {
        ISSUE_NUMBER: ($number | into string)
        REPOSITORY: $repo
        AUTHOR_STATUS: $author_status
        CLOSE_ALLOWED: ($close_allowed | into string)
    }
    write-output prompt (pullfrog-payload $prompt issues_opened $number none false true)
}

export def pr-request []: nothing -> nothing {
    let number = issue-number
    let repo = repository
    let prompt = render-prompt 'pr-judge.md' {
        ISSUE_NUMBER: ($number | into string)
        REPOSITORY: $repo
    }
    write-output prompt (pullfrog-payload $prompt pull_request_opened $number none true true)
}

export def coauthor-email [username: string, user_id: int, user: record]: nothing -> string {
    let public_email = $user | get --optional email | default ''
    let public_email = match $public_email {
        $value if ($value | describe) == 'string' => ($value | str trim)
        _ => ''
    }
    let coauthor_email = if not ($public_email | is-empty) {
        $public_email
    } else {
        let created_at = $user | get --optional created_at
        if ($created_at | describe) != 'string' {
            error make {msg: $"Could not determine when GitHub account ($username) was created"}
        }
        let created_at = $created_at | into datetime
        let legacy_cutoff = '2017-07-18T00:00:00Z' | into datetime
        if $created_at < $legacy_cutoff {
            # GitHub does not expose whether a legacy account switched no-reply formats, so age alone cannot yield a reliable address.
            error make {msg: $"Could not resolve a GitHub email for legacy account ($username) without a public email"}
        } else {
            $"($user_id)+($username)@users.noreply.github.com"
        }
    }
    $coauthor_email
}

export def issue-implementation-request []: nothing -> nothing {
    let number = issue-number
    let repo = repository
    require-open-issue | ignore
    let issue_author = required-env ISSUE_AUTHOR
    let issue_author_id = required-env ISSUE_AUTHOR_ID | into int
    let user = gh-api-json [$"users/($issue_author)"]
    let coauthor_email = (try {
        coauthor-email $issue_author $issue_author_id $user
    } catch {
        null
    })
    if $coauthor_email == null {
        let body = comment-body $COMMENT_MARKER 'Automatic implementation was not started because the issue author GitHub email could not be resolved reliably for co-author attribution. A maintainer can implement the issue manually or provide a verifiable author email.'
        upsert-comment $repo $number $body --require-open-issue
        write-output implementation none
        return
    }
    let implementation_marker = $"<!-- pullfrog-accepted-issue: #($number) request-(random uuid) -->"
    let coauthor_trailer = $"Co-authored-by: ($issue_author) <($coauthor_email)>"
    let implementation_branch = (implementation-branch
        $number
        (required-env GITHUB_RUN_ID | into int)
        (required-env GITHUB_RUN_ATTEMPT | into int)
    )
    let prompt = render-prompt 'issue-implementation.md' {
        ISSUE_NUMBER: ($number | into string)
        REPOSITORY: $repo
    }
    write-output prompt (pullfrog-payload $prompt issues_opened $number none false true)
    write-output implementation_marker $implementation_marker
    write-output coauthor_trailer $coauthor_trailer
    write-output coauthor_email $coauthor_email
    write-output implementation_branch $implementation_branch
    write-output implementation create_pr
}

export def issue-implementation-guard []: nothing -> nothing {
    let repo = repository
    let issue = require-open-issue
    let existing = existing-issue-pull-request $repo $issue.number
    if $existing == null {
        write-output skip 'false'
        return
    }
    print $"Skipping implementation because open PR #($existing.number) already targets issue #($issue.number)."
    write-output skip 'true'
}

export def publish-implementation []: nothing -> nothing {
    let repo = repository
    let issue = require-open-issue
    let existing = existing-issue-pull-request $repo $issue.number
    if $existing != null {
        print $"Skipping publication because open PR #($existing.number) already targets issue #($issue.number)."
        write-output skip 'true'
        return
    }

    let branch = required-env IMPLEMENTATION_BRANCH
    let expected_branch = (implementation-branch
        $issue.number
        (required-env GITHUB_RUN_ID | into int)
        (required-env GITHUB_RUN_ATTEMPT | into int)
    )
    if $branch != $expected_branch {
        error make {msg: 'Implementation branch does not match this workflow run'}
    }

    let result = implementation-result (open --raw (required-env IMPLEMENTATION_METADATA))
    if $result.implementation != 'prepared' {
        error make {msg: 'Only a prepared implementation can be published'}
    }

    let base_sha = required-env IMPLEMENTATION_BASE_SHA
    let head_sha = git-output [rev-parse HEAD]
    if $head_sha != $base_sha {
        error make {msg: 'Publish checkout does not match the implementation base commit'}
    }

    git-run [
        apply
        --index
        --binary
        (required-env IMPLEMENTATION_PATCH)
    ]
    let staged = git-complete [diff --cached --quiet]
    if $staged.exit_code == 0 {
        error make {msg: 'The implementation patch is empty'}
    }
    if $staged.exit_code != 1 {
        error make {msg: $"Could not inspect the staged implementation: ($staged.stderr | str trim)"}
    }

    git-run [config user.name 'github-actions[bot]']
    git-run [config user.email '41898282+github-actions[bot]@users.noreply.github.com']
    git-run [switch '-c' $branch]
    git-run [
        commit
        '-m'
        $result.title
        '-m'
        (required-env COAUTHOR_TRAILER)
    ]
    setup-git-auth

    require-open-issue | ignore
    git-run [push --set-upstream origin $"HEAD:refs/heads/($branch)"]

    require-open-issue | ignore
    let body = (implementation-pull-request-body
        (required-env IMPLEMENTATION_MARKER)
        $result.body
        $issue.number
    )
    let pull_request = gh-api-body POST $"repos/($repo)/pulls" {
        title: $result.title
        head: $branch
        base: (required-env GITHUB_DEFAULT_BRANCH)
        body: $body
    } | from json
    let pull_number = $pull_request | get --optional number
    if ($pull_number | describe) != 'int' or $pull_number <= 0 {
        error make {msg: 'GitHub returned an invalid implementation pull request'}
    }
    print $"Created implementation PR #($pull_number) for issue #($issue.number)."
    write-output pull_request_number ($pull_number | into string)
    write-output skip 'false'
}
