use ./core.nu [
    CONFIDENCE_LEVELS
    IMPLEMENTATION_PRIORITIES
    ISSUE_KINDS
    MAINTENANCE_FITS
    PRIORITY_LABELS
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

export def issue-verdict-record [result: string, close_allowed: bool, --force-implementation]: nothing -> record {
    let raw = parse-result $result
    let expected_fields = [
        confidence
        decision
        implementation
        kind
        maintenance_fit
        priority
        reason
    ]
    if ($raw | columns | sort) != $expected_fields {
        error make {msg: 'Pullfrog returned unexpected issue verdict fields'}
    }
    let kind = parse-required-enum $raw kind $ISSUE_KINDS
    let maintenance_fit = parse-required-enum $raw maintenance_fit $MAINTENANCE_FITS
    let confidence = parse-required-enum $raw confidence $CONFIDENCE_LEVELS
    let decision = parse-required-enum $raw decision [keep_open close needs_human]
    let priority = parse-required-enum $raw priority $PRIORITY_LABELS
    let implementation = parse-required-enum $raw implementation [create_pr none]
    let reason = parse-reason $raw
    let verdict = {
        kind: $kind
        maintenance_fit: $maintenance_fit
        confidence: $confidence
        decision: $decision
        priority: $priority
        implementation: $implementation
        reason: $reason
    }

    # The workflow owns product-scope decisions so prompt drift cannot turn an
    # optional feature or uncertain classification into an automatic PR.
    let verdict = if $verdict.confidence != high {
        $verdict
        | update maintenance_fit needs_review
        | update decision needs_human
        | update implementation none
        | update reason $"Pullfrog was not highly confident; maintainer review is required. ($verdict.reason)"
    } else if $verdict.kind == security {
        let priority = if $verdict.priority == 'priority:critical' {
            'priority:critical'
        } else {
            'priority:high'
        }
        $verdict
        | update maintenance_fit needs_review
        | update decision needs_human
        | update priority $priority
        | update implementation none
    } else if $verdict.kind in [question unclear] {
        $verdict
        | update maintenance_fit needs_review
        | update decision needs_human
        | update implementation none
    } else if $verdict.kind == bug and ($verdict.decision == close or $verdict.maintenance_fit != maintainable) {
        $verdict
        | update maintenance_fit needs_review
        | update decision needs_human
        | update implementation none
    } else if $verdict.kind in [duplicate invalid spam out_of_scope] {
        if $verdict.decision == close {
            $verdict
            | update maintenance_fit excluded
            | update priority 'priority:low'
            | update implementation none
        } else {
            $verdict
            | update maintenance_fit needs_review
            | update decision needs_human
            | update implementation none
        }
    } else if $verdict.kind in [feature_request maintenance] {
        if (
            $verdict.maintenance_fit == excluded
            and $verdict.priority in ['priority:medium' 'priority:low']
        ) {
            $verdict
            | update decision close
            | update priority 'priority:low'
            | update implementation none
        } else {
            $verdict
            | update maintenance_fit needs_review
            | update decision needs_human
            | update implementation none
        }
    } else if $verdict.maintenance_fit == excluded {
        $verdict
        | update maintenance_fit needs_review
        | update decision needs_human
        | update implementation none
    } else if $verdict.maintenance_fit == needs_review {
        $verdict
        | update decision needs_human
        | update implementation none
    } else if $verdict.decision == close {
        $verdict
        | update decision needs_human
        | update implementation none
    } else {
        $verdict
    }

    # The workflow, rather than the model, also owns the hard permission
    # constraints for automatic closure and implementation.
    let verdict = if not $close_allowed {
        let verdict = if $verdict.decision == 'close' {
            $verdict
            | update decision needs_human
            | update reason $"Automatic closure is disabled because author permissions could not be verified; maintainer review is required. ($verdict.reason)"
        } else {
            $verdict
        }
        $verdict | update implementation none
    } else {
        $verdict
    }
    if $force_implementation {
        $verdict
        | update decision keep_open
        | update implementation create_pr
        | update reason $"A maintainer explicitly requested an implementation attempt. ($reason)"
    } else if (
        ($verdict.decision != 'keep_open')
        or (not ($verdict.priority in $IMPLEMENTATION_PRIORITIES))
        or $verdict.kind != bug
        or $verdict.maintenance_fit != maintainable
        or $verdict.confidence != high
    ) {
        $verdict | update implementation none
    } else {
        $verdict
    }
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
