use ./core.nu [IMPLEMENTATION_PRIORITIES PRIORITY_LABELS]

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
    let decision = parse-required-enum $raw decision [keep_open close needs_human]
    let priority = parse-required-enum $raw priority $PRIORITY_LABELS
    let implementation = parse-required-enum $raw implementation [create_pr none]
    let reason = parse-reason $raw
    let verdict = {
        decision: $decision
        priority: $priority
        implementation: $implementation
        reason: $reason
    }

    # The workflow, rather than the model, owns the hard safety constraints for
    # automatic closure and implementation.
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
    } else if ($verdict.decision != 'keep_open') or (not ($verdict.priority in $IMPLEMENTATION_PRIORITIES)) {
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
