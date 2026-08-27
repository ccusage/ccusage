#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

use ./contribution-gate/coauthor.nu [coauthor-validation]
use ./contribution-gate/context.nu [issue-context-record]
use ./contribution-gate/core.nu [parse-issue-number]
use ./contribution-gate/requests.nu [
    coauthor-email
    existing-implementation-pull-request
    implementation-branch
    implementation-pull-request-body
    implementation-result
    render-prompt
]

def expect [name: string, actual, expected]: nothing -> nothing {
    if $actual != $expected {
        let expected_json = $expected | to json
        let actual_json = $actual | to json
        error make {msg: $"($name): expected ($expected_json), got ($actual_json)"}
    }
}

def test-coauthor-validation []: nothing -> nothing {
    let trailer = 'Co-authored-by: alice <alice@users.noreply.github.com>'
    let message = $"Implement the fix\n\n($trailer)"
    let attribution = [
        {email: 'alice@users.noreply.github.com', user_id: 42}
    ]
    (expect
        'accepts exact resolved attribution'
        (coauthor-validation $message $attribution $trailer 'alice@users.noreply.github.com' 42)
        {trailer_ok: true, author_ok: true}
    )
    (expect
        'rejects a missing trailer'
        (coauthor-validation 'Implement the fix' $attribution $trailer 'alice@users.noreply.github.com' 42)
        {trailer_ok: false, author_ok: true}
    )
    expect 'rejects an additional co-author' (
        coauthor-validation $"($message)\nCo-authored-by: bob <bob@example.com>" $attribution $trailer 'alice@users.noreply.github.com' 42
    ) {trailer_ok: false, author_ok: true}
    (expect
        'rejects unresolved attribution'
        (coauthor-validation $message [] $trailer 'alice@users.noreply.github.com' 42)
        {trailer_ok: true, author_ok: false}
    )
    (expect
        'rejects an issue author resolved outside the expected trailer'
        (coauthor-validation
            $message
            [{email: 'bot@example.com', user_id: 42}, {email: 'alice@users.noreply.github.com', user_id: 99}]
            $trailer
            'alice@users.noreply.github.com'
            42
        )
        {trailer_ok: true, author_ok: false}
    )
}

def test-coauthor-email []: nothing -> nothing {
    expect 'keeps a public account email' (
        coauthor-email alice 42 {email: 'alice@example.com' created_at: '2020-01-01T00:00:00Z'}
    ) 'alice@example.com'
    let legacy_result = (try {
        coauthor-email alice 42 {email: null created_at: '2017-07-17T23:59:59Z'}
    } catch {
        null
    })
    expect 'rejects an ambiguous legacy no-reply format' $legacy_result null
    expect 'uses the current no-reply format' (
        coauthor-email alice 42 {email: null created_at: '2017-07-18T00:00:00Z'}
    ) '42+alice@users.noreply.github.com'
}

def test-prompt-rendering []: nothing -> nothing {
    let rendered = render-prompt 'issue-implementation.md' {
        ISSUE_NUMBER: '42'
        REPOSITORY: 'ccusage/ccusage'
    }
    (expect
        'renders prompt values'
        ($rendered | str contains 'Issue number: #42 in ccusage/ccusage.')
        true
    )
    (expect
        'leaves implementation changes uncommitted'
        ($rendered | str contains 'Do not commit, push, or create a pull request.')
        true
    )
    (expect
        'requests PR metadata with a prepared result'
        ($rendered | str contains '"implementation":"prepared"')
        true
    )
    (expect
        'does not leave template placeholders'
        ($rendered | str contains '{{')
        false
    )
}

def test-existing-implementation-pull-request []: nothing -> nothing {
    let matching = {
        number: 100
        body: '<!-- pullfrog-accepted-issue: #42 request-abc -->'
        user: {login: 'github-actions[bot]'}
        head: {
            ref: 'pullfrog/issue-42-run-123'
            repo: {full_name: 'ccusage/ccusage'}
        }
    }
    let pull_requests = [
        $matching
        {
            number: 101
            body: '<!-- pullfrog-accepted-issue: #42 request-forged -->'
            user: {login: alice}
            head: {
                ref: 'feature/copied-marker'
                repo: {full_name: 'alice/ccusage'}
            }
        }
        {
            number: 102
            body: null
            user: {login: 'github-actions[bot]'}
            head: {
                ref: 'pullfrog/issue-420-run-456'
                repo: {full_name: 'ccusage/ccusage'}
            }
        }
    ]

    expect 'finds an existing implementation PR for the same issue' (
        existing-implementation-pull-request $pull_requests 'ccusage/ccusage' 42
    ) $matching
    expect 'does not trust a copied marker in an external PR' (
        existing-implementation-pull-request ($pull_requests | skip 1 | take 1) 'ccusage/ccusage' 42
    ) null
    expect 'does not match another issue branch' (
        existing-implementation-pull-request $pull_requests 'ccusage/ccusage' 7
    ) null
}

def test-implementation-result []: nothing -> nothing {
    expect 'accepts prepared PR metadata' (
        implementation-result '{"implementation":"prepared","title":"fix: count cache writes","body":"Fixes the missing cost.\n\nTests: focused adapter test."}'
    ) {implementation: prepared, title: 'fix: count cache writes', body: "Fixes the missing cost.\n\nTests: focused adapter test."}
    expect 'accepts a declined implementation' (
        implementation-result '{"implementation":"none","title":"","body":""}'
    ) {implementation: none, title: '', body: ''}

    let invalid_results = [
        '{"implementation":"prepared","title":"","body":"body"}'
        '{"implementation":"prepared","title":"title\nsecond line","body":"body"}'
        '{"implementation":"prepared","title":"title","body":""}'
        '{"implementation":"created","title":"title","body":"body"}'
        'not json'
    ]
    $invalid_results | each {|result|
        let outcome = (try {
            implementation-result $result
            'accepted'
        } catch {
            'rejected'
        })
        expect $"rejects invalid implementation result ($result)" $outcome 'rejected'
    } | ignore
}

def test-implementation-publication []: nothing -> nothing {
    expect 'uses a run-owned implementation branch' (
        implementation-branch 42 123 2
    ) 'pullfrog/issue-42-run-123-attempt-2'

    let invalid_run_values = [
        {run_id: 0, run_attempt: 1}
        {run_id: 123, run_attempt: 0}
    ]
    $invalid_run_values | each {|value|
        let outcome = (try {
            implementation-branch 42 $value.run_id $value.run_attempt
            'accepted'
        } catch {
            'rejected'
        })
        expect 'rejects an invalid workflow run identity' $outcome 'rejected'
    } | ignore

    expect 'adds workflow-owned PR metadata' (
        implementation-pull-request-body '<!-- marker -->' "Implemented the fix.\n\nTests: focused test." 42
    ) "<!-- marker -->\n\nImplemented the fix.\n\nTests: focused test.\n\nCloses #42"
}

def test-issue-context []: nothing -> nothing {
    expect 'normalizes an open issue' (
        issue-context-record 42 {
            number: 42
            state: open
            user: {login: alice id: 99}
        }
    ) {number: 42, author: alice, author_id: 99}

    let invalid_records = [
        {
            name: 'rejects a pull request payload'
            issue: {
                number: 42
                state: open
                user: {login: alice, id: 99}
                pull_request: {}
            }
        }
        {
            name: 'rejects a mismatched issue number'
            issue: {
                number: 41
                state: open
                user: {login: alice, id: 99}
            }
        }
        {
            name: 'rejects a closed issue'
            issue: {
                number: 42
                state: closed
                user: {login: alice, id: 99}
            }
        }
        {
            name: 'rejects malformed author data'
            issue: {
                number: 42
                state: open
                user: {login: '', id: 0}
            }
        }
    ]

    $invalid_records | each {|case|
        let result = (try {
            issue-context-record 42 $case.issue
            'accepted'
        } catch {
            'rejected'
        })
        expect $case.name $result rejected
    } | ignore
}

def test-issue-number []: nothing -> nothing {
    expect 'parses a positive integer issue number' (parse-issue-number '42') 42

    [
        '1.5'
        '1e3'
        '0'
        '-1'
        'abc'
        ''
    ] | each {|value|
        let result = (try {
            parse-issue-number $value
            'accepted'
        } catch {
            'rejected'
        })
        expect $"rejects invalid issue number ($value)" $result rejected
    } | ignore
}

def main [] {
    test-coauthor-validation
    test-coauthor-email
    test-prompt-rendering
    test-existing-implementation-pull-request
    test-implementation-result
    test-implementation-publication
    test-issue-context
    test-issue-number
    print 'contribution-gate Nushell tests passed.'
}
