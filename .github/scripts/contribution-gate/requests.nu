use ./core.nu [gh-api-json issue-number repository required-env write-output]

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
        # GitHub's no-reply format changed for older accounts; the cutoff mirrors its documented address formats.
        if $created_at < $legacy_cutoff {
            $"($username)@users.noreply.github.com"
        } else {
            $"($user_id)+($username)@users.noreply.github.com"
        }
    }
    $coauthor_email
}

export def issue-implementation-request []: nothing -> nothing {
    let number = issue-number
    let repo = repository
    let issue_author = required-env ISSUE_AUTHOR
    let issue_author_id = required-env ISSUE_AUTHOR_ID | into int
    let user = gh-api-json [$"users/($issue_author)"]
    let coauthor_email = coauthor-email $issue_author $issue_author_id $user
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
}
