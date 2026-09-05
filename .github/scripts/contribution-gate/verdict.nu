use ./core.nu [IMPLEMENTATION_PRIORITIES PRIORITY_LABELS]

const ISSUE_KINDS = [
    supported_behavior_bug
    security
    maintenance
    documentation
    feature_request
    other
]

const AUTO_IMPLEMENTATION_KINDS = [
    supported_behavior_bug
    maintenance
    documentation
]

def parse-required-enum [verdict: record, field: string, allowed: list<string>]: nothing -> string {
    let value = $verdict | get --optional $field
    if ($value | describe) != 'string' or not ($value in $allowed) {
        error make {msg: $"Pullfrog returned an invalid ($field)"}
    }
    $value
}

def parse-reason [verdict: record]: nothing -> string {
    let reason = $verdict | get --optional reason
    if ($reason | describe) != 'string' {
        error make {msg: 'Pullfrog returned an invalid reason'}
    }
    let reason = $reason | str trim
    if ($reason | is-empty) or ($reason | str length) > 1200 {
        error make {msg: 'Pullfrog returned an invalid reason length'}
    }
    $reason
}

def parse-result [result: string]: nothing -> record {
    let parsed = (
        try {
            $result | from json
        } catch { null }
    )
    match $parsed {
        $verdict if (($verdict | describe) | str starts-with 'record') => $verdict
        _ => (error make {msg: 'Pullfrog did not return a verdict object'})
    }
}

export def issue-verdict-record [
    result: string
    close_allowed: bool
    protected_from_close: bool = false
    implementation_blocked: bool = false
    --force-implementation
]: nothing -> record {
    let raw = parse-result $result
    let decision = parse-required-enum $raw decision [keep_open close needs_human]
    let priority = parse-required-enum $raw priority $PRIORITY_LABELS
    let implementation = parse-required-enum $raw implementation [create_pr none]
    let issue_kind = parse-required-enum $raw issue_kind $ISSUE_KINDS
    let reason = parse-reason $raw
    let verdict = {
        decision: $decision
        priority: $priority
        implementation: $implementation
        issue_kind: $issue_kind
        reason: $reason
    }

    # Explicit maintainer action is the only path that bypasses automatic product scope.
    let verdict = if $force_implementation {
        $verdict
        | update decision keep_open
        | update implementation create_pr
        | update reason $"A maintainer explicitly requested an implementation attempt. ($reason)"
    } else if $issue_kind == 'feature_request' {
        $verdict
        | update decision close
        | update priority 'priority:low'
        | update implementation none
        | update reason $"This is a new optional capability rather than a bug or regression in currently supported behavior. ($reason)"
    } else if $issue_kind == 'security' {
        $verdict
        | update decision needs_human
        | update implementation none
        | update reason $"Security-sensitive reports always require maintainer review. ($reason)"
    } else if $issue_kind == 'other' {
        $verdict
        | update decision needs_human
        | update implementation none
        | update reason $"This issue is outside the automatic implementation categories and requires maintainer review. ($reason)"
    } else if $decision == 'close' {
        $verdict
        | update decision needs_human
        | update implementation none
        | update reason $"Only new feature requests may be closed automatically; maintainer review is required for this issue. ($reason)"
    } else {
        $verdict
    }

    # A repository-controlled label protects known bugs and reviewed issues even if the model misclassifies them.
    let verdict = if (not $force_implementation) and $protected_from_close and $verdict.decision == 'close' {
        $verdict
        | update decision needs_human
        | update implementation none
        | update reason $"Automatic closure is disabled by the issue's trusted labels; maintainer review is required. ($verdict.reason)"
    } else {
        $verdict
    }

    let verdict = if (not $force_implementation) and $implementation_blocked {
        $verdict
        | update decision needs_human
        | update implementation none
        | update reason $"Automatic implementation is disabled by the issue's trusted labels; maintainer review is required. ($verdict.reason)"
    } else {
        $verdict
    }

    # The workflow independently verifies whether this author may be auto-closed.
    let verdict = if (not $close_allowed) and $verdict.decision == 'close' {
        $verdict
        | update decision needs_human
        | update reason $"Automatic closure is disabled because author permissions could not be verified; maintainer review is required. ($verdict.reason)"
    } else {
        $verdict
    }

    let verdict = if (not $force_implementation) and (not ($issue_kind in $AUTO_IMPLEMENTATION_KINDS)) {
        $verdict | update implementation none
    } else if (not $force_implementation) and (not $close_allowed) {
        $verdict | update implementation none
    } else if (not $force_implementation) and (($verdict.decision != 'keep_open') or (not ($verdict.priority in $IMPLEMENTATION_PRIORITIES))) {
        $verdict
        | update implementation none
    } else {
        $verdict
    }
    let triage_label = if $verdict.decision == 'needs_human' {
        'triage:needs-review'
    } else if $verdict.decision == 'close' and $issue_kind == 'feature_request' {
        'triage:excluded'
    } else {
        'triage:maintainable'
    }
    $verdict | insert triage_label $triage_label
}

export def pr-verdict-record [result: string, close_allowed: bool]: nothing -> record {
    let raw = parse-result $result
    let verdict = {
        decision: (parse-required-enum $raw decision [keep_open close needs_human])
        reason: (parse-reason $raw)
    }
    if (not $close_allowed) and $verdict.decision == 'close' {
        $verdict
        | update decision needs_human
        | update reason $"Automatic closure is disabled because author permissions could not be verified; maintainer review is required. ($verdict.reason)"
    } else {
        $verdict
    }
}
