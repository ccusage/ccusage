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
    let manifest = open --raw ($root | path join 'Cargo.toml') | from toml
    let members = $manifest.workspace.members
    | default []
    | each {|member| glob ($root | path join $member 'Cargo.toml') }
    | flatten
    let violations = $members | each {|member|
        let parsed = open --raw $member | from toml
        dependency-tables $parsed | each {|table|
            $table | transpose name spec | each {|dependency|
                let pins_version = match $dependency.spec {
                    {version: $_} => true
                    {} => false
                    _ => true
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
