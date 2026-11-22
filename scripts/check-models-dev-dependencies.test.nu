#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

use std/assert

def main []: nothing -> nothing {
    let fixture = mktemp -d
    try {
        let lock = $fixture | path join bun.lock
        let manifest = $fixture | path join package.json
        '{"packages":{"remeda":["remeda@2.33.7"],"zod":["zod@3.24.2"]},}'
        | save $lock
        [
            {
                dependencies: {remeda: '2.33.7', zod: '3.24.2'}
            }
            {
                dependencies: {remeda: '2.33.8', zod: '3.24.2'}
            }
        ] | enumerate | each {|case|
            $case.item | to json | save --force $manifest
            let result = nu ($env.FILE_PWD | path join check-models-dev-dependencies.nu) $lock $manifest | complete
            assert equal ($result.exit_code == 0) ($case.index == 0) $result.stderr
        } | ignore
    } catch {|err|
        rm -rf $fixture
        error make $err
    }
    rm -rf $fixture
    print 'models.dev dependency drift tests passed.'
}
