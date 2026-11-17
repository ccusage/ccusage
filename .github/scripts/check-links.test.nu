#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#bash --command nu

use std/assert

def main []: nothing -> nothing {
    let script = open ($env.FILE_PWD | path join ../workflows/check-links.yaml)
    | get jobs.check-links.steps
    | where {|step| ($step | get --optional name) == 'Fail on checker execution errors' }
    | first
    | get run
    [
        {external: '0', internal: '0', valid: true}
        {external: '2', internal: '0', valid: true}
        {external: '0', internal: '2', valid: false}
        {external: '1', internal: '0', valid: false}
        {external: '0', internal: '', valid: false}
    ] | each {|case|
        let result = with-env {
            EXTERNAL_EXIT_CODE: $case.external
            INTERNAL_EXIT_CODE: $case.internal
        } { bash -c $script | complete }
        assert equal ($result.exit_code == 0) $case.valid $result.stdout
    } | ignore
    print 'Link check workflow tests passed.'
}
