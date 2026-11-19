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

def main [workspace: path]: nothing -> nothing {
    let root = $workspace | path expand
    let members = glob ($root | path join 'adapters/*/Cargo.toml')
    | append (glob ($root | path join 'crates/*/Cargo.toml'))
    let violations = $members | each {|member|
        let parsed = open --raw $member | from toml
        dependency-tables $parsed | each {|table|
            $table | transpose name spec | each {|dependency|
                let pins_version = if ($dependency.spec | describe | str starts-with 'record') {
                    ($dependency.spec | get --optional version | default '') != ''
                } else {
                    true
                }
                if $pins_version {
                    $"($member | path relative-to $root): ($dependency.name) must inherit its version from [workspace.dependencies]"
                }
            }
        } | flatten
    } | flatten | compact
    if ($violations | is-not-empty) {
        error make {
            msg: ($violations | str join "\n")
        }
    }
}
