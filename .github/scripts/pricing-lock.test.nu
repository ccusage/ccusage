#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

use std/assert
use ./pricing-lock.nu report

def main []: nothing -> nothing {
    let expected = {
        changed: true
        paths: [flake.nix flake.lock]
    }
    assert equal (report $expected | from json) $expected
    let fixture = mktemp -d
    let generated = $fixture | path join generated
    mkdir ($generated | path join rust/pricing)
    '{}' | save ($generated | path join rust/pricing/snapshot.json)
    try {
        [
            $"#!($nu.current-exe)"
            'def --wrapped main [...args: string] {'
            '    if "shell" in $args and $env.TEST_FAIL_GENERATION == "true" { exit 73 }'
            '    if "build" in $args { print $env.TEST_SNAPSHOT_OUTPUT; return }'
            '    print "fixture nix diagnostic"'
            '}'
        ] | str join "\n" | save ($fixture | path join nix)
        [
            $"#!($nu.current-exe)"
            'def --wrapped main [...args: string] {'
            '    if $args == ["diff" "--quiet" "--" "flake.lock"] { return }'
            '    if ($args | first) == "diff" { return }'
            '    exit 74'
            '}'
        ] | str join "\n" | save ($fixture | path join git)
        ^chmod +x ($fixture | path join nix) ($fixture | path join git)
        for fail in [false true] {
            let result = with-env {
                PATH: ($env.PATH | prepend $fixture)
                TEST_SNAPSHOT_OUTPUT: $generated
                TEST_FAIL_GENERATION: ($fail | into string)
            } {
                nu ($env.FILE_PWD | path join update-models-dev-lock.nu) | complete
            }
            assert equal ($result.exit_code != 0) $fail $result.stderr
            if not $fail {
                let parsed = $result.stdout | from json
                assert equal $parsed.changed false
                assert equal $parsed.paths [flake.lock rust/pricing/snapshot.json]
            }
        }
    } catch {|err|
        rm -rf $fixture
        error make $err
    }
    rm -rf $fixture
    print 'Pricing lock fixture tests passed.'
}
