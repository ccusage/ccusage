use ./core.nu [
    COMMENT_MARKER
    comment-body
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

export def existing-implementation-pull-request [pull_requests: list<record>, number: int]: nothing -> any {
    if $number <= 0 {
        error make {msg: 'Issue number must be positive'}
    }
    let marker_prefix = $"<!-- pullfrog-accepted-issue: #($number) "
    let matches = (
        $pull_requests
        | where {|pull_request|
            let body = $pull_request | get --optional body | default ''
            ($body | describe) == 'string' and ($body | str contains $marker_prefix)
        }
    )
    if ($matches | is-empty) {
        null
    } else {
        $matches | first
    }
}

def open-pull-requests [repo: string]: nothing -> list<record> {
    gh-api-json [
        '--paginate'
        '--slurp'
        $"repos/($repo)/pulls?state=open&sort=created&direction=desc&per_page=100"
    ]
    | flatten
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
        upsert-comment $repo $number $body
        write-output implementation none
        return
    }
    let implementation_marker = $"<!-- pullfrog-accepted-issue: #($number) request-(random uuid) -->"
    let coauthor_trailer = $"Co-authored-by: ($issue_author) <($coauthor_email)>"
    let prompt = render-prompt 'issue-implementation.md' {
        ISSUE_NUMBER: ($number | into string)
        REPOSITORY: $repo
        IMPLEMENTATION_MARKER: $implementation_marker
        COAUTHOR_TRAILER: $coauthor_trailer
    }
    write-output prompt (pullfrog-payload $prompt issues_opened $number write false false)
    write-output implementation_marker $implementation_marker
    write-output coauthor_trailer $coauthor_trailer
    write-output coauthor_email $coauthor_email
    write-output implementation create_pr
}

export def issue-implementation-guard []: nothing -> nothing {
    let repo = repository
    let issue = require-open-issue
    let existing = existing-implementation-pull-request (open-pull-requests $repo) $issue.number
    if $existing == null {
        write-output skip 'false'
        return
    }
    print $"Skipping implementation because open PR #($existing.number) already targets issue #($issue.number)."
    write-output skip 'true'
}
