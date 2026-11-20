#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

def dependency-tables [manifest: record]: nothing -> list<record> {
    let sections = ['dependencies' 'dev-dependencies' 'build-dependencies']
    let direct = $sections | each {|section| $manifest | get --optional $section | default {} }
    let targets = $manifest
    | get --optional target
    | default {}
    | values
    | each {|target| $sections | each {|section| $target | get --optional $section | default {} } }
    | flatten
    $direct | append $targets
}

const unsafe_site = 'crates/ccusage-zstd/src/direct.rs'

def lint-violations [root: path, manifest: record, members: list<string>]: nothing -> list<string> {
    let level = $manifest | get --optional workspace.lints.rust.unsafe_code
    let workspace = if $level not-in ['deny' 'forbid'] {
        [
            'Cargo.toml: [workspace.lints.rust] unsafe_code must be "deny" or "forbid"'
        ]
    } else { [] }
    let inheritance = $members | each {|member|
        let lints = open --raw $member | from toml | get --optional lints
        if $lints != {workspace: true} {
            $"($member | path relative-to $root): must inherit [workspace.lints] with [lints] workspace = true"
        }
    } | compact
    let attributes = $members
    | each {|member| glob ($member | path dirname | path join '**/*.rs') --exclude ['**/target/**'] }
    | flatten
    | where {|file| ($file | path relative-to $root) != $unsafe_site }
    | each {|file|
        open --raw $file
        | lines
        | enumerate
        | where {|line| $line.item =~ '\b(?:allow|expect)\s*\([^)]*\bunsafe_code\b' }
        | each {|line|
            $"($file | path relative-to $root):($line.index + 1): unsafe_code may be allowed only in ($unsafe_site)"
        }
    }
    | flatten
    $workspace | append $inheritance | append $attributes
}

def main [workspace: path]: nothing -> nothing {
    let root = $workspace | path expand
    let manifest = open --raw ($root | path join 'Cargo.toml') | from toml
    let managed = $manifest.workspace.dependencies | default {} | columns
    let members = $manifest.workspace.members
    | default []
    | each {|member| glob ($root | path join $member 'Cargo.toml') }
    | flatten
    let lints = lint-violations $root $manifest $members
    let violations = $members | each {|member|
        let parsed = open --raw $member | from toml
        dependency-tables $parsed | each {|table|
            $table | transpose name spec | each {|dependency|
                let escapes_workspace = match $dependency.spec {
                    {workspace: true} => false
                    {version: $_} => true
                    {} => ($dependency.name in $managed)
                    _ => true
                }
                if $escapes_workspace {
                    $"($member | path relative-to $root): ($dependency.name) must inherit from [workspace.dependencies] with workspace = true"
                }
            }
        } | flatten
    } | flatten | compact | append $lints
    if ($violations | is-not-empty) {
        error make {
            msg: ($violations | str join "\n")
        }
    }
}
