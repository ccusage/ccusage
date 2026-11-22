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
    let empty = $fixture | path join empty
    mkdir ($generated | path join rust/pricing) $empty
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
            '    if ($args | first) == "ls-files" {'
            '        if $env.TEST_UNTRACKED == "true" { print --stderr "error: pathspec did not match"; exit 1 }'
            '        return'
            '    }'
            '    if ($args | first) == "diff" { return }'
            '    exit 74'
            '}'
        ] | str join "\n" | save ($fixture | path join git)
        ^chmod +x ($fixture | path join nix) ($fixture | path join git)
        let destination = $fixture | path join destination
        mkdir ($destination | path join rust/pricing)
        let copy_script = $env.FILE_PWD | path join ../../scripts/copy-models-dev-snapshots.nu
        let copied = with-env {
            PATH: ($env.PATH | prepend $fixture)
            TEST_SNAPSHOT_OUTPUT: $generated
            TEST_UNTRACKED: 'true'
        } {
            cd $destination
            nu $copy_script | complete
        }
        assert equal $copied.exit_code 0 $copied.stderr
        assert equal (open --raw ($destination | path join rust/pricing/snapshot.json)) '{}'
        [
            {
                name: 'unchanged snapshots'
                generation: false
                untracked: false
                output: $generated
                fail: false
            }
            {
                name: 'failed generation'
                generation: true
                untracked: false
                output: $generated
                fail: true
            }
            {
                name: 'untracked snapshot path'
                generation: false
                untracked: true
                output: $generated
                fail: true
            }
            {
                name: 'no snapshots'
                generation: false
                untracked: false
                output: $empty
                fail: true
            }
        ] | each {|case|
            let result = with-env {
                PATH: ($env.PATH | prepend $fixture)
                TEST_SNAPSHOT_OUTPUT: $case.output
                TEST_FAIL_GENERATION: ($case.generation | into string)
                TEST_UNTRACKED: ($case.untracked | into string)
            } {
                nu ($env.FILE_PWD | path join update-models-dev-lock.nu) | complete
            }
            assert equal ($result.exit_code != 0) $case.fail $"($case.name): ($result.stderr)"
            if not $case.fail {
                let parsed = $result.stdout | from json
                assert equal $parsed.changed false
                assert equal $parsed.paths [flake.lock rust/pricing/snapshot.json]
            }
        } | ignore
    } catch {|err|
        rm -rf $fixture
        error make $err
    }
    rm -rf $fixture
    print 'Pricing lock fixture tests passed.'
}
