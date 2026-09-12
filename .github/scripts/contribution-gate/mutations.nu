use ./core.nu [
    COMMENT_MARKER
    PRIORITY_LABELS
    TRIAGE_LABELS
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
use ./context.nu [issue-protection-record require-open-issue]
use ./verdict.nu [issue-verdict-record pr-verdict-record]

def ensure-label [repo: string, label: string, color: string]: nothing -> nothing {
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
    require-open-issue | ignore
    gh-api-body POST $"repos/($repo)/labels" {name: $label, color: $color} | ignore
}

def ensure-gate-labels [repo: string]: nothing -> nothing {
    [
        {name: 'priority:critical', color: 'b60205'}
        {name: 'priority:high', color: 'd93f0b'}
        {name: 'priority:medium', color: 'fbca04'}
        {name: 'priority:low', color: '0e8a16'}
        {name: 'triage:resolved', color: '0e8a16'}
        {name: 'triage:maintainable', color: '1d76db'}
        {name: 'triage:excluded', color: 'cfd3d7'}
        {name: 'triage:needs-review', color: 'd4c5f9'}
    ]
    | each {|label| ensure-label $repo $label.name $label.color }
    | ignore
}

def apply-triage-label [
    repo: string
    number: int
    triage_label
    --preserve-protected
]: nothing -> nothing {
    let labels = (
        gh-api-json [
            '--paginate'
            '--slurp'
            $"repos/($repo)/issues/($number)/labels?per_page=100"
        ]
        | flatten
    )

    if $triage_label != null {
        require-open-issue | ignore
        (gh-api-body
            POST
            $"repos/($repo)/issues/($number)/labels"
            {
                labels: [$triage_label]
            }
        ) | ignore
    }

    $labels
    | where {|label|
        let name = $label | get --optional name
        let protected = (
            $preserve_protected
            and ($name in ['triage:maintainable' 'triage:needs-review'])
        )
        ($name in $TRIAGE_LABELS) and $name != $triage_label and not $protected
    }
    | each {|label|
        require-open-issue | ignore
        gh-api-delete $"repos/($repo)/issues/($number)/labels/($label.name)"
    }
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

    require-open-issue | ignore
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
    | each {|label|
        require-open-issue | ignore
        gh-api-delete $"repos/($repo)/issues/($number)/labels/($label.name)"
    }
    | ignore
}

def create-comment [
    repo: string
    number: int
    body: string
    require_open_issue: bool
]: nothing -> nothing {
    if $require_open_issue {
        require-open-issue | ignore
    }
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
    require_open_issue: bool
]: nothing -> nothing {
    if $require_open_issue {
        require-open-issue | ignore
    }
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
        create-comment $repo $number $body $require_open_issue
        return
    }
    (error make {
        msg: (format-gh-error ['PATCH' $endpoint] $result)
    }) | ignore
}

export def is-contribution-gate-comment [comment: record]: nothing -> bool {
    (
        (($comment | get --optional user.login) == 'github-actions[bot]')
        and (($comment | get --optional body | default '') | str contains $COMMENT_MARKER)
    )
}

export def upsert-comment [
    repo: string
    number: int
    body: string
    --require-open-issue
]: nothing -> nothing {
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
        | where {|comment| is-contribution-gate-comment $comment}
        | sort-by created_at
        | last
    )

    match ($existing | get --optional id) {
        null => {
            create-comment $repo $number $body $require_open_issue
        }
        $comment_id => {
            patch-comment $repo $number $comment_id $body $require_open_issue
        }
    }
}

def close-issue [repo: string, number: int]: nothing -> bool {
    let issue = require-open-issue
    if $issue.protected_from_close {
        return false
    }
    gh-api-body PATCH $"repos/($repo)/issues/($number)" {
        state: closed
        state_reason: not_planned
    } | ignore
    sleep 2sec
    let closed_issue = gh-api-json [$"repos/($repo)/issues/($number)"]
    let protection = issue-protection-record $closed_issue
    if $protection.protected_from_close {
        restore-protected-issue
        false
    } else {
        true
    }
}

export def restore-protected-issue []: nothing -> nothing {
    let repo = repository
    let number = issue-number
    let issue = gh-api-json [$"repos/($repo)/issues/($number)"]
    let protection = issue-protection-record $issue
    let closed_by = $issue | get --optional closed_by.login
    let should_restore = (
        $protection.protected_from_close
        and (($issue | get --optional state) == 'closed')
        and (($issue | get --optional state_reason) == 'not_planned')
        and $closed_by == 'github-actions[bot]'
    )
    if not $should_restore {
        return
    }

    # GitHub cannot condition a close PATCH on labels, so this serialized label event repairs the final API race.
    gh-api-body PATCH $"repos/($repo)/issues/($number)" {state: open} | ignore
    ensure-gate-labels $repo
    apply-triage-label $repo $number 'triage:needs-review'
    let body = comment-body $COMMENT_MARKER 'A trusted bug, security, or maintainer-review label was added during automatic closure. The issue was reopened for maintainer review.'
    upsert-comment $repo $number $body --require-open-issue
}

def close-pr [repo: string, number: int]: nothing -> nothing {
    gh-api-body PATCH $"repos/($repo)/pulls/($number)" {state: closed} | ignore
}

export def issue-comment [verdict: record]: nothing -> string {
    let outcome = match $verdict.decision {
        close => 'closed by the contribution gate'
        needs_human => 'left open for maintainer review'
        _ if $verdict.implementation == 'create_pr' => 'kept open; Pullfrog will attempt a focused implementation PR'
        _ => 'kept open'
    }
    let details = if $verdict.triage_label == 'triage:excluded' {
        [
            $"Pullfrog triage: **($verdict.priority)**"
            ''
            'Thanks for the proposal.'
            ''
            $verdict.reason
            ''
            'To keep the core CLI maintainable, new capabilities require explicit maintainer acceptance. Please do not open a core implementation PR unless a maintainer explicitly accepts this request. A fork or external wrapper remains welcome.'
        ]
    } else {
        [
            $"Pullfrog triage: **($verdict.priority)**"
            ''
            $verdict.reason
        ]
    }
    comment-body $COMMENT_MARKER ((
        $details
        | append ''
        | append $"Decision: **($outcome)**."
    ) | str join "\n")
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

def report-failure [
    repo: string
    number: int
    subject: string
    require_open_issue: bool = false
]: nothing -> nothing {
    let body = comment-body $COMMENT_MARKER $"Pullfrog could not complete the automated ($subject) review. The ($subject) was left open for maintainer review."
    upsert-comment $repo $number $body --require-open-issue=$require_open_issue
}

export def issue-verdict []: nothing -> nothing {
    let repo = repository
    let number = issue-number
    let issue = require-open-issue
    let close_allowed = (required-env CLOSE_ALLOWED) == 'true'
    let protected_from_close = $issue.protected_from_close
    let implementation_blocked = $issue.implementation_blocked
    let force_implementation = (optional-env FORCE_IMPLEMENTATION 'false') == 'true'
    let outcome = optional-env JUDGE_OUTCOME failure
    let result = optional-env RESULT ''

    if $outcome != 'success' {
        report-failure $repo $number issue true
        write-output implementation none
        exit 1
    }

    let verdict = (try {
        if $force_implementation {
            issue-verdict-record $result $close_allowed $protected_from_close $implementation_blocked --force-implementation
        } else {
            issue-verdict-record $result $close_allowed $protected_from_close $implementation_blocked
        }
    } catch { null })
    if $verdict == null {
        report-failure $repo $number issue true
        write-output implementation none
        exit 1
    }

    ensure-gate-labels $repo
    apply-priority-label $repo $number $verdict.priority
    if $verdict.decision == 'close' or $verdict.implementation == 'create_pr' {
        apply-triage-label $repo $number $verdict.triage_label --preserve-protected
    } else {
        apply-triage-label $repo $number $verdict.triage_label
    }
    upsert-comment $repo $number (issue-comment $verdict) --require-open-issue

    let final_verdict = if $verdict.decision == 'close' and $close_allowed {
        if (close-issue $repo $number) {
            $verdict
        } else {
            let current_issue = require-open-issue
            let protected_verdict = (
                issue-verdict-record
                    $result
                    $close_allowed
                    $current_issue.protected_from_close
                    $current_issue.implementation_blocked
            )
            apply-priority-label $repo $number $protected_verdict.priority
            apply-triage-label $repo $number $protected_verdict.triage_label
            upsert-comment $repo $number (issue-comment $protected_verdict) --require-open-issue
            $protected_verdict
        }
    } else {
        $verdict
    }
    write-output decision $final_verdict.decision
    write-output priority $final_verdict.priority
    write-output reason $final_verdict.reason
    write-output implementation $final_verdict.implementation
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
