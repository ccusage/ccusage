export const PRIORITY_LABELS = [
    'priority:critical'
    'priority:high'
    'priority:medium'
    'priority:low'
]

export const IMPLEMENTATION_PRIORITIES = ['priority:critical' 'priority:high']

export const TRIAGE_LABELS = [
    'triage:resolved'
    'triage:maintainable'
    'triage:excluded'
    'triage:needs-review'
]

export const COLLABORATOR_PERMISSIONS = [admin maintain write]
export const COMMENT_MARKER = '<!-- pullfrog-contribution-gate -->'

export def required-env [name: string]: nothing -> string {
    match ($env | get --optional $name) {
        null => (error make {msg: $"($name) is required"})
        $value if ($value | is-empty) => (error make {msg: $"($name) is required"})
        $value => $value
    }
}

export def optional-env [name: string, fallback: string]: nothing -> string {
    $env | get --optional $name | default $fallback
}

export def repository []: nothing -> string { required-env GITHUB_REPOSITORY }

export def parse-issue-number [value: string]: nothing -> int {
    let normalized = $value | str trim
    if not ($normalized =~ '^[1-9][0-9]*$') {
        error make {msg: 'ISSUE_NUMBER must be a positive integer'}
    }
    $normalized | into int
}

export def issue-number []: nothing -> int {
    parse-issue-number (required-env ISSUE_NUMBER)
}

# GitHub output files support a delimiter form, which preserves prompts and
# model reasons without serializing them into shell syntax.
export def write-output [name: string, value: string]: nothing -> nothing {
    let delimiter = $"pullfrog_($name)_(random uuid)"
    [
        $"($name)<<($delimiter)"
        $value
        $delimiter
    ]
    | str join "\n"
    | $in + "\n"
    | save --append (required-env GITHUB_OUTPUT)
}

export def gh-api-complete [args: list<string>]: any -> record {
    $in | run-external gh api ...$args | complete
}

export def gh-api-json [args: list<string>]: nothing -> any {
    let result = (gh-api-complete $args)
    if $result.exit_code != 0 {
        error make {
            msg: (format-gh-error $args $result)
        }
    }
    try {
        $result.stdout | from json
    } catch {
        error make {msg: $"gh api returned invalid JSON: ($result.stdout | str trim)"}
    }
}

export def gh-api-body [method: string, endpoint: string, body: record]: nothing -> any {
    let request = $body | to json
    let result = (
        $request
        | gh-api-complete [
            --method
            $method
            --header
            'Content-Type: application/json'
            $endpoint
            --input
            '-'
        ]
    )
    if $result.exit_code != 0 {
        error make {
            msg: (format-gh-error [$method $endpoint] $result)
        }
    }
    $result.stdout
}

export def gh-api-delete [endpoint: string]: nothing -> nothing {
    let result = (gh-api-complete [--method DELETE $endpoint])
    if $result.exit_code != 0 {
        error make {
            msg: (format-gh-error ['DELETE' $endpoint] $result)
        }
    }
}

export def format-gh-error [args: list<string>, result: record]: nothing -> string {
    $"gh api ($args | str join ' ') failed with exit code ($result.exit_code): ($result.stderr | str trim)"
}

export def comment-body [marker: string, body: string]: nothing -> string {
    [$marker $body] | str join "\n\n"
}
