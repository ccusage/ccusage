#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

use ./contribution-gate/coauthor.nu [coauthor-validation]
use ./contribution-gate/context.nu [issue-context-record]
use ./contribution-gate/requests.nu [coauthor-email render-prompt]

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
        IMPLEMENTATION_MARKER: '<!-- marker -->'
        COAUTHOR_TRAILER: 'Co-authored-by: alice <alice@example.com>'
    }
    (expect
        'renders prompt values'
        ($rendered | str contains 'Issue number: #42 in ccusage/ccusage.')
        true
    )
    (expect
        'renders the implementation marker'
        ($rendered | str contains '<!-- marker -->')
        true
    )
    (expect
        'renders the co-author trailer'
        ($rendered | str contains 'Co-authored-by: alice <alice@example.com>')
        true
    )
    (expect
        'does not leave template placeholders'
        ($rendered | str contains '{{')
        false
    )
    (expect
        'requests a structured implementation result'
        ($rendered | str contains '{"implementation":"created"}')
        true
    )
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

def main [] {
    test-coauthor-validation
    test-coauthor-email
    test-prompt-rendering
    test-issue-context
    print 'contribution-gate Nushell tests passed.'
}
