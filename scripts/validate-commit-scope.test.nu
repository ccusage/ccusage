#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

use std/assert
use ./validate-commit-scope.nu validate

def "main validate" [title: string, ...paths: string]: nothing -> nothing {
    validate "test subject" $title $paths
}

def main []: nothing -> nothing {
    let script = $env.CURRENT_FILE
    [
        {
            title: 'feat(codex): totals'
            paths: ['rust/adapters/codex/src/lib.rs']
            valid: true
        }
        {
            title: 'feat(coding): totals'
            paths: ['rust/adapters/codex/src/lib.rs']
            valid: false
        }
        {
            title: 'fix: totals'
            paths: ['rust/adapters/codex/src/lib.rs']
            valid: false
        }
        {
            title: 'fix(adapter): totals'
            paths: ['rust/adapters/common/src/lib.rs']
            valid: true
        }
        {
            title: 'fix(all): totals'
            paths: ['rust/adapters/codex/src/lib.rs' 'rust/adapters/pi/src/lib.rs']
            valid: true
        }
        {
            title: 'fix(deps): totals'
            paths: ['rust/adapters/pi/Cargo.toml']
            valid: true
        }
        {
            title: 'ci: update checks'
            paths: ['.github/workflows/ci.yaml']
            valid: true
        }
        {
            title: 'invalid title'
            paths: []
            valid: false
        }
        {
            title: 'Merge branch main'
            paths: ['rust/adapters/pi/src/lib.rs']
            valid: true
        }
    ]
    | each {|case|
        let result = nu $script validate $case.title ...$case.paths | complete
        assert equal ($result.exit_code == 0) $case.valid $case.title
    }
    | ignore

    let fixture = mktemp -d
    try {
        let mock = $fixture | path join gh
        [
            $"#!($nu.current-exe)"
            'def --wrapped main [...args: string] { print $env.TEST_PR_FILES }'
        ] | str join "\n" | save $mock
        ^chmod +x $mock
        let event = $fixture | path join event.json
        with-env {
            PATH: ($env.PATH | prepend $fixture)
            GITHUB_EVENT_PATH: $event
            GITHUB_REPOSITORY: 'test/repo'
            TITLE_EXEMPT_LABELS: "autorelease: pending\ndependencies\n"
            TEST_PR_FILES: '[[{"filename":"rust/adapters/codex/src/lib.rs"}]]'
        } {
            [
                {title: 'feat(codex): totals', labels: [], valid: true}
                {title: 'feat(coding): totals', labels: [], valid: false}
                {title: 'feat(coding): totals', labels: [{name: dependencies}], valid: true}
                {title: 'feat(coding): totals', labels: [{name: 'autorelease: pending'}], valid: true}
            ]
            | each {|case|
                {pull_request: {number: 7, title: $case.title, labels: $case.labels}}
                | to json | save --force $event
                let result = nu ($env.FILE_PWD | path join validate-commit-scope.nu) pr | complete
                assert equal ($result.exit_code == 0) $case.valid $result.stderr
            }
            | ignore
        }
    } catch {|err|
        rm -rf $fixture
        error make $err
    }
    rm -rf $fixture
    print 'Commit and PR scope tests passed.'
}
