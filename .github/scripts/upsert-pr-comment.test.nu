#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

use std/assert

def main []: nothing -> nothing {
    let fixture = mktemp -d
    try {
        let mock = $fixture | path join gh
        [
            $"#!($nu.current-exe)"
            'def --wrapped main [...args: string] {'
            '    let method = if "--method" in $args { $args | skip (($args | enumerate | where item == "--method" | first | get index) + 1) | first } else { "GET" }'
            '    $method + "\n" | save --append $env.TEST_CALLS'
            '    if $method == "GET" { print $env.TEST_COMMENTS; return }'
            '    if $method == $env.TEST_FAIL_METHOD { print --stderr $"HTTP ($env.TEST_HTTP_CODE)"; exit 1 }'
            '    print "{}"'
            '}'
        ] | str join "\n" | save $mock
        ^chmod +x $mock
        let marker = '<!-- ccusage-perf-comment:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa -->'
        let body = $fixture | path join comment.md
        [$marker '' 'Fixture performance report.'] | str join "\n" | save $body
        let log = $fixture | path join calls.txt
        let existing = [
            [
                {
                    id: 11
                    user: {login: 'github-actions[bot]'}
                    body: $marker
                    created_at: '2026-11-17T00:00:00Z'
                }
            ]
        ] | to json --raw
        [
            {
                comments: '[]'
                method: POST
                code: '401'
                valid: false
                calls: [GET POST]
            }
            {
                comments: $existing
                method: PATCH
                code: '403'
                valid: false
                calls: [GET PATCH]
            }
            {
                comments: $existing
                method: PATCH
                code: '404'
                valid: true
                calls: [GET PATCH POST]
            }
            {
                comments: '[]'
                method: ''
                code: ''
                valid: true
                calls: [GET POST]
            }
        ] | each {|case|
            '' | save --force $log
            let result = with-env {
                PATH: ($env.PATH | prepend $fixture)
                GITHUB_REPOSITORY: test/repo
                PR_NUMBER: '7'
                COMMENT_MARKER: $marker
                COMMENT_FILE: $body
                TEST_CALLS: $log
                TEST_COMMENTS: $case.comments
                TEST_FAIL_METHOD: $case.method
                TEST_HTTP_CODE: $case.code
            } {
                nu ($env.FILE_PWD | path join upsert-pr-comment.nu) | complete
            }
            assert equal ($result.exit_code == 0) $case.valid $result.stderr
            assert equal (open --raw $log | lines) $case.calls
        } | ignore
    } catch {|err|
        rm -rf $fixture
        error make $err
    }
    rm -rf $fixture
    print 'PR comment fixture tests passed.'
}
