#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

const CROSS_CUTTING_SCOPES = [deps, release, pricing, revert]
const WORKSPACE_SCOPES = [adapter, all, rust]
const GENERATED_SUBJECTS = ["Merge ", "Revert ", "fixup!", "squash!", "amend!"]

# Validates the subject in `message_file` against the staged paths.
def main [message_file: path]: nothing -> nothing {
    let paths = git diff --cached --name-only | lines
    validate "commit scope: subject" (commit-subject $message_file) $paths
}

def "main pr" []: nothing -> nothing {
    let pull_request = open $env.GITHUB_EVENT_PATH | get pull_request
    let exempt = $env.TITLE_EXEMPT_LABELS | lines | where {|label| $label != "" }
    if ($pull_request.labels.name | any {|label| $label in $exempt }) {
        return
    }

    let paths = gh api $"repos/($env.GITHUB_REPOSITORY)/pulls/($pull_request.number)/files" --paginate --slurp
    | from json
    | flatten
    | get filename
    validate "PR title" ($pull_request.title | str trim) $paths
}

export def validate [what: string, line: string, paths: list<string>]: nothing -> nothing {
    if ($line | is-empty) or ($GENERATED_SUBJECTS | any {|prefix| $line | str starts-with $prefix }) {
        return
    }

    let parsed = parse-subject $line
    if $parsed == null {
        (reject
            $what
            $line
            "is not a Conventional Commit"
            "Use `<type>(<scope>): <subject>`, for example `feat(codex): add session totals`."
        )
    }

    let owners = path-owners $paths
    let scopes = $parsed.scope
    | default ""
    | split row ","
    | each {|scope| $scope | str trim }
    | where {|scope| $scope != "" }

    match $owners {
        [] => { }
        [$owner] => {
            (check
                $what
                $line
                $scopes
                ($CROSS_CUTTING_SCOPES | prepend $owner)
                $"The change belongs to `($owner)`, so use `($parsed.type)\(($owner)\): ...`."
            )
        }
        [_, _, ..] => {
            (check
                $what
                $line
                $scopes
                (
                    $CROSS_CUTTING_SCOPES
                    | append $WORKSPACE_SCOPES
                    | append $owners
                )
                $"The change spans ($owners | str join ', '), so name one of them or use a scope that covers all of them."
            )
        }
    }
}

# Rejects a subject whose scope is missing from `allowed`.
def check [
    what: string
    line: string
    scopes: list<string>
    allowed: list<string>
    hint: string
]: nothing -> nothing {
    if ($scopes | is-empty) {
        reject $what $line "needs a scope" $hint
    }

    if not ($scopes | all {|scope| $scope in $allowed }) {
        reject $what $line $"is scoped `($scopes | str join ',')`" $hint
    }
}

# Stops the commit with a plain message; `error make` would bury it in a Nushell
# diagnostic that git then indents into noise.
def reject [
    what: string
    line: string
    problem: string
    hint: string
]: nothing -> nothing {
    print --stderr $"($what) ($problem):\n  ($line)\n\n($hint)"
    exit 1
}

# Reads the first line of the commit message that the author actually wrote.
def commit-subject [message_file: path]: nothing -> string {
    open --raw $message_file
    | lines
    | where {|line| not ($line | str starts-with "#") }
    | where {|line| ($line | str trim) != "" }
    | get 0?
    | default ""
    | str trim
}

# Splits a Conventional Commit subject into its type and optional scope.
def parse-subject [line: string]: nothing -> any {
    $line
    | parse --regex '^(?<type>[a-z]+)(?:\((?<scope>[^()]+)\))?!?: \S'
    | get 0?
}

def path-owners [paths: list<string>]: nothing -> list<string> {
    $paths
    | where {|path| ($path | str trim) != "" }
    | each {|path| owner-for-path $path }
    | compact
    | uniq
    | sort
}

# Returns the agent owning a repository-relative path. Only `rust/adapters/*` is
# mapped: each crate is a catch-all whose commits are scoped by feature (`cost`,
# `statusline`, `json`) rather than by crate name, and replaying these rules over
# the log showed that deriving a scope from them rejects good messages.
def owner-for-path [path: string]: nothing -> any {
    match ($path | split row "/") {
        ["rust", "adapters", "common", ..] => "adapter"
        ["rust", "adapters", $agent, ..] if not ($agent | str contains ".") => $agent
        _ => null
    }
}
