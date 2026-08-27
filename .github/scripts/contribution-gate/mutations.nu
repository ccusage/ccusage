use ./core.nu [
    COMMENT_MARKER
    PRIORITY_LABELS
    comment-body
    format-gh-error
    gh-api-body
    gh-api-complete
    gh-api-delete
    gh-api-json
    issue-number
    optional-env
    repository
    required-env
    write-output
]
use ./verdict.nu [issue-verdict-record pr-verdict-record]

def ensure-priority-label [repo: string, label: string, color: string]: nothing -> nothing {
    let endpoint = $"repos/($repo)/labels/($label)"
    let result = (gh-api-complete [$endpoint])
    if $result.exit_code == 0 {
        return
    }
    if not ($result.stderr | str contains 'HTTP 404') {
        error make {
            msg: (format-gh-error [$endpoint] $result)
        }
    }
    gh-api-body POST $"repos/($repo)/labels" {name: $label, color: $color} | ignore
}

def ensure-priority-labels [repo: string]: nothing -> nothing {
    [
        {name: 'priority:critical', color: b60205}
        {name: 'priority:high', color: d93f0b}
        {name: 'priority:medium', color: fbca04}
        {name: 'priority:low', color: 0e8a16}
    ]
    | each {|label| ensure-priority-label $repo $label.name $label.color }
    | ignore
}

def apply-priority-label [repo: string, number: int, priority: string]: nothing -> nothing {
    let labels = (
        gh-api-json [
            '--paginate'
            '--slurp'
            $"repos/($repo)/issues/($number)/labels?per_page=100"
        ]
        | flatten
    )

    (gh-api-body
        POST
        $"repos/($repo)/issues/($number)/labels"
        {
            labels: [$priority]
        }
    ) | ignore

    $labels
    | where {|label|
        let name = $label | get --optional name
        ($name in $PRIORITY_LABELS) and $name != $priority
    }
    | each {|label| gh-api-delete $"repos/($repo)/issues/($number)/labels/($label.name)" }
    | ignore
}

def create-comment [repo: string, number: int, body: string]: nothing -> nothing {
    (gh-api-body
        POST
        $"repos/($repo)/issues/($number)/comments"
        {body: $body}
    ) | ignore
}

def patch-comment [
    repo: string
    number: int
    comment_id: int
    body: string
]: nothing -> nothing {
    let endpoint = $"repos/($repo)/issues/comments/($comment_id)"
    let result = (
        {body: $body}
        | to json
        | gh-api-complete [
            '--method'
            PATCH
            '--header'
            'Content-Type: application/json'
            $endpoint
            '--input'
            '-'
        ]
    )

    if $result.exit_code == 0 {
        return
    }
    if ($result.stderr | str contains 'HTTP 404') {
        create-comment $repo $number $body
        return
    }
    (error make {
        msg: (format-gh-error ['PATCH' $endpoint] $result)
    }) | ignore
}

def upsert-comment [repo: string, number: int, body: string]: nothing -> nothing {
    let comments = (
        gh-api-json [
            '--paginate'
            '--slurp'
            $"repos/($repo)/issues/($number)/comments?per_page=100"
        ]
        | flatten
    )
    let existing = (
        $comments
        | where {|comment|
            ($comment | get --optional user.login) == 'github-actions[bot]'
            and (($comment | get --optional body | default '') | str contains $COMMENT_MARKER)
        }
        | sort-by created_at
        | last
    )

    match ($existing | get --optional id) {
        null => {
            create-comment $repo $number $body
        }
        $comment_id => {
            patch-comment $repo $number $comment_id $body
        }
    }
}

def close-issue [repo: string, number: int]: nothing -> nothing {
    gh-api-body PATCH $"repos/($repo)/issues/($number)" {state: closed} | ignore
}

def close-pr [repo: string, number: int]: nothing -> nothing {
    gh-api-body PATCH $"repos/($repo)/pulls/($number)" {state: closed} | ignore
}

def issue-comment [verdict: record]: nothing -> string {
    let outcome = match $verdict.decision {
        close => 'closed by the contribution gate'
        needs_human => 'left open for maintainer review'
        _ if $verdict.implementation == 'create_pr' => 'kept open; Pullfrog will attempt a focused implementation PR'
        _ => 'kept open'
    }
    comment-body $COMMENT_MARKER ([
        $"Pullfrog triage: **($verdict.priority)**"
        ''
        $verdict.reason
        ''
        $"Decision: **($outcome)**."
    ] | str join "\n")
}

def pr-comment [verdict: record]: nothing -> string {
    let outcome = if $verdict.decision == 'close' {
        'closed by the contribution gate'
    } else {
        'left open for maintainer review'
    }
    comment-body $COMMENT_MARKER ([
        'Pullfrog contribution-gate review'
        ''
        $verdict.reason
        ''
        $"Decision: **($outcome)**."
    ] | str join "\n")
}

def report-failure [repo: string, number: int, subject: string]: nothing -> nothing {
    let body = comment-body $COMMENT_MARKER $"Pullfrog could not complete the automated ($subject) review. The ($subject) was left open for maintainer review."
    upsert-comment $repo $number $body
}

export def issue-verdict []: nothing -> nothing {
    let repo = repository
    let number = issue-number
    let close_allowed = (required-env CLOSE_ALLOWED) == 'true'
    let outcome = optional-env JUDGE_OUTCOME failure
    let result = optional-env RESULT ''

    if $outcome != 'success' {
        report-failure $repo $number issue
        write-output implementation none
        exit 1
    }

    let verdict = (try { issue-verdict-record $result $close_allowed } catch { null })
    if $verdict == null {
        report-failure $repo $number issue
        write-output implementation none
        exit 1
    }

    ensure-priority-labels $repo
    apply-priority-label $repo $number $verdict.priority
    if $verdict.decision == 'close' and $close_allowed {
        close-issue $repo $number
    }
    upsert-comment $repo $number (issue-comment $verdict)
    write-output decision $verdict.decision
    write-output priority $verdict.priority
    write-output reason $verdict.reason
    write-output implementation $verdict.implementation
}

export def pr-verdict []: nothing -> nothing {
    let repo = repository
    let number = issue-number
    let close_allowed = (required-env CLOSE_ALLOWED) == 'true'
    let outcome = optional-env JUDGE_OUTCOME failure
    let result = optional-env RESULT ''

    if $outcome != 'success' {
        report-failure $repo $number PR
        write-output decision needs_human
        exit 1
    }

    let verdict = (try { pr-verdict-record $result $close_allowed } catch { null })
    if $verdict == null {
        report-failure $repo $number PR
        write-output decision needs_human
        exit 1
    }

    if $verdict.decision == 'close' {
        close-pr $repo $number
    }
    upsert-comment $repo $number (pr-comment $verdict)
    write-output decision $verdict.decision
    write-output reason $verdict.reason
}
