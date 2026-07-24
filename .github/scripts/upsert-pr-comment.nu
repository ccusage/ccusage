#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#gh --command nu
def main [] {
    let repository = (required_env GITHUB_REPOSITORY)
    let pr_number = (required_env PR_NUMBER)
    let marker = (required_env COMMENT_MARKER)
    let body = (open --raw (required_env COMMENT_FILE))
    match (find_existing_comment $repository $pr_number $marker) {
        null => (create_comment $repository $pr_number $body)
        $existing => (update_or_recreate_comment $repository $pr_number $existing.id $body)
    }
}
def find_existing_comment [repository: string, pr_number: string, marker: string] {
    gh_api_json [
        --paginate
        --slurp
        $"repos/($repository)/issues/($pr_number)/comments?per_page=100"
    ]
    | flatten
    | where {|comment|
        ((comment_login $comment) == 'github-actions[bot]') and ((comment_body $comment) | str contains $marker)
    }
    | sort-by created_at
    | reverse
    | get --optional 0
}
def comment_login [comment: record] {
    match ($comment | get --optional user) {
        {login: $login} => $login
        _ => null
    }
}
def comment_body [comment: record] {
    match ($comment | get --optional body) {
        $body if ($body | describe) == 'string' => $body
        _ => ''
    }
}
def update_or_recreate_comment [repository: string, pr_number: string, comment_id: int, body: string] {
    let update = (try_update_comment $repository $comment_id $body)
    match $update.status {
        'ok' => null
        'missing' => {
            print --stderr 'Existing PR comment was missing; creating a new comment instead.'
            create_comment $repository $pr_number $body
        }
        'auth' => (print --stderr $"Skipping PR comment because GitHub token cannot update comments: ($update.stderr | str trim)")
        _ => (error make {
            msg: (format_gh_error ['update comment'] $update.result)
        })
    }
}
def required_env [name: string] {
    match ($env | get --optional $name) {
        null => (error make {msg: $"($name) is required"})
        $value if ($value | is-empty) => (error make {msg: $"($name) is required"})
        $value => $value
    }
}
def gh_api_json [args: list<string>] {
    let result = (gh_api_complete $args)
    match $result.exit_code {
        0 => ($result.stdout | from json)
        _ => (error make {
            msg: (format_gh_error $args $result)
        })
    }
}
def create_comment [repository: string, pr_number: string, body: string] {
    let result = (
        (gh_api_with_body
            'POST'
            $"repos/($repository)/issues/($pr_number)/comments"
            $body
        )
    )
    match $result.exit_code {
        0 => null
        _ if (is_comment_write_auth_failure $result.stderr) => (print --stderr $"Skipping PR comment because GitHub token cannot write comments: ($result.stderr | str trim)")
        _ => (error make {
            msg: (format_gh_error ['create comment'] $result)
        })
    }
}
def try_update_comment [repository: string, comment_id: int, body: string] {
    let result = (
        (gh_api_with_body
            'PATCH'
            $"repos/($repository)/issues/comments/($comment_id)"
            $body
        )
    )
    let status = (match $result.exit_code {
        0 => 'ok'
        _ if $result.stderr =~ 'HTTP 404' => 'missing'
        _ if (is_comment_write_auth_failure $result.stderr) => 'auth'
        _ => 'error'
    })
    {status: $status, result: $result, stderr: $result.stderr}
}
def gh_api_with_body [method: string, endpoint: string, body: string] {
    let payload = mktemp -t ccusage-pr-comment.XXXXXX | str trim
    {body: $body} | to json | save --force $payload
    let args = [
        --method
        $method
        --header
        'Content-Type: application/json'
        $endpoint
        --input
        $payload
    ]
    let result = (gh_api_complete $args)
    rm --force $payload
    $result
}
def gh_api_complete [args: list<string>] {
    run-external gh api ...$args | complete
}
def is_comment_write_auth_failure [stderr: string] { ($stderr =~ 'HTTP 401') or ($stderr =~ 'HTTP 403') }
def format_gh_error [args: list<string>, result: record] { $"gh api ($args | str join ' ') failed with exit code ($result.exit_code): ($result.stderr | str trim)" }
