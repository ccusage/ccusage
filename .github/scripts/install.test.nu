#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#just --command nu

use std/assert

def main []: nothing -> nothing {
    let fixture = mktemp -d
    try {
        mkdir ($fixture | path join bin) ($fixture | path join node_modules)
        for module in [apps/ccusage docs rust] {
            mkdir ($fixture | path join $module)
            "build:\n\ntest:\n" | save ($fixture | path join $module justfile)
        }
        for tool in [typed plain] {
            mkdir ($fixture | path join nix/tools $tool)
            '' | save ($fixture | path join nix/tools $tool bun.lock)
        }
        '{}' | save ($fixture | path join nix/tools/typed/tsconfig.json)
        '' | save ($fixture | path join pnpm-lock.yaml)
        cp ($env.FILE_PWD | path join ../../justfile) ($fixture | path join justfile)
        let calls = $fixture | path join calls.txt
        '' | save $calls
        for tool in [pnpm bun] {
            let mock = $fixture | path join bin $tool
            [
                $"#!($nu.current-exe)"
                'def --wrapped main [...args: string] {'
                '    ($env.CURRENT_FILE | path basename) + "\n" | save --append $env.TEST_CALLS'
                '}'
            ] | str join "\n" | save $mock
            ^chmod +x $mock
        }
        let root_lock = $fixture | path join pnpm-lock.yaml
        let typed_lock = $fixture | path join nix/tools/typed/bun.lock
        let plain_lock = $fixture | path join nix/tools/plain/bun.lock
        ^touch --date=2000-01-01 $root_lock $typed_lock $plain_lock
        ^touch --date=2001-01-01 ($fixture | path join node_modules/.install-stamp)
        with-env {
            PATH: ($env.PATH | prepend ($fixture | path join bin))
            TEST_CALLS: $calls
        } {
            let recipe = $fixture | path join justfile
            just --justfile $recipe --working-directory $fixture install if-stale
            assert equal (open --raw $calls | lines) []
            ^touch --date=2002-01-01 $plain_lock
            just --justfile $recipe --working-directory $fixture install if-stale
            assert equal (open --raw $calls | lines) []
            ^touch --date=2002-01-01 $typed_lock
            just --justfile $recipe --working-directory $fixture install if-stale
            assert equal (open --raw $calls | lines) [pnpm bun]
            '' | save --force $calls
            just --justfile $recipe --working-directory $fixture install
            assert equal (open --raw $calls | lines) [pnpm bun]
        }
    } catch {|err|
        rm -rf $fixture
        error make $err
    }
    rm -rf $fixture
    print 'Dependency install trigger tests passed.'
}
