#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#gh --command nu
def main [] {
    let repository = (required_env GITHUB_REPOSITORY)
    let pr_number = (required_env PR_NUMBER)
    let marker = (required_env COMMENT_MARKER)
    let body = (open --raw (required_env COMMENT_FILE))
    let comments = (gh_api_json [
        --paginate
        --slurp
        $"repos/($repository)/issues/($pr_number)/comments?per_page=100"
    ] | flatten)
    let existing = (
        $comments | where {|comment|
			let login = if (($comment.user? | describe) =~ '^record') {
				$comment.user.login?
			} else {
				null
			}
			let comment_body = if (($comment.body? | describe) =~ '^string') {
				$comment.body
			} else {
				''
			}
			$login == 'github-actions[bot]' and ($comment_body | str contains $marker)
		} | sort-by created_at | reverse | get --optional 0
    )
    if $existing == null {
        create_comment $repository $pr_number $body
    } else {
        let update = (try_update_comment $repository $existing.id $body)
        if $update.status == 'ok' {
            return
        }
        if $update.status == 'missing' {
            print --stderr 'Existing PR comment was missing; creating a new comment instead.'
            create_comment $repository $pr_number $body
        } else if $update.status == 'auth' {
            print --stderr $"Skipping PR comment because GitHub token cannot update comments: ($update.stderr | str trim)"
        } else {
            error make {
                msg: (format_gh_error ['update comment'] $update.result)
            }
        }
    }
}
def required_env [name: string] {
    let value = $env | get --optional $name
    if $value == null or ($value | is-empty) {
        error make {msg: $"($name) is required"}
    }
    $value
}
def gh_api_json [args: list<string>] {
    let result = (gh_api_complete $args)
    if $result.exit_code != 0 {
        error make {
            msg: (format_gh_error $args $result)
        }
    }
    $result.stdout | from json
}
def create_comment [repository: string, pr_number: string, body: string] {
    let result = (
        (gh_api_with_body
            'POST'
            $"repos/($repository)/issues/($pr_number)/comments"
            $body
        )
    )
    if $result.exit_code != 0 {
        if (is_comment_write_auth_failure $result.stderr) {
            print --stderr $"Skipping PR comment because GitHub token cannot write comments: ($result.stderr | str trim)"
        } else {
            error make {
                msg: (format_gh_error ['create comment'] $result)
            }
        }
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
    if $result.exit_code == 0 {
        return {status: 'ok', result: $result, stderr: $result.stderr}
    }
    if $result.stderr =~ 'HTTP 404' {
        return {status: 'missing', result: $result, stderr: $result.stderr}
    }
    if (is_comment_write_auth_failure $result.stderr) {
        return {status: 'auth', result: $result, stderr: $result.stderr}
    }
    {status: 'error', result: $result, stderr: $result.stderr}
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
