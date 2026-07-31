#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

# Conventional Commit scopes here name the agent that changed, not the directory
# holding it: `fix(kimi)`, `feat(codex)`. Nothing about a subject line reveals an
# invented scope on its own — `feat(coding)` reads fine until you know the change
# was codex — so this reads the ownership off the staged paths instead.
#
# The commit-msg hook runs this. A squash merge writes the pull request title
# instead of these subjects, so the same rules are applied to PR titles by
# .github/workflows/check-pr-title.yaml; keep the two in step.

# Scopes accepted whatever the changed paths map to, because they describe a
# reason for the change rather than a part of the tree.
const CROSS_CUTTING_SCOPES = [deps, release, pricing, revert]

# Additionally accepted when a change spans several agents at once, where no
# single agent name would be honest.
const WORKSPACE_SCOPES = [adapter, all, rust]

# Prefixes git generates or rewrites later, where the author picks no scope.
const GENERATED_SUBJECTS = ["Merge ", "Revert ", "fixup!", "squash!", "amend!"]

# Validates the subject in `message_file` against the staged paths.
def main [message_file: path]: nothing -> nothing {
    let line = commit-subject $message_file

    if ($line | is-empty) or ($GENERATED_SUBJECTS | any {|prefix| $line | str starts-with $prefix }) {
        return
    }

    let parsed = parse-subject $line
    if $parsed == null {
        (reject
            $line
            "is not a Conventional Commit"
            "Use `<type>(<scope>): <subject>`, for example `feat(codex): add session totals`."
        )
    }

    let owners = staged-owners
    let scopes = $parsed.scope
    | default ""
    | split row ","
    | each {|scope| $scope | str trim }
    | where {|scope| $scope != "" }

    match $owners {

        # Nothing under `rust/adapters/` changed, so no scope is derivable and the
        # author's choice stands.
        [] => { }
        [$owner] => {
            (check
                $line
                $scopes
                ($CROSS_CUTTING_SCOPES | prepend $owner)
                $"The change belongs to `($owner)`, so use `($parsed.type)\(($owner)\): ...`."
            )
        }
        # A change spanning several agents has no single owner, but it still may not
        # claim an unrelated name.
        [_, _, ..] => {
            (check
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
    line: string
    scopes: list<string>
    allowed: list<string>
    hint: string
]: nothing -> nothing {
    if ($scopes | is-empty) {
        reject $line "needs a scope" $hint
    }

    if not ($scopes | any {|scope| $scope in $allowed }) {
        reject $line $"is scoped `($scopes | str join ',')`" $hint
    }
}

# Stops the commit with a plain message; `error make` would bury it in a Nushell
# diagnostic that git then indents into noise.
def reject [line: string, problem: string, hint: string]: nothing -> nothing {
    print --stderr $"commit scope: subject ($problem):\n  ($line)\n\n($hint)"
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

# Lists the agents owning the staged paths.
def staged-owners []: nothing -> list<string> {
    git diff --cached --name-only
    | lines
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
        # A dot means the entry is a file directly under `adapters/`, not an agent.
        ["rust", "adapters", $agent, ..] if not ($agent | str contains ".") => $agent
        _ => null
    }
}
