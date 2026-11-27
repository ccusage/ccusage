#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell nixpkgs#gh nixpkgs#diffutils --command nu

const RULESETS = path self ../.github/rulesets

# The repository's rulesets as code: each `.github/rulesets/*.json` is the body the rulesets
# API takes, and the ruleset of the same name is the live copy. `apply` creates or updates
# the live ones to match; `diff` shows how they differ. See the development skill, "Branch Rules".
def main []: nothing -> nothing {
    help main
}

# Create or update each file's ruleset, leaving those that already match alone.
def "main apply" [
    --repo: string = '{owner}/{repo}' # owner/name; gh fills the default from the git remote
]: nothing -> nothing {
    for ruleset in (rulesets $repo) {
        match $ruleset.id {
            null => {
                (gh-send
                    POST
                    $"repos/($repo)/rulesets"
                    $ruleset.file
                )
                print $"created ($ruleset.name)"
            }
            $id if $ruleset.live == $ruleset.want => {
                print $"($ruleset.name) is up to date"
            }
            $id => {
                (gh-send
                    PUT
                    $"repos/($repo)/rulesets/($id)"
                    $ruleset.file
                )
                print $"updated ($ruleset.name)"
            }
        }
    }
}

# Show how each live ruleset differs from its file, failing when any does.
def "main diff" [
    --repo: string = '{owner}/{repo}' # owner/name; gh fills the default from the git remote
]: nothing -> nothing {
    let results = rulesets $repo
    | each {|ruleset|
        let live = mktemp --tmpdir
        let want = mktemp --tmpdir
        $ruleset.live | to json --indent 2 | save --force $live
        $ruleset.want | to json --indent 2 | save --force $want
        let result = ^diff -u --label $"live ($ruleset.name)" --label $ruleset.file $live $want
        | complete
        rm $live $want
        match $result.exit_code {
            0 => { print $"($ruleset.name) matches ($ruleset.file)" }
            1 => { print --no-newline $result.stdout }
            _ => { error make {msg: $result.stderr} }
        }
        $result.exit_code != 0
    }
    if ($results | any {|it| $it }) {
        exit 1
    }
}

# Each file's ruleset beside the live one of its name (null when there is none), both reduced
# to the fields a file holds.
def rulesets [repo: string]: nothing -> table {
    let live = gh-get $"repos/($repo)/rulesets?includes_parents=false" | flatten
    ls ($RULESETS | path join '*.json' | into glob)
    | each {|file|
        let want = open $file.name | comparable
        let matches = $live | where name == $want.name
        let id = match ($matches | length) {
            0 => null
            1 => $matches.0.id
            _ => { error make {msg: $"($repo) has more than one ruleset named ($want.name)"} }
        }
        {
            name: $want.name
            file: ($file.name | path relative-to (pwd))
            id: $id
            want: $want
            live: (if $id == null { null } else { gh-get $"repos/($repo)/rulesets/($id)" | first | comparable })
        }
    }
}

# The fields a ruleset file holds, with every record's keys sorted, so equal rulesets compare
# and print equal whichever order the API returns them in.
export def comparable []: record -> record {
    select name target enforcement conditions rules bypass_actors | sort-keys
}

def sort-keys []: any -> any {
    let value = $in
    match ($value | describe --detailed).type {
        record => {
            $value
            | columns
            | sort
            | reduce --fold {} {|key, sorted| $sorted | insert $key ($value | get $key | sort-keys) }
        }
        list => {
            $value | each { sort-keys }
        }
        _ => $value
    }
}

# Every page of a GET, one item each.
def gh-get [endpoint: string]: nothing -> list<any> {
    gh-api [--paginate --slurp $endpoint] | from json
}

def gh-send [method: string, endpoint: string, body: path]: nothing -> nothing {
    gh-api [--method $method $endpoint --input $body] | ignore
}

def gh-api [args: list<string>]: nothing -> string {
    let result = ^gh api ...$args | complete
    if $result.exit_code != 0 {
        error make {msg: $"gh api ($args | str join ' '): ($result.stderr | str trim)"}
    }
    $result.stdout
}
